//! The CLI↔daemon wire protocol.
//!
//! Framing is length-prefixed JSON: a 4-byte big-endian `u32` byte count
//! followed by that many bytes of a JSON-encoded [`Envelope`]. This keeps
//! the daemon and CLI decoupled — either can be rebuilt independently as
//! long as [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION) matches.
//!
//! Phase 0 defines only the control verbs (`ping`/`status`/`version`).
//! Site, PHP, and service verbs are added in later phases as new
//! [`Request`]/[`Response`] variants; unknown variants deserialize into
//! the `#[serde(other)]` fallbacks so an older peer degrades gracefully.

use serde::{Deserialize, Serialize};

/// Every message on the wire is wrapped so the peer can check protocol
/// compatibility before acting on the payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol: u32,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(payload: T) -> Self {
        Envelope {
            protocol: crate::PROTOCOL_VERSION,
            payload,
        }
    }
}

/// A command from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness check. Daemon replies [`Response::Pong`].
    Ping,
    /// Human-facing daemon status snapshot.
    Status,
    /// Report the daemon's build version.
    Version,
    /// Ask the daemon to shut down cleanly.
    Shutdown,

    /// List the local `.test` sites the daemon is serving.
    ListSites,
    /// Park a directory: its subdirectories each become a site.
    Park { path: String },
    /// Unpark a previously-parked directory.
    Unpark { path: String },
    /// Link a project directory under an explicit site name.
    Link { name: Option<String>, path: String },
    /// Remove a linked site by name.
    Unlink { name: String },
    /// Point an existing linked site at a different directory, keeping the name
    /// and every setting (PHP, HTTPS, runtime, Xdebug…) it already had.
    Relink { name: String, path: String },
    /// Bulk import parked dirs + named links (one save/reconcile). Links are
    /// `(name, path)` pairs.
    ImportSites { parked: Vec<String>, links: Vec<(String, String)> },
    /// Bulk remove parked dirs + links (by name), one save/reconcile.
    RemoveSites { parked: Vec<String>, links: Vec<String> },
    /// Switch `.test` resolution between `"resolver"` and `"hosts"`.
    SetResolution { mode: String },
    /// Set a linked site's runtime: `fpm` | `octane-swoole` |
    /// `octane-roadrunner` | `octane-frankenphp`.
    SetRuntime { site: String, runtime: String },
    /// Set the Xdebug mode for one site, or the default when `site` is `None`.
    /// `port`/`ide_key` update the shared IDE settings; any field left `None`
    /// is untouched.
    SetXdebug {
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        site: Option<String>,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        ide_key: Option<String>,
    },
    /// Toggle HTTPS for a site (Phase 4 — accepted now, enforced later).
    Secure { name: String, secure: bool },
    /// Turn the SPX flame-graph profiler on or off for a site. The site gets (or
    /// loses) its own php-fpm master with SPX loaded.
    SetProfile { site: String, on: bool },
    /// Set (or clear, when `script` is None) a site's opcache preload script,
    /// relative to its project root. A preloaded site gets its own php-fpm master
    /// with `opcache.preload` set.
    SetPreload { site: String, script: Option<String> },
    /// Manage reverse proxies. `action` ∈ set|remove (list is via ListSites).
    Proxy { action: String, name: String, target: Option<String> },
    /// Pin a PHP version for a site (or set the default when `site` is None).
    UsePhp { version: String, site: Option<String> },
    /// Re-read the config from disk and reconcile backends.
    Reload,
    /// Hard-reset all backends: stop + reap all php-fpm/Octane servers, rebuild.
    RepairBackends,
    /// Manage database/cache services and instances.
    /// `action` ∈ list|versions|create|start|stop|restart|delete.
    Service {
        action: String,
        /// Instance name (create/start/stop/restart/delete) or engine filter (versions).
        name: Option<String>,
        /// Engine for `create`/`versions` (postgres|mysql|mariadb|redis).
        #[serde(default)]
        engine: Option<String>,
        /// Version label for `create`.
        #[serde(default)]
        version: Option<String>,
        /// Port for `create`.
        #[serde(default)]
        port: Option<u16>,
    },
    /// Database operation. `action` ∈ list|create|drop|backup|restore. `port`
    /// targets a specific instance (else the engine's default port); `file` is
    /// the dump path for backup/restore.
    Db {
        action: String,
        engine: String,
        name: Option<String>,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        file: Option<String>,
    },
    /// Manage the TLDs sites answer on. `action` ∈ list|add|remove.
    Tld { action: String, name: Option<String> },
    /// Branch-aware databases for a linked site (Postgres).
    /// `action` ∈ attach|detach|switch|branches|drop-branch.
    /// `branch`: target for switch (defaults to the checked-out branch) or the
    /// branch to drop. `database`: base DB name for attach (defaults to the
    /// project's `.env` `DB_DATABASE`). `port`: instance port (default 5432).
    BranchDb {
        action: String,
        site: String,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        database: Option<String>,
        #[serde(default)]
        port: Option<u16>,
    },

    /// Apply a project's `dpl.toml` spec: link the site and set every captured
    /// setting in one save + reconcile. `path` is the project root.
    ApplySpec { path: String, spec: crate::spec::SiteSpec },
    /// Capture the current settings of the site at `path` as a `SiteSpec`
    /// (returned as a TOML `Message`), for `dpl up --save`.
    ExportSpec { path: String },

    /// Any op this build doesn't understand (peer is newer).
    #[serde(other)]
    Unknown,
}

/// The daemon's reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Version {
        version: String,
    },
    Status {
        status: DaemonStatus,
    },
    /// The local sites the daemon is serving, plus the ports/TLD in use.
    Sites {
        sites: Vec<SiteInfo>,
        http_port: u16,
        tld: String,
    },
    /// Generic acknowledgement for fire-and-forget ops, with a human message.
    Ok,
    /// Acknowledgement carrying a short status line for the CLI to print.
    Message {
        text: String,
    },
    /// Local database/cache services and their state.
    ServiceList {
        services: Vec<ServiceInfo>,
    },
    /// Available engine versions discovered from DBngin/Homebrew.
    Versions {
        versions: Vec<VersionInfo>,
    },
    /// A list of lines (e.g. database names) for the CLI to print.
    Lines {
        lines: Vec<String>,
    },
    /// The daemon rejected or failed the request.
    Error {
        message: String,
    },

    #[serde(other)]
    Unknown,
}

/// One local site, as reported to the CLI/GUI over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub name: String,
    pub host: String,
    pub url: String,
    pub path: String,
    pub docroot: String,
    pub source: String,
    #[serde(default)]
    pub php: Option<String>,
    #[serde(default)]
    pub secure: bool,
    /// True when a PHP backend is currently running for this site.
    #[serde(default)]
    pub serving: bool,
    /// Runtime: `None`/`"fpm"` = php-fpm, else an Octane server.
    #[serde(default)]
    pub runtime: Option<String>,
    /// Detected project type, e.g. `"Laravel (^12)"`, `"Symfony"`, `"WordPress"`.
    #[serde(default)]
    pub framework: Option<String>,
    /// PHP version the project requires (composer.json `require.php`, e.g. `"^8.3"`).
    #[serde(default)]
    pub requires_php: Option<String>,
    /// Effective Xdebug mode, e.g. `"off"`, `"debug"`, `"debug,develop"`.
    #[serde(default)]
    pub xdebug: Option<String>,
    /// Whether Xdebug is installed for this site's PHP version at all.
    #[serde(default)]
    pub xdebug_installed: bool,
    /// Whether the SPX profiler is on for this site.
    #[serde(default)]
    pub profile: bool,
    /// Whether SPX is installed for this site's PHP version at all.
    #[serde(default)]
    pub profile_installed: bool,
    /// This site's opcache preload script (relative to the project root), if set.
    #[serde(default)]
    pub preload: Option<String>,
    /// Pinned Node version for this site (from `.nvmrc`/`.node-version`/
    /// `package.json`), if any.
    #[serde(default)]
    pub node: Option<String>,
    /// Which file the Node pin came from (`.nvmrc`, `.node-version`, `package.json`).
    #[serde(default)]
    pub node_source: Option<String>,
    /// Branch-aware base database, when attached (see `dpl db attach`).
    #[serde(default)]
    pub database: Option<String>,
    /// Which git branch's data is live in `database`.
    #[serde(default)]
    pub db_branch: Option<String>,
}

/// One local database/cache service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Instance name, or the engine name for an external default-port service.
    pub name: String,
    pub engine: String,
    pub port: u16,
    pub installed: bool,
    pub running: bool,
    /// Running, but managed by something else (DBngin, Postgres.app, brew
    /// services) rather than by our daemon — we can use it but not stop it.
    #[serde(default)]
    pub external: bool,
    /// Version label for a managed instance (empty for external services).
    #[serde(default)]
    pub version: String,
}

/// An installed engine version we can spin an instance from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub engine: String,
    pub version: String,
    /// Where it came from (`dbngin`, `homebrew`).
    pub source: String,
}

/// Snapshot of what the daemon is currently doing. Grows as later phases
/// add sites, PHP pools, and services; every field is defaulted so an
/// older CLI can still decode a newer daemon's status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    /// Seconds since the daemon started.
    pub uptime_secs: u64,
    /// Whether the HTTP/HTTPS proxy is listening (Phase 2+).
    #[serde(default)]
    pub proxy_running: bool,
    /// Number of registered local sites (Phase 2+).
    #[serde(default)]
    pub site_count: usize,
}

/// Errors specific to speaking the protocol (framing/handshake), distinct
/// from the daemon returning a [`Response::Error`] payload.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("daemon speaks protocol v{daemon}, this client speaks v{client}; upgrade the mismatched binary")]
    VersionMismatch { client: u32, daemon: u32 },

    #[error("frame too large: {0} bytes")]
    FrameTooLarge(u32),

    #[error("connection closed mid-frame")]
    Truncated,
}

/// Hard cap on a single frame (4 MiB) so a corrupt length prefix can't make
/// us try to allocate gigabytes.
pub const MAX_FRAME: u32 = 4 * 1024 * 1024;
