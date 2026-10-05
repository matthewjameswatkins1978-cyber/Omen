//! Omen-owned portable shell syntax plan.
//!
//! `deno_task_shell` is used only as a parser. This module lowers its syntax
//! tree into Omen-owned nodes and rejects syntax outside Omen's interactive
//! grammar; it never invokes the upstream shell executor.

use deno_task_shell::parser as upstream;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLine {
    pub items: Vec<ShellListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellListItem {
    pub backgrounded: bool,
    pub sequence: ShellSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellSequence {
    BooleanChain {
        first: ShellPipeline,
        rest: Vec<(ShellBooleanOperator, ShellPipeline)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellBooleanOperator {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPipeline {
    pub commands: Vec<ShellCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub environment: Vec<ShellAssignment>,
    pub words: Vec<ShellWord>,
    pub redirects: Vec<ShellRedirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAssignment {
    pub name: String,
    pub value: ShellWord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWord {
    pub parts: Vec<ShellWordPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellWordPart {
    Text(String),
    Variable(String),
    Tilde,
    CommandSubstitution(Box<ShellLine>),
    Quoted(Vec<ShellWordPart>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRedirect {
    pub fd: Option<ShellRedirectFd>,
    pub operation: ShellRedirectOperation,
    pub target: ShellRedirectTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRedirectFd {
    Fd(u32),
    StdoutStderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRedirectOperation {
    Input,
    Overwrite,
    Append,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRedirectTarget {
    Word(ShellWord),
    Fd(u32),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ShellParseError {
    #[error("shell syntax is invalid: {0}")]
    Syntax(String),
    #[error("shell syntax is not supported by Omen: {0}")]
    Unsupported(String),
}

/// Parse one interactive shell line into Omen-owned syntax nodes.
///
/// The returned tree is syntax only. Expansion, alias resolution, dispatch,
/// authority, and physical execution remain Omen responsibilities.
pub fn parse(input: &str) -> Result<ShellLine, ShellParseError> {
    if input.trim().is_empty() {
        return Ok(ShellLine { items: Vec::new() });
    }

    // Upstream currently parses `$NAME` but treats `${NAME}` as literal text.
    // Normalize only valid braced identifiers outside single quotes so both
    // required forms reach the same Omen variable node without expanding
    // single-quoted text.
    let normalized = normalize_braced_variables(input);
    let parsed =
        upstream::parse(&normalized).map_err(|error| ShellParseError::Syntax(error.to_string()))?;
    lower_line(parsed)
}

impl ShellLine {
    /// Whether this line uses semantics beyond Omen's existing argv scanner.
    pub fn requires_portable_shell(&self) -> bool {
        self.items.len() != 1
            || self.items.iter().any(|item| {
                item.backgrounded
                    || match &item.sequence {
                        ShellSequence::BooleanChain { first, rest } => {
                            !rest.is_empty()
                                || pipeline_requires_portable_shell(first)
                                || rest
                                    .iter()
                                    .any(|(_, pipeline)| pipeline_requires_portable_shell(pipeline))
                        }
                    }
            })
    }
}

fn pipeline_requires_portable_shell(pipeline: &ShellPipeline) -> bool {
    pipeline.commands.len() != 1
        || pipeline.commands.iter().any(|command| {
            !command.environment.is_empty()
                || !command.redirects.is_empty()
                || command.words.iter().any(word_requires_portable_shell)
        })
}

fn word_requires_portable_shell(word: &ShellWord) -> bool {
    word.parts
        .iter()
        .any(|part| word_part_requires_portable_shell(part, false))
}

fn word_part_requires_portable_shell(part: &ShellWordPart, quoted: bool) -> bool {
    match part {
        ShellWordPart::Variable(_) | ShellWordPart::CommandSubstitution(_) => true,
        ShellWordPart::Tilde => !quoted,
        ShellWordPart::Text(text) => {
            !quoted && text.chars().any(|ch| matches!(ch, '*' | '?' | '['))
        }
        ShellWordPart::Quoted(parts) => {
            parts.is_empty()
                || parts
                    .iter()
                    .any(|part| word_part_requires_portable_shell(part, true))
        }
    }
}

/// Expand one Omen shell word into zero or more argv words.
///
/// Globs are resolved against the session cwd, sorted, and retain the literal
/// pattern when there are no matches. Command substitution is left to the
/// execution layer because evaluating it can dispatch consequential commands.
pub fn expand_word(word: &ShellWord, cwd: &Path) -> Result<Vec<String>, ShellParseError> {
    expand_word_with_lookup(word, cwd, &|name| {
        std::env::var_os(name)
            .map(|value| {
                value.into_string().map_err(|_| {
                    ShellParseError::Unsupported(format!(
                        "environment variable {name} is not valid UTF-8"
                    ))
                })
            })
            .transpose()
    })
}

fn expand_word_with_lookup(
    word: &ShellWord,
    cwd: &Path,
    lookup: &impl Fn(&str) -> Result<Option<String>, ShellParseError>,
) -> Result<Vec<String>, ShellParseError> {
    let mut value = String::new();
    let mut pathname_pattern = false;
    append_word_parts(
        &word.parts,
        false,
        lookup,
        &mut value,
        &mut pathname_pattern,
    )?;

    if !pathname_pattern {
        return Ok(vec![value]);
    }

    let absolute_pattern = Path::new(&value).is_absolute();
    let mut pattern = if absolute_pattern {
        value.clone()
    } else {
        cwd.join(&value)
            .to_str()
            .ok_or_else(|| {
                ShellParseError::Unsupported(
                    "current directory is not valid UTF-8 in the current argv model".into(),
                )
            })?
            .to_string()
    };
    // Match the common-shell default: `**` has no recursive special meaning
    // unless a future explicit Omen option enables globstar.
    while pattern.contains("**") {
        pattern = pattern.replace("**", "*");
    }
    let paths = match glob::glob_with(
        &pattern,
        glob::MatchOptions {
            case_sensitive: !cfg!(windows),
            require_literal_separator: true,
            require_literal_leading_dot: true,
        },
    ) {
        Ok(paths) => paths,
        Err(_) => return Ok(vec![value]),
    };
    let mut matches = Vec::new();
    for path in paths {
        matches.push(path.map_err(|error| {
            ShellParseError::Unsupported(format!("glob traversal failed: {error}"))
        })?);
    }
    if matches.is_empty() {
        return Ok(vec![value]);
    }
    matches.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    matches
        .into_iter()
        .map(|path| {
            let projected = if absolute_pattern {
                path
            } else {
                path.strip_prefix(cwd).map(PathBuf::from).unwrap_or(path)
            };
            projected.into_os_string().into_string().map_err(|_| {
                ShellParseError::Unsupported(
                    "glob matched a path that is not representable as UTF-8 argv".into(),
                )
            })
        })
        .collect()
}

fn append_word_parts(
    parts: &[ShellWordPart],
    quoted: bool,
    lookup: &impl Fn(&str) -> Result<Option<String>, ShellParseError>,
    output: &mut String,
    pathname_pattern: &mut bool,
) -> Result<(), ShellParseError> {
    for part in parts {
        match part {
            ShellWordPart::Text(text) => {
                output.push_str(text);
                if !quoted && text.chars().any(|ch| matches!(ch, '*' | '?' | '[')) {
                    *pathname_pattern = true;
                }
            }
            ShellWordPart::Variable(name) => {
                let value = lookup(name)?.unwrap_or_default();
                if !quoted && value.chars().any(|ch| matches!(ch, '*' | '?' | '[')) {
                    *pathname_pattern = true;
                }
                output.push_str(&value);
            }
            ShellWordPart::Tilde if !quoted && output.is_empty() => {
                let home = if cfg!(windows) {
                    lookup("USERPROFILE")?.or(lookup("HOME")?)
                } else {
                    lookup("HOME")?
                };
                if let Some(home) = home {
                    if home.chars().any(|ch| matches!(ch, '*' | '?' | '[')) {
                        *pathname_pattern = true;
                    }
                    output.push_str(&home);
                } else {
                    output.push('~');
                }
            }
            ShellWordPart::Tilde => output.push('~'),
            ShellWordPart::CommandSubstitution(_) => {
                return Err(ShellParseError::Unsupported(
                    "command substitution requires the Omen execution dispatcher".into(),
                ));
            }
            ShellWordPart::Quoted(parts) => {
                append_word_parts(parts, true, lookup, output, pathname_pattern)?;
            }
        }
    }
    Ok(())
}

fn normalize_braced_variables(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut quote = None;
    let mut chars = input.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '\\' && quote != Some('\'') {
            output.push(ch);
            if let Some((_, escaped)) = chars.next() {
                output.push(escaped);
            }
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                output.push(ch);
                quote = None;
                continue;
            }
            if open == '\'' {
                output.push(ch);
                continue;
            }
        }
        if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
            output.push(ch);
            continue;
        }
        if ch == '$' && chars.peek().is_some_and(|(_, next)| *next == '{') {
            chars.next();
            let mut identifier = String::new();
            let mut closed = false;
            while let Some((_, next)) = chars.peek().copied() {
                if next == '}' {
                    chars.next();
                    closed = true;
                    break;
                }
                if next.is_ascii_alphanumeric() || next == '_' {
                    identifier.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            // A braced identifier must start with an ASCII letter or `_`.
            if closed
                && identifier
                    .as_bytes()
                    .first()
                    .is_some_and(|first| first.is_ascii_alphabetic() || *first == b'_')
            {
                output.push('$');
                output.push_str(&identifier);
            } else {
                output.push_str("${");
                output.push_str(&identifier);
                if closed {
                    output.push('}');
                }
            }
            continue;
        }
        output.push(ch);
    }
    output
}

fn lower_line(line: upstream::SequentialList) -> Result<ShellLine, ShellParseError> {
    let items = line
        .items
        .into_iter()
        .map(|item| {
            Ok(ShellListItem {
                backgrounded: item.is_async,
                sequence: lower_sequence(item.sequence)?,
            })
        })
        .collect::<Result<_, ShellParseError>>()?;
    Ok(ShellLine { items })
}

fn lower_sequence(sequence: upstream::Sequence) -> Result<ShellSequence, ShellParseError> {
    let mut first = None;
    let mut rest = Vec::new();
    flatten_boolean(sequence, &mut first, &mut rest)?;
    match first {
        Some(first) => Ok(ShellSequence::BooleanChain { first, rest }),
        None => Err(ShellParseError::Unsupported(
            "environment assignment without a command".into(),
        )),
    }
}

fn flatten_boolean(
    sequence: upstream::Sequence,
    first: &mut Option<ShellPipeline>,
    rest: &mut Vec<(ShellBooleanOperator, ShellPipeline)>,
) -> Result<(), ShellParseError> {
    match sequence {
        upstream::Sequence::BooleanList(list) => {
            flatten_boolean(list.current, first, rest)?;
            let operator = match list.op {
                upstream::BooleanListOperator::And => ShellBooleanOperator::And,
                upstream::BooleanListOperator::Or => ShellBooleanOperator::Or,
            };
            let mut next_first = None;
            let mut next_rest = Vec::new();
            flatten_boolean(list.next, &mut next_first, &mut next_rest)?;
            let Some(next_first) = next_first else {
                return Err(ShellParseError::Unsupported(
                    "environment assignment without a command".into(),
                ));
            };
            rest.push((operator, next_first));
            rest.extend(next_rest);
            Ok(())
        }
        upstream::Sequence::Pipeline(pipeline) => {
            let pipeline = lower_pipeline(pipeline)?;
            if first.is_none() {
                *first = Some(pipeline);
            } else {
                // The upstream parser encodes boolean chains recursively. A
                // nested right-hand sequence is appended by its parent after
                // the connecting operator, preserving textual left-to-right
                // evaluation across mixed `&&` and `||`.
                return Err(ShellParseError::Unsupported(
                    "invalid boolean-chain structure".into(),
                ));
            }
            Ok(())
        }
        upstream::Sequence::ShellVar(_) => Err(ShellParseError::Unsupported(
            "environment assignment without a command".into(),
        )),
    }
}

fn lower_pipeline(pipeline: upstream::Pipeline) -> Result<ShellPipeline, ShellParseError> {
    if pipeline.negated {
        return Err(ShellParseError::Unsupported(
            "negated pipelines are not part of Omen's shell grammar".into(),
        ));
    }

    let mut commands = Vec::new();
    lower_pipeline_inner(pipeline.inner, &mut commands)?;
    Ok(ShellPipeline { commands })
}

fn lower_pipeline_inner(
    inner: upstream::PipelineInner,
    commands: &mut Vec<ShellCommand>,
) -> Result<(), ShellParseError> {
    match inner {
        upstream::PipelineInner::Command(command) => commands.push(lower_command(command)?),
        upstream::PipelineInner::PipeSequence(sequence) => {
            if sequence.op != upstream::PipeSequenceOperator::Stdout {
                return Err(ShellParseError::Unsupported(
                    "combined stdout/stderr pipelines (`|&`) are not part of Omen's shell grammar"
                        .into(),
                ));
            }
            commands.push(lower_command(sequence.current)?);
            lower_pipeline_inner(sequence.next, commands)?;
        }
    }
    Ok(())
}

fn lower_command(command: upstream::Command) -> Result<ShellCommand, ShellParseError> {
    let upstream::Command { inner, redirect } = command;
    let upstream::CommandInner::Simple(command) = inner else {
        return Err(ShellParseError::Unsupported(
            "subshells are not part of Omen's shell grammar".into(),
        ));
    };

    let environment = command
        .env_vars
        .into_iter()
        .map(lower_assignment)
        .collect::<Result<_, _>>()?;
    let words = command
        .args
        .into_iter()
        .map(lower_word)
        .collect::<Result<_, _>>()?;
    let redirects = redirect
        .into_iter()
        .map(lower_redirect)
        .collect::<Result<_, _>>()?;
    Ok(ShellCommand {
        environment,
        words,
        redirects,
    })
}

fn lower_assignment(assignment: upstream::EnvVar) -> Result<ShellAssignment, ShellParseError> {
    Ok(ShellAssignment {
        name: assignment.name,
        value: lower_word(assignment.value)?,
    })
}

fn lower_word(word: upstream::Word) -> Result<ShellWord, ShellParseError> {
    let parts = word
        .into_parts()
        .into_iter()
        .map(lower_word_part)
        .collect::<Result<_, _>>()?;
    Ok(ShellWord { parts })
}

fn lower_word_part(part: upstream::WordPart) -> Result<ShellWordPart, ShellParseError> {
    match part {
        upstream::WordPart::Text(text) => Ok(ShellWordPart::Text(text)),
        upstream::WordPart::Variable(name) => Ok(ShellWordPart::Variable(name)),
        upstream::WordPart::Tilde => Ok(ShellWordPart::Tilde),
        upstream::WordPart::Command(line) => Ok(ShellWordPart::CommandSubstitution(Box::new(
            lower_line(line)?,
        ))),
        upstream::WordPart::Quoted(parts) => Ok(ShellWordPart::Quoted(
            parts
                .into_iter()
                .map(lower_word_part)
                .collect::<Result<_, _>>()?,
        )),
        upstream::WordPart::Brace(_) => Err(ShellParseError::Unsupported(
            "brace expansion is not part of Omen's shell grammar".into(),
        )),
    }
}

fn lower_redirect(redirect: upstream::Redirect) -> Result<ShellRedirect, ShellParseError> {
    let fd = redirect.maybe_fd.map(|fd| match fd {
        upstream::RedirectFd::Fd(fd) => ShellRedirectFd::Fd(fd),
        upstream::RedirectFd::StdoutStderr => ShellRedirectFd::StdoutStderr,
    });
    let operation = match redirect.op {
        upstream::RedirectOp::Input(_) => ShellRedirectOperation::Input,
        upstream::RedirectOp::Output(upstream::RedirectOpOutput::Overwrite) => {
            ShellRedirectOperation::Overwrite
        }
        upstream::RedirectOp::Output(upstream::RedirectOpOutput::Append) => {
            ShellRedirectOperation::Append
        }
    };
    let target = match redirect.io_file {
        upstream::IoFile::Word(word) => ShellRedirectTarget::Word(lower_word(word)?),
        upstream::IoFile::Fd(fd) => ShellRedirectTarget::Fd(fd),
    };
    Ok(ShellRedirect {
        fd,
        operation,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(input: &str) -> (ShellPipeline, Vec<(ShellBooleanOperator, ShellPipeline)>) {
        let parsed = parse(input).expect("valid Omen shell grammar");
        assert_eq!(parsed.items.len(), 1);
        let ShellSequence::BooleanChain { first, rest } = &parsed.items[0].sequence;
        (first.clone(), rest.clone())
    }

    #[test]
    fn parses_invocation_pipeline_boolean_and_sequence_precedence() {
        let parsed = parse("first && second | third; fourth || fifth & sixth").unwrap();
        assert_eq!(parsed.items.len(), 3);
        assert!(!parsed.items[0].backgrounded);
        assert!(parsed.items[1].backgrounded);
        assert!(!parsed.items[2].backgrounded);

        let ShellSequence::BooleanChain { first, rest } = &parsed.items[0].sequence;
        assert_eq!(first.commands.len(), 1);
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].0, ShellBooleanOperator::And);
        assert_eq!(rest[0].1.commands.len(), 2);

        let ShellSequence::BooleanChain { first, rest } = &parsed.items[1].sequence;
        assert_eq!(
            first.commands[0].words[0].parts,
            [ShellWordPart::Text("fourth".into())]
        );
        assert_eq!(rest[0].0, ShellBooleanOperator::Or);
        assert_eq!(
            rest[0].1.commands[0].words[0].parts,
            [ShellWordPart::Text("fifth".into())]
        );
    }

    #[test]
    fn parses_quotes_variables_tilde_substitution_globs_and_command_environment() {
        let (pipeline, _) =
            chain("MODE=dev echo '$HOME' '${HOME}' \"$HOME\" ~ ${HOME} \"${HOME}\" $(echo x) *.rs");
        let command = &pipeline.commands[0];
        assert_eq!(command.environment[0].name, "MODE");
        assert_eq!(
            command.words[1].parts,
            [ShellWordPart::Quoted(vec![ShellWordPart::Text(
                "$HOME".into()
            )])]
        );
        assert_eq!(
            command.words[2].parts,
            [ShellWordPart::Quoted(vec![ShellWordPart::Text(
                "${HOME}".into()
            )])]
        );
        assert_eq!(
            command.words[3].parts,
            [ShellWordPart::Quoted(vec![ShellWordPart::Variable(
                "HOME".into()
            )])]
        );
        assert_eq!(command.words[4].parts, [ShellWordPart::Tilde]);
        assert_eq!(
            command.words[5].parts,
            [ShellWordPart::Variable("HOME".into())]
        );
        assert_eq!(
            command.words[6].parts,
            [ShellWordPart::Quoted(vec![ShellWordPart::Variable(
                "HOME".into()
            )])]
        );
        assert!(matches!(
            command.words[7].parts.as_slice(),
            [ShellWordPart::CommandSubstitution(_)]
        ));
        assert_eq!(command.words[8].parts, [ShellWordPart::Text("*.rs".into())]);
    }

    #[test]
    fn parses_input_output_append_and_stderr_redirects() {
        let parsed =
            parse("read < input; write > output; append >> output; warn 2> errors").unwrap();
        let operations = parsed
            .items
            .iter()
            .map(|item| {
                let ShellSequence::BooleanChain { first, .. } = &item.sequence;
                first.commands[0].redirects[0].clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(operations[0].operation, ShellRedirectOperation::Input);
        assert_eq!(operations[1].operation, ShellRedirectOperation::Overwrite);
        assert_eq!(operations[2].operation, ShellRedirectOperation::Append);
        assert_eq!(operations[3].fd, Some(ShellRedirectFd::Fd(2)));
    }

    #[test]
    fn rejects_shell_language_constructs_and_unimplemented_extensions() {
        for input in [
            "(echo nested)",
            "! echo negated",
            "echo {one,two}",
            "echo a |& cat",
        ] {
            assert!(
                matches!(parse(input), Err(ShellParseError::Unsupported(_))),
                "{input}"
            );
        }
        assert!(matches!(
            parse("echo < input > output"),
            Err(ShellParseError::Syntax(_))
        ));
    }

    #[test]
    fn mixed_boolean_operators_are_retained_in_source_order() {
        let (_, rest) = chain("a && b || c && d");
        assert_eq!(
            rest.iter()
                .map(|(operator, _)| *operator)
                .collect::<Vec<_>>(),
            [
                ShellBooleanOperator::And,
                ShellBooleanOperator::Or,
                ShellBooleanOperator::And
            ]
        );
    }

    #[test]
    fn blank_line_is_an_empty_no_op() {
        assert_eq!(parse("  \t").unwrap(), ShellLine { items: Vec::new() });
    }

    #[test]
    fn braced_variable_normalization_preserves_single_quotes_and_escapes() {
        assert_eq!(
            normalize_braced_variables("${A} '${B}' \"${C}\" \\${D}"),
            "$A '${B}' \"$C\" \\${D}"
        );
    }

    #[test]
    fn expands_variables_without_expanding_single_quoted_content() {
        let parsed =
            parse("echo '$OMEN_SHELL_TEST' \"$OMEN_SHELL_TEST\" ${OMEN_SHELL_TEST}").unwrap();
        let ShellSequence::BooleanChain { first, .. } = &parsed.items[0].sequence;
        let environment = std::collections::HashMap::from([(
            "OMEN_SHELL_TEST".to_string(),
            "portable value".to_string(),
        )]);
        let lookup = |name: &str| Ok(environment.get(name).cloned());
        let expanded = first.commands[0].words[1..]
            .iter()
            .map(|word| expand_word_with_lookup(word, Path::new("."), &lookup).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            expanded,
            [
                vec!["$OMEN_SHELL_TEST".to_string()],
                vec!["portable value".to_string()],
                vec!["portable value".to_string()]
            ]
        );
    }

    #[test]
    fn deterministic_globbing_sorts_matches_and_respects_quoting() {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("zeta.txt"), "").unwrap();
        std::fs::write(cwd.path().join("alpha.txt"), "").unwrap();
        std::fs::write(cwd.path().join(".hidden.txt"), "").unwrap();

        let unquoted = parse("echo *.txt").unwrap();
        let ShellSequence::BooleanChain { first, .. } = &unquoted.items[0].sequence;
        assert_eq!(
            expand_word(&first.commands[0].words[1], cwd.path()).unwrap(),
            ["alpha.txt", "zeta.txt"]
        );

        let quoted = parse("echo '*.txt'").unwrap();
        let ShellSequence::BooleanChain { first, .. } = &quoted.items[0].sequence;
        assert_eq!(
            expand_word(&first.commands[0].words[1], cwd.path()).unwrap(),
            ["*.txt"]
        );

        let unmatched = ShellWord {
            parts: vec![ShellWordPart::Text("missing-*.txt".into())],
        };
        assert_eq!(
            expand_word(&unmatched, cwd.path()).unwrap(),
            ["missing-*.txt"]
        );
    }
}
