//! Shared types for the `dpl` toolchain: filesystem paths, the CLI↔daemon
//! IPC protocol, local-dev configuration, errors, and the cross-platform
//! seams (service manager, DNS resolver, trust store) whose concrete
//! implementations live in `dpld`/`dpl-helper`.
//!
//! Nothing here needs privilege or the network — it is the vocabulary the
//! rest of the workspace speaks.

pub mod branchdb;
pub mod config;
pub mod error;
pub mod ipc;
pub mod paths;
pub mod php;
pub mod seams;
pub mod node;
pub mod sites;
pub mod spx;
pub mod tools;
pub mod xdebug;

pub use error::{CoreError, Result};

/// Version of the IPC wire protocol. Bumped when [`ipc::Request`] or
/// [`ipc::Response`] change shape incompatibly; the daemon rejects a
/// mismatched client so a stale binary fails loudly instead of silently
/// misbehaving.
pub const PROTOCOL_VERSION: u32 = 1;
