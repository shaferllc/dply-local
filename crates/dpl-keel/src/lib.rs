//! Keel Cloud client for `dpl keel …` — the same role `dpl-dply` plays for
//! dply, pointed at Keel Cloud's `/api/v1` (Bearer-token REST; see
//! keel-cloud's CloudApiController).
//!
//! Auth differs from dply on purpose: Keel Cloud has no device flow — tokens
//! (`keel_…`) are minted in the web UI at `/tokens` and pasted once into
//! `dpl keel login`. Stored in `~/.keel/cloud.json`; the `KEEL_CLOUD_TOKEN` /
//! `KEEL_CLOUD_URL` environment variables (the MCP client's convention)
//! override the file, so both clients can share one setup.

mod client;
mod config;
mod error;

pub use client::KeelClient;
pub use config::{KeelConfig, DEFAULT_URL};
pub use error::{KeelApiError, Result};

/// Sent as the User-Agent on every API call.
pub fn user_agent() -> String {
    format!("dpl/{}", env!("CARGO_PKG_VERSION"))
}
