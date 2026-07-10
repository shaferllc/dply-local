//! Supervises php-fpm master processes — one per (PHP binary, Xdebug mode).
//!
//! This is the speed win over `php -S`: php-fpm keeps a pool of workers with
//! opcache warm, so requests don't pay per-request interpreter startup. One
//! master serves every site sharing a PHP version *and* an Xdebug mode; the
//! proxy passes each request's `SCRIPT_FILENAME`/`DOCUMENT_ROOT` over FastCGI,
//! so a single pool handles many document roots.
//!
//! Xdebug is why the key is a pair rather than just the binary. `xdebug.mode` is
//! read once when the PHP process starts, and cannot be set per-request or via a
//! pool's `php_admin_value` — only through the `XDEBUG_MODE` environment
//! variable on the process itself. Xdebug's `MINIT` runs in the **master**,
//! before workers fork, so the variable has to be set when we spawn the master.
//! Two sites on PHP 8.3 wanting different modes therefore need two masters.
//!
//! Sites with Xdebug off (the common case) all share one master that never loads
//! the extension, so they pay nothing for the feature.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dpl_core::xdebug::{self, Mode};
use tokio::process::{Child, Command};

/// Identifies one php-fpm master: a PHP binary plus the Xdebug mode it starts
/// with. Ordered so it can key a `BTreeMap`.
pub type MasterKey = (PathBuf, Mode);

struct Master {
    port: u16,
    child: Child,
}

pub struct FpmManager {
    masters: BTreeMap<MasterKey, Master>,
    /// Shared IDE-side Xdebug settings, refreshed from config each reconcile.
    xdebug_settings: xdebug::Settings,
}

impl FpmManager {
    pub fn new() -> Self {
        FpmManager { masters: BTreeMap::new(), xdebug_settings: xdebug::Settings::default() }
    }

    pub fn count(&self) -> usize {
        self.masters.len()
    }

    /// Update the IDE-side Xdebug settings. Returns true when they changed, so
    /// the caller knows the running masters hold a stale loader ini and must be
    /// restarted for a new port or IDE key to take effect.
    pub fn set_xdebug_settings(&mut self, settings: xdebug::Settings) -> bool {
        let changed = self.xdebug_settings != settings;
        self.xdebug_settings = settings;
        changed
    }

    /// Kill php-fpm masters left over from a previous daemon. `kill_on_drop`
    /// doesn't fire when launchd SIGKILLs the daemon, so masters accumulate
    /// across restarts (each holds a pool and leaks). Call once at startup,
    /// before the first reconcile, so we start from a clean slate.
    pub fn kill_orphans() {
        // Match only our own pool configs under ~/.dpl/php/ — never Valet's or
        // the user's other php-fpm instances.
        if let Ok(dir) = dpl_core::paths::dpl_dir(None) {
            let pattern = dir.join("php").display().to_string();
            let _ = std::process::Command::new("pkill")
                .arg("-f")
                .arg(&pattern)
                .status();
        }
    }

    /// The FastCGI address for a PHP binary at a given Xdebug mode, if its
    /// master is running.
    pub fn addr_for(&self, php_bin: &Path, mode: &Mode) -> Option<SocketAddr> {
        self.masters
            .get(&(php_bin.to_path_buf(), mode.clone()))
            .map(|m| SocketAddr::from(([127, 0, 0, 1], m.port)))
    }

    /// Ensure a php-fpm master is running for `php_bin` at `mode`, returning its
    /// port.
    pub fn ensure(&mut self, php_bin: &Path, mode: &Mode) -> Result<u16> {
        let key: MasterKey = (php_bin.to_path_buf(), mode.clone());
        if let Some(m) = self.masters.get_mut(&key) {
            // Reap if it died; otherwise reuse.
            if matches!(m.child.try_wait(), Ok(Some(_))) {
                self.masters.remove(&key);
            } else {
                return Ok(m.port);
            }
        }

        let fpm_bin = derive_fpm(php_bin)
            .with_context(|| format!("no php-fpm found next to {}", php_bin.display()))?;
        let port = free_port()?;
        let conf = write_conf(php_bin, mode, port)?;
        let log = log_file(php_bin, mode)?;

        // Inject the dumps receiver env so `dumps()` works with zero config in
        // any served site. `clear_env = no` in the pool config lets these reach
        // the PHP workers.
        let mut cmd = Command::new(&fpm_bin);
        cmd.arg("--nodaemonize")
            .arg("--fpm-config")
            .arg(&conf)
            .env("DUMPS_HOST", "127.0.0.1")
            .env("DUMPS_PORT", crate::dumps::port().to_string())
            .env("DUMPS_ENABLED", "1");

        // The pool runs with `clear_env = no`, so whatever environment launched
        // the daemon reaches the PHP workers. Two Xdebug variables have to be
        // neutralised or the user's shell quietly wins over dpl's config:
        //
        // - XDEBUG_CONFIG (e.g. `idekey=VSCODE`, commonly exported by dotfiles)
        //   overrides ini settings, so `dpl xdebug ide`/`port` would do nothing.
        // - XDEBUG_MODE is set below for *every* master, `off` included. Left to
        //   the ambient value, an inherited `XDEBUG_MODE=debug` would switch on
        //   an "off" master whenever the system conf.d already loads Xdebug.
        cmd.env_remove("XDEBUG_CONFIG").env("XDEBUG_MODE", mode.as_str());

        // Only *load* Xdebug into masters that want it. `PHP_INI_SCAN_DIR` points
        // at dpl's own conf.d, prefixed with `:` so PHP still reads the system
        // one. Sites with Xdebug off never load the extension at all.
        if !mode.is_off() {
            match xdebug::write_loader(None, php_bin, &self.xdebug_settings) {
                Ok(Some(dir)) => {
                    cmd.env("PHP_INI_SCAN_DIR", xdebug::scan_dir_env(&dir));
                }
                Ok(None) => {
                    tracing::warn!(
                        php = %php_bin.display(),
                        %mode,
                        "Xdebug requested but not installed for this PHP; \
                         serving without it (try `dpl php ext-install <version> xdebug`)"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "writing the Xdebug loader ini failed"),
            }
        }

        let child = cmd
            .stdout(std::process::Stdio::from(log.try_clone().unwrap_or_else(|_| log_null())))
            .stderr(std::process::Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {}", fpm_bin.display()))?;

        tracing::info!(fpm = %fpm_bin.display(), port, %mode, "php-fpm master started");
        self.masters.insert(key, Master { port, child });
        Ok(port)
    }

    /// Stop masters no longer wanted by any site.
    pub fn retain(&mut self, keep: &BTreeSet<MasterKey>) {
        let drop: Vec<MasterKey> = self
            .masters
            .keys()
            .filter(|k| !keep.contains(*k))
            .cloned()
            .collect();
        for k in drop {
            if let Some(mut m) = self.masters.remove(&k) {
                let _ = m.child.start_kill();
                tracing::info!(php = %k.0.display(), mode = %k.1, "php-fpm master stopped");
            }
        }
    }
}

/// Derive the `php-fpm` binary from a `php` binary path:
/// `.../bin/php` → `.../sbin/php-fpm`, else a sibling `php-fpm`, else `$PATH`.
fn derive_fpm(php_bin: &Path) -> Option<PathBuf> {
    let dir = php_bin.parent()?;
    let candidates = [
        dir.parent().map(|p| p.join("sbin/php-fpm")),
        Some(dir.join("php-fpm")),
    ];
    for c in candidates.into_iter().flatten() {
        if c.is_file() {
            return Some(c);
        }
    }
    which("php-fpm")
}

/// The directory holding one master's generated config and logs. Masters differ
/// only by Xdebug mode, so the mode is part of the path.
fn master_dir(php_bin: &Path, mode: &Mode) -> Result<PathBuf> {
    Ok(dpl_core::paths::php_dir(None, php_bin)?.join(format!("fpm-{}", mode.slug())))
}

/// Write (or overwrite) a minimal php-fpm config for this master and return its
/// path. `ondemand` keeps idle footprint near zero — which is what makes a
/// second, Xdebug-enabled master cheap to leave running.
fn write_conf(php_bin: &Path, mode: &Mode, port: u16) -> Result<PathBuf> {
    let dir = master_dir(php_bin, mode)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let log = dir.join("fpm.log");
    let conf = dir.join("php-fpm.conf");
    let body = format!(
        "[global]\n\
         error_log = {log}\n\
         daemonize = no\n\
         \n\
         [www]\n\
         listen = 127.0.0.1:{port}\n\
         listen.backlog = 256\n\
         pm = ondemand\n\
         pm.max_children = 40\n\
         pm.process_idle_timeout = 30s\n\
         pm.max_requests = 500\n\
         request_terminate_timeout = 90s\n\
         catch_workers_output = yes\n\
         clear_env = no\n",
        log = log.display(),
    );
    std::fs::write(&conf, body).with_context(|| format!("writing {}", conf.display()))?;
    Ok(conf)
}

fn log_file(php_bin: &Path, mode: &Mode) -> Result<std::fs::File> {
    let dir = master_dir(php_bin, mode)?;
    std::fs::create_dir_all(&dir).ok();
    std::fs::File::create(dir.join("master.log")).context("opening fpm master log")
}

fn log_null() -> std::fs::File {
    std::fs::OpenOptions::new()
        .write(true)
        .open(if cfg!(windows) { "NUL" } else { "/dev/null" })
        .expect("open null device")
}

fn free_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

use dpl_core::tools::which;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masters_for_different_modes_get_different_dirs() {
        let php = Path::new("/opt/homebrew/opt/php@8.3/bin/php");
        let off = master_dir(php, &Mode::off()).unwrap();
        let dbg = master_dir(php, &Mode::parse("debug").unwrap()).unwrap();
        assert_ne!(off, dbg);
        assert!(off.ends_with("fpm-off"));
        assert!(dbg.ends_with("fpm-debug"));
        // Both hang off the same per-binary directory, which also holds the
        // shared conf.d the loader ini is written into.
        assert_eq!(off.parent(), dbg.parent());
    }

    #[test]
    fn equivalent_mode_spellings_share_one_master_dir() {
        let php = Path::new("/opt/homebrew/opt/php@8.3/bin/php");
        let a = master_dir(php, &Mode::parse("debug,develop").unwrap()).unwrap();
        let b = master_dir(php, &Mode::parse("develop,debug").unwrap()).unwrap();
        assert_eq!(a, b);
    }
}
