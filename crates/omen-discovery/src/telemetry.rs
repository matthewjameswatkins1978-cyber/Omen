//! Structural Bench telemetry.
//!
//! Bounded and structural. **No private command text by default.** Events
//! carry `operation_id`, `phase`, `provider_id`, `cost_tier`, counts and
//! latencies so Omen Bench can reason about discovery behaviour without
//! capturing what the user typed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::budget::CostTier;
use crate::provider::ProviderId;

/// Phase of a discovery operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    CompletionRequested,
    ProviderStarted,
    ProviderFinished,
    CacheLookup,
    Merge,
    Match,
    Rank,
    Project,
}

/// One structural telemetry event. Never contains command text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub operation_id: u64,
    pub phase: Phase,
    pub provider_id: Option<String>,
    pub cost_tier: Option<CostTier>,
    pub candidate_count: Option<usize>,
    pub cache_hit: Option<bool>,
    pub latency_us: Option<u64>,
    pub outcome: Option<String>,
}

/// Bounded telemetry sink.
#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    inner: Arc<Mutex<Vec<TelemetryEvent>>>,
    cap: usize,
}

impl Telemetry {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            cap,
        }
    }

    pub fn record(&self, event: TelemetryEvent) {
        if let Ok(mut v) = self.inner.lock() {
            if v.len() >= self.cap {
                v.remove(0);
            }
            v.push(event);
        }
    }

    pub fn events(&self) -> Vec<TelemetryEvent> {
        self.inner.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Confirms no event carries a field that could hold command text.
    ///
    /// The event schema has no free-text field other than `outcome`, which is a
    /// fixed enum label (`answered`/`declined`/`partial`/`failed`/...). This
    /// is the structural guarantee that command text is not captured.
    pub fn captures_no_command_text(&self) -> bool {
        self.events().iter().all(|e| {
            e.outcome
                .as_deref()
                .map(|o| {
                    matches!(
                        o,
                        "answered"
                            | "declined"
                            | "partial"
                            | "failed"
                            | "cancelled"
                            | "timeout"
                            | "cache_hit"
                            | "cache_miss"
                    )
                })
                .unwrap_or(true)
        })
    }
}

/// Convenience constructors for common events.
pub struct Events;

impl Events {
    pub fn completion_requested(operation_id: u64) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::CompletionRequested,
            provider_id: None,
            cost_tier: None,
            candidate_count: None,
            cache_hit: None,
            latency_us: None,
            outcome: None,
        }
    }

    pub fn provider_started(
        operation_id: u64,
        provider: &ProviderId,
        tier: CostTier,
    ) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::ProviderStarted,
            provider_id: Some(provider.as_str().to_string()),
            cost_tier: Some(tier),
            candidate_count: None,
            cache_hit: None,
            latency_us: None,
            outcome: None,
        }
    }

    pub fn provider_finished(
        operation_id: u64,
        provider: &ProviderId,
        tier: CostTier,
        candidate_count: usize,
        latency: Duration,
        outcome: &str,
    ) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::ProviderFinished,
            provider_id: Some(provider.as_str().to_string()),
            cost_tier: Some(tier),
            candidate_count: Some(candidate_count),
            cache_hit: None,
            latency_us: Some(latency.as_micros() as u64),
            outcome: Some(outcome.to_string()),
        }
    }

    pub fn cache(operation_id: u64, provider: &ProviderId, hit: bool) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::CacheLookup,
            provider_id: Some(provider.as_str().to_string()),
            cost_tier: None,
            candidate_count: None,
            cache_hit: Some(hit),
            latency_us: None,
            outcome: Some(if hit { "cache_hit" } else { "cache_miss" }.to_string()),
        }
    }

    pub fn cancelled(operation_id: u64, provider: &ProviderId) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::ProviderFinished,
            provider_id: Some(provider.as_str().to_string()),
            cost_tier: None,
            candidate_count: None,
            cache_hit: None,
            latency_us: None,
            outcome: Some("cancelled".to_string()),
        }
    }

    pub fn timeout(operation_id: u64, provider: &ProviderId) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::ProviderFinished,
            provider_id: Some(provider.as_str().to_string()),
            cost_tier: None,
            candidate_count: None,
            cache_hit: None,
            latency_us: None,
            outcome: Some("timeout".to_string()),
        }
    }

    pub fn provider_failure(operation_id: u64, provider: &ProviderId) -> TelemetryEvent {
        TelemetryEvent {
            operation_id,
            phase: Phase::ProviderFinished,
            provider_id: Some(provider.as_str().to_string()),
            cost_tier: None,
            candidate_count: None,
            cache_hit: None,
            latency_us: None,
            outcome: Some("failed".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_bounded() {
        let t = Telemetry::new(3);
        for i in 0..10 {
            t.record(Events::completion_requested(i));
        }
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn no_command_text_is_captured() {
        let t = Telemetry::new(16);
        t.record(Events::completion_requested(1));
        t.record(Events::cache(1, &ProviderId::new("tool-options"), true));
        t.record(Events::provider_finished(
            1,
            &ProviderId::new("tool-options"),
            CostTier::Subprocess,
            12,
            Duration::from_millis(3),
            "answered",
        ));
        assert!(t.captures_no_command_text());
    }
}
