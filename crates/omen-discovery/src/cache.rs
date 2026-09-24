//! Typed discovery cache.
//!
//! Cache identity binds truth to **the thing that makes it valid**: tool
//! metadata binds to executable path + digest + version + platform; directory
//! state binds to filesystem identity/freshness evidence; environment state
//! binds to an environment fingerprint.
//!
//! **Never silently serve expired candidate truth as fresh truth.** Cheap
//! stale detection beats constant regeneration.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::candidate::DiscoveredCandidate;
use crate::identity::ProvenanceKey;
use crate::provider::ProviderId;

/// What makes a cached result valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheIdentity {
    /// Tool metadata: binds to executable identity and platform.
    Tool {
        path: String,
        digest: Option<String>,
        version: Option<String>,
        platform: String,
    },
    /// Directory listing: binds to filesystem identity/freshness evidence.
    Directory {
        path: String,
        mtime_unix: Option<u64>,
        platform: String,
    },
    /// Environment-derived state.
    Environment { fingerprint: u64 },
    /// Project-scoped state.
    Project {
        root: String,
        manifest_digest: Option<String>,
    },
    /// Static / compile-time knowledge.
    Static,
}

/// Explicit freshness state. Never collapsed into a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Freshness {
    /// Valid and current.
    Fresh,
    /// May be superseded; usable but marked.
    Stale,
    /// Must not be served as fresh truth.
    Expired,
}

/// Cache key: provider + the identity that makes the result valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    pub provider: ProviderId,
    pub identity: CacheIdentity,
}

/// One cached entry.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub candidates: Arc<[DiscoveredCandidate]>,
    pub freshness: Freshness,
    pub obtained_at_unix: u64,
    pub provenances: Vec<ProvenanceKey>,
}

/// Bounded discovery cache. Shared, cheap to clone via `Arc<Mutex<..>>`.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryCache {
    inner: Arc<Mutex<HashMap<CacheKey, CacheEntry>>>,
}

impl DiscoveryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a cache entry. Returns `None` on miss; returns an entry with
    /// [`Freshness::Expired`] filtered out so expired truth is never served.
    pub fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(key)?;
        if entry.freshness == Freshness::Expired {
            return None;
        }
        Some(entry.clone())
    }

    /// Looks up without freshness filtering (for inspection/telemetry).
    pub fn get_raw(&self, key: &CacheKey) -> Option<CacheEntry> {
        let map = self.inner.lock().ok()?;
        map.get(key).cloned()
    }

    /// Stores a fresh entry.
    pub fn put(
        &self,
        key: CacheKey,
        candidates: Vec<DiscoveredCandidate>,
        provenances: Vec<ProvenanceKey>,
        obtained_at_unix: u64,
    ) {
        self.put_with_freshness(
            key,
            candidates,
            provenances,
            obtained_at_unix,
            Freshness::Fresh,
        );
    }

    /// Stores an entry with explicit freshness.
    pub fn put_with_freshness(
        &self,
        key: CacheKey,
        candidates: Vec<DiscoveredCandidate>,
        provenances: Vec<ProvenanceKey>,
        obtained_at_unix: u64,
        freshness: Freshness,
    ) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(
                key,
                CacheEntry {
                    candidates: candidates.into(),
                    freshness,
                    obtained_at_unix,
                    provenances,
                },
            );
        }
    }

    /// Marks an entry stale (e.g. a bound identity component may have changed).
    pub fn mark_stale(&self, key: &CacheKey) {
        if let Ok(mut map) = self.inner.lock()
            && let Some(e) = map.get_mut(key)
        {
            e.freshness = Freshness::Stale;
        }
    }

    /// Invalidates an entry entirely (e.g. executable digest changed).
    pub fn invalidate(&self, key: &CacheKey) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(key);
        }
    }

    /// Drops every entry whose predicate returns true. Used for bulk
    /// invalidation when an identity component (PATH, digest) changes.
    pub fn invalidate_where(&self, pred: impl Fn(&CacheKey) -> bool) {
        if let Ok(mut map) = self.inner.lock() {
            map.retain(|k, _| !pred(k));
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, AuthorityClass};
    use crate::candidate::TextSpan;
    use crate::identity::{EvidenceIdentity, SemanticKey, SemanticNamespace};
    use crate::kind::Kind;

    fn cand(v: &str) -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            SemanticKey::new(
                Kind::Option,
                v,
                SemanticNamespace::Tool { tool: "git".into() },
            ),
            ProvenanceKey::new(
                "tool-options",
                AuthorityClass::HelpHarvest,
                EvidenceIdentity::Static,
            ),
            v,
            Authority::HelpHarvest {
                tool: "git".into(),
                tool_version: None,
                harvested_at_unix: 0,
            },
            TextSpan::at(0),
        )
    }

    fn tool_key(digest: &str) -> CacheKey {
        CacheKey {
            provider: ProviderId::new("tool-options"),
            identity: CacheIdentity::Tool {
                path: "/usr/bin/git".into(),
                digest: Some(digest.into()),
                version: Some("2.0".into()),
                platform: "linux".into(),
            },
        }
    }

    #[test]
    fn hit_and_miss() {
        let cache = DiscoveryCache::new();
        let key = tool_key("aaa");
        assert!(cache.get(&key).is_none());
        cache.put(key.clone(), vec![cand("--verbose")], vec![], 0);
        assert!(cache.get(&key).is_some());
    }

    #[test]
    fn identity_binds_to_digest() {
        let cache = DiscoveryCache::new();
        cache.put(tool_key("aaa"), vec![cand("--verbose")], vec![], 0);
        assert!(
            cache.get(&tool_key("bbb")).is_none(),
            "changed executable digest must not reuse cached truth"
        );
    }

    #[test]
    fn expired_is_never_served() {
        let cache = DiscoveryCache::new();
        let key = tool_key("aaa");
        cache.put_with_freshness(
            key.clone(),
            vec![cand("--verbose")],
            vec![],
            0,
            Freshness::Expired,
        );
        assert!(
            cache.get(&key).is_none(),
            "expired truth must not be served as fresh"
        );
        assert!(cache.get_raw(&key).is_some(), "but remains inspectable");
    }

    #[test]
    fn stale_is_served_but_marked() {
        let cache = DiscoveryCache::new();
        let key = tool_key("aaa");
        cache.put(key.clone(), vec![cand("--verbose")], vec![], 0);
        cache.mark_stale(&key);
        let e = cache.get(&key).unwrap();
        assert_eq!(e.freshness, Freshness::Stale);
    }

    #[test]
    fn invalidate_removes_entry() {
        let cache = DiscoveryCache::new();
        let key = tool_key("aaa");
        cache.put(key.clone(), vec![cand("--verbose")], vec![], 0);
        cache.invalidate(&key);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn bulk_invalidate_by_predicate() {
        let cache = DiscoveryCache::new();
        cache.put(tool_key("aaa"), vec![cand("--verbose")], vec![], 0);
        cache.put(tool_key("bbb"), vec![cand("--quiet")], vec![], 0);
        cache.invalidate_where(
            |k| matches!(&k.identity, CacheIdentity::Tool { path, .. } if path.contains("git")),
        );
        assert!(cache.is_empty());
    }
}
