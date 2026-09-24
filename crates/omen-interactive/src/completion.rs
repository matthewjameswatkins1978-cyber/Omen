//! Deterministic completion and ghost-suggestion engine for the human shell.
//!
//! Architecture: ONE typed candidate model -> many deterministic bounded
//! sources -> ONE ranking authority -> ghost presentation OR candidate menu.
//!
//! Completion assists editing; it is not a second grammar. Every insertion is
//! validated to round-trip through the accepted argv grammar
//! ([`crate::grammar`]) before it is offered. Anything the grammar cannot
//! faithfully represent is declined rather than corrupted.
//!
//! Hot-path contract (keystroke): pure computation over the parsed buffer,
//! cwd, immutable command authority, and bounded local completion state. No
//! subprocess, no network, no model, no PATH scan, no recursive traversal, no
//! sleep/retry. Bounded single-directory reads are permitted for path
//! completion and are capped by [`bounds`].

use crate::commands;
use crate::grammar::{self, CursorToken, GrammarScanner, TypedReference};
use omen_core::ValidityState;
use omen_knowledge::{Database, FactRegistry};
use reedline::{Completer, CompletionResult, Hinter, Span, Suggestion};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Centralised conservative bounds. Never scatter magic limits.
pub mod bounds {
    use std::time::Duration;

    /// Maximum PATH directories examined during out-of-band command discovery.
    pub const MAX_PATH_DIRS: usize = 64;
    /// Maximum directory entries examined per PATH directory.
    pub const MAX_PATH_ENTRIES_PER_DIR: usize = 256;
    /// Maximum command names retained in the PATH command cache.
    pub const MAX_COMMAND_NAMES: usize = 512;
    /// Maximum filesystem entries scanned for one path-completion request.
    pub const MAX_FS_SCAN: usize = 1024;
    /// Maximum filesystem entries retained for one path-completion request.
    pub const MAX_FS_ENTRIES: usize = 256;
    /// Maximum candidates retained per source.
    pub const MAX_CANDIDATES_PER_SOURCE: usize = 32;
    /// Maximum final candidate set exposed to the UI.
    pub const MAX_FINAL_CANDIDATES: usize = 24;
    /// Maximum display length for a candidate label.
    pub const MAX_DISPLAY_CHARS: usize = 80;
    /// Score margin required between best and runner-up before a ghost is shown.
    pub const GHOST_MIN_SCORE_MARGIN: i32 = 30;
    /// PATH command cache freshness window.
    pub const PATH_CACHE_TTL: Duration = Duration::from_secs(60);
}

/// Explicit candidate vocabulary. Only kinds backed by real accepted-main data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CandidateKind {
    /// Executable available on PATH (bounded cache).
    Command,
    /// Shell intrinsic handled by the interactive session (`cd`, `exit`, `quit`).
    Intrinsic,
    /// Canonical Omen semantic action (`:status`, ...).
    OmenAction,
    /// Command-line option syntax metadata.
    Option,
    /// Filesystem file path.
    Path,
    /// Filesystem directory path.
    Directory,
    /// Typed reference (`@last`, `@failed`, `@fact.x`, ...).
    TypedReference,
    /// Workspace target from composition config.
    WorkspaceTarget,
    /// Managed service (`proc://`).
    Service,
    /// Semantic resource (fact, symbol, package, task).
    Resource,
}

/// Provenance of a candidate (which deterministic source produced it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CandidateSource {
    /// `commands::OMEN_ACTIONS`.
    OmenActionAuthority,
    /// `commands::SHELL_INTRINSICS`.
    IntrinsicAuthority,
    /// Bounded PATH command cache.
    PathCommandCache,
    /// Bounded single-directory filesystem listing.
    Filesystem,
    /// `TypedReference::STATIC_HANDLES` plus live fact/service names.
    TypedReferenceAuthority,
    /// Static tool/action syntax metadata.
    SyntaxMetadata,
    /// Hot semantic index (facts, symbols, packages, tasks, services).
    HotSemanticIndex,
}

/// The explicit edit a candidate performs on the editable buffer.
///
/// The UI MUST apply this edit. It must not infer replacement spans from
/// display text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEdit {
    /// Raw byte range in the buffer that `insertion_text` replaces.
    /// A zero-width range at the cursor means a pure insertion.
    pub replacement_range: Range<usize>,
    /// Raw canonical text placed at `replacement_range`.
    pub insertion_text: String,
}

/// ONE candidate model shared by every source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    /// Human-facing label. May differ from insertion text (unquoted view).
    pub display_text: String,
    /// Decoded literal value this candidate represents.
    pub literal: String,
    pub kind: CandidateKind,
    pub source: CandidateSource,
    /// Deterministic ranking score. Higher is better.
    pub score: i32,
    /// The explicit buffer edit.
    pub edit: CompletionEdit,
    /// Optional short detail for the candidate menu.
    pub detail: Option<String>,
}

/// Cached active fact entry in the hot semantic index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFact {
    pub resource_uri: String,
    pub validity: ValidityState,
}

/// Bounded hot semantic cache owned per-session.
///
/// Populated strictly out-of-band (session startup, prompt boundary, after
/// execution, PATH-environment change). The keystroke hot path only reads this
/// structure; it never refreshes it.
#[derive(Debug, Clone, Default)]
pub struct HotSemanticIndex {
    /// Bounded active facts from the Fact Registry.
    pub active_facts: Vec<CachedFact>,
    /// Bounded command names discovered on PATH (out-of-band only).
    pub path_commands: Vec<String>,
    /// Bounded in-memory symbols for action-argument completion.
    pub cached_symbols: Vec<String>,
    /// Bounded in-memory known packages.
    pub known_packages: Vec<String>,
    /// Bounded in-memory known tasks.
    pub known_tasks: Vec<String>,
    /// Managed service names snapshot.
    pub known_services: Vec<String>,
    /// Truncation flags so callers can tell incomplete coverage from absence.
    pub path_commands_truncated: bool,
    path_scanned_at: Option<Instant>,
}

impl HotSemanticIndex {
    /// Refreshes DB-backed and in-memory local state out-of-band.
    pub fn refresh(&mut self, _cwd: &Path, db: Option<&Database>) {
        if let Some(db_conn) = db {
            if let Ok(facts) = FactRegistry::list_active_facts(db_conn, bounds::MAX_COMMAND_NAMES) {
                self.active_facts = facts
                    .into_iter()
                    .map(|f| CachedFact {
                        resource_uri: f.resource_uri.to_string(),
                        validity: f.validity,
                    })
                    .collect();
            }
        } else {
            self.active_facts.clear();
        }

        self.known_services = crate::services::ServiceRegistry::global()
            .list()
            .into_iter()
            .map(|s| s.name)
            .collect();

        self.refresh_path_commands(false);
    }

    /// Refreshes the bounded PATH command cache out-of-band.
    ///
    /// `force` bypasses the freshness TTL. Never call this on a keystroke.
    pub fn refresh_path_commands(&mut self, force: bool) {
        let fresh = self
            .path_scanned_at
            .is_some_and(|t| t.elapsed() < bounds::PATH_CACHE_TTL);
        if fresh && !force {
            return;
        }
        let scanned = commands::list_path_commands(
            bounds::MAX_PATH_DIRS,
            bounds::MAX_PATH_ENTRIES_PER_DIR,
            bounds::MAX_COMMAND_NAMES,
        );
        self.path_commands_truncated = scanned.len() >= bounds::MAX_COMMAND_NAMES;
        self.path_commands = scanned;
        self.path_scanned_at = Some(Instant::now());
    }

    /// Direct injection of PATH commands (tests and explicit cache refresh).
    pub fn update_path_commands(&mut self, commands: Vec<String>) {
        self.path_commands = commands;
        self.path_scanned_at = Some(Instant::now());
    }

    /// Updates semantic index entries (symbols, packages, tasks) out-of-band.
    pub fn update_semantics(
        &mut self,
        symbols: Vec<String>,
        packages: Vec<String>,
        tasks: Vec<String>,
    ) {
        self.cached_symbols = symbols;
        self.known_packages = packages;
        self.known_tasks = tasks;
    }

    pub fn update_fact(&mut self, info: &omen_ipc::FactInfo) {
        let validity = match info.validity.as_str() {
            "CURRENT" => ValidityState::Current,
            "DIRTY" => ValidityState::Dirty,
            "STALE" => ValidityState::Stale,
            "SUPERSEDED" => ValidityState::Superseded,
            "HISTORICAL" => ValidityState::Historical,
            _ => ValidityState::Dirty,
        };
        if let Some(existing) = self
            .active_facts
            .iter_mut()
            .find(|f| f.resource_uri == info.resource_uri)
        {
            existing.validity = validity;
        } else {
            self.active_facts.push(CachedFact {
                resource_uri: info.resource_uri.clone(),
                validity,
            });
            if self.active_facts.len() > bounds::MAX_COMMAND_NAMES {
                self.active_facts.remove(0);
            }
        }
    }

    pub fn mark_fact_dirty(&mut self, uri_or_id: &str) {
        let mut found = false;
        for fact in &mut self.active_facts {
            if fact.resource_uri == uri_or_id {
                fact.validity = ValidityState::Dirty;
                found = true;
            }
        }
        if !found {
            self.active_facts.push(CachedFact {
                resource_uri: uri_or_id.to_string(),
                validity: ValidityState::Dirty,
            });
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &omen_ipc::SharedIndexSnapshot) {
        self.active_facts = snapshot
            .facts
            .iter()
            .map(|f| CachedFact {
                resource_uri: f.resource_uri.clone(),
                validity: match f.validity.as_str() {
                    "CURRENT" => ValidityState::Current,
                    "DIRTY" => ValidityState::Dirty,
                    "STALE" => ValidityState::Stale,
                    "SUPERSEDED" => ValidityState::Superseded,
                    "HISTORICAL" => ValidityState::Historical,
                    _ => ValidityState::Dirty,
                },
            })
            .collect();
    }
}

/// Completion input state. Everything the engine may read on the hot path.
#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub cwd: PathBuf,
    pub hot_index: HotSemanticIndex,
}

impl Default for CompletionContext {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            hot_index: HotSemanticIndex::default(),
        }
    }
}

/// Where in the line the cursor sits, for source selection.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandPosition {
    /// Cursor is in the first word (command / action name).
    CommandName,
    /// Cursor is in an argument of `:action`.
    ActionArg { action: String },
    /// Cursor is in an argument of an executable.
    ExecArg { command: String },
}

/// Builds the explicit edit for `literal` against the cursor token.
///
/// Mid-token: compatible candidates insert ONLY the missing middle at the
/// cursor; the raw suffix stays intact. Incompatible suffixes are declined
/// rather than destructively replaced.
///
/// End-of-token unquoted: the typed span may be replaced by a freshly
/// canonical token (quoting may need to change).
///
/// Every accepted edit round-trips through the accepted grammar to the same
/// literal. Anything else is declined.
pub fn build_edit(token: &CursorToken, literal: &str, cursor: usize) -> Option<CompletionEdit> {
    if token.split_unsafe {
        return None;
    }
    let prefix = token.decoded_prefix.as_str();
    let suffix = token.decoded_suffix.as_str();
    if literal.is_empty() {
        return None;
    }
    if !literal.starts_with(prefix) {
        return None;
    }
    if !literal.ends_with(suffix) {
        return None;
    }
    if prefix.len() + suffix.len() > literal.len() {
        return None;
    }

    let middle = &literal[prefix.len()..literal.len() - suffix.len()];

    let end_of_token = suffix.is_empty();
    let inside_open_quote = token.quote_at_cursor.is_some();

    if end_of_token && !inside_open_quote {
        // Fresh canonical token; quoting may be introduced or changed.
        if middle.is_empty() && token.raw_prefix == literal {
            // Already exactly the candidate. Nothing to insert.
            return None;
        }
        let insertion_text = grammar::quote_literal(literal)?;
        if grammar::decode_single_word(&insertion_text).as_deref() != Some(literal) {
            return None;
        }
        return Some(CompletionEdit {
            replacement_range: token.span.clone(),
            insertion_text,
        });
    }

    // Pure insertion at the cursor: mid-token or inside an open quote.
    if middle.is_empty() && !token.quote_unclosed {
        return None;
    }

    let mut attempts: Vec<String> = Vec::new();
    if let Some(enc) = grammar::encode_middle(middle, token.quote_at_cursor, &token.raw_suffix) {
        attempts.push(enc.clone());
        if token.quote_unclosed {
            let mut closed = enc;
            closed.push(token.quote_at_cursor.unwrap_or('"'));
            attempts.push(closed);
        }
    } else if token.quote_unclosed
        && let Some(q) = token.quote_at_cursor
    {
        attempts.push(q.to_string());
    }

    for insertion_text in attempts {
        let mut new_raw = String::new();
        new_raw.push_str(&token.raw_prefix);
        new_raw.push_str(&insertion_text);
        new_raw.push_str(&token.raw_suffix);
        if grammar::decode_single_word(&new_raw).as_deref() == Some(literal) {
            return Some(CompletionEdit {
                replacement_range: cursor..cursor,
                insertion_text,
            });
        }
    }
    None
}

/// Deterministic ranking authority.
///
/// Signals (documented, stable, non-learned):
/// 1. candidate kind base priority (context-valid kinds only are scored)
/// 2. prefix quality (exact > case-sensitive prefix > case-insensitive prefix)
/// 3. length penalty (prefer less remaining typing), capped
/// 4. locality bias (directories once a path parent is typed)
///
/// Late tie-breaker: lexicographic on (kind, literal). Alphabetical order is
/// never a ranking preference, only a stability tie-break.
fn score_candidate(literal: &str, kind: CandidateKind, prefix: &str, prefix_cs: bool) -> i32 {
    let mut s = match kind {
        CandidateKind::OmenAction => 900,
        CandidateKind::Intrinsic => 880,
        CandidateKind::Command => 860,
        CandidateKind::TypedReference => 840,
        CandidateKind::Resource => 760,
        CandidateKind::Service => 740,
        CandidateKind::WorkspaceTarget => 720,
        CandidateKind::Directory => 700,
        CandidateKind::Path => 680,
        CandidateKind::Option => 600,
    };
    if literal == prefix {
        s += 200;
    } else if prefix_cs {
        s += 100;
    } else {
        s += 60;
    }
    s -= (literal.chars().count().min(40) as i32) / 4;
    if kind == CandidateKind::Directory && (prefix.contains('/') || prefix.contains('\\')) {
        s += 15;
    }
    s
}

fn prefix_quality(literal: &str, prefix: &str) -> Option<bool> {
    if prefix.is_empty() {
        return Some(true);
    }
    if literal.starts_with(prefix) {
        return Some(true);
    }
    if literal.to_lowercase().starts_with(&prefix.to_lowercase()) {
        return Some(false);
    }
    None
}

fn truncate_display(s: &str) -> String {
    if s.chars().count() <= bounds::MAX_DISPLAY_CHARS {
        return s.to_string();
    }
    s.chars().take(bounds::MAX_DISPLAY_CHARS).collect()
}

/// The typed deterministic completion core.
///
/// Callable as plain Rust with no terminal, no pixels, and no I/O beyond one
/// bounded directory read for path completion.
pub struct CompletionEngine;

impl CompletionEngine {
    /// Produces the bounded, ranked candidate set for `buffer`/`cursor`.
    pub fn complete(
        ctx: &CompletionContext,
        buffer: &str,
        cursor: usize,
    ) -> Vec<CompletionCandidate> {
        let cursor = cursor.min(buffer.len());
        if !buffer.is_char_boundary(cursor) {
            return Vec::new();
        }
        let token = match grammar::token_at_cursor(buffer, cursor) {
            Some(t) => t,
            None => synthetic_token(cursor),
        };
        if token.split_unsafe {
            return Vec::new();
        }

        let position = command_position(buffer, &token);
        let prefix = token.decoded_prefix.clone();

        let mut raw: Vec<RawCandidate> = Vec::new();
        gather_sources(ctx, &token, &position, &prefix, &mut raw);

        let mut out: Vec<CompletionCandidate> = Vec::new();
        for r in raw {
            if out.len() >= bounds::MAX_CANDIDATES_PER_SOURCE * 4 {
                break;
            }
            let Some(pq) = prefix_quality(&r.literal, &prefix) else {
                continue;
            };
            if !token.decoded_suffix.is_empty()
                && (!r.literal.starts_with(token.decoded_prefix.as_str())
                    || !r.literal.ends_with(token.decoded_suffix.as_str()))
            {
                continue;
            }
            let Some(edit) = build_edit(&token, &r.literal, cursor) else {
                continue;
            };
            let score = score_candidate(&r.literal, r.kind, &prefix, pq) + r.score_bias;
            out.push(CompletionCandidate {
                display_text: truncate_display(&r.display.unwrap_or_else(|| r.literal.clone())),
                literal: r.literal,
                kind: r.kind,
                source: r.source,
                score,
                edit,
                detail: r.detail,
            });
        }

        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.kind.cmp(&a.kind))
                .then_with(|| a.literal.cmp(&b.literal))
                .then_with(|| a.source.cmp(&b.source))
        });
        out.dedup_by(|a, b| a.edit == b.edit && a.literal == b.literal);
        out.truncate(bounds::MAX_FINAL_CANDIDATES);
        out
    }

    /// Returns the single calm ghost candidate, if confidence is high enough.
    ///
    /// A ghost is NOT simply candidate[0]. It requires: context validity,
    /// prefix compatibility, suffix compatibility when mid-token, a safe edit,
    /// and a unique or decisively better best candidate. When in doubt: quiet.
    pub fn ghost(
        ctx: &CompletionContext,
        buffer: &str,
        cursor: usize,
    ) -> Option<CompletionCandidate> {
        if cursor < buffer.len() {
            // Ghosts only appear at the live end of input; mid-line the explicit
            // menu is the calm interaction.
            return None;
        }
        let candidates = Self::complete(ctx, buffer, cursor);
        if candidates.is_empty() {
            return None;
        }
        let best = candidates[0].clone();
        if best.edit.replacement_range.start != cursor
            && best.edit.replacement_range.end != cursor
            && !(best.edit.replacement_range.start <= cursor
                && cursor <= best.edit.replacement_range.end)
        {
            return None;
        }
        // Ghost is append-only in the line editor: only safe when the edit is a
        // pure insertion at the cursor, or a full-span replace that is itself a
        // pure extension of the raw prefix.
        let token =
            grammar::token_at_cursor(buffer, cursor).unwrap_or_else(|| synthetic_token(cursor));
        let append_safe = if best.edit.replacement_range.start == best.edit.replacement_range.end {
            best.edit.replacement_range.start == cursor
        } else {
            best.edit.insertion_text.starts_with(&token.raw_prefix)
                && best.edit.replacement_range == token.span
        };
        if !append_safe {
            return None;
        }

        let decisive = if candidates.len() == 1 {
            true
        } else {
            best.score - candidates[1].score >= bounds::GHOST_MIN_SCORE_MARGIN
        };
        if !decisive {
            return None;
        }
        Some(best)
    }

    /// The append-only hint string for the ghost (what the editor shows).
    pub fn ghost_hint(
        ctx: &CompletionContext,
        buffer: &str,
        cursor: usize,
    ) -> Option<(String, CompletionCandidate)> {
        let best = Self::ghost(ctx, buffer, cursor)?;
        let token =
            grammar::token_at_cursor(buffer, cursor).unwrap_or_else(|| synthetic_token(cursor));
        let hint = if best.edit.replacement_range.start == best.edit.replacement_range.end {
            best.edit.insertion_text.clone()
        } else {
            best.edit.insertion_text[token.raw_prefix.len()..].to_string()
        };
        if hint.is_empty() {
            return None;
        }
        Some((hint, best))
    }
}

struct RawCandidate {
    literal: String,
    display: Option<String>,
    kind: CandidateKind,
    source: CandidateSource,
    detail: Option<String>,
    score_bias: i32,
}

impl RawCandidate {
    fn new(literal: impl Into<String>, kind: CandidateKind, source: CandidateSource) -> Self {
        Self {
            literal: literal.into(),
            display: None,
            kind,
            source,
            detail: None,
            score_bias: 0,
        }
    }

    fn with_detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }

    fn with_bias(mut self, b: i32) -> Self {
        self.score_bias = b;
        self
    }
}

fn synthetic_token(cursor: usize) -> CursorToken {
    CursorToken {
        span: cursor..cursor,
        decoded_prefix: String::new(),
        decoded_suffix: String::new(),
        quote_at_cursor: None,
        raw_prefix: String::new(),
        raw_suffix: String::new(),
        literal: String::new(),
        split_unsafe: false,
        quote_unclosed: false,
    }
}

fn command_position(buffer: &str, token: &CursorToken) -> CommandPosition {
    // Words fully before the current token determine the position.
    let words_before = GrammarScanner::split_words(&buffer[..token.span.start.min(buffer.len())]);
    if words_before.is_empty() {
        return CommandPosition::CommandName;
    }
    let first = words_before[0].clone();
    if first.starts_with(':') {
        let action = first.trim_start_matches(':').to_string();
        return CommandPosition::ActionArg { action };
    }
    CommandPosition::ExecArg { command: first }
}

fn gather_sources(
    ctx: &CompletionContext,
    token: &CursorToken,
    position: &CommandPosition,
    prefix: &str,
    out: &mut Vec<RawCandidate>,
) {
    let starts_colon = prefix.starts_with(':');
    let starts_at = prefix.starts_with('@');

    // Omen semantic actions: canonical `:name` in command-name position.
    if matches!(position, CommandPosition::CommandName) && (starts_colon || prefix.is_empty()) {
        for action in commands::OMEN_ACTIONS {
            out.push(RawCandidate::new(
                format!(":{action}"),
                CandidateKind::OmenAction,
                CandidateSource::OmenActionAuthority,
            ));
        }
        if starts_colon {
            return;
        }
    }

    if starts_at {
        gather_typed_refs(ctx, prefix, out);
        return;
    }

    if matches!(position, CommandPosition::CommandName) && !starts_colon {
        for name in commands::SHELL_INTRINSICS {
            out.push(RawCandidate::new(
                *name,
                CandidateKind::Intrinsic,
                CandidateSource::IntrinsicAuthority,
            ));
        }
        for name in &ctx.hot_index.path_commands {
            out.push(RawCandidate::new(
                name.clone(),
                CandidateKind::Command,
                CandidateSource::PathCommandCache,
            ));
        }
    }

    match position {
        CommandPosition::ActionArg { action } => {
            gather_action_args(ctx, action, prefix, out);
            let subs = commands::omen_action_subcommands(action);
            for s in subs {
                out.push(RawCandidate::new(
                    *s,
                    CandidateKind::Option,
                    CandidateSource::SyntaxMetadata,
                ));
            }
        }
        CommandPosition::ExecArg { command } => {
            let cmd = command.trim_start_matches(':');
            for s in commands::tool_subcommands(cmd) {
                out.push(RawCandidate::new(
                    *s,
                    CandidateKind::Option,
                    CandidateSource::SyntaxMetadata,
                ));
            }
        }
        CommandPosition::CommandName => {}
    }

    // Path completion: argument position, or command position when the token
    // looks like an explicit path or Windows drive designator.
    let path_ok = match position {
        CommandPosition::CommandName => {
            prefix.contains('/')
                || prefix.contains('\\')
                || prefix.starts_with('.')
                || crate::commands::is_drive_path(prefix)
        }
        _ => true,
    };
    if path_ok && !starts_colon && !starts_at {
        let dirs_only = matches!(position, CommandPosition::ExecArg { command } if command == "cd");
        gather_paths(ctx, token, prefix, dirs_only, out);
    }
}

fn gather_typed_refs(ctx: &CompletionContext, prefix: &str, out: &mut Vec<RawCandidate>) {
    for h in TypedReference::STATIC_HANDLES {
        out.push(RawCandidate::new(
            *h,
            CandidateKind::TypedReference,
            CandidateSource::TypedReferenceAuthority,
        ));
    }
    if prefix.starts_with("@fact.") || "@fact.".starts_with(prefix) || prefix == "@" {
        for f in &ctx.hot_index.active_facts {
            let name = f
                .resource_uri
                .strip_prefix("fact://")
                .unwrap_or(&f.resource_uri)
                .to_string();
            let lit = format!("@fact.{name}");
            let validity = format!("{:?}", f.validity);
            let bias = if f.validity == ValidityState::Dirty {
                50
            } else {
                0
            };
            out.push(
                RawCandidate::new(
                    lit,
                    CandidateKind::Resource,
                    CandidateSource::HotSemanticIndex,
                )
                .with_detail(format!("fact ({validity})"))
                .with_bias(bias),
            );
        }
    }
    if prefix.starts_with("@service.") || "@service.".starts_with(prefix) || prefix == "@" {
        for s in &ctx.hot_index.known_services {
            out.push(RawCandidate::new(
                format!("@service.{s}"),
                CandidateKind::Service,
                CandidateSource::HotSemanticIndex,
            ));
        }
    }
}

fn gather_action_args(
    ctx: &CompletionContext,
    action: &str,
    _prefix: &str,
    out: &mut Vec<RawCandidate>,
) {
    match action {
        "symbol" | "def" | "refs" => {
            for s in &ctx.hot_index.cached_symbols {
                out.push(RawCandidate::new(
                    s.clone(),
                    CandidateKind::Resource,
                    CandidateSource::HotSemanticIndex,
                ));
            }
        }
        "packages" => {
            for p in &ctx.hot_index.known_packages {
                out.push(RawCandidate::new(
                    p.clone(),
                    CandidateKind::Resource,
                    CandidateSource::HotSemanticIndex,
                ));
            }
        }
        "tasks" => {
            for t in &ctx.hot_index.known_tasks {
                out.push(RawCandidate::new(
                    t.clone(),
                    CandidateKind::Resource,
                    CandidateSource::HotSemanticIndex,
                ));
            }
        }
        "stop" | "status" | "services" => {
            for s in &ctx.hot_index.known_services {
                out.push(RawCandidate::new(
                    format!("@service.{s}"),
                    CandidateKind::Service,
                    CandidateSource::HotSemanticIndex,
                ));
            }
        }
        "rerun" | "show" | "inspect" | "why" | "history" => {
            for h in TypedReference::STATIC_HANDLES {
                out.push(RawCandidate::new(
                    *h,
                    CandidateKind::TypedReference,
                    CandidateSource::TypedReferenceAuthority,
                ));
            }
        }
        _ => {}
    }
}

struct PathEntry {
    name: String,
    is_dir: bool,
}

fn gather_paths(
    ctx: &CompletionContext,
    token: &CursorToken,
    prefix: &str,
    dirs_only: bool,
    out: &mut Vec<RawCandidate>,
) {
    let entries = list_dir_bounded(ctx, prefix);
    let sep = preferred_sep(prefix);
    let (parent_raw, _leaf) = split_path_prefix(prefix);

    for e in entries {
        if dirs_only && !e.is_dir {
            continue;
        }
        let mut literal = String::new();
        literal.push_str(&parent_raw);
        if !parent_raw.is_empty() && !parent_raw.ends_with('/') && !parent_raw.ends_with('\\') {
            literal.push(sep);
        }
        literal.push_str(&e.name);
        if e.is_dir {
            literal.push(sep);
        }
        let kind = if e.is_dir {
            CandidateKind::Directory
        } else {
            CandidateKind::Path
        };
        out.push(
            RawCandidate::new(literal, kind, CandidateSource::Filesystem).with_display(e.name),
        );
    }

    if !dirs_only || prefix.is_empty() || prefix == "." || prefix == ".." {
        // no-op placeholder for future workspace targets
    }
    let _ = token;
}

impl RawCandidate {
    fn with_display(mut self, d: impl Into<String>) -> Self {
        self.display = Some(d.into());
        self
    }
}

fn preferred_sep(prefix: &str) -> char {
    if prefix.contains('\\') && !prefix.contains('/') {
        '\\'
    } else if prefix.contains('/') {
        '/'
    } else if cfg!(windows) {
        '\\'
    } else {
        '/'
    }
}

/// Splits a typed path into (parent_with_separators, leaf_prefix).
///
/// Handles Windows drive designators: `D:` → (`D:\`, ``).
fn split_path_prefix(prefix: &str) -> (String, String) {
    // Bare drive designator: `D:` is a path prefix for the drive root.
    if crate::commands::is_drive_designator(prefix).is_some() {
        return (format!("{prefix}\\"), String::new());
    }
    let bytes = prefix.as_bytes();
    let mut split_at = None;
    for (i, b) in bytes.iter().enumerate().rev() {
        if *b == b'/' || *b == b'\\' {
            split_at = Some(i);
            break;
        }
    }
    match split_at {
        Some(i) => (prefix[..=i].to_string(), prefix[i + 1..].to_string()),
        None => (String::new(), prefix.to_string()),
    }
}

fn list_dir_bounded(ctx: &CompletionContext, prefix: &str) -> Vec<PathEntry> {
    let (parent_raw, leaf) = split_path_prefix(prefix);
    let parent_path = if parent_raw.is_empty() {
        ctx.cwd.clone()
    } else {
        let p = PathBuf::from(&parent_raw);
        if p.is_absolute() { p } else { ctx.cwd.join(&p) }
    };

    let Ok(read_dir) = std::fs::read_dir(&parent_path) else {
        return Vec::new();
    };

    let mut scanned: Vec<PathEntry> = Vec::new();
    for (i, entry) in read_dir.flatten().enumerate() {
        if i >= bounds::MAX_FS_SCAN {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if leaf.is_empty() || name.to_lowercase().starts_with(&leaf.to_lowercase()) {
            scanned.push(PathEntry { name, is_dir });
        }
    }

    scanned.sort_by(|a, b| a.name.cmp(&b.name).then(a.is_dir.cmp(&b.is_dir)));
    scanned.truncate(bounds::MAX_FS_ENTRIES);
    scanned
}

/// Reedline adapter: exposes the typed engine as a `Completer`.
pub struct OmenCompleter {
    context: Arc<Mutex<CompletionContext>>,
}

impl OmenCompleter {
    pub fn new(context: Arc<Mutex<CompletionContext>>) -> Self {
        Self { context }
    }

    pub fn context(&self) -> Arc<Mutex<CompletionContext>> {
        self.context.clone()
    }

    /// Typed core result (machine-readable; no terminal scraping required).
    pub fn complete_typed(&mut self, line: &str, pos: usize) -> Vec<CompletionCandidate> {
        let Ok(ctx) = self.context.lock() else {
            return Vec::new();
        };
        CompletionEngine::complete(&ctx, line, pos)
    }

    /// Reedline-shaped result. `value` is the canonical insertion text and
    /// `span` is the explicit replacement range.
    pub fn complete_items(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        self.complete_typed(line, pos)
            .into_iter()
            .map(|c| {
                let append_whitespace = c.edit.replacement_range.start
                    == c.edit.replacement_range.end
                    && c.kind != CandidateKind::Directory
                    && c.kind != CandidateKind::Path
                    && pos == line.len();
                Suggestion {
                    value: c.edit.insertion_text,
                    description: {
                        let kind = match c.kind {
                            CandidateKind::Command => "command",
                            CandidateKind::Intrinsic => "intrinsic",
                            CandidateKind::OmenAction => "semantic action",
                            CandidateKind::Option => "option",
                            CandidateKind::Path => "path",
                            CandidateKind::Directory => "directory",
                            CandidateKind::TypedReference => "typed reference",
                            CandidateKind::WorkspaceTarget => "workspace target",
                            CandidateKind::Service => "service",
                            CandidateKind::Resource => "resource",
                        };
                        match c.detail {
                            Some(d) => Some(format!("{kind}: {d}")),
                            None => Some(kind.to_string()),
                        }
                    },
                    extra: None,
                    span: Span {
                        start: c.edit.replacement_range.start,
                        end: c.edit.replacement_range.end,
                    },
                    append_whitespace,
                    display_override: Some(c.display_text),
                    match_indices: None,
                    style: None,
                }
            })
            .collect()
    }
}

impl Completer for OmenCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let items = self.complete_items(line, pos);
        CompletionResult::fresh(items)
    }
}

/// Ghost-suggestion hinter. Append-only; calm by construction.
///
/// Retains the current safe ghost suffix so Reedline's `HistoryHintComplete`
/// (Right / End) can insert it into the editable buffer via [`complete_hint`].
/// The cached accept payload is cleared on every `handle()` call and is only
/// populated when [`CompletionEngine::ghost_hint`] approves a ghost. The
/// visible ghost and the accepted payload are always the same safe edit.
pub struct OmenHinter {
    completer: Arc<Mutex<OmenCompleter>>,
    /// Raw unformatted safe ghost suffix approved by `CompletionEngine::ghost_hint`.
    current_hint: String,
}

impl OmenHinter {
    pub fn new(completer: Arc<Mutex<OmenCompleter>>) -> Self {
        Self {
            completer,
            current_hint: String::new(),
        }
    }
}

impl Hinter for OmenHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        _history: &dyn reedline::History,
        _use_ansi: bool,
        _cwd: &str,
    ) -> String {
        // Always clear the accept payload first: stale hints must never survive
        // a buffer change, cursor move, ambiguity, lock failure, or empty input.
        self.current_hint.clear();
        if line.is_empty() {
            return String::new();
        }
        let Ok(comp) = self.completer.lock() else {
            return String::new();
        };
        let Ok(ctx) = comp.context.lock() else {
            return String::new();
        };
        match CompletionEngine::ghost_hint(&ctx, line, pos) {
            Some((hint, _)) => {
                self.current_hint = hint.clone();
                hint
            }
            None => String::new(),
        }
    }

    fn complete_hint(&self) -> String {
        self.current_hint.clone()
    }

    fn next_hint_token(&self) -> String {
        // F1 ghost hints are always single-token word suffixes (e.g. `"us"`,
        // `"go"`, `"tor"`). The first semantic token is the whole hint.
        // Partial-token acceptance is not intentionally supported in F1.
        self.current_hint.clone()
    }
}
