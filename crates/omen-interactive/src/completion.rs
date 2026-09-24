//! Deterministic completion and ghost-suggestion engine for the human shell.
//!
//! **The live completion authority is the M1 discovery substrate.** Every
//! candidate this module presents originates from
//! [`crate::discovery::DiscoveryRuntime`] (providers -> semantic merge ->
//! matcher -> LensRanker); this module owns only:
//!
//! - the **projection boundary**: `RankedCandidate` -> grammar-validated edit
//!   -> [`CompletionCandidate`] -> [`reedline::Suggestion`];
//! - the **interaction seam**: Reedline `Completer` (`Fresh` / `Stale` /
//!   `Pending` + `poll_completion`) and the ghost hinter;
//! - session contexts ([`CompletionContext`], [`HotSemanticIndex`]) from
//!   which the runtime's [`LiveSnapshot`](crate::discovery::LiveSnapshot) is
//!   built.
//!
//! Completion assists editing; it is not a second grammar. Every insertion is
//! validated to round-trip through the accepted argv grammar
//! ([`crate::grammar`]) before it is offered. Anything the grammar cannot
//! faithfully represent is declined rather than corrupted.
//!
//! Hot-path contract (keystroke): the repaint paths ([`OmenCompleter::complete_typed`],
//! [`OmenCompleter::readiness`], [`OmenCompleter::ghost_view`]) **observe**
//! inline truth only — they never dispatch background work and never block.
//! The completion request ([`Completer::complete`](reedline::Completer::complete))
//! dispatches TIER 1B/2 work out-of-band and reports `Pending`/`Stale` until
//! [`OmenCompleter::poll_completion`](reedline::Completer::poll_completion)
//! settles it. The static [`CompletionEngine`] API pumps to a settled result
//! for tests and sync tooling.

use crate::commands;
use crate::discovery::{CompletionPhase, DiscoveryRuntime, LiveSnapshot};
use crate::grammar::{self, CursorToken};
use omen_core::ValidityState;
use omen_discovery::candidate::RankedCandidate;
use omen_discovery::kind::Kind;
use omen_discovery::providers::{HotFact, HotIndexSnapshot};
use omen_discovery::scheduler::WorkStatus;
use omen_knowledge::{Database, FactRegistry};
use reedline::{
    Completer, CompletionOrigin, CompletionResult, CompletionStatus, Hinter, Span, Suggestion,
};
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

/// ONE candidate model shared by every presentation path.
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
    /// Optional detail for the candidate menu (description detail/short text).
    pub detail: Option<String>,
}

/// Tab-gating readiness: distinguishes a *proven* zero from discovery that is
/// still in flight.
///
/// M0 gated Tab on a candidate count alone; M1 adds PENDING ASYNC DISCOVERY,
/// which must not be swallowed as though discovery proved zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionReadiness {
    /// Discovery finished; zero candidates. Tab declines cleanly.
    FinalZero,
    /// Discovery finished; `n` candidates are available now.
    Ready(usize),
    /// Async discovery is in flight; `inline` candidates are known so far
    /// (possibly none). Tab must not be treated as a proven zero.
    Pending { inline: usize },
}

/// Shared readiness cell consulted by the edit mode on every Tab.
pub type ReadinessCell = Arc<Mutex<CompletionReadiness>>;

/// Creates the shared readiness cell.
pub fn new_readiness() -> ReadinessCell {
    Arc::new(Mutex::new(CompletionReadiness::FinalZero))
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

// ---------------------------------------------------------------------------
// SNAPSHOT — interactive context -> discovery runtime inputs
// ---------------------------------------------------------------------------

/// Builds the discovery snapshot from the session completion context.
fn snapshot_from(ctx: &CompletionContext) -> LiveSnapshot {
    LiveSnapshot {
        cwd: ctx.cwd.to_string_lossy().into_owned(),
        path_commands: ctx.hot_index.path_commands.clone(),
        path_commands_truncated: ctx.hot_index.path_commands_truncated,
        hot: hot_snapshot(&ctx.hot_index),
    }
}

fn hot_snapshot(hot: &HotSemanticIndex) -> HotIndexSnapshot {
    HotIndexSnapshot {
        facts: hot
            .active_facts
            .iter()
            .map(|f| HotFact {
                resource_uri: f.resource_uri.clone(),
                validity: format!("{:?}", f.validity),
            })
            .collect(),
        symbols: hot.cached_symbols.clone(),
        packages: hot.known_packages.clone(),
        tasks: hot.known_tasks.clone(),
        services: hot.known_services.clone(),
    }
}

// ---------------------------------------------------------------------------
// GRAMMAR-SAFE EDIT APPLICATION (unchanged M0 contract)
// ---------------------------------------------------------------------------

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

/// Whether `buffer`/`cursor` may be completed at all (safe token boundary,
/// no split-unsafe token). Mirrors M0's pre-discovery guards so unsafe input
/// never dispatches background work.
fn token_allows(buffer: &str, cursor: usize) -> bool {
    let cursor = cursor.min(buffer.len());
    if !buffer.is_char_boundary(cursor) {
        return false;
    }
    !matches!(grammar::token_at_cursor(buffer, cursor), Some(t) if t.split_unsafe)
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

fn truncate_display(s: &str) -> String {
    if s.chars().count() <= bounds::MAX_DISPLAY_CHARS {
        return s.to_string();
    }
    s.chars().take(bounds::MAX_DISPLAY_CHARS).collect()
}

// ---------------------------------------------------------------------------
// PROJECTION — RankedCandidate -> CompletionCandidate (the one human projection)
// ---------------------------------------------------------------------------

/// Maps a discovery [`Kind`] onto the presentation vocabulary.
fn map_kind(kind: Kind) -> CandidateKind {
    match kind {
        Kind::Command => CandidateKind::Command,
        Kind::Intrinsic => CandidateKind::Intrinsic,
        Kind::OmenAction => CandidateKind::OmenAction,
        Kind::Option | Kind::Subcommand => CandidateKind::Option,
        Kind::File => CandidateKind::Path,
        Kind::Directory => CandidateKind::Directory,
        Kind::Reference => CandidateKind::TypedReference,
        Kind::Resource | Kind::ArgumentValue | Kind::Capability | Kind::HistoryItem => {
            CandidateKind::Resource
        }
        Kind::Service => CandidateKind::Service,
        Kind::Workspace => CandidateKind::WorkspaceTarget,
    }
}

/// Maps provider provenance onto the presentation source vocabulary.
fn map_source(provenance: &omen_discovery::identity::ProvenanceKey) -> CandidateSource {
    match provenance.provider.as_str() {
        "omen-actions" => CandidateSource::OmenActionAuthority,
        "intrinsics" => CandidateSource::IntrinsicAuthority,
        "path-commands" => CandidateSource::PathCommandCache,
        "filesystem-paths" => CandidateSource::Filesystem,
        "references" => CandidateSource::TypedReferenceAuthority,
        "hot-semantic-index" => CandidateSource::HotSemanticIndex,
        _ => CandidateSource::SyntaxMetadata,
    }
}

/// Projects ranked discovery truth into grammar-validated presentation
/// candidates. The ONE human projection; the machine projection serialises
/// the same [`RankedCandidate`]s.
///
/// Candidates whose edits cannot round-trip through the accepted grammar are
/// declined here (never corrupted). Result order is the ranker's order.
pub(crate) fn project_ranked(
    ranked: &[RankedCandidate],
    buffer: &str,
    cursor: usize,
    include_descriptions: bool,
) -> Vec<CompletionCandidate> {
    let cursor = cursor.min(buffer.len());
    if !buffer.is_char_boundary(cursor) {
        return Vec::new();
    }
    let token = grammar::token_at_cursor(buffer, cursor).unwrap_or_else(|| synthetic_token(cursor));
    if token.split_unsafe {
        return Vec::new();
    }

    let mut out: Vec<CompletionCandidate> = Vec::new();
    for r in ranked {
        if out.len() >= bounds::MAX_FINAL_CANDIDATES {
            break;
        }
        let literal = r.value().insert.clone();
        let Some(edit) = build_edit(&token, &literal, cursor) else {
            continue;
        };
        let discovered = &r.matched.discovered;
        let detail = if include_descriptions {
            discovered
                .description
                .as_ref()
                .and_then(|d| d.detail.clone().or_else(|| Some(d.short.clone())))
        } else {
            None
        };
        out.push(CompletionCandidate {
            display_text: truncate_display(
                &discovered
                    .display
                    .as_ref()
                    .map(|d| d.label.clone())
                    .unwrap_or_else(|| literal.clone()),
            ),
            literal,
            kind: map_kind(r.semantic().kind),
            source: map_source(&discovered.provenance),
            score: (r.rank_score.0 * 10.0).round() as i32,
            edit,
            detail,
        });
    }
    out
}

/// Descriptions included (default presentation).
pub(crate) fn project_ranked_default(
    ranked: &[RankedCandidate],
    buffer: &str,
    cursor: usize,
) -> Vec<CompletionCandidate> {
    project_ranked(ranked, buffer, cursor, true)
}

// ---------------------------------------------------------------------------
// GHOST POLICY (M0 calm-ghost rules over M1 truth)
// ---------------------------------------------------------------------------

/// Returns the single calm ghost candidate, if confidence is high enough.
///
/// A ghost is NOT simply candidate[0]. It requires: end-of-line cursor,
/// context validity, an append-only-safe edit, and a unique or decisively
/// better best candidate. The candidate itself is always M1 truth — ghosts
/// never invent a candidate absent from discovery.
fn ghost_from(
    candidates: &[CompletionCandidate],
    buffer: &str,
    cursor: usize,
) -> Option<CompletionCandidate> {
    if cursor < buffer.len() {
        // Ghosts only appear at the live end of input; mid-line the explicit
        // menu is the calm interaction.
        return None;
    }
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
    let token = grammar::token_at_cursor(buffer, cursor).unwrap_or_else(|| synthetic_token(cursor));
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
fn ghost_hint_from(
    best: &CompletionCandidate,
    buffer: &str,
    cursor: usize,
) -> Option<(String, CompletionCandidate)> {
    let token = grammar::token_at_cursor(buffer, cursor).unwrap_or_else(|| synthetic_token(cursor));
    let hint = if best.edit.replacement_range.start == best.edit.replacement_range.end {
        best.edit.insertion_text.clone()
    } else {
        best.edit.insertion_text[token.raw_prefix.len()..].to_string()
    };
    if hint.is_empty() {
        return None;
    }
    Some((hint, best.clone()))
}

// ---------------------------------------------------------------------------
// COMPLETION ENGINE — static, synchronous convenience (tests / sync tooling)
// ---------------------------------------------------------------------------

thread_local! {
    /// Per-thread default discovery runtime for the static API. Persistent
    /// per thread (no thread-farm-per-call) and backed by the same shared
    /// scheduler worker as the live session.
    static STATIC_RUNTIME: std::cell::RefCell<DiscoveryRuntime> =
        std::cell::RefCell::new(DiscoveryRuntime::new());
}

/// The typed deterministic completion core, backed by M1 discovery.
///
/// Callable as plain Rust with no terminal and no pixels. Pumps background
/// work to a settled result (bounded deadline) so callers observe complete
/// truth synchronously. **The live editor path does not use this**: the
/// completer's `Fresh`/`Stale`/`Pending` seam is non-blocking.
pub struct CompletionEngine;

impl CompletionEngine {
    /// Produces the bounded, ranked candidate set for `buffer`/`cursor`.
    pub fn complete(
        ctx: &CompletionContext,
        buffer: &str,
        cursor: usize,
    ) -> Vec<CompletionCandidate> {
        if !token_allows(buffer, cursor) {
            return Vec::new();
        }
        let snap = snapshot_from(ctx);
        STATIC_RUNTIME.with(|rt| {
            let view =
                rt.borrow_mut()
                    .pump(buffer, cursor, &snap, DiscoveryRuntime::settle_deadline());
            project_ranked_default(&view.ranked, buffer, cursor)
        })
    }

    /// Returns the single calm ghost candidate, if confidence is high enough.
    pub fn ghost(
        ctx: &CompletionContext,
        buffer: &str,
        cursor: usize,
    ) -> Option<CompletionCandidate> {
        if cursor < buffer.len() {
            return None;
        }
        if !token_allows(buffer, cursor) {
            return None;
        }
        let candidates = Self::complete(ctx, buffer, cursor);
        ghost_from(&candidates, buffer, cursor)
    }

    /// The append-only hint string for the ghost.
    pub fn ghost_hint(
        ctx: &CompletionContext,
        buffer: &str,
        cursor: usize,
    ) -> Option<(String, CompletionCandidate)> {
        let best = Self::ghost(ctx, buffer, cursor)?;
        ghost_hint_from(&best, buffer, cursor)
    }
}

// ---------------------------------------------------------------------------
// REEDLINE ADAPTER — the live session's one completer
// ---------------------------------------------------------------------------

/// Reedline adapter: the live session's completion source, backed by the
/// persistent M1 [`DiscoveryRuntime`].
///
/// - [`Completer::complete`] issues a completion **request**: dispatches
///   TIER 1B/2 work out-of-band, never waits, and answers `Fresh` or
///   `Stale`/`Pending` bound to the originating buffer/cursor.
/// - [`Completer::poll_completion`] reports background progress to the
///   Reedline event loop (`Idle` / `Pending` / `Ready`).
/// - The typed/ghost/readiness paths observe inline truth only (no dispatch,
///   no blocking) so repaints stay cheap.
pub struct OmenCompleter {
    context: Arc<Mutex<CompletionContext>>,
    runtime: Arc<Mutex<DiscoveryRuntime>>,
}

impl OmenCompleter {
    pub fn new(context: Arc<Mutex<CompletionContext>>) -> Self {
        Self {
            context,
            runtime: Arc::new(Mutex::new(DiscoveryRuntime::new())),
        }
    }

    pub fn context(&self) -> Arc<Mutex<CompletionContext>> {
        self.context.clone()
    }

    /// The session's persistent discovery runtime (registry, scheduler,
    /// caches, pending identity, telemetry).
    pub fn runtime(&self) -> Arc<Mutex<DiscoveryRuntime>> {
        self.runtime.clone()
    }

    fn snapshot(&self) -> LiveSnapshot {
        match self.context.lock() {
            Ok(ctx) => snapshot_from(&ctx),
            Err(e) => snapshot_from(&e.into_inner()),
        }
    }

    fn with_runtime<R>(&self, f: impl FnOnce(&mut DiscoveryRuntime) -> R) -> R {
        match self.runtime.lock() {
            Ok(mut rt) => f(&mut rt),
            Err(e) => f(&mut e.into_inner()),
        }
    }

    /// Read-only discovery view for the current origin (no dispatch).
    pub fn observe(&self, line: &str, pos: usize) -> crate::discovery::DiscoveryView {
        if !token_allows(line, pos) {
            return crate::discovery::DiscoveryView::empty_at(line, pos);
        }
        let snap = self.snapshot();
        self.with_runtime(|rt| rt.observe(line, pos.min(line.len()), &snap))
    }

    /// Completion request for the current origin (dispatches async work).
    pub fn request(&self, line: &str, pos: usize) -> crate::discovery::DiscoveryView {
        if !token_allows(line, pos) {
            return crate::discovery::DiscoveryView::empty_at(line, pos);
        }
        let snap = self.snapshot();
        self.with_runtime(|rt| rt.request(line, pos.min(line.len()), &snap))
    }

    /// Typed core result (machine-readable; no terminal scraping required).
    ///
    /// Non-blocking: observes inline + already-settled truth.
    pub fn complete_typed(&mut self, line: &str, pos: usize) -> Vec<CompletionCandidate> {
        let view = self.observe(line, pos);
        let descriptions = self
            .runtime
            .lock()
            .map(|rt| rt.config().descriptions)
            .unwrap_or(true);
        project_ranked(&view.ranked, line, pos, descriptions)
    }

    /// Tab readiness for the current origin: `FinalZero` vs `Ready(n)` vs
    /// `Pending` (async discovery in flight or triggered but not final).
    pub fn readiness(&mut self, line: &str, pos: usize) -> CompletionReadiness {
        let view = self.observe(line, pos);
        match view.phase {
            CompletionPhase::Fresh => {
                if view.ranked.is_empty() {
                    CompletionReadiness::FinalZero
                } else {
                    CompletionReadiness::Ready(view.ranked.len())
                }
            }
            CompletionPhase::Computing => CompletionReadiness::Pending {
                inline: view.ranked.len(),
            },
        }
    }

    /// The calm ghost over M1 truth (non-blocking observe path).
    pub fn ghost_view(&mut self, line: &str, pos: usize) -> Option<CompletionCandidate> {
        if pos < line.len() || !token_allows(line, pos) {
            return None;
        }
        let ghosts_enabled = self
            .runtime
            .lock()
            .map(|rt| rt.config().ghosts)
            .unwrap_or(true);
        if !ghosts_enabled {
            return None;
        }
        let candidates = self.complete_typed(line, pos);
        ghost_from(&candidates, line, pos)
    }

    /// The ghost's append-safe hint payload.
    pub fn ghost_view_hint(
        &mut self,
        line: &str,
        pos: usize,
    ) -> Option<(String, CompletionCandidate)> {
        let best = self.ghost_view(line, pos)?;
        ghost_hint_from(&best, line, pos)
    }

    /// Reedline-shaped suggestion projection for a presentation candidate.
    fn to_suggestion(c: CompletionCandidate, line_len: usize, pos: usize) -> Suggestion {
        let append_whitespace = c.edit.replacement_range.start == c.edit.replacement_range.end
            && c.kind != CandidateKind::Directory
            && c.kind != CandidateKind::Path
            && pos == line_len;
        let kind_label = kind_label(c.kind);
        let description = match &c.detail {
            Some(d) if d.as_str() == kind_label => Some(kind_label.to_string()),
            Some(d) => Some(format!("{kind_label}: {d}")),
            None => Some(kind_label.to_string()),
        };
        Suggestion {
            value: c.edit.insertion_text,
            description,
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
    }

    /// Reedline-shaped result for the typed candidate list.
    pub fn complete_items(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let line_len = line.len();
        self.complete_typed(line, pos)
            .into_iter()
            .map(|c| Self::to_suggestion(c, line_len, pos))
            .collect()
    }
}

fn kind_label(kind: CandidateKind) -> &'static str {
    match kind {
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
    }
}

impl Completer for OmenCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let view = self.request(line, pos);
        let descriptions = self
            .runtime
            .lock()
            .map(|rt| rt.config().descriptions)
            .unwrap_or(true);
        let line_len = line.len();
        let items: Vec<Suggestion> = project_ranked(&view.ranked, line, pos, descriptions)
            .into_iter()
            .map(|c| Self::to_suggestion(c, line_len, pos.min(line.len())))
            .collect();
        match view.phase {
            CompletionPhase::Fresh => CompletionResult::fresh(items),
            CompletionPhase::Computing => CompletionResult::stale_or_pending(
                items.into(),
                CompletionOrigin::new(line, pos.min(line.len())),
            ),
        }
    }

    fn poll_completion(&mut self) -> CompletionStatus {
        self.with_runtime(|rt| match rt.poll() {
            WorkStatus::Idle => CompletionStatus::Idle,
            WorkStatus::Pending => CompletionStatus::Pending,
            WorkStatus::Ready => CompletionStatus::Ready,
        })
    }
}

/// Ghost-suggestion hinter. Append-only; calm by construction.
///
/// Retains the current safe ghost suffix so Reedline's `HistoryHintComplete`
/// (Right / End) can insert it into the editable buffer via [`Hinter::complete_hint`].
/// The cached accept payload is cleared on every `handle()` call and is only
/// populated when the M1 ghost view approves a ghost. The visible ghost and
/// the accepted payload are always the same safe edit.
///
/// Also maintains the shared [`ReadinessCell`] that
/// [`crate::interaction::OmenEditMode`] consults to gate zero-candidate Tab.
pub struct OmenHinter {
    completer: Arc<Mutex<OmenCompleter>>,
    /// Raw unformatted safe ghost suffix approved by the M1 ghost view.
    current_hint: String,
    /// Shared readiness for Tab gating (updated on every repaint).
    candidate_readiness: ReadinessCell,
}

impl OmenHinter {
    pub fn new(completer: Arc<Mutex<OmenCompleter>>, candidate_readiness: ReadinessCell) -> Self {
        Self {
            completer,
            current_hint: String::new(),
            candidate_readiness,
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

        // Refresh the shared readiness so OmenEditMode can gate Tab (including
        // the pending state: zero inline + in-flight work is not a final zero).
        let readiness = match self.completer.lock() {
            Ok(mut c) => c.readiness(line, pos),
            Err(e) => e.into_inner().readiness(line, pos),
        };
        if let Ok(mut c) = self.candidate_readiness.lock() {
            *c = readiness;
        }

        if line.is_empty() {
            return String::new();
        }
        let mut comp = match self.completer.lock() {
            Ok(c) => c,
            Err(e) => e.into_inner(),
        };
        match comp.ghost_view_hint(line, pos) {
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
