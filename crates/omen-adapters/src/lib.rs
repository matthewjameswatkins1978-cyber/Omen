//! Omen tool adapters (ThreadMoth, Cargo, Git, ripgrep).

pub mod cargo;
pub mod git;
pub mod ripgrep;
pub mod threadmoth;

pub use git::{GitAdapter, GitStatusResult};
pub use ripgrep::{RipgrepAdapter, RipgrepMatch, RipgrepSearchResult};
