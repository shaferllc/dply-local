//! Where the toolchain keeps its files. Local-dev state lives under
//! `~/.dpl`; the runtime socket prefers `$XDG_RUNTIME_DIR` and falls back
//! to `~/.dpl/run`. The dply auth store deliberately points at
//! `~/.dply/config.json` so we share a login with the existing `dply` CLI.

use std::path::PathBuf;

use crate::error::{CoreError, Result};

/// Resolve the user's home directory, honouring an explicit override
/// (the `--config-dir` escape hatch) before `$HOME`.
pub fn home(override_home: Option<&str>) -> Result<PathBuf> {
    if let Some(h) = override_home {
        if !h.is_empty() {
            return Ok(PathBuf::from(h));
        }
    }
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .ok_or(CoreError::NoHome),
    }
}

/// `~/.dpl` — root of all local-dev state.
pub fn dpl_dir(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(home(override_home)?.join(".dpl"))
}

/// `~/.dpl/config.toml` — parked dirs, linked sites, per-site settings.
pub fn local_config(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(dpl_dir(override_home)?.join("config.toml"))
}

/// `~/.dply/config.json` — shared dply auth store (same file the PHP
/// `dply` CLI uses, so a login in either tool works in both).
pub fn dply_config(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(home(override_home)?.join(".dply").join("config.json"))
}

/// Directory for the CLI↔daemon unix socket. Prefers the per-user runtime
/// dir on Linux; everywhere else uses `~/.dpl/run`.
pub fn runtime_dir(override_home: Option<&str>) -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("dpl"));
        }
    }
    Ok(dpl_dir(override_home)?.join("run"))
}

/// Path to the daemon control socket.
pub fn daemon_socket(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(runtime_dir(override_home)?.join("dpld.sock"))
}

/// `~/.dpl/certs` — local CA + per-site leaf certificates (Phase 4).
pub fn certs_dir(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(dpl_dir(override_home)?.join("certs"))
}

/// `~/.dpl/logs` — daemon and per-service logs.
pub fn logs_dir(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(dpl_dir(override_home)?.join("logs"))
}

/// `~/.dpl/mail` — captured emails (one `.eml` per message).
pub fn mail_dir(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(dpl_dir(override_home)?.join("mail"))
}

/// `~/.dpl/services` — per-instance data directories.
pub fn services_dir(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(dpl_dir(override_home)?.join("services"))
}

/// `~/.dpl/services.toml` — the service-instance registry.
pub fn services_config(override_home: Option<&str>) -> Result<PathBuf> {
    Ok(dpl_dir(override_home)?.join("services.toml"))
}

