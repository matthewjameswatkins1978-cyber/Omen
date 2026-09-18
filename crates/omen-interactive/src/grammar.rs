use omen_core::CoreError;

/// The parsed input lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputLane {
    /// Ordinary executable dispatch (e.g. `cargo test`, `git status`)
    Executable { argv: Vec<String> },

    /// Omen semantic action prefixed by `:` (e.g. `:status`, `:test auth`, `:why @last`)
    SemanticAction { action: String, args: Vec<String> },

    /// Optional AI reasoning lane prefixed by `?` (e.g. `? why is auth failing?`)
    AiReasoning { query: String },
}

/// Strongly typed reference identifier extracted from `@handle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedReference {
    /// `@last` - Most recent operation in current interactive session
    Last,
    /// `@last.failed` - Most recent failing target in current interactive session
    LastFailed,
    /// `@last.artifact` - Primary artifact of last operation
    LastArtifact,
    /// `@last.changed` - Resources changed by last operation
    LastChanged,
    /// `@last.output` - Raw output bounded slice
    LastOutput,
    /// `@failed` - Unambiguous failed target in current session
    Failed,
    /// `@errors` - Known current compiler or runtime errors
    Errors,
    /// `@fact.<name>` - Specific fact in registry
    Fact(String),
    /// `@service.<name>` - Specific managed service or process
    Service(String),
    /// Unrecognized handle
    Other(String),
}

impl TypedReference {
    pub fn parse(handle: &str) -> Option<Self> {
        let stripped = handle.strip_prefix('@')?;
        let lower = stripped.to_lowercase();

        match lower.as_str() {
            "last" => Some(Self::Last),
            "last.failed" => Some(Self::LastFailed),
            "last.artifact" => Some(Self::LastArtifact),
            "last.changed" => Some(Self::LastChanged),
            "last.output" => Some(Self::LastOutput),
            "failed" => Some(Self::Failed),
            "errors" => Some(Self::Errors),
            other => {
                if let Some(fact_name) = other.strip_prefix("fact.") {
                    Some(Self::Fact(fact_name.to_string()))
                } else if let Some(svc_name) = other.strip_prefix("service.") {
                    Some(Self::Service(svc_name.to_string()))
                } else {
                    Some(Self::Other(other.to_string()))
                }
            }
        }
    }
}

pub struct GrammarScanner;

impl GrammarScanner {
    /// Scans raw input into one of the three input lanes.
    pub fn scan(input: &str) -> Result<InputLane, CoreError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(InputLane::Executable { argv: vec![] });
        }

        // 1. AI Reasoning Lane: begins with `?`
        if let Some(query) = trimmed.strip_prefix('?') {
            return Ok(InputLane::AiReasoning {
                query: query.trim().to_string(),
            });
        }

        // 2. Semantic Action Lane: begins with `:`
        if let Some(action_str) = trimmed.strip_prefix(':') {
            let parts = Self::split_words(action_str);
            if parts.is_empty() {
                return Err(CoreError::SchemaViolation(
                    "Missing action name after ':'".into(),
                ));
            }
            let action = parts[0].clone();
            let args = parts[1..].to_vec();
            return Ok(InputLane::SemanticAction { action, args });
        }

        // 3. Ordinary Executable Invocation: standard argv splitting
        let argv = Self::split_words(trimmed);
        Ok(InputLane::Executable { argv })
    }

    /// Word splitter preserving Windows path separators, double and single quotes,
    /// and handling spaces, literal/trailing backslashes, and incomplete quotes.
    pub fn split_words(s: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut current = String::new();
        let mut in_quotes: Option<char> = None;
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            match in_quotes {
                Some(quote_char) => {
                    if c == '\\' {
                        // In double quotes, \" produces literal "
                        if quote_char == '"' && chars.peek() == Some(&'"') {
                            chars.next();
                            current.push('"');
                        } else {
                            // Backslash inside quotes is preserved literally (e.g. "C:\Program Files\App")
                            current.push('\\');
                        }
                    } else if c == quote_char {
                        in_quotes = None;
                    } else {
                        current.push(c);
                    }
                }
                None => {
                    if c == '"' || c == '\'' {
                        in_quotes = Some(c);
                    } else if c == '\\' {
                        // Outside quotes: check if escaping a quote (\")
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            current.push('"');
                        } else {
                            // Literal backslash / Windows path separator (e.g. C:\Users, \\server\share, dir\)
                            current.push('\\');
                        }
                    } else if c.is_whitespace() {
                        if !current.is_empty() {
                            words.push(current.clone());
                            current.clear();
                        }
                    } else {
                        current.push(c);
                    }
                }
            }
        }

        // If string ends while in unclosed quotes, preserve whatever was read
        if !current.is_empty() {
            words.push(current);
        }

        words
    }
}
