use serde::{Deserialize, Serialize};

/// Tool runtime profile describing CLI hints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProfile {
    pub tool_id: String,
    pub binary: String,
    pub description: String,
}
