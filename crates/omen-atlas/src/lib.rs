//! Omen Tool Atlas, Runtime Profiles, and Validator Harness.

pub mod profile;
pub mod syntax;
pub mod tool;
pub mod validator;

pub use profile::RuntimeProfile;
pub use syntax::{CommandSyntaxSpecification, FlagSyntaxSpecification};
pub use tool::{
    ToolInstance, ToolUnderstanding, ValidationState, compute_binary_fingerprint,
    find_binary_on_path,
};
pub use validator::ToolValidator;
