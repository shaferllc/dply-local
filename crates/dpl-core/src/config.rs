//! Local-dev configuration persisted at `~/.dpl/config.toml`.
//!
//! This is the daemon's source of truth for what to serve: which folders
//! are "parked" (every subdirectory auto-serves as `<dir>.test`), which
//! individual projects are "linked", and per-site overrides (PHP version,
//! HTTPS). Phase 0 only defines the shape and load/save; the daemon starts
//! consuming it in Phase 2.
//!
//! Writes are atomic (temp file + rename) and the file is created 0600 so
//! another local user can't read paths/secrets out of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// The default TLD local sites answer on when none is configured.
pub const SITE_TLD: &str = "test";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalConfig {
    /// TLDs sites answer on (e.g. `["test", "localhost"]`). The first is
    /// "primary" and used to build a site's canonical URL; requests to any
    /// listed TLD are served. Empty means the built-in default (`test`).
    pub tlds: Vec<String>,
    /// Folders whose immediate subdirectories each become a `<name>.<tld>` site.
    pub parked: Vec<PathBuf>,
    /// Explicitly linked projects, keyed by site name.
    pub links: BTreeMap<String, Link>,
    /// Default PHP version applied to sites with no explicit override
    /// (e.g. `"8.4"`). `None` means "use whatever `php` is on PATH".
    pub default_php: Option<String>,
}

impl LocalConfig {
    /// Configured TLDs, falling back to the built-in default.
    pub fn tlds(&self) -> Vec<String> {
        if self.tlds.is_empty() {
            vec![SITE_TLD.to_string()]
        } else {
            self.tlds.clone()
        }
    }

    /// The primary TLD used to build canonical site URLs.
    pub fn primary_tld(&self) -> String {
        self.tlds.first().cloned().unwrap_or_else(|| SITE_TLD.to_string())
    }
}

/// A user-created database/cache instance: a specific engine version on its own
/// port with an isolated data directory. Persisted in `~/.dpl/services.toml`
/// so it survives restarts and can be supervised on boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInstance {
    /// Unique instance name (also the data-dir folder).
    pub name: String,
    /// Engine: `postgres` | `mysql` | `mariadb` | `redis`.
    pub engine: String,
    /// Version label (e.g. `17.0`), matched against discovered binaries.
    pub version: String,
    /// Loopback port this instance listens on.
    pub port: u16,
}

/// The service-instance registry (`~/.dpl/services.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesConfig {
    pub instances: Vec<ServiceInstance>,
}

impl ServicesConfig {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) if raw.trim().is_empty() => Ok(Self::default()),
            Ok(raw) => toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
            }
        }
        let body = toml::to_string_pretty(self)?;
        atomic_write(path, body.as_bytes())
    }
}

/// A single linked project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Absolute path to the project root.
    pub path: PathBuf,
    /// Pinned PHP version for this site, if any (overrides `default_php`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    /// Whether this site is served over HTTPS (Phase 4).
    #[serde(default)]
    pub secure: bool,
}

impl LocalConfig {
    /// Load from `path`, returning defaults if the file doesn't exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) if raw.trim().is_empty() => Ok(Self::default()),
            Ok(raw) => toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    /// Serialize and atomically write to `path`, creating the parent
    /// directory (0700) and the file (0600) if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let body = toml::to_string_pretty(self)?;
        atomic_write(path, body.as_bytes())
    }
}

/// Create `dir` (and parents) with mode 0700 on Unix. A no-op if it exists.
fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
    set_mode(dir, 0o700)?;
    Ok(())
}

/// Write `bytes` to `path` via a temp file + rename so a reader never sees
/// a half-written config. The file ends up mode 0600.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| CoreError::io(&tmp, e))?;
    set_mode(&tmp, 0o600)?;
    std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, perms).map_err(|e| CoreError::io(path, e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
