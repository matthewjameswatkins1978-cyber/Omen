//! Reedline/interactive integration of the discovery substrate.
//!
//! This module owns the projection boundary between `omen-discovery` semantic
//! truth and the interactive presentation, plus the **persistent session
//! runtime** the live completer drives:
//!
//! ```text
//! grammar (one parser)  ->  ProviderContext
//! providers             ->  DiscoveredCandidate[]
//! merge + match + rank  ->  RankedCandidate[]      (DiscoveryView)
//! projection            ->  CompletionCandidate / reedline::Suggestion
//! ```
//!
//! The [`DiscoveryRuntime`] owns the session-scoped [`ProviderRegistry`],
//! [`Scheduler`] handle, caches, pending-request identity, telemetry and the
//! last completed result. Nothing rebuilds a registry or a worker per
//! keystroke; the shared scheduler worker parks when idle.
//!
//! Two request modes exist, and only one may touch the editor thread:
//!
//! - [`DiscoveryRuntime::request`] — the completion request (Tab / menu
//!   settle). Dispatches TIER 1B/2 work provider-faithfully and returns
//!   **without waiting** (`Fresh` or `Computing`).
//! - [`DiscoveryRuntime::observe`] — the read-only view for ghosts and Tab
//!   gating on every repaint. Never dispatches.
//!
//! Late results are bound to their originating buffer/cursor *and* the
//! operation they were dispatched under: a result from a superseded request is
//! dropped, never presented as current truth.
//!
//! Reedline owns interaction state (cursor, menu, Enter). Omen owns semantic
//! state (what the cursor means). Nothing here intercepts Enter.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use omen_discovery::budget::{CostTier, DiscoveryBudget};
use omen_discovery::candidate::{DiscoveredCandidate, RankedCandidate};
use omen_discovery::config::LensConfig;
use omen_discovery::context::{
    ActiveToken, CommandPosition, EnvFacts, ExecutableIdentity, ProjectContext, ProviderContext,
};
use omen_discovery::outcome::{DeclineReason, PartialReason, ProviderError};
use omen_discovery::provider::{DiscoveryProvider, ProviderId};
use omen_discovery::providers::{
    FilesystemPathProvider, HelpHarvestCache, HelpHarvestProvider, HotIndexProvider,
    HotIndexSnapshot, IntrinsicsProvider, OmenActionProvider, OmenKnowledge, PathCommandCache,
    PathCommandsProvider, ReferenceProvider, SubcommandProvider, ToolIdentity, ToolSpecProvider,
};
use omen_discovery::registry::{DiscoveryResult, ProviderRegistry};
use omen_discovery::scheduler::{Scheduler, WorkResult, WorkStatus};
use omen_discovery::telemetry::{Events, Telemetry};

use crate::commands;

/// Hard wall-clock bound for asynchronous completion work in tests and sync
/// tooling. The interactive path never waits; it polls.
const ASYNC_SETTLE_DEADLINE: Duration = Duration::from_secs(2);
/// After this long without a background result the request settles with the
/// evidence collected so far. Failure must return control (standing rule 2).
const ASYNC_ABANDON_AFTER: Duration = Duration::from_secs(3);

/// Process-wide help-harvest cache so TIER 2 harvests survive across
/// runtimes, keystrokes and tests.
fn global_harvest_cache() -> &'static HelpHarvestCache {
    static CACHE: std::sync::OnceLock<HelpHarvestCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(HelpHarvestCache::new)
}

/// Builds the canonical Omen knowledge from `crate::commands` (single source
/// of truth; no second handwritten action list).
pub fn omen_knowledge() -> Arc<OmenKnowledge> {
    Arc::new(OmenKnowledge {
        actions: commands::OMEN_ACTIONS,
        action_subcommands: commands::omen_action_subcommands,
        shell_intrinsics: commands::SHELL_INTRINSICS,
        tool_subcommands: commands::tool_subcommands,
        is_drive_designator: commands::is_drive_designator,
        typed_handles: crate::grammar::TypedReference::STATIC_HANDLES,
        action_reference_handles: commands::action_reference_handles,
    })
}

/// Identifies the harvest cache binding for the current platform.
fn current_tool_identity() -> ToolIdentity {
    ToolIdentity {
        path: "harvest".into(),
        digest: None,
        version: None,
        platform: if cfg!(windows) {
            "windows".into()
        } else if cfg!(target_os = "macos") {
            "macos".into()
        } else {
            "linux".into()
        },
        locale: "C".into(),
    }
}

// ---------------------------------------------------------------------------
// LIVE SNAPSHOT — what the interactive layer injects per request
// ---------------------------------------------------------------------------

/// Owned snapshot of the session state providers read.
///
/// Built by the completer from its [`crate::completion::CompletionContext`]
/// under one short lock, then handed to the runtime; the runtime never locks
/// interactive types itself.
#[derive(Debug, Clone, Default)]
pub struct LiveSnapshot {
    pub cwd: String,
    /// Bounded PATH command names (refreshed out-of-band).
    pub path_commands: Vec<String>,
    pub path_commands_truncated: bool,
    /// Bounded hot semantic index (facts, symbols, packages, tasks, services).
    pub hot: HotIndexSnapshot,
}

impl LiveSnapshot {
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// CONTEXT BUILDING — the ONE parser/semantic authority
// ---------------------------------------------------------------------------

/// Maps the grammar's cursor token into the discovery context's active token.
fn active_token_from(token: &crate::grammar::CursorToken) -> ActiveToken {
    ActiveToken {
        span: omen_discovery::candidate::TextSpan::new(token.span.start, token.span.end),
        decoded_prefix: token.decoded_prefix.clone(),
        decoded_suffix: token.decoded_suffix.clone(),
        literal: token.literal.clone(),
        raw_prefix: token.raw_prefix.clone(),
        raw_suffix: token.raw_suffix.clone(),
        split_unsafe: token.split_unsafe,
        quote_unclosed: token.quote_unclosed,
    }
}

/// Computes the semantic command position from the words before the cursor.
///
/// This is the ONE command-position authority; providers never re-derive it.
fn compute_command_position(buffer: &str, token: &crate::grammar::CursorToken) -> CommandPosition {
    let before = &buffer[..token.span.start.min(buffer.len())];
    let words = crate::grammar::GrammarScanner::split_words(before);
    if words.is_empty() {
        return CommandPosition::CommandName;
    }
    let first = words[0].clone();
    if first.starts_with(':') {
        return CommandPosition::ActionArg {
            action: first.trim_start_matches(':').to_string(),
        };
    }
    let chain: Vec<String> = words[1..]
        .iter()
        .filter(|w| !w.starts_with('-'))
        .cloned()
        .collect();
    let query = token.decoded_prefix.as_str();
    if query.starts_with('-') {
        CommandPosition::OptionName {
            command: first,
            chain,
        }
    } else {
        CommandPosition::ExecArg {
            command: first,
            chain,
            option_value_of: None,
        }
    }
}

/// Builds the bounded semantic context from the parsed input.
pub fn build_provider_context(buffer: &str, cursor: usize, cwd: &str) -> ProviderContext {
    let cursor = cursor.min(buffer.len());
    let token = match crate::grammar::token_at_cursor(buffer, cursor) {
        Some(t) => t,
        None => crate::grammar::CursorToken {
            span: cursor..cursor,
            decoded_prefix: String::new(),
            decoded_suffix: String::new(),
            quote_at_cursor: None,
            raw_prefix: String::new(),
            raw_suffix: String::new(),
            literal: String::new(),
            split_unsafe: false,
            quote_unclosed: false,
        },
    };

    let command_position = compute_command_position(buffer, &token);
    let active_token = active_token_from(&token);
    let replacement_span =
        omen_discovery::candidate::TextSpan::new(token.span.start, token.span.end);

    let known_executable = match &command_position {
        CommandPosition::ExecArg { command, .. } | CommandPosition::OptionName { command, .. } => {
            Some(ExecutableIdentity {
                name: command.clone(),
                path: None,
                version: None,
                digest: None,
            })
        }
        _ => None,
    };

    ProviderContext {
        buffer: buffer.to_string(),
        cursor,
        active_token,
        replacement_span,
        command_position,
        known_executable,
        working_dir: cwd.to_string(),
        project: ProjectContext::default(),
        env: EnvFacts {
            platform: if cfg!(windows) {
                "windows".into()
            } else {
                "unix".into()
            },
            path_var: std::env::var("PATH").ok(),
            shell: std::env::var("SHELL")
                .ok()
                .or_else(|| std::env::var("COMSPEC").ok()),
        },
        depth: omen_discovery::budget::DiscoveryDepth::Normal,
    }
}

// ---------------------------------------------------------------------------
// REQUEST / VIEW TYPES
// ---------------------------------------------------------------------------

/// Whether a view is authoritative yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionPhase {
    /// All applicable providers (inline and async) have answered for this
    /// exact origin. The view is final.
    Fresh,
    /// Background work remains for this origin (or is triggered but not yet
    /// dispatched). The view is the inline subset and may still grow.
    Computing,
}

/// One discovery view over an exact origin (buffer + cursor).
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryView {
    /// Ranked presentation candidates known so far.
    pub ranked: Vec<RankedCandidate>,
    /// Fresh vs still-computing.
    pub phase: CompletionPhase,
    /// Origin buffer these results were computed against.
    pub buffer: String,
    /// Origin cursor (insertion point) these results were computed against.
    pub cursor: usize,
    /// Background providers still outstanding for this request.
    pub deferred: Vec<ProviderId>,
    /// Background providers whose results are already collected for this
    /// request. They merge into `ranked` when the request completes (so a
    /// `Computing` view may list `arrived` entries not yet reflected there).
    pub arrived: Vec<(ProviderId, CostTier)>,
    /// Providers that produced candidates.
    pub answered: Vec<ProviderId>,
    /// Providers that deliberately declined.
    pub declined: Vec<(ProviderId, DeclineReason)>,
    /// Providers that returned bounded partial results.
    pub partial: Vec<(ProviderId, PartialReason)>,
    /// Providers that failed (isolated; never poisons siblings).
    pub failed: Vec<(ProviderId, ProviderError)>,
    /// Raw observations backing `ranked` (pre-merge), including observations
    /// merged away — so async re-merge never loses provenance.
    pub batch: Vec<DiscoveredCandidate>,
}

impl DiscoveryView {
    pub fn is_final(&self) -> bool {
        self.phase == CompletionPhase::Fresh
    }

    pub fn has_pending_work(&self) -> bool {
        !self.deferred.is_empty()
    }

    /// An empty, final view for an origin that cannot be completed at all
    /// (unsafe token / non-boundary cursor): a true zero, dispatch-free.
    pub fn empty_at(buffer: &str, cursor: usize) -> Self {
        Self {
            ranked: Vec::new(),
            phase: CompletionPhase::Fresh,
            buffer: buffer.to_string(),
            cursor,
            deferred: Vec::new(),
            arrived: Vec::new(),
            answered: Vec::new(),
            declined: Vec::new(),
            partial: Vec::new(),
            failed: Vec::new(),
            batch: Vec::new(),
        }
    }

    /// A *final* clean decline: no candidates, no failures, nothing pending.
    /// A Computing view with zero candidates is **not** a proven zero.
    pub fn is_final_zero(&self) -> bool {
        self.is_final() && self.ranked.is_empty() && self.failed.is_empty()
    }
}

/// An in-flight asynchronous request.
struct ActiveRequest {
    operation_id: u64,
    buffer: String,
    cursor: usize,
    ctx: ProviderContext,
    inline: DiscoveryResult,
    expected: Vec<ProviderId>,
    arrived: Vec<WorkResult>,
    started: Instant,
}

/// A completed (all-background-work-settled) request awaiting adoption.
struct CompletedRequest {
    buffer: String,
    cursor: usize,
    view: DiscoveryView,
    digested: bool,
}

// ---------------------------------------------------------------------------
// RUNTIME
// ---------------------------------------------------------------------------

/// Persistent session-scoped discovery state.
///
/// One runtime per completer/session: the registry's providers, the scheduler
/// handle, the caches, the pending-request identity, telemetry and the last
/// completed set all survive across keystrokes.
pub struct DiscoveryRuntime {
    registry: ProviderRegistry,
    path_cache: PathCommandCache,
    hot: Arc<Mutex<HotIndexSnapshot>>,
    config: LensConfig,
    active: Option<ActiveRequest>,
    completed: Option<CompletedRequest>,
}

impl DiscoveryRuntime {
    /// A runtime with the default M1 provider set and default config.
    pub fn new() -> Self {
        Self::with_config(LensConfig::default())
    }

    /// A runtime honouring `config` (e.g. `deterministic_only()` omits the
    /// TIER 2 harvest provider entirely — no harvest, no dispatch).
    pub fn with_config(config: LensConfig) -> Self {
        let knowledge = omen_knowledge();
        let hot = Arc::new(Mutex::new(HotIndexSnapshot::default()));
        let path_cache = PathCommandCache::new();
        let mut registry = ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(512));

        registry.register(Box::new(OmenActionProvider::new(knowledge.clone())));
        registry.register(Box::new(IntrinsicsProvider::new(knowledge.clone())));
        registry.register(Box::new(ReferenceProvider::new(knowledge.clone())));
        registry.register(Box::new(HotIndexProvider::new(hot.clone())));
        registry.register(Box::new(SubcommandProvider::new(knowledge.clone())));
        registry.register(Box::new(ToolSpecProvider::new(
            omen_discovery::providers::default_tool_specs(),
        )));
        registry.register(Box::new(PathCommandsProvider::new(path_cache.clone())));
        registry.register(Box::new(FilesystemPathProvider::new()));
        if config.tool_metadata_harvesting {
            let harvest = HelpHarvestProvider::new(global_harvest_cache().clone());
            harvest.cache().set_identity(current_tool_identity());
            registry.register(Box::new(harvest));
        }

        Self {
            registry,
            path_cache,
            hot,
            config,
            active: None,
            completed: None,
        }
    }

    /// Registers an additional provider (tests, future Machine Lens wiring).
    pub fn register_provider(&mut self, provider: Box<dyn DiscoveryProvider>) {
        self.registry.register(provider);
    }

    pub fn config(&self) -> &LensConfig {
        &self.config
    }

    pub fn registry(&self) -> &ProviderRegistry {
        &self.registry
    }

    pub fn telemetry(&self) -> &Telemetry {
        self.registry.telemetry()
    }

    pub fn scheduler(&self) -> &Scheduler {
        self.registry.scheduler()
    }

    /// Injects session state (PATH names, hot index) before a request.
    fn sync(&mut self, snap: &LiveSnapshot) {
        self.path_cache
            .set(snap.path_commands.clone(), snap.path_commands_truncated);
        if let Ok(mut h) = self.hot.lock() {
            *h = snap.hot.clone();
        }
    }

    /// Seeds the shared path cache from another cache (tests/fixtures).
    pub fn sync_path_from(&mut self, cache: &PathCommandCache) {
        self.path_cache.set(cache.names(), cache.truncated());
    }

    // ---- REQUEST (dispatching; interactive completion path) ----

    /// Runs a completion request for an exact origin.
    ///
    /// Never waits for background work: TIER 1B/2 providers are dispatched
    /// provider-faithfully and the returned view reports
    /// [`CompletionPhase::Computing`] until [`DiscoveryRuntime::poll`] settles
    /// the request. A repeat request for an identical origin adopts the
    /// completed result instead of redispatching.
    pub fn request(&mut self, buffer: &str, cursor: usize, snap: &LiveSnapshot) -> DiscoveryView {
        self.sync(snap);

        // Adopt a completed result for the exact same origin (also the
        // settle path: this is what breaks the dispatch loop).
        if let Some(done) = &mut self.completed
            && done.buffer == buffer
            && done.cursor == cursor
        {
            done.digested = true;
            return done.view.clone();
        }
        // Still computing for this exact origin: report inline state, do not
        // redispatch.
        if let Some(active) = &self.active
            && active.buffer == buffer
            && active.cursor == cursor
        {
            return computing_view(active);
        }

        // A different origin supersedes: cancel outstanding tracking (the
        // scheduler's latest-query-wins drops queued work; a running result
        // for the old operation is discarded by operation id).
        self.active = None;
        self.completed = None;

        let ctx = build_provider_context(buffer, cursor, &snap.cwd);
        let budget = DiscoveryBudget::allowing_subprocess();
        let (inline, mut deferred) = self.registry.discover_inline(&ctx, &budget);

        if deferred.is_empty() {
            return final_view(buffer, cursor, inline, Vec::new());
        }

        let operation_id = deferred[0].operation_id;
        let expected: Vec<ProviderId> = deferred.iter().map(|w| w.provider.clone()).collect();
        let async_budget = DiscoveryBudget::allowing_subprocess();
        for work in deferred.drain(..) {
            self.registry.dispatch(work, async_budget.clone());
        }

        let active = ActiveRequest {
            operation_id,
            buffer: buffer.to_string(),
            cursor,
            ctx,
            inline,
            expected,
            arrived: Vec::new(),
            started: Instant::now(),
        };
        let view = computing_view(&active);
        self.active = Some(active);
        view
    }

    // ---- OBSERVE (read-only; ghosts and Tab gating on repaint) ----

    /// The read-only truth view for `buffer`/`cursor`.
    ///
    /// Never dispatches background work. Uses the completed result when the
    /// origin matches, the in-flight inline state when it is computing, and
    /// otherwise runs only inline providers. A view whose inline run still
    /// *triggers* async providers reports [`CompletionPhase::Computing`] —
    /// zero candidates + pending work is **not** a final zero.
    pub fn observe(&mut self, buffer: &str, cursor: usize, snap: &LiveSnapshot) -> DiscoveryView {
        self.sync(snap);

        if let Some(done) = &self.completed
            && done.buffer == buffer
            && done.cursor == cursor
        {
            return done.view.clone();
        }
        if let Some(active) = &self.active
            && active.buffer == buffer
            && active.cursor == cursor
        {
            return computing_view(active);
        }

        let ctx = build_provider_context(buffer, cursor, &snap.cwd);
        let budget = DiscoveryBudget::allowing_subprocess();
        let (inline, deferred) = self.registry.discover_inline(&ctx, &budget);
        let pending: Vec<ProviderId> = deferred.iter().map(|w| w.provider.clone()).collect();
        let phase = if pending.is_empty() {
            CompletionPhase::Fresh
        } else {
            CompletionPhase::Computing
        };
        DiscoveryView {
            ranked: inline.ranked,
            phase,
            buffer: buffer.to_string(),
            cursor,
            deferred: pending,
            arrived: Vec::new(),
            answered: inline.answered,
            declined: inline.declined,
            partial: inline.partial,
            failed: inline.failed,
            batch: inline.batch,
        }
    }

    // ---- POLL (the Reedline poll_completion seam) ----

    /// Collects background results for the live request.
    ///
    /// Returns [`WorkStatus::Ready`] exactly when the active request has
    /// settled (all expected results arrived, or the abandon deadline
    /// elapsed) or an unadopted completed result is waiting. Results for
    /// superseded operations are dropped.
    pub fn poll(&mut self) -> WorkStatus {
        let drained = self.registry.scheduler().drain();

        if let Some(active) = &mut self.active {
            for wr in drained {
                if wr.operation_id != active.operation_id {
                    // Late result from a superseded request: dropped, never
                    // presented as current truth.
                    continue;
                }
                active.arrived.push(wr);
            }

            let arrived_ids: Vec<&ProviderId> =
                active.arrived.iter().map(|w| &w.provider).collect();
            let pending = active
                .expected
                .iter()
                .filter(|p| !arrived_ids.contains(p))
                .count();
            let abandoned = active.started.elapsed() >= ASYNC_ABANDON_AFTER;

            if pending == 0 || abandoned {
                let finished = self.active.take().expect("active request");
                let view = self.finish(finished);
                self.completed = Some(CompletedRequest {
                    buffer: view.buffer.clone(),
                    cursor: view.cursor,
                    view,
                    digested: false,
                });
                return WorkStatus::Ready;
            }
            return WorkStatus::Pending;
        }

        // No active request. Drop stray results (they belong to cancelled
        // operations) and surface an unadopted completed result.
        let _ = drained;
        match &self.completed {
            Some(done) if !done.digested => WorkStatus::Ready,
            _ => WorkStatus::Idle,
        }
    }

    /// Settles an active request: folds every arrived background result into
    /// the preserved raw batch and re-runs the one pipeline.
    fn finish(&mut self, active: ActiveRequest) -> DiscoveryView {
        let mut batch = active.inline.batch.clone();
        let mut answered = active.inline.answered.clone();
        let mut declined = active.inline.declined.clone();
        let mut partial = active.inline.partial.clone();
        let mut failed = active.inline.failed.clone();
        let mut arrived = Vec::new();

        for wr in &active.arrived {
            arrived.push((wr.provider.clone(), wr.tier));
            self.registry.telemetry().record(Events::provider_finished(
                active.operation_id,
                &wr.provider,
                wr.tier,
                wr.outcome.candidates().len(),
                wr.latency,
                match &wr.outcome {
                    omen_discovery::outcome::ProviderOutcome::Answered { .. } => "answered",
                    omen_discovery::outcome::ProviderOutcome::Declined { .. } => "declined",
                    omen_discovery::outcome::ProviderOutcome::Partial { .. } => "partial",
                    omen_discovery::outcome::ProviderOutcome::Failed { .. } => "failed",
                },
            ));
            match &wr.outcome {
                omen_discovery::outcome::ProviderOutcome::Answered { candidates } => {
                    answered.push(wr.provider.clone());
                    batch.extend(candidates.iter().cloned());
                }
                omen_discovery::outcome::ProviderOutcome::Declined { reason } => {
                    declined.push((wr.provider.clone(), *reason));
                }
                omen_discovery::outcome::ProviderOutcome::Partial { candidates, reason } => {
                    partial.push((wr.provider.clone(), *reason));
                    batch.extend(candidates.iter().cloned());
                }
                omen_discovery::outcome::ProviderOutcome::Failed { error } => {
                    failed.push((wr.provider.clone(), error.clone()));
                }
            }
        }

        let ranked = self.registry.rank_batch(batch.clone(), &active.ctx);
        DiscoveryView {
            ranked,
            phase: CompletionPhase::Fresh,
            buffer: active.buffer,
            cursor: active.cursor,
            deferred: Vec::new(),
            arrived,
            answered,
            declined,
            partial,
            failed,
            batch,
        }
    }

    // ---- PUMP (tests / sync tooling only — never the editor path) ----

    /// Requests and polls until the origin settles or `deadline` elapses.
    ///
    /// This is the synchronous convenience used by tests and the static
    /// [`crate::completion::CompletionEngine`] API. The live completer never
    /// calls it: it returns `Fresh`/`Computing` and relies on
    /// [`DiscoveryRuntime::poll`].
    pub fn pump(
        &mut self,
        buffer: &str,
        cursor: usize,
        snap: &LiveSnapshot,
        deadline: Duration,
    ) -> DiscoveryView {
        let started = Instant::now();
        let mut view = self.request(buffer, cursor, snap);
        while view.phase == CompletionPhase::Computing && started.elapsed() < deadline {
            self.poll();
            std::thread::sleep(Duration::from_millis(1));
            view = self.request(buffer, cursor, snap);
        }
        view
    }

    /// Deadline used by [`DiscoveryRuntime::pump`] callers by default.
    pub fn settle_deadline() -> Duration {
        ASYNC_SETTLE_DEADLINE
    }
}

impl Default for DiscoveryRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn computing_view(active: &ActiveRequest) -> DiscoveryView {
    let arrived: Vec<(ProviderId, CostTier)> = active
        .arrived
        .iter()
        .map(|w| (w.provider.clone(), w.tier))
        .collect();
    let arrived_ids: Vec<&ProviderId> = active.arrived.iter().map(|w| &w.provider).collect();
    let deferred: Vec<ProviderId> = active
        .expected
        .iter()
        .filter(|p| !arrived_ids.contains(p))
        .cloned()
        .collect();

    DiscoveryView {
        ranked: active.inline.ranked.clone(),
        phase: CompletionPhase::Computing,
        buffer: active.buffer.clone(),
        cursor: active.cursor,
        deferred,
        arrived,
        answered: active.inline.answered.clone(),
        declined: active.inline.declined.clone(),
        partial: active.inline.partial.clone(),
        failed: active.inline.failed.clone(),
        batch: active.inline.batch.clone(),
    }
}

fn final_view(
    buffer: &str,
    cursor: usize,
    inline: DiscoveryResult,
    arrived: Vec<(ProviderId, CostTier)>,
) -> DiscoveryView {
    DiscoveryView {
        ranked: inline.ranked,
        phase: CompletionPhase::Fresh,
        buffer: buffer.to_string(),
        cursor,
        deferred: Vec::new(),
        arrived,
        answered: inline.answered,
        declined: inline.declined,
        partial: inline.partial,
        failed: inline.failed,
        batch: inline.batch,
    }
}

// ---------------------------------------------------------------------------
// ONE-SHOT CONVENIENCE (tests and reports)
// ---------------------------------------------------------------------------

/// One discovery run's inputs, derived from the single input grammar.
pub struct DiscoveryRequest<'a> {
    pub buffer: &'a str,
    pub cursor: usize,
    pub cwd: &'a str,
    pub path_cache: &'a PathCommandCache,
}

/// Runs the discovery pipeline to a settled result for one request.
///
/// Builds a runtime, pumps background work to completion (bounded) and
/// returns the final view as a [`DiscoveryResult`]. This is a test/reporting
/// convenience; the live completer uses [`DiscoveryRuntime`] directly.
pub fn run_discovery(req: &DiscoveryRequest<'_>) -> DiscoveryResult {
    let mut runtime = DiscoveryRuntime::new();
    let snap = LiveSnapshot {
        cwd: req.cwd.to_string(),
        path_commands: req.path_cache.names(),
        path_commands_truncated: req.path_cache.truncated(),
        hot: HotIndexSnapshot::default(),
    };
    let view = runtime.pump(
        req.buffer,
        req.cursor,
        &snap,
        DiscoveryRuntime::settle_deadline(),
    );
    DiscoveryResult {
        ranked: view.ranked,
        declined: view.declined,
        failed: view.failed,
        partial: view.partial,
        answered: view.answered,
        batch: view.batch,
    }
}

/// Machine-serialisable projection of a ranked set.
///
/// Preserves the same [`SemanticKey`](omen_discovery::identity::SemanticKey)s,
/// authority truth, supporting provenance and replacement spans as the human
/// projection. The public Machine Lens command is M2+; this proves the
/// internal representation is ready without shipping it.
pub fn to_machine_json(ranked: &[RankedCandidate]) -> String {
    serde_json::to_string(ranked).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omen_discovery::authority::{Authority, AuthorityClass};
    use omen_discovery::candidate::{Description, DiscoveredCandidate as Cand, TextSpan};
    use omen_discovery::identity::{
        EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace,
    };
    use omen_discovery::kind::Kind;
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn path_cache(cmds: &[&str]) -> PathCommandCache {
        let c = PathCommandCache::new();
        c.set(cmds.iter().map(|s| s.to_string()).collect(), false);
        c
    }

    fn req<'a>(
        buffer: &'a str,
        cursor: usize,
        cwd: &'a str,
        cache: &'a PathCommandCache,
    ) -> DiscoveryRequest<'a> {
        DiscoveryRequest {
            buffer,
            cursor,
            cwd,
            path_cache: cache,
        }
    }

    fn snapshot(cache: &PathCommandCache, cwd: &str) -> LiveSnapshot {
        LiveSnapshot {
            cwd: cwd.to_string(),
            path_commands: cache.names(),
            path_commands_truncated: cache.truncated(),
            hot: HotIndexSnapshot::default(),
        }
    }

    fn runtime_with_path(cache: &PathCommandCache) -> DiscoveryRuntime {
        let mut rt = DiscoveryRuntime::new();
        // The registered path provider shares `rt.path_cache`, so syncing the
        // snapshot's names updates what the provider reads.
        rt.sync_path_from(cache);
        rt
    }

    // ---- HEADCOUNT / CONTEXT ----

    #[test]
    fn command_name_position_offers_omen_actions_and_commands() {
        let cache = path_cache(&["cargo", "git"]);
        let r = run_discovery(&req("ca", 2, ".", &cache));
        let kinds: Vec<Kind> = r.ranked.iter().map(|x| x.semantic().kind).collect();
        assert!(
            kinds.contains(&Kind::Command),
            "PATH command surfaced: {kinds:?}"
        );
    }

    #[test]
    fn option_name_position_uses_tool_context() {
        let cache = path_cache(&["git"]);
        let r = run_discovery(&req("git --ver", 9, ".", &cache));
        assert!(!r.ranked.is_empty());
        for c in &r.ranked {
            assert!(c.value().insert.starts_with("--"), "{:?}", c.value());
        }
    }

    #[test]
    fn zero_candidate_is_clean_decline() {
        let cache = path_cache(&[]);
        let r = run_discovery(&req("zzzznothingmatches", 18, ".", &cache));
        assert!(r.ranked.is_empty());
        assert!(r.is_clean_decline());
    }

    #[test]
    fn context_builder_derives_option_name_position() {
        let ctx = build_provider_context("git --ver", 9, ".");
        assert!(ctx.is_option_name_position());
        assert_eq!(ctx.tool_in_scope(), Some("git"));
    }

    // ---- HEADLINE ACCEPTANCE FLOWS ----

    #[test]
    fn flow_a_git_options_with_descriptions() {
        let cache = path_cache(&["git"]);
        let r = run_discovery(&req("git --", 6, ".", &cache));
        assert!(
            !r.ranked.is_empty(),
            "git --<Tab> must surface real options"
        );
        let mut saw_description = false;
        for c in &r.ranked {
            assert_eq!(c.semantic().kind, Kind::Option);
            assert!(c.value().insert.starts_with("--"), "{:?}", c.value());
            assert!(matches!(
                c.authority(),
                Authority::OmenFact { .. }
                    | Authority::ToolNative { .. }
                    | Authority::HelpHarvest { .. }
                    | Authority::InstalledSpec { .. }
            ));
            if c.matched.discovered.description.is_some() {
                saw_description = true;
            }
        }
        assert!(saw_description, "real authority provides descriptions");
    }

    #[test]
    fn flow_b_cargo_subcommands() {
        let cache = path_cache(&["cargo"]);
        let r = run_discovery(&req("cargo ", 6, ".", &cache));
        assert!(!r.ranked.is_empty(), "cargo <Tab> must surface subcommands");
        let subcommands: Vec<String> = r
            .ranked
            .iter()
            .filter(|c| c.semantic().kind == Kind::Subcommand)
            .map(|c| c.value().insert.clone())
            .collect();
        assert!(
            subcommands.iter().any(|s| s == "build") && subcommands.iter().any(|s| s == "test"),
            "cargo subcommands surfaced: {subcommands:?}"
        );
    }

    /// Flow D shape against **current** canonical truth (H2 has not merged):
    /// the Omen action surface comes from the shared authority, not a stale
    /// list. Re-prove `omen authority <Tab>` after H2 reconciliation.
    #[test]
    fn flow_d_omen_action_surface_comes_from_canonical_truth() {
        let cache = path_cache(&[]);
        let r = run_discovery(&req(":sta", 4, ".", &cache));
        let actions: Vec<String> = r
            .ranked
            .iter()
            .filter(|c| c.semantic().kind == Kind::OmenAction)
            .map(|c| c.value().insert.clone())
            .collect();
        assert!(
            actions.contains(&":status".to_string()),
            "canonical action surfaced: {actions:?}"
        );
        for a in &actions {
            let name = a.trim_start_matches(':');
            assert!(
                commands::OMEN_ACTIONS.contains(&name),
                "{a} must come from the shared action authority"
            );
        }
    }

    #[test]
    fn flow_e_long_tail_remains_reachable_and_deterministic() {
        let cache = path_cache(&["git"]);
        let a = run_discovery(&req("git --", 6, ".", &cache));
        let b = run_discovery(&req("git --", 6, ".", &cache));
        let names = |r: &DiscoveryResult| -> Vec<String> {
            r.ranked.iter().map(|c| c.value().insert.clone()).collect()
        };
        assert_eq!(names(&a), names(&b), "same input => same order");
        assert_eq!(a.ranked[0].value().insert, "--verbose");
    }

    #[test]
    fn flow_f_mistyped_command_declines_without_fuzzy_invention() {
        let cache = path_cache(&["cargo", "git"]);
        let r = run_discovery(&req("mistypedcomm", 12, ".", &cache));
        assert!(
            r.ranked.is_empty(),
            "M1 deterministic mode invents nothing for a mistyped command: {:?}",
            r.ranked
                .iter()
                .map(|c| &c.value().insert)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn flow_h_multi_provider_collision_merges_semantic_candidate() {
        let cache = path_cache(&["git"]);
        let r = run_discovery(&req("git --verbose", 13, ".", &cache));
        let matches: Vec<_> = r
            .ranked
            .iter()
            .filter(|c| c.value().insert == "--verbose")
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "same semantic candidate merges, never duplicates"
        );
    }

    // ---- INTEGRATED MULTI-PROVIDER PIPELINE PROOF (Flow H, whole pipeline) ----

    /// Deterministic fixture provider emitting fixed candidates.
    struct FixtureProvider {
        id: &'static str,
        tier: CostTier,
        candidates: Vec<Cand>,
    }

    impl DiscoveryProvider for FixtureProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new(self.id)
        }
        fn supported_kinds(&self) -> &'static [Kind] {
            &[Kind::Option, Kind::Subcommand]
        }
        fn triggers(&self, _ctx: &ProviderContext) -> omen_discovery::provider::TriggerDecision {
            omen_discovery::provider::TriggerDecision::Apply
        }
        fn cost_tier(&self) -> CostTier {
            self.tier
        }
        fn determinism(&self) -> omen_discovery::provider::Determinism {
            omen_discovery::provider::Determinism::Deterministic
        }
        fn authority_capabilities(&self) -> &'static [AuthorityClass] {
            &[AuthorityClass::ToolNative, AuthorityClass::HelpHarvest]
        }
        fn freshness(&self) -> omen_discovery::provider::FreshnessPolicy {
            omen_discovery::provider::FreshnessPolicy::Never
        }
        fn discover(
            &mut self,
            ctx: &ProviderContext,
            _b: &DiscoveryBudget,
        ) -> omen_discovery::outcome::ProviderOutcome {
            let mut out = Vec::new();
            for c in &self.candidates {
                let mut c = c.clone();
                c.replacement_span = ctx.replacement_span;
                out.push(c);
            }
            if out.is_empty() {
                omen_discovery::outcome::ProviderOutcome::Declined {
                    reason: DeclineReason::NoMatch,
                }
            } else {
                omen_discovery::outcome::ProviderOutcome::Answered { candidates: out }
            }
        }
    }

    fn fixture_option(
        provider: &str,
        value: &str,
        class: AuthorityClass,
        authority: Authority,
        description: Option<Description>,
    ) -> Cand {
        let mut c = Cand::new(
            SemanticKey::new(
                Kind::Option,
                value,
                SemanticNamespace::Tool { tool: "git".into() },
            ),
            ProvenanceKey::new(provider, class, EvidenceIdentity::Static),
            value,
            authority,
            TextSpan::at(0),
        );
        if let Some(d) = description {
            c = c.with_description(d);
        }
        c
    }

    #[test]
    fn integrated_multi_provider_collision_survives_to_ranked_state() {
        // ONE request: provider A (stronger) and provider B observe the SAME
        // SemanticKey with different provenance and different descriptions.
        let mut rt = DiscoveryRuntime::with_config(LensConfig::deterministic_only());
        rt.register_provider(Box::new(FixtureProvider {
            id: "fixture-a",
            tier: CostTier::MemoryOnly,
            candidates: vec![fixture_option(
                "fixture-a",
                "--probe",
                AuthorityClass::ToolNative,
                Authority::ToolNative {
                    tool: "git".into(),
                    spec_version: None,
                },
                Some(Description::short("A description")),
            )],
        }));
        rt.register_provider(Box::new(FixtureProvider {
            id: "fixture-b",
            tier: CostTier::MemoryOnly,
            candidates: vec![fixture_option(
                "fixture-b",
                "--probe",
                AuthorityClass::HelpHarvest,
                Authority::HelpHarvest {
                    tool: "git".into(),
                    tool_version: None,
                    harvested_at_unix: 0,
                },
                Some(Description::short("B description")),
            )],
        }));

        let snap = LiveSnapshot::new(".");
        let view = rt.observe("git --probe", 11, &snap);

        let probes: Vec<&RankedCandidate> = view
            .ranked
            .iter()
            .filter(|c| c.value().insert == "--probe")
            .collect();
        assert_eq!(probes.len(), 1, "one semantic candidate, no duplicate UI");
        let probe = probes[0];

        // Primary evidence = stronger authority (ToolNative).
        assert!(
            matches!(probe.authority(), Authority::ToolNative { .. }),
            "strongest validity evidence is primary"
        );
        // Supporting provenance preserved THROUGH ranking.
        assert_eq!(probe.supporting.len(), 1);
        assert_eq!(
            probe.supporting[0].provenance.provider.as_str(),
            "fixture-b",
            "both provenance identities inspectable"
        );
        assert!(matches!(
            probe.supporting[0].authority,
            Authority::HelpHarvest { .. }
        ));
        // Deterministic merged description: primary wins wholesale.
        assert_eq!(
            probe.matched.discovered.description.as_ref().unwrap().short,
            "A description"
        );
        // Supporting description preserved on the evidence itself.
        assert_eq!(
            probe.supporting[0].description.as_ref().unwrap().short,
            "B description"
        );
        // Human projection: exactly one suggestion for --probe. (The query is
        // partial so the grammar-validated edit is non-degenerate.)
        let human = crate::completion::project_ranked_default(&view.ranked, "git --pro", 9);
        assert_eq!(
            human.iter().filter(|c| c.literal == "--probe").count(),
            1,
            "no duplicate UI suggestion"
        );
        // Machine projection carries the same evidence.
        let machine: Vec<RankedCandidate> =
            serde_json::from_str(&to_machine_json(&view.ranked)).unwrap();
        let m_probe = machine
            .iter()
            .find(|c| c.value().insert == "--probe")
            .unwrap();
        assert_eq!(m_probe.supporting, probe.supporting);

        // Different semantic key, same text: stays distinct.
        rt.register_provider(Box::new(FixtureProvider {
            id: "fixture-c",
            tier: CostTier::MemoryOnly,
            candidates: vec![{
                let mut c = fixture_option(
                    "fixture-c",
                    "--probe",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                    None,
                );
                c.semantic = SemanticKey::new(
                    Kind::Subcommand,
                    "--probe",
                    SemanticNamespace::Tool { tool: "git".into() },
                );
                c
            }],
        }));
        let view2 = rt.observe("git --probe", 11, &snap);
        assert_eq!(
            view2
                .ranked
                .iter()
                .filter(|c| c.value().insert == "--probe")
                .count(),
            2,
            "different Kind with same text remains a different semantic object"
        );
    }

    // ---- ASYNC REAL-PATH PROOF (slow provider, origin binding) ----

    /// Provider that blocks until released, then emits for the buffer it saw.
    struct GatedProvider {
        started: Arc<AtomicBool>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        /// Sentinel emitted only when the request buffer starts with this.
        emit_for_prefix: &'static str,
        sentinel: &'static str,
    }

    impl DiscoveryProvider for GatedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("gated-slow")
        }
        fn supported_kinds(&self) -> &'static [Kind] {
            &[Kind::Option]
        }
        fn triggers(&self, ctx: &ProviderContext) -> omen_discovery::provider::TriggerDecision {
            if matches!(ctx.command_position, CommandPosition::OptionName { .. }) {
                omen_discovery::provider::TriggerDecision::Apply
            } else {
                omen_discovery::provider::TriggerDecision::Skip
            }
        }
        fn cost_tier(&self) -> CostTier {
            CostTier::BlockingLocal
        }
        fn determinism(&self) -> omen_discovery::provider::Determinism {
            omen_discovery::provider::Determinism::Deterministic
        }
        fn authority_capabilities(&self) -> &'static [AuthorityClass] {
            &[AuthorityClass::Static]
        }
        fn freshness(&self) -> omen_discovery::provider::FreshnessPolicy {
            omen_discovery::provider::FreshnessPolicy::Never
        }
        fn discover(
            &mut self,
            ctx: &ProviderContext,
            _b: &DiscoveryBudget,
        ) -> omen_discovery::outcome::ProviderOutcome {
            self.started.store(true, Ordering::SeqCst);
            let (m, cv) = &*self.gate;
            let mut released = m.lock().unwrap_or_else(|e| e.into_inner());
            let deadline = Instant::now() + Duration::from_secs(5);
            while !*released && Instant::now() < deadline {
                released = cv
                    .wait_timeout(released, Duration::from_millis(50))
                    .map(|(g, _)| g)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
            if !ctx.buffer.starts_with(self.emit_for_prefix) {
                return omen_discovery::outcome::ProviderOutcome::Declined {
                    reason: DeclineReason::NoMatch,
                };
            }
            let span = ctx.replacement_span;
            omen_discovery::outcome::ProviderOutcome::Answered {
                candidates: vec![Cand::new(
                    SemanticKey::new(
                        Kind::Option,
                        self.sentinel,
                        SemanticNamespace::Tool { tool: "git".into() },
                    ),
                    ProvenanceKey::new(
                        "gated-slow",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    self.sentinel,
                    Authority::Static,
                    span,
                )],
            }
        }
    }

    fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The shared scheduler worker executes one job at a time; gated fixtures
    /// must not run concurrently or they would serialise against each other's
    /// gates. One gated test at a time.
    fn gated_serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn gated_runtime(
        gate: &Arc<(Mutex<bool>, Condvar)>,
        started: &Arc<AtomicBool>,
    ) -> DiscoveryRuntime {
        let mut rt = DiscoveryRuntime::with_config(LensConfig::deterministic_only());
        rt.register_provider(Box::new(GatedProvider {
            started: started.clone(),
            gate: gate.clone(),
            emit_for_prefix: "git --aaa",
            sentinel: "--aaa-late",
        }));
        rt
    }

    #[test]
    fn slow_provider_completes_out_of_band_and_origin_still_current() {
        let _serial = gated_serial();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new(AtomicBool::new(false));
        let mut rt = gated_runtime(&gate, &started);
        let snap = LiveSnapshot::new(".");

        // 1. Request begins; the editor thread gets control back immediately.
        let v1 = rt.request("git --aaa", 9, &snap);
        assert_eq!(
            v1.phase,
            CompletionPhase::Computing,
            "request returns without waiting for the slow provider"
        );
        assert_eq!(
            v1.deferred.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            ["gated-slow"]
        );
        assert!(v1.ranked.is_empty(), "nothing inline at an unknown option");

        // 2. The provider really started (out of band) and is still gated:
        //    polling reports Pending, not Ready.
        wait_until(|| started.load(Ordering::SeqCst), "slow provider to start");
        assert_eq!(rt.poll(), WorkStatus::Pending);

        // 3. Provider completes later; poll reports Ready; origin unchanged.
        *gate.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wait_until(|| rt.poll() == WorkStatus::Ready, "poll to report Ready");

        let v2 = rt.request("git --aaa", 9, &snap);
        assert_eq!(v2.phase, CompletionPhase::Fresh);
        assert!(
            v2.ranked.iter().any(|c| c.value().insert == "--aaa-late"),
            "candidate appears once async truth arrives for a still-current origin"
        );
        assert_eq!(v2.deferred, Vec::<ProviderId>::new());
        assert!(
            v2.arrived
                .iter()
                .any(|(p, t)| p.as_str() == "gated-slow" && *t == CostTier::BlockingLocal),
            "real provider identity and tier on arrival"
        );
        // Settled: nothing left to poll.
        assert_eq!(rt.poll(), WorkStatus::Idle);
    }

    #[test]
    fn stale_result_from_superseded_request_is_never_served_as_current() {
        let _serial = gated_serial();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new(AtomicBool::new(false));
        let mut rt = gated_runtime(&gate, &started);
        let snap = LiveSnapshot::new(".");

        // Start request A (gated); wait until its provider is mid-discover.
        let va = rt.request("git --aaa", 9, &snap);
        assert_eq!(va.phase, CompletionPhase::Computing);
        wait_until(
            || started.load(Ordering::SeqCst),
            "slow provider A to start",
        );

        // The user edits the buffer: request B supersedes A.
        let vb = rt.request("git --bbb", 9, &snap);
        assert_eq!(
            vb.phase,
            CompletionPhase::Computing,
            "B dispatched its own work"
        );
        assert!(
            vb.ranked.is_empty(),
            "B has not been served A's inline truth"
        );

        // A's gated work finishes now (the buffer it saw was A's).
        *gate.0.lock().unwrap_or_else(|e| e.into_inner()) = true;

        // Drain every background result deterministically (bounded).
        wait_until(
            || rt.scheduler().outstanding() == 0 && rt.poll() != WorkStatus::Pending,
            "all background work to drain",
        );

        // B settles WITHOUT A's sentinel: A's operation result was dropped.
        let settled = rt.request("git --bbb", 9, &snap);
        assert_eq!(
            settled.phase,
            CompletionPhase::Fresh,
            "B's own work settled"
        );
        assert!(
            !settled
                .ranked
                .iter()
                .any(|c| c.value().insert == "--aaa-late"),
            "late result from a superseded request must not serve as current truth: {:?}",
            settled
                .ranked
                .iter()
                .map(|c| c.value().insert.clone())
                .collect::<Vec<_>>()
        );
        // Stronger: the operation filter drops the arrival entirely — A's
        // observation never even enters B's batch (the matcher could not have
        // matched `--aaa-late` against `--bbb` anyway).
        assert!(
            !settled.batch.iter().any(|c| c.value.insert == "--aaa-late"),
            "superseded observation must not enter the surviving request's evidence batch"
        );
        // Nothing outstanding, nothing unadopted: no phantom Ready.
        assert_eq!(rt.poll(), WorkStatus::Idle);
    }

    // ---- PENDING VS FINAL ZERO (runtime level) ----

    #[test]
    fn pending_discovery_is_not_a_final_zero() {
        // Option-name position on an unknown tool: nothing inline, but the
        // TIER 2 harvest applies — discovery is NOT final.
        let cache = path_cache(&[]);
        let mut rt = runtime_with_path(&cache);
        let snap = snapshot(&cache, ".");
        let v = rt.observe("unknown_tool --xyz", 18, &snap);
        assert!(v.ranked.is_empty());
        assert_eq!(v.phase, CompletionPhase::Computing);
        assert_eq!(
            v.deferred.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            ["help-harvest"],
            "would-trigger work is pending, not proven zero"
        );
        assert!(!v.is_final_zero(), "zero + pending work is not final zero");

        // Command-name position with no async applicability: final zero.
        let z = rt.observe("zzzz", 4, &snap);
        assert!(z.ranked.is_empty());
        assert_eq!(z.phase, CompletionPhase::Fresh);
        assert!(z.deferred.is_empty());
        assert!(z.is_final_zero(), "true zero declines cleanly");
    }

    #[test]
    fn observe_never_dispatches_background_work() {
        let cache = path_cache(&[]);
        let mut rt = runtime_with_path(&cache);
        let snap = snapshot(&cache, ".");
        let _ = rt.observe("unknown_tool --xyz", 18, &snap);
        assert_eq!(
            rt.scheduler().outstanding(),
            0,
            "observe is read-only: no dispatch, no worker jobs"
        );
    }

    // ---- HELP HARVEST INTEGRATED ASYNC PROOF ----

    #[test]
    fn help_harvest_runs_through_the_deferred_path_with_subprocess_tier() {
        // `cargo` is guaranteed wherever `cargo test` runs.
        let cache = path_cache(&["cargo"]);
        let mut rt = runtime_with_path(&cache);
        let snap = snapshot(&cache, ".");

        let t0 = Instant::now();
        let v1 = rt.request("cargo --", 8, &snap);
        let request_latency = t0.elapsed();
        assert_eq!(v1.phase, CompletionPhase::Computing);
        assert_eq!(
            v1.deferred.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            ["help-harvest"],
            "help-harvest deferred (OptionName skips filesystem)"
        );
        assert!(
            request_latency < Duration::from_millis(500),
            "request must not wait for the harvest subprocess (took {request_latency:?})"
        );

        // Settle through the real async path (pump is the test-side poll).
        let v2 = rt.pump("cargo --", 8, &snap, Duration::from_secs(10));
        assert_eq!(v2.phase, CompletionPhase::Fresh);
        assert_eq!(v2.deferred, Vec::<ProviderId>::new());
        assert_eq!(
            v2.arrived
                .iter()
                .map(|(p, _)| p.as_str())
                .collect::<Vec<_>>(),
            ["help-harvest"],
            "the REAL help-harvest work ran — no filesystem substitution"
        );
        assert_eq!(
            v2.arrived[0].1,
            CostTier::Subprocess,
            "tier stays Subprocess"
        );

        // Candidates carry HelpHarvest authority (cargo has options outside
        // Omen's owned spec: --locked, --offline, ...).
        let harvest_authority = v2
            .ranked
            .iter()
            .any(|c| matches!(c.authority(), Authority::HelpHarvest { .. }))
            || v2.ranked.iter().any(|c| {
                c.supporting
                    .iter()
                    .any(|s| matches!(s.authority, Authority::HelpHarvest { .. }))
            });
        assert!(
            harvest_authority,
            "harvested evidence visible in the final ranked set"
        );

        // Cache path used on the repeat request: no second dispatch.
        let v3 = rt.request("cargo --", 8, &snap);
        assert_eq!(v3.phase, CompletionPhase::Fresh, "completed result adopted");
        let _ = rt.observe("cargo --", 8, &snap);
        assert_eq!(rt.scheduler().outstanding(), 0, "no redispatch after adopt");
    }

    #[test]
    fn prose_help_harvest_declines_rather_than_guessing() {
        // `git --help` is prose with zero strict option lines: the harvest
        // must decline through the integrated path (fail-closed).
        let cache = path_cache(&["git"]);
        let mut rt = runtime_with_path(&cache);
        let snap = snapshot(&cache, ".");
        let v = rt.pump("git --", 6, &snap, Duration::from_secs(10));
        assert_eq!(v.phase, CompletionPhase::Fresh);
        assert_eq!(
            v.arrived
                .iter()
                .map(|(p, _)| p.as_str())
                .collect::<Vec<_>>(),
            ["help-harvest"],
            "harvest executed"
        );
        assert!(
            v.declined
                .iter()
                .any(|(p, r)| p.as_str() == "help-harvest" && *r == DeclineReason::NoMatch),
            "prose help declines (never guesses): {:?}",
            v.declined
        );
        assert!(
            !v.ranked
                .iter()
                .any(|c| matches!(c.authority(), Authority::HelpHarvest { .. })),
            "no fabricated harvested options"
        );
    }

    // ---- PROJECTION EQUIVALENCE (acceptance item 20) ----

    #[test]
    fn human_and_machine_projections_agree() {
        let cache = path_cache(&["cargo", "git"]);
        let r = run_discovery(&req("ca", 2, ".", &cache));
        assert!(!r.ranked.is_empty());

        let machine: Vec<RankedCandidate> =
            serde_json::from_str(&to_machine_json(&r.ranked)).unwrap();
        assert_eq!(
            machine.len(),
            r.ranked.len(),
            "machine view preserves all candidates"
        );
        let human = crate::completion::project_ranked_default(&r.ranked, "ca", 2);
        assert_eq!(
            human.len(),
            r.ranked.len(),
            "human view preserves all candidates"
        );

        for (i, (human_src, machine_item)) in r.ranked.iter().zip(machine.iter()).enumerate() {
            assert_eq!(
                human_src.semantic(),
                machine_item.semantic(),
                "SemanticKey preserved across projections"
            );
            assert_eq!(
                human_src.authority(),
                machine_item.authority(),
                "authority truth preserved across projections"
            );
            assert_eq!(
                human_src.span(),
                machine_item.span(),
                "replacement span preserved across projections"
            );
            assert_eq!(
                human_src.supporting, machine_item.supporting,
                "supporting provenance preserved in the machine view"
            );
            // The human projection is derived from the same ranked item.
            assert_eq!(human[i].literal, human_src.value().insert);
        }
    }
}
