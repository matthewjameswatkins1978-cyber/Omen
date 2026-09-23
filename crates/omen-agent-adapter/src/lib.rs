//! Omen Agent Adapter Kit.
//!
//! Small stable plug/socket contract for out-of-process agent adapters.
//! Extensions add capabilities; they do not enter the constitution.
//!
//! Wire protocol: `omen.agent-adapter/0.1` — newline-delimited JSON frames
//! over a child process stdio (patterns borrowed from `omen-mcp`
//! `run_stream` and the Python `transport.py` supervisor). A new protocol
//! was required because MCP is a tool-host surface (Omen serves tools to
//! models) and the daemon IPC bus is a trusted local bus with workspace
//! state; neither carries adapter translation with explicit version
//! negotiation, tool-request round trips, and cancellation.

pub mod binding;
pub mod env_policy;
pub mod lifecycle;
pub mod manifest;
pub mod negotiate;
pub mod protocol;

pub use binding::*;
pub use env_policy::*;
pub use lifecycle::*;
pub use manifest::*;
pub use negotiate::*;
pub use protocol::*;
