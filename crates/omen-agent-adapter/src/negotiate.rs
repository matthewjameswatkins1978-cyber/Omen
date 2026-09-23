//! Explicit compatibility negotiation. Compatible binds, required
//! gaps refuse explicitly, optional gaps degrade truthfully. Incompatible
//! semantics are never silently reinterpreted.

use serde::{Deserialize, Serialize};

/// What Omen offers at handshake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OmenOffer {
    pub omen_contract: String,
    pub adapter_protocols: Vec<String>,
    pub required_features: Vec<String>,
    pub optional_features: Vec<String>,
}

/// What the adapter answered in its hello.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterOffer {
    pub protocol_version: String,
    pub features: Vec<String>,
    pub contract_versions: Vec<String>,
}

/// Successful negotiation: selected protocol plus truthful degradation list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Negotiated {
    pub protocol_version: String,
    pub active_features: Vec<String>,
    pub degraded_features: Vec<String>,
}

/// Explicit refusal: machine-readable reason, no fallback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NegotiationRefusal {
    pub reason: NegotiationRefusalReason,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NegotiationRefusalReason {
    IncompatibleProtocol,
    IncompatibleContract,
    MissingRequiredFeature,
}

pub fn negotiate(
    omen: &OmenOffer,
    adapter: &AdapterOffer,
) -> Result<Negotiated, NegotiationRefusal> {
    // Highest-preference common protocol wins; none in common refuses.
    let mut selected: Option<String> = None;
    for proto in &omen.adapter_protocols {
        if adapter.protocol_version == *proto {
            selected = Some(proto.clone());
            break;
        }
    }
    let Some(protocol_version) = selected else {
        return Err(NegotiationRefusal {
            reason: NegotiationRefusalReason::IncompatibleProtocol,
            message: format!(
                "no common adapter protocol (omen offers {:?}, adapter speaks {:?})",
                omen.adapter_protocols, adapter.protocol_version
            ),
        });
    };
    if !adapter.contract_versions.contains(&omen.omen_contract) {
        return Err(NegotiationRefusal {
            reason: NegotiationRefusalReason::IncompatibleContract,
            message: format!(
                "adapter requires contracts {:?}, omen speaks {:?}",
                adapter.contract_versions, omen.omen_contract
            ),
        });
    }
    for required in &omen.required_features {
        if !adapter.features.iter().any(|f| f == required) {
            return Err(NegotiationRefusal {
                reason: NegotiationRefusalReason::MissingRequiredFeature,
                message: format!("adapter lacks required feature {required:?}"),
            });
        }
    }
    let mut active = Vec::new();
    let mut degraded = Vec::new();
    for optional in &omen.optional_features {
        if adapter.features.iter().any(|f| f == optional) {
            active.push(optional.clone());
        } else {
            degraded.push(optional.clone());
        }
    }
    Ok(Negotiated {
        protocol_version,
        active_features: active,
        degraded_features: degraded,
    })
}
