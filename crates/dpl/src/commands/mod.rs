//! Command handlers, split by family: `local` drives the daemon's `.test`
//! sites; `dply` is the platform subtree.

pub mod daemon;
pub mod doctor;
pub mod dply;
pub mod local;
pub mod mail;
pub mod parity;
pub mod valet;
pub mod xdebug;

use dpl_core::ipc::Response;

/// Turn an unexpected daemon response into an error — shared by the local
/// command handlers and `main`.
pub fn unexpected(resp: Response) -> anyhow::Result<()> {
    match resp {
        Response::Error { message } => anyhow::bail!("daemon error: {message}"),
        other => anyhow::bail!("unexpected daemon response: {other:?}"),
    }
}
