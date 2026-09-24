//! Reedline/interactive integration of the discovery substrate.
//!
//! This module owns the projection boundary between `omen-discovery` semantic
//! truth and the interactive presentation:
//!
//! ```text
//! grammar (one parser)  ->  ProviderContext
//! providers             ->  DiscoveredCandidate[]
//! merge + match + rank  ->  RankedCandidate[]
//! projection            ->  CompletionCandidate / reedline::Suggestion
//! ```
//!
//! Reedline owns interaction state (cursor, menu, Enter). Omen owns semantic
//! state (what the cursor means). Nothing here intercepts Enter.

use std::sync::{Arc, OnceLock};

use omen_discovery::budget::{CostTier, DiscoveryBudget, DiscoveryDepth};
use omen_discovery::candidate::{RankedCandidate, TextSpan};
use omen_discovery::context::{
    ActiveToken, CommandPosition, EnvFacts, ExecutableIdentity, ProjectContext, ProviderContext,
};
use omen_discovery::provider::DiscoveryProvider;
use omen_discovery::providers::{
    FilesystemPathProvider, HelpHarvestCache, HelpHarvestProvider, OmenActionProvider,
    OmenKnowledge, PathCommandCache, PathCommandsProvider, SubcommandProvider, ToolIdentity,
    ToolSpecProvider,
};
use omen_discovery::registry::{DiscoveryResult, ProviderRegistry};
use omen_discovery::scheduler::Scheduler;
use omen_discovery::telemetry::Telemetry;

use crate::commands;

/// Process-wide help-harvest cache so TIER 2 harvests survive across keystrokes.
fn global_harvest_cache() -> &'static HelpHarvestCache {
    static CACHE: OnceLock<HelpHarvestCache> = OnceLock::new();
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
    })
}

/// Identifies the current platform/executable for cache binding.
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

/// One discovery run's inputs, derived from the single input grammar.
pub struct DiscoveryRequest<'a> {
    pub buffer: &'a str,
    pub cursor: usize,
    pub cwd: &'a str,
    pub path_cache: &'a PathCommandCache,
    /// When true, filesystem path candidates are restricted to directories.
    pub dirs_only: bool,
}

/// Maps the grammar's cursor token into the discovery context's active token.
fn active_token_from(token: &crate::grammar::CursorToken) -> ActiveToken {
    ActiveToken {
        span: TextSpan::new(token.span.start, token.span.end),
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
pub fn build_provider_context(req: &DiscoveryRequest<'_>) -> ProviderContext {
    let cursor = req.cursor.min(req.buffer.len());
    let token = match crate::grammar::token_at_cursor(req.buffer, cursor) {
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

    let command_position = compute_command_position(req.buffer, &token);
    let active_token = active_token_from(&token);
    let replacement_span = TextSpan::new(token.span.start, token.span.end);

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
        buffer: req.buffer.to_string(),
        cursor,
        active_token,
        replacement_span,
        command_position,
        known_executable,
        working_dir: req.cwd.to_string(),
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
        depth: DiscoveryDepth::Normal,
    }
}

/// A registry wired with the M1 providers for one request.
fn build_registry() -> ProviderRegistry {
    ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(256))
}

/// Runs the inline discovery pipeline for a request.
///
/// Returns the aggregate result plus the providers deferred to the async path
/// (TIER 1B/2). The caller may dispatch those through
/// [`ProviderRegistry::scheduler`]; the editor loop is never blocked.
pub fn run_discovery(req: &DiscoveryRequest<'_>) -> DiscoveryResult {
    let ctx = build_provider_context(req);
    let knowledge = omen_knowledge();

    let mut reg = build_registry();
    reg.register(Box::new(OmenActionProvider::new(knowledge.clone())));
    reg.register(Box::new(SubcommandProvider::new(knowledge.clone())));
    reg.register(Box::new(ToolSpecProvider::new(
        omen_discovery::providers::default_tool_specs(),
    )));
    reg.register(Box::new(PathCommandsProvider::new(req.path_cache.clone())));
    reg.register(Box::new(if req.dirs_only {
        FilesystemPathProvider::directories_only()
    } else {
        FilesystemPathProvider::new()
    }));

    let harvest = HelpHarvestProvider::new(global_harvest_cache().clone());
    harvest.cache().set_identity(current_tool_identity());
    reg.register(Box::new(harvest));

    let budget = DiscoveryBudget::allowing_subprocess();
    let (mut result, deferred) = reg.discover_inline(&ctx, &budget);

    // TIER 1B/2 providers cannot run inline. The filesystem provider is the
    // common path-completion source; run it through the scheduler and merge its
    // result so callers observe a complete ranked set without blocking.
    for pid in deferred {
        let ctx2 = ctx.clone();
        let dirs_only = req.dirs_only;
        let cwd = req.cwd.to_string();
        let budget2 = DiscoveryBudget::allowing_subprocess();
        let dispatched = reg.scheduler().dispatch(
            pid.clone(),
            CostTier::BlockingLocal,
            budget2,
            Box::new(move |_b| {
                let mut p = if dirs_only {
                    FilesystemPathProvider::directories_only()
                } else {
                    FilesystemPathProvider::new()
                };
                let mut ctx = ctx2;
                ctx.working_dir = cwd;
                p.discover(&ctx, &DiscoveryBudget::allowing_subprocess())
            }),
        );
        if dispatched {
            // Collect the background result synchronously here so the caller
            // observes a complete set; the editor loop is not on this path for
            // interactive keystrokes (those use the async poll seam).
            if let Some(work) = reg.scheduler().take_result() {
                for c in work.outcome.candidates() {
                    result.ranked = merge_one(result.ranked, c.clone(), &ctx);
                }
            }
        }
    }

    result
}

/// Merges one late candidate into an already-ranked list, re-ranking
/// deterministically. Keeps the presentation order stable.
fn merge_one(
    ranked: Vec<RankedCandidate>,
    candidate: omen_discovery::candidate::DiscoveredCandidate,
    ctx: &ProviderContext,
) -> Vec<RankedCandidate> {
    use omen_discovery::matcher::Matcher;
    use omen_discovery::merge::merge_by_semantic;
    use omen_discovery::rank::LensRanker;

    let mut batch: Vec<omen_discovery::candidate::DiscoveredCandidate> = ranked
        .iter()
        .map(|r| r.matched.discovered.clone())
        .collect();
    batch.push(candidate);
    let merged = merge_by_semantic(batch);
    let query = ctx.query();
    let mut pairs = Vec::new();
    for m in merged {
        let discovered = m.primary.clone();
        if let Some(matched) = Matcher::match_one(&discovered, query) {
            pairs.push((m, matched));
        }
    }
    LensRanker::rank(pairs, ctx)
}

/// Projects a [`RankedCandidate`] into a Reedline [`reedline::Suggestion`].
///
/// This is the human-facing projection of the same semantic truth the machine
/// view serialises. Reedline owns interaction state; the suggestion is pure
/// presentation over Omen-owned semantic state.
pub fn to_suggestion(ranked: &RankedCandidate) -> reedline::Suggestion {
    use omen_discovery::candidate::CandidateValue;
    let discovered = &ranked.matched.discovered;
    let CandidateValue {
        insert,
        append_whitespace,
    } = &discovered.value;
    let kind = discovered.semantic.kind;
    let append =
        *append_whitespace && !kind.is_path_like() && discovered.replacement_span.is_empty();

    reedline::Suggestion {
        value: insert.clone(),
        description: {
            let label = kind.label();
            match &discovered.description {
                Some(d) => Some(format!("{label}: {}", d.short)),
                None => Some(label.to_string()),
            }
        },
        extra: discovered
            .description
            .as_ref()
            .and_then(|d| d.detail.clone())
            .map(|d| vec![d]),
        span: reedline::Span {
            start: discovered.replacement_span.start,
            end: discovered.replacement_span.end,
        },
        append_whitespace: append,
        display_override: discovered
            .display
            .as_ref()
            .map(|d| d.label.clone())
            .or_else(|| Some(insert.clone())),
        match_indices: Some(ranked.matched.match_indices.clone()),
        style: None,
    }
}

/// Machine-serialisable projection of a ranked set.
///
/// Preserves the same [`SemanticKey`](omen_discovery::identity::SemanticKey)s,
/// authority truth and replacement spans as the human projection. The public
/// Machine Lens command is M2+; this proves the internal representation is
/// ready without shipping it.
pub fn to_machine_json(ranked: &[RankedCandidate]) -> String {
    serde_json::to_string(ranked).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omen_discovery::kind::Kind;

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
            dirs_only: false,
        }
    }

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
        let cache = path_cache(&["git"]);
        let ctx = build_provider_context(&req("git --ver", 9, ".", &cache));
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
            // Concrete authority, never fabricated.
            assert!(matches!(
                c.authority(),
                omen_discovery::authority::Authority::OmenFact { .. }
                    | omen_discovery::authority::Authority::ToolNative { .. }
                    | omen_discovery::authority::Authority::HelpHarvest { .. }
                    | omen_discovery::authority::Authority::InstalledSpec { .. }
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

    #[test]
    fn flow_e_long_tail_remains_reachable_and_deterministic() {
        let cache = path_cache(&["git"]);
        let a = run_discovery(&req("git --", 6, ".", &cache));
        let b = run_discovery(&req("git --", 6, ".", &cache));
        let names = |r: &DiscoveryResult| -> Vec<String> {
            r.ranked.iter().map(|c| c.value().insert.clone()).collect()
        };
        assert_eq!(names(&a), names(&b), "same input => same order");
        // The declared semantic order is respected (ordinal 0 first).
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
        // The git option appears from Omen-owned spec; if a harvest also
        // observed it they must merge into ONE semantic candidate with all
        // provenance preserved and strongest evidence primary.
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

    #[test]
    fn human_and_machine_projections_agree() {
        // Acceptance item 20: one RankedCandidate set projects to the human
        // Reedline view and the machine-serialisable view preserving the same
        // SemanticKeys, authority truth and replacement spans.
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

        for (human_src, machine_item) in r.ranked.iter().zip(machine.iter()) {
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

            let suggestion = to_suggestion(human_src);
            assert_eq!(
                suggestion.span.start,
                human_src.span().start,
                "human projection spans match semantic truth"
            );
            assert_eq!(
                suggestion.span.end,
                human_src.span().end,
                "human projection spans match semantic truth"
            );
        }
    }
}
