//! Omen wire schemas and serialization definitions.

pub mod contract;
pub mod result;

pub use contract::{ExecutionContractWire, LeaseRequestWire, StdioConfigWire};
pub use result::{EnforcementReportWire, ExecutionResultWire, ProcessExitWire};
