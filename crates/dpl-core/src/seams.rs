//! Cross-platform seams.
//!
//! The three things that differ between macOS and Linux — how a background
//! service is registered, how `.test` is wired into the resolver, and how a
//! CA is trusted — are expressed as traits here. `dpld`/`dpl-helper`
//! provide the concrete launchd/systemd, `/etc/resolver`/NSS, and
//! keychain/`ca-certificates` implementations (Phases 3–7). Defining the
//! contracts now keeps that platform code from leaking into the shared
//! vocabulary later.

use std::path::PathBuf;

/// Which host OS a concrete seam targets. Handy for `doctor` output and for
/// picking an implementation at runtime when `#[cfg]` isn't enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Other,
}

impl Platform {
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Platform::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            Platform::Linux
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Platform::Other
        }
    }
}

/// Registers/starts the daemon as a user background service
/// (launchd on macOS, systemd --user on Linux).
pub trait ServiceManager {
    type Error;
    fn install(&self, exe: &std::path::Path) -> Result<(), Self::Error>;
    fn uninstall(&self) -> Result<(), Self::Error>;
    fn start(&self) -> Result<(), Self::Error>;
    fn stop(&self) -> Result<(), Self::Error>;
    fn is_running(&self) -> Result<bool, Self::Error>;
}

/// Wires the `.test` TLD to the local DNS responder
/// (`/etc/resolver/test` on macOS, NSS/`systemd-resolved` on Linux).
/// Privileged — implemented in `dpl-helper`.
pub trait Resolver {
    type Error;
    /// Point `tld` at `127.0.0.1:<port>`.
    fn install(&self, tld: &str, port: u16) -> Result<(), Self::Error>;
    fn uninstall(&self, tld: &str) -> Result<(), Self::Error>;
    fn is_installed(&self, tld: &str) -> Result<bool, Self::Error>;
}

/// Adds/removes the local CA from the system trust store so browsers accept
/// per-site certs without warnings. Privileged — implemented in
/// `dpl-helper`.
pub trait TrustStore {
    type Error;
    fn trust_ca(&self, ca_pem: &PathBuf) -> Result<(), Self::Error>;
    fn untrust_ca(&self, ca_pem: &PathBuf) -> Result<(), Self::Error>;
    fn is_trusted(&self, ca_pem: &PathBuf) -> Result<bool, Self::Error>;
}
