//! Independent Lens optionality.
//!
//! There is deliberately **no** `smart_mode = true`. Each capability is an
//! independently selectable flag so a deterministic-only user and a
//! full-experience user are both served cleanly by the same substrate.
//!
//! M1 implements `tool_metadata_harvesting`, `descriptions` and
//! `safety_metadata`. The rest are reserved seams for M2 and are present so
//! the surface does not need to be redesigned later.

use serde::{Deserialize, Serialize};

/// Independent feature selection for Lens discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensConfig {
    /// Inline ghost suggestions (M0 hinter).
    pub ghosts: bool,
    /// TIER 2 deterministic tool-metadata harvest (help/spec ingestion).
    pub tool_metadata_harvesting: bool,
    /// Show descriptions in the chooser.
    pub descriptions: bool,
    /// Carry safety metadata on candidates (UI presentation deferred).
    pub safety_metadata: bool,
    /// Fuzzy broadening (M2 seam; not activated in M1).
    pub fuzzy_matching: bool,
    /// Personal frequency/recency ranking (M2 seam; off by default).
    pub personal_ranking: bool,
    /// History/Recall provider (M2 seam).
    pub history_provider: bool,
    /// AI explanations (M2 seam). Never candidate authority.
    pub ai_explanations: bool,
    /// TIER 3 network providers (opt-in; never on ordinary Tab).
    pub network_providers: bool,
    /// Public Machine Lens exposure (M2).
    pub machine_lens_exposure: bool,
}

impl Default for LensConfig {
    fn default() -> Self {
        Self {
            ghosts: true,
            tool_metadata_harvesting: true,
            descriptions: true,
            safety_metadata: true,
            fuzzy_matching: false,
            personal_ranking: false,
            history_provider: false,
            ai_explanations: false,
            network_providers: false,
            machine_lens_exposure: false,
        }
    }
}

impl LensConfig {
    /// A strictly deterministic configuration: no harvest, no personal
    /// signals, no AI, no network. Ordinary Tier 0/1A Tab completion only.
    ///
    /// This must remain coherent — that is the optionality guarantee.
    pub fn deterministic_only() -> Self {
        Self {
            ghosts: true,
            tool_metadata_harvesting: false,
            descriptions: true,
            safety_metadata: true,
            fuzzy_matching: false,
            personal_ranking: false,
            history_provider: false,
            ai_explanations: false,
            network_providers: false,
            machine_lens_exposure: false,
        }
    }

    /// Whether any non-deterministic capability is enabled.
    pub fn has_non_deterministic(&self) -> bool {
        self.fuzzy_matching
            || self.personal_ranking
            || self.ai_explanations
            || self.network_providers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_smart_mode_switch() {
        // There is no `smart_mode` field; each capability is independent.
        let c = LensConfig::default();
        assert!(c.tool_metadata_harvesting);
        assert!(c.descriptions);
        // M2 seams are off by default.
        assert!(!c.fuzzy_matching);
        assert!(!c.personal_ranking);
        assert!(!c.ai_explanations);
        assert!(!c.network_providers);
    }

    #[test]
    fn deterministic_only_is_coherent_and_non_deterministic_free() {
        let c = LensConfig::deterministic_only();
        assert!(!c.has_non_deterministic());
        assert!(
            !c.tool_metadata_harvesting,
            "no harvest in deterministic mode"
        );
        assert!(c.descriptions, "still coherent");
    }

    #[test]
    fn every_flag_is_independently_selectable() {
        let mut c = LensConfig::deterministic_only();
        c.fuzzy_matching = true;
        assert!(
            c.fuzzy_matching && !c.personal_ranking,
            "flags do not drag each other"
        );
    }
}
