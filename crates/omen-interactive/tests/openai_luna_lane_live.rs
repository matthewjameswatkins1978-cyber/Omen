//! Live Omen AI-lane proofs with openai-luna selected.
//!
//! Ignored by default so CI never spends credits:
//! `cargo test -p omen-interactive --test openai_luna_lane_live -- --ignored --nocapture`

use omen_interactive::ai_lane::AiLaneDispatcher;
use omen_interactive::session::InteractiveSession;
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout};
use tempfile::tempdir;

fn require_key() {
    let ok = std::env::var("OPENAI_API_KEY")
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    assert!(ok, "OPENAI_API_KEY must be set for live AI-lane smoke");
}

/// Proof 2 — reasoning query goes through openai-luna with provider_calls = 1.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live OpenAI call; requires OPENAI_API_KEY"]
async fn live_ai_lane_luna_reasoning_smoke() {
    require_key();
    run_with_test_timeout(
        "live_ai_lane_luna_reasoning_smoke",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_SESSION");
            let dir = tempdir().unwrap();
            let mut session = InteractiveSession::new(dir.path().to_path_buf(), None).unwrap();

            session.use_agent_provider("openai-luna").expect(
                "openai-luna must be registered when OPENAI_API_KEY is present in the process environment",
            );
            let active = session.agent_registry.active_descriptor();
            assert_eq!(active.id, "openai-luna");
            assert_eq!(active.model.as_deref(), Some("gpt-6-luna"));
            assert_eq!(
                active.credential_source.as_deref(),
                Some("environment:OPENAI_API_KEY")
            );
            let active_dbg = format!("{active:?}");
            let key = std::env::var("OPENAI_API_KEY").unwrap();
            assert!(!active_dbg.contains(&key));

            ctx.phase("DISPATCH_REASONING");
            let out = AiLaneDispatcher::dispatch_with_session(
                "explain why a command returning exit code 2 is not necessarily an Omen runtime failure",
                &session.session_id,
                session.cwd.as_path(),
                session.db.as_ref(),
                session.agent_provider.as_deref(),
                Some(&session.comp_ctx),
            )
            .unwrap();

            assert!(out.configured, "AI lane must be configured");
            assert_eq!(
                out.stats.provider_calls, 1,
                "reasoning query must invoke the model exactly once"
            );
            assert!(
                out.response_text.starts_with("Agent"),
                "response must come through Agent lane"
            );
            assert!(
                out.response_text.len() > 40,
                "expected a substantive explanation"
            );
            assert!(
                !out.response_text.contains(&key),
                "response must not leak the API key"
            );

            ctx.phase("DETERMINISTIC_ZERO_MODEL");
            let det = AiLaneDispatcher::dispatch_with_session(
                "what folder am I in?",
                &session.session_id,
                session.cwd.as_path(),
                session.db.as_ref(),
                session.agent_provider.as_deref(),
                Some(&session.comp_ctx),
            )
            .unwrap();
            assert_eq!(
                det.stats.provider_calls, 0,
                "deterministic queries must never call the model"
            );
            assert!(det.response_text.contains("folder") || det.response_text.contains("Agent"));
        },
    )
    .await;
}
