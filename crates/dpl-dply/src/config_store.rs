//! Reads/writes `~/.dply/config.json` — the **same file the PHP `dply` CLI
//! uses** — so a login in either tool is honoured by both. Direct port of
//! dply-cli's `ConfigStore.php`.
//!
//! On-disk shape:
//! ```json
//! {
//!   "default_host": "https://dply.io",
//!   "hosts": {
//!     "https://dply.io":    { "token": "...", "user": {...}, "updated_at": "..." },
//!     "https://dplyi.test": { "token": "...", "user": {...}, "updated_at": "..." }
//!   }
//! }
//! ```
//! Written mode 0600 in a 0700 directory so other local users can't harvest
//! the token.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{DplyApiError, Result};

/// Final fallback host, matching dply-cli.
const DEFAULT_HOST: &str = "https://dply.io";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub default_host: Option<String>,
    #[serde(default)]
    pub hosts: BTreeMap<String, HostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    pub token: String,
    #[serde(default)]
    pub user: Value,
    #[serde(default)]
    pub updated_at: String,
}

/// Handle over the config file. `home_override` mirrors the CLI's
/// `--config-dir` escape hatch; `None` means "use `$HOME`".
pub struct ConfigStore {
    home_override: Option<String>,
}

impl ConfigStore {
    pub fn new(home_override: Option<String>) -> Self {
        ConfigStore { home_override }
    }

    pub fn path(&self) -> Result<PathBuf> {
        Ok(dpl_core::paths::dply_config(self.home_override.as_deref())?)
    }

    /// Load the file, returning defaults if it doesn't exist. A corrupt file
    /// is a hard error (matching dply-cli, which tells the user to delete it).
    pub fn load(&self) -> Result<ConfigFile> {
        let path = self.path()?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ConfigFile::default()),
            Err(e) => return Err(dpl_core::CoreError::io(&path, e).into()),
        };
        if raw.trim().is_empty() {
            return Ok(ConfigFile::default());
        }
        serde_json::from_str(&raw).map_err(|source| DplyApiError::Decode {
            context: format!(
                "config file at {} (delete it and run `dpl dply login` again)",
                path.display()
            ),
            source,
        })
    }

    /// Serialize and atomically write, creating `~/.dply` (0700) and the file
    /// (0600) as needed.
    pub fn save(&self, data: &ConfigFile) -> Result<()> {
        let path = self.path()?;
        if let Some(dir) = path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|e| dpl_core::CoreError::io(dir, e))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
                }
            }
        }
        let mut body = serde_json::to_string_pretty(data).map_err(|source| {
            DplyApiError::Decode {
                context: "serializing dply config".into(),
                source,
            }
        })?;
        body.push('\n');
        dpl_core::config::atomic_write(&path, body.as_bytes())?;
        Ok(())
    }

    /// Normalize a host string: trim, lowercase, drop trailing slash — the
    /// canonical key form used everywhere.
    pub fn normalize_host(host: &str) -> String {
        host.trim().to_lowercase().trim_end_matches('/').to_string()
    }

    /// Resolve the host for this invocation. Precedence matches dply-cli:
    /// explicit flag → `$DPLY_HOST` → stored `default_host` → fallback.
    pub fn resolve_host(&self, flag: Option<&str>) -> Result<String> {
        let host = flag
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| std::env::var("DPLY_HOST").ok().filter(|s| !s.is_empty()))
            .or_else(|| self.load().ok().and_then(|c| c.default_host))
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        Ok(host.trim_end_matches('/').to_string())
    }

    pub fn default_host(&self) -> Result<Option<String>> {
        Ok(self.load()?.default_host)
    }

    pub fn host_entry(&self, host: &str) -> Result<Option<HostEntry>> {
        let key = Self::normalize_host(host);
        Ok(self.load()?.hosts.get(&key).cloned())
    }

    /// Token for `host`. `$DPLY_TOKEN` wins (used by the in-app console),
    /// then the stored per-host token.
    pub fn token_for(&self, host: &str) -> Result<Option<String>> {
        if let Ok(t) = std::env::var("DPLY_TOKEN") {
            if !t.is_empty() {
                return Ok(Some(t));
            }
        }
        Ok(self
            .host_entry(host)?
            .map(|e| e.token)
            .filter(|t| !t.is_empty()))
    }

    /// Store a token (and optional user profile) for `host`, optionally
    /// making it the new default.
    pub fn set_host(
        &self,
        host: &str,
        token: &str,
        user: Value,
        make_default: bool,
    ) -> Result<()> {
        let key = Self::normalize_host(host);
        let mut data = self.load()?;
        data.hosts.insert(
            key.clone(),
            HostEntry {
                token: token.to_string(),
                user,
                updated_at: now_atom(),
            },
        );
        if make_default || data.default_host.is_none() {
            data.default_host = Some(key);
        }
        self.save(&data)
    }

    /// Forget a host's token; repoint `default_host` if it was the default.
    pub fn forget_host(&self, host: &str) -> Result<()> {
        let key = Self::normalize_host(host);
        let mut data = self.load()?;
        if data.hosts.remove(&key).is_none() {
            return Ok(());
        }
        if data.default_host.as_deref() == Some(key.as_str()) {
            data.default_host = data.hosts.keys().next().cloned();
        }
        self.save(&data)
    }
}

/// RFC 3339 timestamp, matching PHP's `DATE_ATOM`.
fn now_atom() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}
