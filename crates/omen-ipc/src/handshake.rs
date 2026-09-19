use crate::codec::MAX_FRAME_SIZE;
use crate::error::LocalIpcError;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_FAMILY: &str = "omen.local-ipc";
pub const PROTOCOL_VERSION_1: u32 = 1;
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[PROTOCOL_VERSION_1];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientHello {
    pub protocol_version_family: String,
    pub supported_versions: Vec<u32>,
    pub client_instance_id: String,
    pub product_version: String,
    pub platform: String,
    pub requested_features: Vec<String>,
}

impl ClientHello {
    pub fn new(client_instance_id: String, requested_features: Vec<String>) -> Self {
        Self {
            protocol_version_family: PROTOCOL_FAMILY.to_string(),
            supported_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            client_instance_id,
            product_version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            requested_features,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonHello {
    pub selected_protocol_version: u32,
    pub daemon_instance_id: String,
    pub product_version: String,
    pub supported_features: Vec<String>,
    pub max_frame_size: usize,
}

impl DaemonHello {
    pub fn new(daemon_instance_id: String, selected_protocol_version: u32) -> Self {
        Self {
            selected_protocol_version,
            daemon_instance_id,
            product_version: env!("CARGO_PKG_VERSION").to_string(),
            supported_features: vec![
                "events".into(),
                "services".into(),
                "execution".into(),
                "hot_index".into(),
            ],
            max_frame_size: MAX_FRAME_SIZE,
        }
    }
}

pub fn negotiate_protocol_version(
    client_family: &str,
    client_versions: &[u32],
) -> Result<u32, LocalIpcError> {
    if client_family != PROTOCOL_FAMILY {
        return Err(LocalIpcError::ProtocolVersionUnsupported(format!(
            "Unsupported protocol family '{client_family}', expected '{PROTOCOL_FAMILY}'"
        )));
    }

    // Pick highest supported version present in both
    for &daemon_ver in SUPPORTED_PROTOCOL_VERSIONS.iter().rev() {
        if client_versions.contains(&daemon_ver) {
            return Ok(daemon_ver);
        }
    }

    Err(LocalIpcError::ProtocolVersionUnsupported(format!(
        "No mutually supported protocol version. Client offered {client_versions:?}, daemon supports {SUPPORTED_PROTOCOL_VERSIONS:?}"
    )))
}
