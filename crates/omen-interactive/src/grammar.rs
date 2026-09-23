use omen_core::CoreError;
use std::ops::Range;

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

    /// Canonical static handles recognised by [`TypedReference::parse`] with
    /// no additional name argument. Used by completion as the typed-reference
    /// authority; dynamic `@fact.` / `@service.` names come from live
    /// registries.
    pub const STATIC_HANDLES: &'static [&'static str] = &[
        "@last",
        "@last.artifact",
        "@last.changed",
        "@last.failed",
        "@last.output",
        "@failed",
        "@errors",
    ];
}

/// One scanned word with its raw span and decoded literal.
///
/// This is the span-enriched view of the accepted argv grammar. Decoded
/// literals project exactly onto [`GrammarScanner::split_words`] once
/// empty literals (e.g. `""`) are filtered out, matching accepted-main
/// behaviour where empty words are dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedWord {
    /// Decoded literal value (quotes and escapes interpreted).
    pub literal: String,
    /// Raw byte span of this word in the source buffer, including quotes.
    pub span: Range<usize>,
    /// Opening quote character, if the word began inside a quote.
    pub open_quote: Option<char>,
    /// True when the source ended while this word's quote was still open.
    pub quote_unclosed: bool,
}

/// Cursor-aware view of the word being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorToken {
    /// Raw byte span of the whole word in the buffer.
    pub span: Range<usize>,
    /// Decoded literal of the portion of the word strictly before the cursor.
    pub decoded_prefix: String,
    /// Decoded literal of the portion of the word at or after the cursor.
    pub decoded_suffix: String,
    /// Quote context in effect at the cursor (`None` when unquoted).
    pub quote_at_cursor: Option<char>,
    /// Raw text before the cursor within the word.
    pub raw_prefix: String,
    /// Raw text from the cursor to the end of the word.
    pub raw_suffix: String,
    /// Full decoded literal of the word.
    pub literal: String,
    /// True when the cursor falls inside an escape sequence or a multi-byte
    /// character boundary such that a non-destructive split is unsafe.
    pub split_unsafe: bool,
    /// True when the word's quote is still open at end of input.
    pub quote_unclosed: bool,
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
    ///
    /// This is a projection of [`GrammarScanner::scan_words_with_spans`]: empty
    /// literals are dropped, matching accepted-main behaviour.
    pub fn split_words(s: &str) -> Vec<String> {
        scan_words_with_spans(s)
            .into_iter()
            .filter(|w| !w.literal.is_empty())
            .map(|w| w.literal)
            .collect()
    }
}

/// Scans `s` into words with raw spans, decoded literals, and quote state.
///
/// This is the ONE accepted argv grammar, enriched with positional
/// information. [`GrammarScanner::split_words`] is a projection of this
/// scanner and must not diverge from it.
pub fn scan_words_with_spans(s: &str) -> Vec<ScannedWord> {
    let mut words: Vec<ScannedWord> = Vec::new();
    let mut current = String::new();
    let mut in_quotes: Option<char> = None;
    let mut open_quote: Option<char> = None;
    let mut word_start: Option<usize> = None;
    let mut word_end: usize = 0;
    let mut raw_started = false;

    let mut idx = 0usize;
    let mut chars = s.char_indices().peekable();

    let flush = |words: &mut Vec<ScannedWord>,
                 current: &mut String,
                 open_quote: &mut Option<char>,
                 word_start: &mut Option<usize>,
                 word_end: &mut usize,
                 raw_started: &mut bool,
                 in_quotes: Option<char>,
                 at_eof: bool| {
        if *raw_started || !current.is_empty() {
            let start = word_start.unwrap_or(*word_end);
            words.push(ScannedWord {
                literal: std::mem::take(current),
                span: start..*word_end,
                open_quote: *open_quote,
                quote_unclosed: at_eof && in_quotes.is_some(),
            });
        }
        *open_quote = None;
        *word_start = None;
        *raw_started = false;
    };

    while let Some((i, c)) = chars.next() {
        idx = i;
        let c_len = c.len_utf8();
        let mark = |word_start: &mut Option<usize>,
                    word_end: &mut usize,
                    raw_started: &mut bool,
                    i: usize,
                    end: usize| {
            if word_start.is_none() {
                *word_start = Some(i);
            }
            *word_end = end;
            *raw_started = true;
        };

        match in_quotes {
            Some(quote_char) => {
                mark(
                    &mut word_start,
                    &mut word_end,
                    &mut raw_started,
                    i,
                    i + c_len,
                );
                if c == '\\' {
                    // In double quotes, \" produces literal "
                    if quote_char == '"' && matches!(chars.peek(), Some((_, '"'))) {
                        let (_, n) = chars.next().unwrap();
                        current.push('"');
                        idx = i;
                        word_end = i + c_len + n.len_utf8();
                    } else {
                        // Backslash inside quotes is preserved literally
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
                    mark(
                        &mut word_start,
                        &mut word_end,
                        &mut raw_started,
                        i,
                        i + c_len,
                    );
                    if open_quote.is_none() && current.is_empty() {
                        open_quote = Some(c);
                    }
                    in_quotes = Some(c);
                } else if c == '\\' {
                    mark(
                        &mut word_start,
                        &mut word_end,
                        &mut raw_started,
                        i,
                        i + c_len,
                    );
                    // Outside quotes: check if escaping a quote (\")
                    if matches!(chars.peek(), Some((_, '"'))) {
                        let (_, n) = chars.next().unwrap();
                        current.push('"');
                        word_end = i + c_len + n.len_utf8();
                    } else {
                        // Literal backslash / Windows path separator
                        current.push('\\');
                    }
                } else if c.is_whitespace() {
                    flush(
                        &mut words,
                        &mut current,
                        &mut open_quote,
                        &mut word_start,
                        &mut word_end,
                        &mut raw_started,
                        in_quotes,
                        false,
                    );
                } else {
                    mark(
                        &mut word_start,
                        &mut word_end,
                        &mut raw_started,
                        i,
                        i + c_len,
                    );
                    current.push(c);
                }
            }
        }
    }

    let end = s.len().max(idx);
    if word_end < end && raw_started {
        word_end = end;
    }
    if raw_started || !current.is_empty() {
        flush(
            &mut words,
            &mut current,
            &mut open_quote,
            &mut word_start,
            &mut word_end,
            &mut raw_started,
            in_quotes,
            true,
        );
    }

    words
}

/// Analyses the word under `cursor` into decoded prefix/suffix halves.
///
/// Returns `None` when `cursor` does not fall inside a scanned word (the
/// caller is between words or at a fresh token boundary). An empty synthetic
/// token at `cursor` is then appropriate.
pub fn token_at_cursor(buffer: &str, cursor: usize) -> Option<CursorToken> {
    let cursor = cursor.min(buffer.len());
    if !buffer.is_char_boundary(cursor) {
        return Some(CursorToken {
            span: cursor..cursor,
            decoded_prefix: String::new(),
            decoded_suffix: String::new(),
            quote_at_cursor: None,
            raw_prefix: String::new(),
            raw_suffix: String::new(),
            literal: String::new(),
            split_unsafe: true,
            quote_unclosed: false,
        });
    }

    let words = scan_words_with_spans(buffer);
    let word = words
        .iter()
        .find(|w| w.span.start <= cursor && cursor <= w.span.end)?;

    let raw = &buffer[word.span.clone()];
    let offset_in_word = cursor - word.span.start;

    let mut decoded_prefix = String::new();
    let mut decoded_suffix = String::new();
    // Replay the accepted grammar from scratch over the word's raw text so the
    // walk cannot diverge from `scan_words_with_spans`.
    let mut in_quotes: Option<char> = None;
    let mut split_unsafe = false;

    // Walk the word's raw characters with the accepted grammar state machine,
    // recording decoded output on each side of the cursor.
    let mut chars = raw.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let c_len = c.len_utf8();
        let char_start = i;
        let char_end = i + c_len;
        let mut consumed_end = char_end;
        let mut decoded: Vec<char> = Vec::new();

        if char_start < offset_in_word && offset_in_word < char_end {
            split_unsafe = true;
        }

        match in_quotes {
            Some(quote_char) => {
                if c == '\\' {
                    if quote_char == '"' && matches!(chars.peek(), Some((_, '"'))) {
                        let (_, n) = chars.next().unwrap();
                        consumed_end = char_end + n.len_utf8();
                        decoded.push('"');
                    } else {
                        decoded.push('\\');
                    }
                } else if c == quote_char {
                    in_quotes = None;
                } else {
                    decoded.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    in_quotes = Some(c);
                } else if c == '\\' {
                    if matches!(chars.peek(), Some((_, '"'))) {
                        let (_, n) = chars.next().unwrap();
                        consumed_end = char_end + n.len_utf8();
                        decoded.push('"');
                    } else {
                        decoded.push('\\');
                    }
                } else if c.is_whitespace() {
                    // Whitespace only separates words outside quotes; within a
                    // scanned word it cannot appear unquoted.
                    decoded.push(c);
                } else {
                    decoded.push(c);
                }
            }
        }

        if offset_in_word > char_start && offset_in_word < consumed_end {
            split_unsafe = true;
        }

        // Characters fully before the cursor contribute to the prefix;
        // characters at/after the cursor contribute to the suffix. Escape
        // pairs that straddle the cursor are rejected via split_unsafe and
        // assigned wholly to one side to keep decoding total.
        let target = if consumed_end <= offset_in_word {
            &mut decoded_prefix
        } else {
            &mut decoded_suffix
        };
        for ch in decoded {
            target.push(ch);
        }
    }

    Some(CursorToken {
        span: word.span.clone(),
        decoded_prefix,
        decoded_suffix,
        quote_at_cursor: in_quotes_after_cursor(raw, offset_in_word, word.open_quote),
        raw_prefix: raw[..offset_in_word.min(raw.len())].to_string(),
        raw_suffix: raw[offset_in_word.min(raw.len())..].to_string(),
        literal: word.literal.clone(),
        split_unsafe,
        quote_unclosed: word.quote_unclosed,
    })
}

fn in_quotes_after_cursor(raw: &str, offset: usize, _open_quote: Option<char>) -> Option<char> {
    let mut in_quotes: Option<char> = None;
    let mut chars = raw.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if i >= offset {
            break;
        }
        match in_quotes {
            Some(quote_char) => {
                if c == '\\' {
                    if quote_char == '"' && matches!(chars.peek(), Some((_, '"'))) {
                        chars.next();
                    }
                } else if c == quote_char {
                    in_quotes = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    in_quotes = Some(c);
                } else if c == '\\' && matches!(chars.peek(), Some((_, '"'))) {
                    chars.next();
                }
            }
        }
    }
    in_quotes
}

/// Encodes `literal` as a complete canonical token for this grammar.
///
/// Returns `None` when the literal cannot be faithfully represented (see
/// grammar limitations below). UNKNOWN is better than corrupt argv.
///
/// Limitations of the accepted grammar:
/// - the empty string cannot be represented (`""` decodes to a dropped word)
/// - a literal containing BOTH apostrophe and double quote AND ending in a
///   backslash cannot be represented (double quotes cannot escape `\`)
pub fn quote_literal(literal: &str) -> Option<String> {
    if literal.is_empty() {
        return None;
    }

    let needs_quotes = literal
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'');

    if !needs_quotes {
        return Some(literal.to_string());
    }

    if !literal.contains('\'') {
        return Some(format!("'{literal}'"));
    }

    // Must use double quotes; escape embedded double quotes as \".
    let mut inner = String::new();
    for c in literal.chars() {
        if c == '"' {
            inner.push_str("\\\"");
        } else {
            inner.push(c);
        }
    }
    // A trailing backslash would escape the closing quote in this grammar.
    if inner.ends_with('\\') {
        return None;
    }
    Some(format!("\"{inner}\""))
}

/// Encodes `literal` as text to insert inside an existing quote context.
///
/// Returns `None` when the middle cannot be encoded without quote surgery.
pub fn encode_middle(literal: &str, quote: Option<char>, next_raw: &str) -> Option<String> {
    match quote {
        Some('\'') => {
            if literal.contains('\'') {
                return None;
            }
            Some(literal.to_string())
        }
        Some('"') => {
            let mut out = String::new();
            for c in literal.chars() {
                if c == '"' {
                    out.push_str("\\\"");
                } else {
                    out.push(c);
                }
            }
            // Trailing backslash would escape whatever raw char follows.
            if out.ends_with('\\') && next_raw.starts_with('"') {
                return None;
            }
            Some(out)
        }
        Some(_) => None,
        None => {
            if literal.is_empty() {
                return Some(String::new());
            }
            if literal
                .chars()
                .any(|c| c.is_whitespace() || c == '"' || c == '\'')
            {
                return None;
            }
            if literal.ends_with('\\') && next_raw.starts_with('"') {
                return None;
            }
            Some(literal.to_string())
        }
    }
}

/// Round-trips `literal` through [`quote_literal`] and the accepted parser.
pub fn round_trips(literal: &str) -> bool {
    match quote_literal(literal) {
        Some(canonical) => GrammarScanner::split_words(&canonical) == vec![literal.to_string()],
        None => false,
    }
}

/// Decodes a single complete word from `raw` using the accepted grammar.
/// Returns `None` when `raw` does not decode to exactly one word.
pub fn decode_single_word(raw: &str) -> Option<String> {
    let words = GrammarScanner::split_words(raw);
    if words.len() == 1 {
        Some(words.into_iter().next().unwrap())
    } else {
        None
    }
}
