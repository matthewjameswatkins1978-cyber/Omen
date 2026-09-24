//! Omen H2 live Tethers authority closure.
//!
//! Division of responsibility (absolute):
//!
//! - **Tethers controls**: current authority, admission, approval records,
//!   durable intent, replay truth, provider outcome truth.
//! - **Omen executes**: physical spawn, timeout/cancellation/process-tree
//!   semantics, machine observation, execution evidence, truthful outcome
//!   reporting.
//!
//! This crate is an **admission seam, not an execution-engine migration**:
//! once a valid current COMMIT exists, physical execution runs through the
//! existing [`omen_engine`] substrate unchanged. There is no Omen-side
//! permission engine, no cached approval, no standing permission: every
//! consequential step is admitted under current truth, and before a valid
//! COMMIT the physical spawn count is exactly zero.
//!
//! Law: Plan != permission. Prepare != commit. A previous admission is
//! never a reusable permission slip.

pub mod admission;
pub mod binding;
pub mod contract;
pub mod digest;
pub mod dispatch;
pub mod error;
pub mod executor;
pub mod fetch;
pub mod gate;
pub mod identity;
pub mod managed;
pub mod outcome;
pub mod protocol;
pub mod run;
pub mod transport;

pub use admission::{
    AdmitExecute, ApprovalDecision, ApprovalOutcome, AuthorityIntent, CommitOutcome, PrepareOutcome,
};
pub use binding::{
    ExecutionBinding, FixtureProvision, VerifiedExecutionBinding, resolve_execution,
};
pub use contract::{AuthorityProjection, AuthorityState, HumanOutcome};
pub use digest::{canonical_digest, verify_argument_digest};
pub use dispatch::{VerifiedDispatch, verify_dispatch};
pub use error::AuthorityError;
pub use executor::{CountingExecutor, ExecAttempt, PhysicalExecutor, SupervisorExecutor};
pub use fetch::{FetchedCompanion, fetch_companion};
pub use gate::{GateProcess, GateSpawnConfig};
pub use identity::{ExpectedGate, TETHERS_SOURCE_SHA};
pub use managed::{CompanionPaths, GateCompanion, resolve_companion};
pub use outcome::{OutcomeJournal, OutcomeRecord, PendingOutcome};
pub use protocol::{
    AUTHORITY_PROTOCOL, ApprovalInfo, CommitResult, DispatchRecord, OutcomeClassification,
    OutcomeResult, PrepareResult, TETHERS_PRODUCT_VERSION,
};
pub use run::{AskPolicy, RunReport, parse_ask_policy, report_json, run_once};
pub use transport::{GateTransport, RoundtripError};
