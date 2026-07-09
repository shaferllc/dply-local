//! Supervises Laravel Octane application servers (Swoole / RoadRunner /
//! FrankenPHP) — one long-lived worker process per site that uses a non-`fpm`
//! runtime.
//!
//! Unlike php-fpm (which the proxy talks to over FastCGI), an Octane server is
//! an HTTP server in its own right. The daemon starts `php artisan octane:start`
//! in the project root on a private loopback port and reverse-proxies the
//! site's `.test` host to it — exactly the upstream role the `proxy` feature
//! already implements.

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use tokio::process::{Child, Command};

struct Server {
    port: u16,
    /// The runtime this server was started for (e.g. `octane-swoole`); a change
    /// forces a restart.
    runtime: String,
    php_bin: PathBuf,
    child: Child,
}

pub struct AppServers {
    /// Keyed by site name.
    servers: BTreeMap<String, Server>,
}

impl AppServers {
    pub fn new() -> Self {
        AppServers { servers: BTreeMap::new() }
    }

    /// The loopback port a site's Octane server is listening on, if running.
    pub fn port_for(&self, site: &str) -> Option<u16> {
        self.servers.get(site).map(|s| s.port)
    }

    /// Ensure an Octane server is running for `site`. Returns the port to proxy
    /// to, or `None` if it couldn't start (e.g. Octane isn't installed in the
    /// project — the site then falls back to php-fpm).
    pub fn ensure(&mut self, site: &str, runtime: &str, project: &Path, php_bin: &Path) -> Option<u16> {
        // Reuse a healthy server on the same runtime + PHP binary.
        if let Some(s) = self.servers.get_mut(site) {
            let alive = matches!(s.child.try_wait(), Ok(None));
            if alive && s.runtime == runtime && s.php_bin == php_bin {
                return Some(s.port);
            }
            let _ = s.child.start_kill();
            self.servers.remove(site);
        }

        // Octane needs the project's `artisan` and the `laravel/octane` package
        // installed; without either, fall back to php-fpm (so the site still
        // serves) rather than proxying to a server that never starts.
        if !project.join("artisan").is_file() || !project.join("vendor/laravel/octane").is_dir() {
            tracing::warn!(site, "octane runtime set but laravel/octane isn't installed — using php-fpm");
            return None;
        }

        let server = runtime.strip_prefix("octane-").unwrap_or(runtime);
        let port = free_port()?;
        let child = Command::new(php_bin)
            .arg("artisan")
            .arg("octane:start")
            .arg("--server")
            .arg(server)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .current_dir(project)
            .env("DUMPS_HOST", "127.0.0.1")
            .env("DUMPS_PORT", crate::dumps::port().to_string())
            .env("DUMPS_ENABLED", "1")
            .stdout(std::process::Stdio::from(log_file(site)))
            .stderr(std::process::Stdio::from(log_file(site)))
            .kill_on_drop(true)
            .spawn()
            .ok()?;

        tracing::info!(site, runtime, port, "octane server started");
        self.servers.insert(site.to_string(), Server {
            port,
            runtime: runtime.to_string(),
            php_bin: php_bin.to_path_buf(),
            child,
        });
        Some(port)
    }

    /// Stop servers for sites no longer using an Octane runtime.
    pub fn retain(&mut self, keep: &BTreeSet<String>) {
        let stale: Vec<String> = self.servers.keys().filter(|k| !keep.contains(*k)).cloned().collect();
        for site in stale {
            if let Some(mut s) = self.servers.remove(&site) {
                let _ = s.child.start_kill();
                tracing::info!(site, "octane server stopped");
            }
        }
    }
}

/// Append-mode log file for a site's Octane server (`~/.dpl/logs/<site>-octane.log`).
fn log_file(site: &str) -> std::fs::File {
    let path = dpl_core::paths::logs_dir(None)
        .map(|d| d.join(format!("{site}-octane.log")))
        .unwrap_or_else(|_| PathBuf::from("/dev/null"));
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/tmp")));
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|_| std::fs::OpenOptions::new().write(true).open("/dev/null").expect("null"))
}

fn free_port() -> Option<u16> {
    let l = TcpListener::bind("127.0.0.1:0").ok()?;
    l.local_addr().ok().map(|a| a.port())
}
