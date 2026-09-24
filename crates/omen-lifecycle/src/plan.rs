//! PLAN != APPLY (H items 8, 10).
//!
//! Every consequential deletion or mutation is first rendered as an
//! inspectable [`ActionPlan`], then applied by [`apply_plan`] which
//! revalidates protection and identity at apply time. A stale plan — one
//! whose world changed between plan and apply — refuses rather than deletes
//! something newly protected.

use crate::error::LifecycleError;
use crate::state::StateItem;
use serde::{Deserialize, Serialize};

pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// One consequential item inside a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedItem {
    pub identity: String,
    pub role: String,
    pub reason: String,
    pub size_bytes: Option<u64>,
    pub consequence: String,
    pub protected_at_plan: bool,
    /// Identity fingerprint captured at plan time (file hash, digest, or
    /// slot id). Apply refuses when it no longer matches.
    pub fingerprint: Option<String>,
}

impl PlannedItem {
    pub fn from_state(item: &StateItem, reason: &str, consequence: &str) -> Self {
        Self {
            identity: item.identity.clone(),
            role: format!("{:?}", item.role).to_lowercase(),
            reason: reason.to_string(),
            size_bytes: item.size_bytes,
            consequence: consequence.to_string(),
            protected_at_plan: item.is_protected(),
            fingerprint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub kind: String,
    pub created_at: String,
    pub items: Vec<PlannedItem>,
    pub total_bytes: Option<u64>,
}

impl ActionPlan {
    pub fn new(kind: &str, items: Vec<PlannedItem>) -> Self {
        let total = if items.iter().all(|i| i.size_bytes.is_some()) {
            Some(items.iter().map(|i| i.size_bytes.unwrap_or(0)).sum())
        } else {
            None
        };
        Self {
            schema_version: PLAN_SCHEMA_VERSION,
            plan_id: format!("plan_{}", &hex::encode(rand_hex())[..12]),
            kind: kind.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            items,
            total_bytes: total,
        }
    }

    pub fn empty(kind: &str) -> Self {
        Self::new(kind, Vec::new())
    }
}

fn rand_hex() -> [u8; 8] {
    use sha2::{Digest, Sha256};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let mut h = Sha256::new();
    h.update(nanos.to_le_bytes());
    h.update(pid.to_le_bytes());
    h.finalize()[..8].try_into().unwrap()
}

pub fn plan_path(base: &std::path::Path, kind: &str, plan_id: &str) -> std::path::PathBuf {
    base.join("update").join(format!("{kind}-{plan_id}.json"))
}

pub fn save_plan(
    base: &std::path::Path,
    plan: &ActionPlan,
) -> Result<std::path::PathBuf, LifecycleError> {
    let path = plan_path(base, &plan.kind, &plan.plan_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(plan).unwrap())
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(path)
}

pub fn load_plan(path: &std::path::Path) -> Result<ActionPlan, LifecycleError> {
    let bytes = std::fs::read(path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let plan: ActionPlan =
        serde_json::from_slice(&bytes).map_err(|e| LifecycleError::Manifest(e.to_string()))?;
    if plan.schema_version != PLAN_SCHEMA_VERSION {
        return Err(LifecycleError::Compat(format!(
            "plan schema {} unsupported",
            plan.schema_version
        )));
    }
    Ok(plan)
}

/// Revalidation verdict for one planned item at apply time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revalidate {
    /// Still the same unprotected object: safe to act.
    Proceed,
    /// Newly protected or identity changed: refuse this item.
    Refuse { reason: String },
}

/// Apply a plan item-by-item. `revalidate` is called for every item at apply
/// time and MUST consult live truth (protection store, file identity). Any
/// refusal aborts the whole apply with [`LifecycleError::StalePlan`] —
/// never a partial deletion followed by silence. Returns per-item outcomes
/// so partial application is representable (H doctrine item 9).
pub fn apply_plan(
    plan: &ActionPlan,
    revalidate: &dyn Fn(&PlannedItem) -> Result<Revalidate, LifecycleError>,
    act: &dyn Fn(&PlannedItem) -> Result<String, LifecycleError>,
) -> ApplyReport {
    let mut report = ApplyReport {
        plan_id: plan.plan_id.clone(),
        kind: plan.kind.clone(),
        completed: Vec::new(),
        refused: Vec::new(),
        failed: Vec::new(),
        stale: false,
    };
    for item in &plan.items {
        match revalidate(item) {
            Ok(Revalidate::Proceed) => match act(item) {
                Ok(detail) => report.completed.push(AppliedItem {
                    identity: item.identity.clone(),
                    detail,
                }),
                Err(e) => {
                    report.failed.push(FailedItem {
                        identity: item.identity.clone(),
                        error: e.to_string(),
                    });
                    // Stop at first failure: no blind continuation past an
                    // ambiguous mutation. Remaining items stay unresolved.
                    for rest in plan
                        .items
                        .iter()
                        .skip_while(|i| i.identity != item.identity)
                        .skip(1)
                    {
                        report.refused.push(RefusedItem {
                            identity: rest.identity.clone(),
                            reason: "apply aborted after earlier failure".to_string(),
                        });
                    }
                    break;
                }
            },
            Ok(Revalidate::Refuse { reason }) => {
                report.refused.push(RefusedItem {
                    identity: item.identity.clone(),
                    reason,
                });
                // A stale plan refuses the whole apply (H item 11).
                for rest in plan
                    .items
                    .iter()
                    .skip_while(|i| i.identity != item.identity)
                    .skip(1)
                {
                    report.refused.push(RefusedItem {
                        identity: rest.identity.clone(),
                        reason: "apply aborted: plan went stale".to_string(),
                    });
                }
                report.stale = true;
                break;
            }
            Err(e) => {
                report.failed.push(FailedItem {
                    identity: item.identity.clone(),
                    error: e.to_string(),
                });
                break;
            }
        }
    }
    report
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedItem {
    pub identity: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefusedItem {
    pub identity: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedItem {
    pub identity: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyReport {
    pub plan_id: String,
    pub kind: String,
    pub completed: Vec<AppliedItem>,
    pub refused: Vec<RefusedItem>,
    pub failed: Vec<FailedItem>,
    #[serde(default)]
    pub stale: bool,
}

impl ApplyReport {
    pub fn fully_applied(&self) -> bool {
        self.failed.is_empty() && self.refused.is_empty()
    }
}

/// Protection snapshot used by revalidators: the live set of protected
/// identities plus identity fingerprints.
#[derive(Debug, Default)]
pub struct ProtectionSnapshot {
    pub protected: std::collections::BTreeSet<String>,
    pub fingerprints: std::collections::BTreeMap<String, String>,
}

impl ProtectionSnapshot {
    /// Shared revalidation rule: refuse when protected now, or when the
    /// fingerprint changed since plan time.
    pub fn revalidate(&self, item: &PlannedItem) -> Revalidate {
        if self.protected.contains(&item.identity) {
            return Revalidate::Refuse {
                reason: format!("{} became protected after plan", item.identity),
            };
        }
        if let (Some(want), Some(have)) = (
            item.fingerprint.as_ref(),
            self.fingerprints.get(&item.identity),
        ) && want != have
        {
            return Revalidate::Refuse {
                reason: format!("{} identity changed after plan", item.identity),
            };
        }
        Revalidate::Proceed
    }
}
