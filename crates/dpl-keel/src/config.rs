//! Token + URL storage: `~/.keel/cloud.json`, overridable by the environment.
//!
//! Precedence (highest first): `$KEEL_CLOUD_TOKEN` / `$KEEL_CLOUD_URL` (the
//! keel-mcp convention, so one setup serves both clients) → the stored file →
//! the production default URL.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{KeelApiError, Result};

pub const DEFAULT_URL: &str = "https://app.keeljs.cloud";

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileShape {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

pub struct KeelConfig {
    path: PathBuf,
}

impl KeelConfig {
    /// `home_override` mirrors dpl's `--config-dir` for test isolation.
    pub fn new(home_override: Option<String>) -> Result<Self> {
        let home = dpl_core::paths::home(home_override.as_deref())
            .map_err(|e| KeelApiError::Config(e.to_string()))?;
        Ok(KeelConfig { path: home.join(".keel/cloud.json") })
    }

    fn read(&self) -> FileShape {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Effective base URL: env → file → production default.
    pub fn url(&self) -> String {
        std::env::var("KEEL_CLOUD_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.read().url)
            .unwrap_or_else(|| DEFAULT_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    /// Effective token: env → file. None = not logged in.
    pub fn token(&self) -> Option<String> {
        std::env::var("KEEL_CLOUD_TOKEN")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.read().token)
    }

    /// Persist a token (and optionally a non-default URL), 0600 in a 0700 dir.
    pub fn login(&self, token: &str, url: Option<&str>) -> Result<()> {
        let shape = FileShape {
            url: url
                .map(|u| u.trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty() && u != DEFAULT_URL),
            token: Some(token.trim().to_string()),
        };
        let dir = self.path.parent().expect("config path has a parent");
        std::fs::create_dir_all(dir).map_err(|e| KeelApiError::Config(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let body = serde_json::to_string_pretty(&shape).map_err(|e| KeelApiError::Config(e.to_string()))?;
        std::fs::write(&self.path, body).map_err(|e| KeelApiError::Config(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Forget the stored token (the URL is kept so a re-login lands on the
    /// same instance).
    pub fn logout(&self) -> Result<()> {
        let mut shape = self.read();
        shape.token = None;
        let body = serde_json::to_string_pretty(&shape).map_err(|e| KeelApiError::Config(e.to_string()))?;
        if self.path.exists() {
            std::fs::write(&self.path, body).map_err(|e| KeelApiError::Config(e.to_string()))?;
        }
        Ok(())
    }
}
