//! A Rust client for the **dply** v1 API, mirroring the reference PHP CLI
//! (`Apps/dply-cli`). It is deliberately self-contained and privilege-free:
//! the `dpl` CLI links it to run `dpl dply …` commands, and it can be reused
//! by any other Rust tooling that wants to drive dply.
//!
//! The three pieces that mirror dply-cli's `app/Services`:
//! - [`ConfigStore`] ↔ `ConfigStore.php` — the shared `~/.dply/config.json`
//!   token store, host-resolution precedence, and `$DPLY_TOKEN` override.
//! - [`DeviceFlow`] ↔ `DeviceFlow.php` — the OAuth device-authorization login.
//! - [`DplyClient`] ↔ `DplyClient.php` — the bearer-authed HTTP wrapper with
//!   the dply error envelope decoded into [`DplyApiError`].
//!
//! Typed endpoints live under [`endpoints`], grouped the same way the CLI's
//! command tree is (edge, sites, servers, insights, imports, operator).

pub mod client;
pub mod config_store;
pub mod device_flow;
pub mod endpoints;
pub mod error;
pub mod models;

pub use client::DplyClient;
pub use config_store::ConfigStore;
pub use device_flow::{DeviceCode, DeviceFlow, DeviceStatus, PollOutcome};
pub use error::{DplyApiError, Result};

/// User-Agent sent on every request, matching the `dply-cli/<version>`
/// convention so server-side logs group our traffic sensibly.
pub fn user_agent() -> String {
    format!("dpl/{}", env!("CARGO_PKG_VERSION"))
}
