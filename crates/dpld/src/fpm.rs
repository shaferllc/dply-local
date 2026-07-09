//! Supervises php-fpm master processes — one per distinct PHP binary in use.
//!
//! This is the speed win over `php -S`: php-fpm keeps a pool of workers with
//! opcache warm, so requests don't pay per-request interpreter startup. One
//! master serves every site on that PHP version; the proxy passes each
//! request's `SCRIPT_FILENAME`/`DOCUMENT_ROOT` over FastCGI, so a single pool
//! handles many document roots.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::{Child, Command};

struct Master {
    port: u16,
    child: Child,
}

pub struct FpmManager {
    /// Keyed by the `php` binary path; the matching `php-fpm` is derived.
    masters: BTreeMap<PathBuf, Master>,
}

impl FpmManager {
    pub fn new() -> Self {
        FpmManager { masters: BTreeMap::new() }
    }

    pub fn count(&self) -> usize {
        self.masters.len()
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

    /// The FastCGI address for a PHP binary, if its master is running.
    pub fn addr_for(&self, php_bin: &Path) -> Option<SocketAddr> {
        self.masters
            .get(php_bin)
            .map(|m| SocketAddr::from(([127, 0, 0, 1], m.port)))
    }

    /// Ensure a php-fpm master is running for `php_bin`, returning its port.
    pub fn ensure(&mut self, php_bin: &Path) -> Result<u16> {
        if let Some(m) = self.masters.get_mut(php_bin) {
            // Reap if it died; otherwise reuse.
            if matches!(m.child.try_wait(), Ok(Some(_))) {
                self.masters.remove(php_bin);
            } else {
                return Ok(m.port);
            }
        }

        let fpm_bin = derive_fpm(php_bin)
            .with_context(|| format!("no php-fpm found next to {}", php_bin.display()))?;
        let port = free_port()?;
        let conf = write_conf(php_bin, port)?;
        let log = log_file(php_bin)?;

        // Inject the dumps receiver env so `dumps()` works with zero config in
        // any served site. `clear_env = no` in the pool config lets these reach
        // the PHP workers.
        let child = Command::new(&fpm_bin)
            .arg("--nodaemonize")
            .arg("--fpm-config")
            .arg(&conf)
            .env("DUMPS_HOST", "127.0.0.1")
            .env("DUMPS_PORT", crate::dumps::port().to_string())
            .env("DUMPS_ENABLED", "1")
            .stdout(std::process::Stdio::from(log.try_clone().unwrap_or_else(|_| log_null())))
            .stderr(std::process::Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {}", fpm_bin.display()))?;

        tracing::info!(fpm = %fpm_bin.display(), port, "php-fpm master started");
        self.masters.insert(php_bin.to_path_buf(), Master { port, child });
        Ok(port)
    }

    /// Stop masters whose PHP binary is no longer used by any site.
    pub fn retain(&mut self, keep: &BTreeSet<PathBuf>) {
        let drop: Vec<PathBuf> = self
            .masters
            .keys()
            .filter(|k| !keep.contains(*k))
            .cloned()
            .collect();
        for k in drop {
            if let Some(mut m) = self.masters.remove(&k) {
                let _ = m.child.start_kill();
                tracing::info!(php = %k.display(), "php-fpm master stopped");
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

/// Write (or overwrite) a minimal php-fpm config for this binary and return its
/// path. `ondemand` keeps idle footprint near zero.
fn write_conf(php_bin: &Path, port: u16) -> Result<PathBuf> {
    let dir = dpl_core::paths::dpl_dir(None)?.join("php").join(key(php_bin));
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

fn log_file(php_bin: &Path) -> Result<std::fs::File> {
    let dir = dpl_core::paths::dpl_dir(None)?.join("php").join(key(php_bin));
    std::fs::create_dir_all(&dir).ok();
    std::fs::File::create(dir.join("master.log")).context("opening fpm master log")
}

fn log_null() -> std::fs::File {
    std::fs::OpenOptions::new()
        .write(true)
        .open(if cfg!(windows) { "NUL" } else { "/dev/null" })
        .expect("open null device")
}

/// A filesystem-safe key derived from the php binary path.
fn key(php_bin: &Path) -> String {
    php_bin
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn free_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

fn which(name: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("/usr/bin/env")
        .arg("which")
        .arg(name)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}
