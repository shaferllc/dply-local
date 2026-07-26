//! Supervises per-site Node dev servers — `npm run dev` and its pnpm/yarn/bun
//! equivalents — so a Vite or Next dev process is something the daemon owns
//! rather than something you keep a terminal tab open for.
//!
//! This is deliberately *not* a runtime the way Octane is. A Laravel site's dev
//! server doesn't serve the site: PHP still does, and the page it renders pulls
//! its assets from Vite on Vite's own port. Routing the site at the dev server
//! would break exactly the setup that's most common here. So the daemon supervises
//! the process and reports where it's listening; pointing a `.test` host at a port
//! remains `dpl proxy`'s job.
//!
//! Supervision means: start on demand, restart when it dies, back off when it
//! dies *repeatedly* (a broken script must not become a spin loop), and stop
//! cleanly — including the process tree, since `npm run dev` is a launcher that
//! forks the real server and killing only the launcher strands it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

/// How long to wait after each successive failed start. A dev server that dies
/// instantly is almost always a broken script or a missing dependency, and
/// hammering it helps nobody; after [`MAX_FAILURES`] we stop until the user
/// intervenes, so the daemon never spins on a project that can't run.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
const MAX_FAILURES: u32 = 5;

/// A start that survives this long is treated as a success, resetting the
/// backoff — a dev server that ran for a minute and then crashed is a different
/// animal from one that never came up.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);

struct Dev {
    /// package.json script being run, e.g. `dev`.
    script: String,
    /// The package manager it was started with — a change forces a restart.
    agent: String,
    project: PathBuf,
    child: Option<Child>,
    /// The child's process *group*, so the whole tree can be signalled.
    pgid: Option<u32>,
    started: Instant,
    /// Consecutive failed starts. Reset once a run stays up.
    failures: u32,
    /// Earliest time we may try again after a failure.
    retry_at: Option<Instant>,
    /// Port scraped from the server's own output, once it announces one.
    port: Option<u16>,
    /// Why it isn't running, when it isn't.
    detail: Option<String>,
    log: PathBuf,
}

/// What `dpl dev` and the GUI display for one site.
#[derive(Clone, Debug)]
pub struct DevInfo {
    pub site: String,
    pub script: String,
    pub agent: String,
    pub running: bool,
    pub port: Option<u16>,
    /// Human explanation when it isn't running (exit status, backoff, gave up).
    pub detail: Option<String>,
    pub log: String,
}

pub struct DevServers {
    servers: BTreeMap<String, Dev>,
}

impl DevServers {
    pub fn new() -> Self {
        DevServers { servers: BTreeMap::new() }
    }

    /// Ensure a dev server is configured for `site` and start it if it isn't
    /// running. Idempotent: called on every reconcile and on every supervise
    /// tick, so it must be cheap and must not restart a healthy server.
    pub fn ensure(&mut self, site: &str, script: &str, project: &Path) {
        let agent = dpl_core::node::detect_agent(project).agent.as_str().to_string();

        if let Some(dev) = self.servers.get_mut(site) {
            let same = dev.script == script && dev.agent == agent && dev.project == project;
            if same {
                if dev.is_alive() {
                    return;
                }
            } else {
                // Reconfigured — tear the old one down and start clean.
                dev.stop();
                self.servers.remove(site);
            }
        }

        let entry = self.servers.entry(site.to_string()).or_insert_with(|| Dev {
            script: script.to_string(),
            agent: agent.clone(),
            project: project.to_path_buf(),
            child: None,
            pgid: None,
            started: Instant::now(),
            failures: 0,
            retry_at: None,
            port: None,
            detail: None,
            log: log_path(site),
        });
        entry.script = script.to_string();
        entry.agent = agent;
        entry.project = project.to_path_buf();

        if entry.failures >= MAX_FAILURES {
            return;
        }
        if entry.retry_at.map(|t| Instant::now() < t).unwrap_or(false) {
            return;
        }
        entry.start(site);
    }

    /// Stop and forget dev servers for sites that no longer want one.
    pub fn retain(&mut self, keep: &BTreeSet<String>) {
        let stale: Vec<String> =
            self.servers.keys().filter(|k| !keep.contains(*k)).cloned().collect();
        for site in stale {
            if let Some(mut dev) = self.servers.remove(&site) {
                dev.stop();
                tracing::info!(site, "dev server stopped");
            }
        }
    }

    /// One supervision pass: reap the dead, restart what should be running, and
    /// pick up any port the server has announced since we last looked.
    pub fn supervise(&mut self) {
        let names: Vec<String> = self.servers.keys().cloned().collect();
        for site in names {
            let Some(dev) = self.servers.get_mut(&site) else { continue };
            if dev.is_alive() {
                if dev.port.is_none() {
                    dev.port = scrape_port(&dev.log);
                }
                // A run that has stayed up has earned a clean slate.
                if dev.failures > 0 && dev.started.elapsed() >= HEALTHY_AFTER {
                    dev.failures = 0;
                    dev.detail = None;
                }
                continue;
            }
            if dev.failures >= MAX_FAILURES {
                continue;
            }
            if dev.retry_at.map(|t| Instant::now() < t).unwrap_or(false) {
                continue;
            }
            tracing::info!(site = %site, "dev server restarting");
            dev.start(&site);
        }
    }

    /// Forget a site's failure history and start it again — `dpl dev restart`.
    /// The reset is the point: a server that gave up must be recoverable without
    /// toggling the feature off and on.
    pub fn restart(&mut self, site: &str) -> bool {
        let Some(dev) = self.servers.get_mut(site) else { return false };
        dev.stop();
        dev.failures = 0;
        dev.retry_at = None;
        dev.detail = None;
        dev.port = None;
        dev.start(site);
        true
    }

    pub fn statuses(&self) -> Vec<DevInfo> {
        self.servers
            .iter()
            .map(|(site, dev)| DevInfo {
                site: site.clone(),
                script: dev.script.clone(),
                agent: dev.agent.clone(),
                running: dev.child.is_some(),
                port: dev.port,
                detail: dev.detail.clone(),
                log: dev.log.to_string_lossy().into_owned(),
            })
            .collect()
    }

    /// The port a site's dev server is listening on, if it has announced one.
    pub fn port(&self, site: &str) -> Option<u16> {
        self.servers.get(site).and_then(|d| d.port)
    }

    pub fn is_running(&self, site: &str) -> bool {
        self.servers.get(site).map(|d| d.child.is_some()).unwrap_or(false)
    }
}

impl Dev {
    /// Whether the child is still up, reaping it (and recording why it went) if
    /// it has exited since the last check.
    fn is_alive(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else { return false };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                let long_enough = self.started.elapsed() >= HEALTHY_AFTER;
                self.child = None;
                self.pgid = None;
                self.port = None;
                if long_enough {
                    self.failures = 1;
                } else {
                    self.failures += 1;
                }
                self.detail = Some(if self.failures >= MAX_FAILURES {
                    format!(
                        "exited ({status}) {} times — giving up. Fix it, then `dpl dev restart`.",
                        self.failures
                    )
                } else {
                    format!("exited ({status}) — restarting")
                });
                self.retry_at = Some(Instant::now() + backoff_for(self.failures));
                false
            }
            // The handle is unusable; treat it as gone rather than assume health.
            Err(_) => {
                self.child = None;
                self.pgid = None;
                false
            }
        }
    }

    fn start(&mut self, site: &str) {
        let pin = dpl_core::node::read_pin(&self.project);
        let agent = dpl_core::node::Agent::parse(&self.agent)
            .unwrap_or(dpl_core::node::Agent::Npm);
        let args = dpl_core::node::run_args(&self.script, &[]);
        // The same invocation `dpl node run` builds, so a dev server starts on
        // the site's pinned Node version rather than whatever the daemon
        // inherited from launchd.
        let inv = dpl_core::node::pinned_invocation(
            dpl_core::node::detect_manager(),
            dpl_core::node::nvm_script().as_deref(),
            pin.as_ref(),
            agent.as_str(),
            &args,
        );

        let mut cmd = Command::new(&inv.program);
        cmd.args(&inv.args)
            .current_dir(&self.project)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(open_log(&self.log)))
            .stderr(std::process::Stdio::from(open_log(&self.log)))
            // Its own process group, so stopping it can signal the whole tree —
            // `npm run dev` forks the actual server and exits-by-proxy, so
            // killing just the child we hold would strand the real process.
            .process_group(0)
            .kill_on_drop(true);

        match cmd.spawn() {
            Ok(child) => {
                self.pgid = child.id();
                self.child = Some(child);
                self.started = Instant::now();
                self.port = None;
                self.detail = None;
                tracing::info!(site, script = %self.script, agent = %self.agent, "dev server started");
            }
            Err(e) => {
                self.failures += 1;
                self.detail = Some(format!("could not start {}: {e}", inv.program));
                self.retry_at = Some(Instant::now() + backoff_for(self.failures));
                tracing::warn!(site, error = %e, "dev server failed to start");
            }
        }
    }

    /// Stop the server and everything it spawned.
    fn stop(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            // A negative pid means "the process group" to kill(1) — the whole
            // npm → node tree, not just the launcher we happen to hold.
            let _ = std::process::Command::new("/bin/kill")
                .arg("-TERM")
                .arg(format!("-{pgid}"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        self.port = None;
    }
}

fn backoff_for(failures: u32) -> Duration {
    let idx = (failures.max(1) as usize - 1).min(BACKOFF.len() - 1);
    BACKOFF[idx]
}

/// Find the port a dev server announced in its own output.
///
/// Every tool in this space prints a "Local: http://localhost:5173/" banner, so
/// reading it back is far more reliable than guessing a port and forcing it on a
/// CLI whose flag differs per tool. Only the tail is read — these logs are
/// append-only and long-lived.
fn scrape_port(log: &Path) -> Option<u16> {
    let text = tail(log, 16_384)?;
    // Later lines win: a server that had to fall back to another port prints
    // the one it actually got after the one it wanted.
    let mut found = None;
    for marker in ["localhost:", "127.0.0.1:", "0.0.0.0:"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(marker) {
            let start = from + at + marker.len();
            let digits: String = text[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = digits.parse::<u16>() {
                if port > 0 {
                    found = Some((start, port));
                }
            }
            from = start.max(from + at + 1);
        }
    }
    found.map(|(_, port)| port)
}

/// The last `limit` bytes of a file, as lossy UTF-8.
fn tail(path: &Path, limit: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(limit);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// `~/.dpl/logs/<site>-dev.log`.
pub fn log_path(site: &str) -> PathBuf {
    dpl_core::paths::logs_dir(None)
        .map(|d| d.join(format!("{site}-dev.log")))
        .unwrap_or_else(|_| PathBuf::from("/dev/null"))
}

fn open_log(path: &Path) -> std::fs::File {
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/tmp")));
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|_| {
            std::fs::OpenOptions::new().write(true).open("/dev/null").expect("null")
        })
}

/// How often the daemon checks on its dev servers. Frequent enough that a crash
/// is noticed while you're still looking at the browser, cheap enough to ignore:
/// a `try_wait` per server plus a log tail for the ones without a known port.
const SUPERVISE_EVERY: Duration = Duration::from_secs(3);

/// Watch supervised dev servers, restarting any that die. Runs for the daemon's
/// lifetime; a dev server exiting is not a mutation, so nothing else would
/// notice it.
pub async fn watch(state: crate::server::DaemonState) {
    loop {
        tokio::time::sleep(SUPERVISE_EVERY).await;
        state.registry.lock().await.supervise_dev();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_for(1), Duration::from_secs(2));
        assert_eq!(backoff_for(2), Duration::from_secs(10));
        assert_eq!(backoff_for(4), Duration::from_secs(60));
        assert_eq!(backoff_for(99), Duration::from_secs(60));
    }

    #[test]
    fn reads_the_port_a_dev_server_announces() {
        let dir = std::env::temp_dir().join(format!("dpl-dev-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("vite.log");

        std::fs::write(&log, "  VITE v5.0.0  ready in 300 ms\n\n  ➜  Local:   http://localhost:5173/\n")
            .unwrap();
        assert_eq!(scrape_port(&log), Some(5173));

        // Port already taken → the tool retries and prints the one it got. The
        // later announcement is the true one.
        std::fs::write(
            &log,
            "Port 5173 is in use, trying another one...\n  ➜  Local: http://localhost:5174/\n",
        )
        .unwrap();
        assert_eq!(scrape_port(&log), Some(5174));

        std::fs::write(&log, "- ready started server on 0.0.0.0:3000, url: http://localhost:3000\n")
            .unwrap();
        assert_eq!(scrape_port(&log), Some(3000));

        std::fs::write(&log, "no address here\n").unwrap();
        assert_eq!(scrape_port(&log), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
