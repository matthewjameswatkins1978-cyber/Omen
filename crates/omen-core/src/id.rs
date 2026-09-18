use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

macro_rules! define_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Result<Self, CoreError> {
                let s = id.into();
                if s.trim().is_empty() {
                    return Err(CoreError::InvalidId(format!(
                        "{} identifier cannot be empty",
                        stringify!($name)
                    )));
                }
                // Disallow newlines, control characters, or non-printable ASCII
                if s.chars().any(|c| c.is_control()) {
                    return Err(CoreError::InvalidId(format!(
                        "{} identifier contains control characters: {:?}",
                        stringify!($name),
                        s
                    )));
                }
                Ok(Self(s))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = CoreError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }
    };
}

define_id!(ResourceId, "res");
define_id!(ToolId, "tool");
define_id!(ExecutionId, "exec");
define_id!(ActionId, "action");
define_id!(FactId, "fact");
define_id!(ArtifactId, "artifact");
define_id!(ProcessId, "proc");
define_id!(InteractiveSessionId, "sess");
define_id!(OperationId, "op");

impl InteractiveSessionId {
    /// Generates a genuinely unique opaque session ID using UUID v4.
    pub fn generate() -> Self {
        Self(format!("sess-{}", uuid::Uuid::new_v4()))
    }
}
