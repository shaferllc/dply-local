//! Supervises Laravel Octane application servers (Swoole / RoadRunner /
//! FrankenPHP) — one long-lived worker process per site that uses a non-`fpm`
//! runtime.
//!
//! Unlike php-fpm (which the proxy talks to over FastCGI), an Octane server is
//! an HTTP server in its own right. The daemon starts `php artisan octane:start`
//! in the project root on a private loopback port and reverse-proxies the
//! site's `.test` host to it — exactly the upstream role the `proxy` feature
//! already implements.
//!
//! That speed has a cost php-fpm doesn't: the worker holds your application in
//! memory, so a class you just edited is not the class still being served. So
//! the daemon watches each Octane site's sources and reloads its workers when
//! they change — the same job Octane's own `--watch` does, but without the Node
//! and chokidar dependency it needs, and owned by the process that can fall back
//! to a hard restart when the graceful path fails.
//!
//! Two levels of "bring my code back":
//!
//! * **reload** — `php artisan octane:reload`, which cycles the workers while
//!   the listener stays up. Fast, keeps the port, and is what a file change
//!   triggers.
//! * **restart** — kill the server and start it again on the same port. What a
//!   failed reload falls back to, and what `dpl octane restart` does when the
//!   server has wedged itself past reloading.
//!
//! The port survives a restart deliberately: the routing table holds it, and a
//! server that came back on a different port would leave every request for that
//! site pointed at nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

/// How long to wait after each successive failed start. An Octane server that
/// dies instantly is a boot error — a missing extension, a fatal in a service
/// provider — and hammering it helps nobody; after [`MAX_FAILURES`] we stop
/// until the user intervenes with `dpl octane restart`.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
];
const MAX_FAILURES: u32 = 5;

/// A run that survives this long counts as a success and clears the backoff.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);

/// How long a stopped server gets to release its port before its replacement
/// tries to bind it. Long enough for a graceful shutdown, short enough that a
/// restart still feels like one.
const RESTART_SETTLE: Duration = Duration::from_secs(2);

struct Server {
    port: u16,
    /// The runtime this server was started for (e.g. `octane-swoole`); a change
    /// forces a restart.
    runtime: String,
    php_bin: PathBuf,
    project: PathBuf,
    child: Option<Child>,
    /// The child's process *group*, so the whole tree can be signalled —
    /// RoadRunner and FrankenPHP run as children of the artisan process, and
    /// killing only what we hold would strand the real server on the port.
    pgid: Option<u32>,
    started: Instant,
    /// Whether the daemon reloads this server when the project's sources change.
    watch: bool,
    /// Fingerprint of the watched tree at the last scan. `None` until the first
    /// scan establishes a baseline — the baseline itself must never reload.
    fingerprint: Option<u64>,
    /// A change has been seen and not yet acted on.
    dirty: bool,
    /// An `octane:reload` we started and haven't collected yet.
    reloading: Option<std::process::Child>,
    /// How many times we've reloaded this server, for `dpl octane`.
    reloads: u32,
    /// Consecutive failed starts. Reset once a run stays up.
    failures: u32,
    /// Earliest time we may try again after a failure.
    retry_at: Option<Instant>,
    /// Why it isn't running, when it isn't.
    detail: Option<String>,
}

/// What `dpl octane` and the GUI display for one Octane site.
#[derive(Clone, Debug)]
pub struct AppServerInfo {
    pub site: String,
    /// e.g. `octane-frankenphp`.
    pub runtime: String,
    /// The loopback port the proxy forwards this site to.
    pub port: u16,
    pub running: bool,
    /// Whether source changes reload this server.
    pub watch: bool,
    pub reloads: u32,
    /// Human explanation when it isn't running (exit status, backoff, gave up).
    pub detail: Option<String>,
    pub log: String,
}

pub struct AppServers {
    /// Keyed by site name.
    servers: BTreeMap<String, Server>,
    /// Process groups we've asked to stop. Swoole's master survives the SIGTERM
    /// that stops the artisan launcher, and an Octane server that outlives its
    /// supervisor is one still holding the port its replacement needs — so every
    /// pass finishes the job with SIGKILL.
    dying: Vec<u32>,
    /// The groups currently written to [`groups_file`], so the file is only
    /// rewritten when the set actually changes.
    recorded: Vec<u32>,
}

impl AppServers {
    pub fn new() -> Self {
        AppServers { servers: BTreeMap::new(), dying: Vec::new(), recorded: Vec::new() }
    }

    /// Kill Octane servers left behind by a previous daemon, before any of ours
    /// start. The counterpart to [`crate::fpm::FpmManager::kill_orphans`], and
    /// just as necessary: a daemon that was SIGKILLed never got to stop its
    /// servers, and Octane refuses to start a second one ("Server is already
    /// running") — so without this the site 502s until someone finds the stray
    /// process by hand.
    ///
    /// Only the groups we recorded ourselves are signalled, so an Octane server
    /// you started in your own terminal is none of our business.
    pub fn kill_orphans() {
        let Some(path) = groups_file() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        for pgid in text.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
            tracing::info!(pgid, "killing orphaned octane server from a previous daemon");
            signal_group(pgid, "-KILL");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Record the process groups we own, so the next daemon can clean up after
    /// this one if it doesn't get to do so itself.
    fn record_groups(&mut self) {
        let live: Vec<u32> = self.servers.values().filter_map(|s| s.pgid).collect();
        if live == self.recorded {
            return;
        }
        if let Some(path) = groups_file() {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/tmp")));
            let body = live.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("\n");
            let _ = std::fs::write(&path, body);
        }
        self.recorded = live;
    }

    /// Ensure an Octane server is configured for `site`. Returns the port to
    /// proxy to, or `None` if it couldn't start (e.g. Octane isn't installed in
    /// the project — the site then falls back to php-fpm).
    ///
    /// This is configuration, not supervision: an existing server on the same
    /// runtime keeps its port and its process even if that process has died,
    /// because [`AppServers::supervise`] owns liveness. Reconciles happen on
    /// every mutation, and letting them restart a crashing server would undo the
    /// backoff that stops a boot error from becoming a spin loop.
    pub fn ensure(
        &mut self,
        site: &str,
        runtime: &str,
        project: &Path,
        php_bin: &Path,
        watch: bool,
    ) -> Option<u16> {
        // Reuse a server on the same runtime + PHP binary.
        let reusable = self
            .servers
            .get(site)
            .map(|s| s.runtime == runtime && s.php_bin == php_bin && s.project == project)
            .unwrap_or(false);
        if reusable {
            let s = self.servers.get_mut(site).expect("just checked");
            // Watching turned back on: re-baseline, so edits made while it was
            // off don't fire a reload the moment it comes back.
            if watch && !s.watch {
                s.fingerprint = None;
                s.dirty = false;
            }
            s.watch = watch;
            return Some(s.port);
        }
        if let Some(mut s) = self.servers.remove(site) {
            let pgid = s.stop();
            self.dying.extend(pgid);
        }

        // Octane needs the project's `artisan` and the `laravel/octane` package
        // installed; without either, fall back to php-fpm (so the site still
        // serves) rather than proxying to a server that never starts.
        if !project.join("artisan").is_file() || !project.join("vendor/laravel/octane").is_dir() {
            tracing::warn!(site, "octane runtime set but laravel/octane isn't installed — using php-fpm");
            return None;
        }

        let port = free_port()?;
        let mut server = Server {
            port,
            runtime: runtime.to_string(),
            php_bin: php_bin.to_path_buf(),
            project: project.to_path_buf(),
            child: None,
            pgid: None,
            started: Instant::now(),
            watch,
            fingerprint: None,
            dirty: false,
            reloading: None,
            reloads: 0,
            failures: 0,
            retry_at: None,
            detail: None,
        };
        server.spawn(site);
        // A server that couldn't even be spawned has no upstream to route to;
        // php-fpm serves the site instead of a port nothing is listening on.
        server.child.as_ref()?;
        self.servers.insert(site.to_string(), server);
        self.record_groups();
        Some(port)
    }

    /// Stop servers for sites no longer using an Octane runtime.
    pub fn retain(&mut self, keep: &BTreeSet<String>) {
        let stale: Vec<String> = self.servers.keys().filter(|k| !keep.contains(*k)).cloned().collect();
        for site in stale {
            if let Some(mut s) = self.servers.remove(&site) {
                let pgid = s.stop();
                self.dying.extend(pgid);
                tracing::info!(site, "octane server stopped");
            }
        }
        self.record_groups();
    }

    /// The sites whose sources should be fingerprinted this tick, with the
    /// project root to scan. Handed out so the caller can do the walk *off* the
    /// registry lock — the proxy takes that lock on every request, and a stat
    /// walk of a Laravel app is not something to hold it for.
    pub fn watch_targets(&self) -> Vec<(String, PathBuf)> {
        self.servers
            .iter()
            .filter(|(_, s)| s.watch)
            .map(|(site, s)| (site.clone(), s.project.clone()))
            .collect()
    }

    /// One supervision pass, given this tick's source fingerprints (as produced
    /// by [`fingerprint`] for each of [`AppServers::watch_targets`]): collect
    /// finished reloads, restart what died, and reload what changed.
    pub fn supervise(&mut self, scans: &[(String, u64)]) {
        // Anything asked to stop on an earlier pass has had its grace period.
        for pgid in std::mem::take(&mut self.dying) {
            signal_group(pgid, "-KILL");
        }

        let sites: Vec<String> = self.servers.keys().cloned().collect();
        for site in sites {
            // A reload that failed leaves the old code in memory — the one
            // outcome this feature exists to prevent — so fall back to the
            // blunt instrument.
            let failed = self.servers.get_mut(&site).map(|s| s.settle_reload(&site)).unwrap_or(false);
            if failed {
                self.begin_restart(&site);
                continue;
            }

            let Some(s) = self.servers.get_mut(&site) else { continue };

            if !s.is_alive() {
                if s.failures >= MAX_FAILURES {
                    continue;
                }
                if s.retry_at.map(|t| Instant::now() < t).unwrap_or(false) {
                    continue;
                }
                tracing::info!(site = %site, "octane server restarting");
                s.spawn(&site);
                continue;
            }
            if s.failures > 0 && s.started.elapsed() >= HEALTHY_AFTER {
                s.failures = 0;
                s.detail = None;
            }

            let Some((_, fp)) = scans.iter().find(|(n, _)| n == &site) else { continue };
            let mut restart = false;
            match classify(s.fingerprint, *fp, s.dirty, s.reloading.is_some()) {
                Tick::Baseline => s.fingerprint = Some(*fp),
                Tick::Changed => {
                    s.fingerprint = Some(*fp);
                    s.dirty = true;
                }
                Tick::Settled => {
                    s.dirty = false;
                    restart = s.reload(&site);
                }
                Tick::Idle => {}
            }
            if restart {
                self.begin_restart(&site);
            }
        }
        self.record_groups();
    }

    /// Gracefully reload one site's workers — `dpl octane reload`.
    pub fn reload(&mut self, site: &str) -> bool {
        let Some(s) = self.servers.get_mut(site) else { return false };
        if s.is_alive() {
            if s.reload(site) {
                self.begin_restart(site);
            }
        } else {
            // Nothing to reload into; bring it back instead, which is what the
            // user meant.
            self.begin_restart(site);
        }
        true
    }

    /// Bounce one site's server, clearing any give-up state — `dpl octane restart`.
    pub fn restart(&mut self, site: &str) -> bool {
        if !self.servers.contains_key(site) {
            return false;
        }
        self.begin_restart(site);
        true
    }

    /// Stop a server now and start it again on a later pass.
    ///
    /// Deliberately not stop-then-spawn: the replacement binds the same port —
    /// the routing table holds it — and the process being replaced does not let
    /// go of it the instant it's signalled. Spawning inline means racing a
    /// server that is still shutting down and losing, which reads in the log as
    /// "Port already in use" and costs a full backoff cycle to recover from.
    fn begin_restart(&mut self, site: &str) {
        let pgid = self.servers.get_mut(site).and_then(|s| s.begin_restart());
        self.dying.extend(pgid);
    }

    pub fn statuses(&self) -> Vec<AppServerInfo> {
        self.servers
            .iter()
            .map(|(site, s)| AppServerInfo {
                site: site.clone(),
                runtime: s.runtime.clone(),
                port: s.port,
                running: s.child.is_some(),
                watch: s.watch,
                reloads: s.reloads,
                detail: s.detail.clone(),
                log: log_path(site).to_string_lossy().into_owned(),
            })
            .collect()
    }
}

impl Server {
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
                if long_enough {
                    self.failures = 1;
                } else {
                    self.failures += 1;
                }
                self.detail = Some(if self.failures >= MAX_FAILURES {
                    format!(
                        "exited ({status}) {} times — giving up. Fix it, then `dpl octane restart`.",
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

    /// The bare server name Octane's commands take: `swoole`, `frankenphp`, …
    fn server_name(&self) -> String {
        self.runtime.strip_prefix("octane-").unwrap_or(&self.runtime).to_string()
    }

    /// Start `octane:start` on this server's (fixed) port.
    fn spawn(&mut self, site: &str) {
        let server = self.server_name();
        let mut cmd = Command::new(&self.php_bin);
        cmd.arg("artisan")
            .arg("octane:start")
            .arg("--server")
            .arg(&server)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(self.port.to_string())
            .current_dir(&self.project)
            .env("DUMPS_HOST", "127.0.0.1")
            .env("DUMPS_PORT", crate::dumps::port().to_string())
            .env("DUMPS_ENABLED", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(open_log(site)))
            .stderr(std::process::Stdio::from(open_log(site)))
            // Its own process group: RoadRunner and FrankenPHP are separate
            // binaries the artisan process launches, so stopping the server
            // means signalling the tree, not the launcher.
            .process_group(0)
            .kill_on_drop(true);

        match cmd.spawn() {
            Ok(child) => {
                self.pgid = child.id();
                self.child = Some(child);
                self.started = Instant::now();
                self.detail = None;
                tracing::info!(site, runtime = %self.runtime, port = self.port, "octane server started");
            }
            Err(e) => {
                self.failures += 1;
                self.detail = Some(format!("could not start: {e}"));
                self.retry_at = Some(Instant::now() + backoff_for(self.failures));
                tracing::warn!(site, error = %e, "octane server failed to start");
            }
        }
    }

    /// Ask Octane to cycle its workers. Started, not waited on: booting artisan
    /// takes a few hundred milliseconds, and this runs under the lock the proxy
    /// needs to route requests. [`Server::settle_reload`] collects it.
    ///
    /// Returns whether the caller must restart the server instead.
    fn reload(&mut self, site: &str) -> bool {
        let mut cmd = std::process::Command::new(&self.php_bin);
        cmd.arg("artisan")
            .arg("octane:reload")
            // Without this, Octane reloads whatever `config/octane.php` names —
            // which is the project's default, not necessarily the server this
            // site is running. It then reports the server isn't running, and
            // the code you just saved stays in memory.
            .arg("--server")
            .arg(self.server_name())
            .current_dir(&self.project)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(open_log(site)))
            .stderr(std::process::Stdio::from(open_log(site)));
        match cmd.spawn() {
            Ok(child) => {
                self.reloading = Some(child);
                self.reloads += 1;
                tracing::info!(site, "octane workers reloading");
                false
            }
            Err(e) => {
                tracing::warn!(site, error = %e, "octane:reload could not run — restarting instead");
                true
            }
        }
    }

    /// Collect a reload started on an earlier tick, reporting whether it failed
    /// and the server therefore needs restarting.
    fn settle_reload(&mut self, site: &str) -> bool {
        let Some(child) = self.reloading.as_mut() else { return false };
        match child.try_wait() {
            Ok(None) => false,
            Ok(Some(status)) => {
                self.reloading = None;
                if status.success() {
                    false
                } else {
                    tracing::warn!(site, %status, "octane:reload failed — restarting the server");
                    true
                }
            }
            Err(_) => {
                self.reloading = None;
                false
            }
        }
    }

    /// Stop the server and arrange for it to come back on a later pass,
    /// forgetting any failure history — a restart the user (or a failed reload)
    /// asked for is not a crash. Returns the process group left to clean up.
    fn begin_restart(&mut self) -> Option<u32> {
        let pgid = self.stop();
        self.failures = 0;
        self.detail = Some("restarting…".into());
        self.dirty = false;
        self.fingerprint = None;
        self.retry_at = Some(Instant::now() + RESTART_SETTLE);
        pgid
    }

    /// Stop the server and everything it spawned, returning the process group
    /// for the caller to finish off once it's had time to go quietly.
    fn stop(&mut self) -> Option<u32> {
        if let Some(mut child) = self.reloading.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let pgid = self.pgid.take();
        if let Some(pgid) = pgid {
            // The whole artisan → swoole/rr/frankenphp tree, not just the
            // launcher we hold: the launcher exiting doesn't take the server
            // with it, and the server is what holds the port.
            signal_group(pgid, "-TERM");
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        pgid
    }
}

/// What one tick's fingerprint says about a watched project.
#[derive(Debug, PartialEq, Eq)]
enum Tick {
    /// The first scan. It establishes what "unchanged" means and must never
    /// reload — otherwise every daemon start bounces every Octane site.
    Baseline,
    /// Something moved. Note it and wait: an editor's save, a `git checkout` and
    /// a `composer install` all land as a burst of writes, and reloading into
    /// the middle of one boots an application that is briefly inconsistent
    /// with itself.
    Changed,
    /// Quiet again after a change — reload now.
    Settled,
    Idle,
}

/// The debounce rule: reload on the first tick where the tree has stopped
/// moving. A reload already in flight leaves the flag set, so an edit made
/// during one is picked up by the next tick instead of being swallowed.
fn classify(previous: Option<u64>, now: u64, dirty: bool, reloading: bool) -> Tick {
    match previous {
        None => Tick::Baseline,
        Some(prev) if prev != now => Tick::Changed,
        _ if dirty && !reloading => Tick::Settled,
        _ => Tick::Idle,
    }
}

/// Signal a whole process group. A negative pid means "the group" to kill(1),
/// which is how everything the server spawned gets the signal too.
fn signal_group(pgid: u32, signal: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .arg(signal)
        .arg(format!("-{pgid}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn backoff_for(failures: u32) -> Duration {
    let idx = (failures.max(1) as usize - 1).min(BACKOFF.len() - 1);
    BACKOFF[idx]
}

/// Directory names the walk never descends into. `cache` covers
/// `bootstrap/cache`, whose compiled manifests Laravel rewrites as a side effect
/// of running artisan — including the artisan we run to reload, which is exactly
/// how a watcher turns into a loop.
const SKIP_DIRS: [&str; 5] = ["node_modules", "vendor", "storage", "cache", ".git"];

/// The roots Octane's own default `watch` list covers, minus `storage` (logs)
/// and `vendor` (composer's job, and caught by `composer.lock` below).
const WATCH_DIRS: [&str; 7] =
    ["app", "bootstrap", "config", "database", "routes", "resources", "public"];

/// Files that change the application without living under a watched directory.
const WATCH_FILES: [&str; 3] = [".env", "composer.lock", "artisan"];

/// Cap on directory entries visited per scan. A project that blows past this
/// is one where a full walk every tick would cost more than the feature is
/// worth; the fingerprint stays stable-ish rather than pretending otherwise.
const SCAN_BUDGET: usize = 20_000;

/// A cheap fingerprint of everything a running worker holds in memory: every
/// watched `.php` file plus `.env` and `composer.lock`, folded into one number
/// from name + size + mtime.
///
/// Contents are never read — an mtime is enough to mean "you saved something",
/// which is the question being asked. Files are combined with wrapping addition
/// so the result doesn't depend on directory order (no sort needed) and an edit,
/// a create, a delete and a rename all move it.
///
/// Directories carry a hash of their own path down to their children, so a file
/// is identified by (where it is, what it's called) without building or hashing
/// a full path per file — at several thousand files a tick, that allocation is
/// most of the work that isn't a `stat`.
pub fn fingerprint(project: &Path) -> u64 {
    let mut sum: u64 = 0;
    for file in WATCH_FILES {
        let path = project.join(file);
        if let Ok(md) = std::fs::metadata(&path) {
            sum = sum.wrapping_add(stamp(hash_of(0, file.as_bytes()), &[], &md));
        }
    }

    let mut budget = SCAN_BUDGET;
    let mut stack: Vec<(PathBuf, u64)> = WATCH_DIRS
        .iter()
        .map(|d| (project.join(d), hash_of(0, d.as_bytes())))
        .filter(|(p, _)| p.is_dir())
        .collect();
    while let Some((dir, dir_hash)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if budget == 0 {
                return sum;
            }
            budget -= 1;
            let name = entry.file_name();
            let bytes = name.as_encoded_bytes();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                let skip = bytes.first() == Some(&b'.')
                    || SKIP_DIRS.iter().any(|d| d.as_bytes() == bytes);
                if !skip {
                    stack.push((entry.path(), hash_of(dir_hash, bytes)));
                }
                continue;
            }
            if !bytes.ends_with(b".php") {
                continue;
            }
            if let Ok(md) = entry.metadata() {
                sum = sum.wrapping_add(stamp(dir_hash, bytes, &md));
            }
        }
    }
    sum
}

/// FNV-1a over `bytes`, seeded with `seed` — small, allocation-free, and this
/// is a change detector, not a checksum.
fn hash_of(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = if seed == 0 { 0xcbf2_9ce4_8422_2325 } else { seed };
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn stamp(dir_hash: u64, name: &[u8], md: &std::fs::Metadata) -> u64 {
    let mut hash = hash_of(dir_hash, name);
    hash = hash_of(hash, &md.len().to_le_bytes());
    if let Ok(t) = md.modified() {
        if let Ok(since) = t.duration_since(std::time::UNIX_EPOCH) {
            hash = hash_of(hash, &since.as_secs().to_le_bytes());
            hash = hash_of(hash, &since.subsec_nanos().to_le_bytes());
        }
    }
    hash
}

/// `~/.dpl/run/octane-groups` — the process groups of the Octane servers this
/// daemon is supervising, one per line. Read once at startup by
/// [`AppServers::kill_orphans`].
fn groups_file() -> Option<PathBuf> {
    dpl_core::paths::runtime_dir(None).ok().map(|d| d.join("octane-groups"))
}

/// `~/.dpl/logs/<site>-octane.log`.
pub fn log_path(site: &str) -> PathBuf {
    dpl_core::paths::logs_dir(None)
        .map(|d| d.join(format!("{site}-octane.log")))
        .unwrap_or_else(|_| PathBuf::from("/dev/null"))
}

/// Append-mode log file for a site's Octane server.
fn open_log(site: &str) -> std::fs::File {
    let path = log_path(site);
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

/// How often the daemon checks on its Octane servers and their sources. A save
/// reloads within one to two ticks — fast enough that the reload has usually
/// landed before you've switched to the browser, and slow enough that a burst of
/// writes is one reload rather than ten.
const SUPERVISE_EVERY: Duration = Duration::from_secs(1);

/// The slowest we'll ever go. A big enough project pushes the interval out (see
/// [`next_tick`]), but never so far that a save feels unwatched.
const SUPERVISE_AT_MOST: Duration = Duration::from_secs(5);

/// A background watcher must not cost real CPU, so the scan gets at most a
/// twenty-fifth of one core: whatever the last pass took, wait 25× as long
/// before the next one.
///
/// The walk is one `stat` per watched file, and on macOS that's 10–20µs each no
/// matter how it's issued — so the interval has to follow the project's size
/// rather than pretend every project is small. Most apps scan in a millisecond
/// or two and sit at the 1s floor; a 5,000-file monolith takes ~55ms and settles
/// near 1.4s, half a second slower to notice a save in exchange for not spinning
/// a core on a laptop that's meant to be idle. (Event-driven FSEvents would cost
/// nothing at rest; it also means a watcher thread and a native dependency per
/// site, which this doesn't yet earn.)
const DUTY_DIVISOR: u32 = 25;

fn next_tick(scan_took: Duration) -> Duration {
    (scan_took * DUTY_DIVISOR).clamp(SUPERVISE_EVERY, SUPERVISE_AT_MOST)
}

/// Watch supervised Octane servers: restart any that die, reload any whose
/// sources changed. Runs for the daemon's lifetime.
///
/// The scan deliberately happens between the two locks, not inside them: the
/// proxy takes the registry lock on every request, so a walk of every PHP file
/// in every Octane project must not be holding it.
pub async fn watch(state: crate::server::DaemonState) {
    let mut every = SUPERVISE_EVERY;
    loop {
        tokio::time::sleep(every).await;

        let targets = state.registry.lock().await.appserver_watch_targets();
        let (scans, took) = if targets.is_empty() {
            (Vec::new(), Duration::ZERO)
        } else {
            tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                let scans: Vec<(String, u64)> = targets
                    .into_iter()
                    .map(|(site, project)| (site, fingerprint(&project)))
                    .collect();
                (scans, started.elapsed())
            })
            .await
            .unwrap_or((Vec::new(), Duration::ZERO))
        };
        every = next_tick(took);

        state.registry.lock().await.supervise_appservers(&scans);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_for(1), Duration::from_secs(2));
        assert_eq!(backoff_for(3), Duration::from_secs(15));
        assert_eq!(backoff_for(99), Duration::from_secs(60));
    }

    #[test]
    fn a_save_reloads_once_the_writes_stop() {
        // The first scan only learns what the project looks like.
        assert_eq!(classify(None, 7, false, false), Tick::Baseline);
        // Nothing happening stays nothing happening.
        assert_eq!(classify(Some(7), 7, false, false), Tick::Idle);

        // A save is noticed, then acted on when the next tick finds it quiet.
        assert_eq!(classify(Some(7), 9, false, false), Tick::Changed);
        assert_eq!(classify(Some(9), 9, true, false), Tick::Settled);

        // A burst — file after file — reloads once, at the end, not per file.
        assert_eq!(classify(Some(9), 10, true, false), Tick::Changed);
        assert_eq!(classify(Some(10), 11, true, false), Tick::Changed);
        assert_eq!(classify(Some(11), 11, true, false), Tick::Settled);

        // An edit made while a reload is running waits for that reload, rather
        // than stacking a second one or being forgotten.
        assert_eq!(classify(Some(11), 11, true, true), Tick::Idle);
        assert_eq!(classify(Some(11), 11, true, false), Tick::Settled);
    }

    #[test]
    fn fingerprint_moves_on_what_the_workers_hold() {
        let dir = std::env::temp_dir().join(format!("dpl-octane-{}", std::process::id()));
        let app = dir.join("app");
        std::fs::create_dir_all(app.join("Models")).unwrap();
        std::fs::create_dir_all(dir.join("vendor/big")).unwrap();
        std::fs::write(app.join("Models/User.php"), "<?php class User {}").unwrap();
        std::fs::write(dir.join(".env"), "APP_ENV=local").unwrap();
        let base = fingerprint(&dir);

        // An edit moves it.
        std::fs::write(app.join("Models/User.php"), "<?php class User { public $x; }").unwrap();
        let edited = fingerprint(&dir);
        assert_ne!(base, edited);

        // A new file moves it; deleting it again comes back to the same place.
        std::fs::write(app.join("Models/Post.php"), "<?php class Post {}").unwrap();
        let added = fingerprint(&dir);
        assert_ne!(edited, added);
        std::fs::remove_file(app.join("Models/Post.php")).unwrap();
        assert_eq!(edited, fingerprint(&dir));

        // Vendor churn and logs don't: composer.lock speaks for the former, and
        // nothing should reload the app because it wrote a log line.
        std::fs::write(dir.join("vendor/big/Thing.php"), "<?php").unwrap();
        std::fs::create_dir_all(dir.join("storage/logs")).unwrap();
        std::fs::write(dir.join("storage/logs/laravel.log"), "boom").unwrap();
        assert_eq!(edited, fingerprint(&dir));

        // Environment changes do.
        std::fs::write(dir.join(".env"), "APP_ENV=local\nAPP_DEBUG=true").unwrap();
        assert_ne!(edited, fingerprint(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

