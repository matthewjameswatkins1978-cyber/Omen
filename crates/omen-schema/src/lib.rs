//! Omen wire schemas and serialization definitions.

pub mod contract;
pub mod result;

pub use contract::{
    ExecutionConstraintsWire, ExecutionContractWire, IntentWire, LeaseRequestWire,
    RequiredAssuranceWire, SCHEMA_VERSION_EXECUTION, StdioConfigWire,
};
pub use result::{
    EnforcementReportWire, ExecutionResultWire, ProcessExitWire, SCHEMA_VERSION_RESULT,
};
