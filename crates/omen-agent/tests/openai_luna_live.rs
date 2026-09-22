//! Live OpenAI Luna proofs.
//!
//! These tests are `#[ignore]` by default so normal CI never spends credits.
//! Run explicitly with:
//! `cargo test -p omen-agent --test openai_luna_live -- --ignored --nocapture`

use omen_agent::{
    AgentContext, AgentProvider, AgentRequest, AgentTurn, OPENAI_LUNA_PROVIDER_ID,
    OpenAiLunaProvider,
};
use omen_core::InteractiveSessionId;
use std::path::PathBuf;

fn require_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
}

fn sample_request(prompt: &str) -> AgentRequest {
    AgentRequest {
        prompt: prompt.into(),
        context: AgentContext::new(
            "ws-live",
            PathBuf::from("."),
            InteractiveSessionId::new("sess-live").unwrap(),
            PathBuf::from("."),
        ),
        conversation: Vec::<AgentTurn>::new(),
    }
}

/// Proof 1 — raw provider smoke against the live Responses API.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live OpenAI call; requires OPENAI_API_KEY"]
async fn live_luna_raw_smoke() {
    let Some(_key) = require_key() else {
        panic!("OPENAI_API_KEY must be set for live smoke");
    };

    let provider = OpenAiLunaProvider::from_env();
    assert_eq!(provider.model(), "gpt-6-luna");
    assert!(provider.has_credential());
    assert_eq!(provider.descriptor().id, OPENAI_LUNA_PROVIDER_ID);
    assert_eq!(
        provider.descriptor().credential_source.as_deref(),
        Some("environment:OPENAI_API_KEY")
    );

    let dbg = format!("{provider:?}");
    let key = std::env::var("OPENAI_API_KEY").unwrap();
    assert!(!dbg.contains(&key), "Debug must never contain the API key");

    let response = provider
        .respond(sample_request("Reply with exactly OMEN_LUNA_OK"))
        .await
        .expect("live Luna call must succeed");

    assert!(
        response.message.contains("OMEN_LUNA_OK"),
        "expected token in message, got: {}",
        response.message
    );
    assert!(response.proposed_actions.is_empty());
    let resp_dbg = format!("{response:?}");
    assert!(!resp_dbg.contains(&key));
}
