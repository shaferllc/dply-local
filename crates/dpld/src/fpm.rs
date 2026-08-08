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
/// Identifies a php-fpm master: its PHP binary, Xdebug mode, whether the SPX
/// profiler is loaded, and an optional opcache preload script. The last three are
/// separate axes because each forks a dedicated master: a profiled site needs SPX
/// in its pool, and a preloaded site needs its own `opcache.preload` — neither of
/// which a plain shared site must pay for. `None` preload = the shared master.
pub type MasterKey = (PathBuf, Mode, bool, Option<PathBuf>);

/// php-fpm serves these two paths itself, before the request ever reaches PHP.
/// Both are namespaced so they cannot collide with a route in the user's app —
/// the pool is shared by every site on a PHP version, so a bare `/status` would
/// shadow a real page on any of them.
const STATUS_PATH: &str = "/dpl-fpm-status";
const PING_PATH: &str = "/dpl-fpm-ping";

/// Requests slower than this are flagged by php-fpm. High enough that ordinary
/// local page loads don't spam the log, low enough to catch what a developer
/// would call "slow" — and far below the 90s hard terminate, so a request is
/// diagnosed long before it is killed.
///
/// Worth knowing what this buys on macOS: php-fpm logs *which* script and URI
/// ran long, in the pool's error log, and increments `slow requests` on the
/// status page. It then tries to attach to the worker for a PHP backtrace and
/// fails — macOS refuses `task_for_pid()` to a master that isn't root — so the
/// `slowlog` file stays empty here. The directive is still set because the
/// detection above depends on it, and because the file does fill on Linux.
const SLOWLOG_TIMEOUT: u32 = 5;

/// How long a status scrape may take. A saturated pool cannot answer at all —
/// the status page is handled by a worker, and there are none free — so this is
/// deliberately short: the scrape failing *is* the signal, and waiting longer
/// only delays the supervision tick.
const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

struct Master {
    port: u16,
    child: Child,
    /// The most recent successful status scrape, and when it happened.
    ///
    /// Cached rather than fetched on demand because the moment you most want it
    /// — a request just 502'd — is exactly when the pool is too busy to serve
    /// it. A stale-but-real number explains the failure; a scrape that also
    /// times out explains nothing.
    stats: Option<dpl_core::ipc::FpmPoolStats>,
    /// Consecutive failed starts, driving the backoff ladder.
    failures: u32,
    /// When the master may next be respawned after failing.
    retry_at: Option<std::time::Instant>,
    /// Why this pool isn't healthy, when it isn't.
    detail: Option<String>,
}

pub struct FpmManager {
    masters: BTreeMap<MasterKey, Master>,
    /// Shared IDE-side Xdebug settings, refreshed from config each reconcile.
    xdebug_settings: xdebug::Settings,
    /// Workers allowed per pool, from config. Applied when a master's config is
    /// written, so a change only reaches a pool that is (re)started.
    max_children: u32,
    /// Masters that died, with how many times in a row and when the last one
    /// went. Keyed the same as `masters`, so a respawn inherits the history
    /// rather than starting the ladder over.
    failed: BTreeMap<MasterKey, (u32, std::time::Instant)>,
}

/// How long to wait after each successive failed start, mirroring the ladder
/// Octane servers use. A master that exits immediately is a boot error — a
/// broken ini, an extension that segfaults on load — and hammering it only
/// fills the log; after [`MAX_FAILURES`] we stop until `dpl fpm restart`.
const BACKOFF: [std::time::Duration; 4] = [
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(15),
    std::time::Duration::from_secs(60),
];
const MAX_FAILURES: u32 = 5;

fn backoff_for(failures: u32) -> std::time::Duration {
    let idx = (failures.max(1) as usize - 1).min(BACKOFF.len() - 1);
    BACKOFF[idx]
}

impl FpmManager {
    pub fn new() -> Self {
        FpmManager {
            masters: BTreeMap::new(),
            xdebug_settings: xdebug::Settings::default(),
            max_children: dpl_core::config::DEFAULT_FPM_MAX_CHILDREN,
            failed: BTreeMap::new(),
        }
    }

    /// Update the per-pool worker ceiling. Returns true when it changed, so the
    /// caller can drop the running masters — `pm.max_children` is baked into the
    /// config file a master read at startup, so a live pool keeps the old value.
    pub fn set_max_children(&mut self, max_children: u32) -> bool {
        let changed = self.max_children != max_children;
        self.max_children = max_children;
        changed
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
        kill_orphaned_workers();
    }

    /// The FastCGI address for a PHP binary at a given Xdebug mode (and preload
    /// script, if any), if its master is running.
    pub fn addr_for(
        &self,
        php_bin: &Path,
        mode: &Mode,
        profile: bool,
        preload: Option<&Path>,
    ) -> Option<SocketAddr> {
        self.masters
            .get(&(php_bin.to_path_buf(), mode.clone(), profile, preload.map(Path::to_path_buf)))
            .map(|m| SocketAddr::from(([127, 0, 0, 1], m.port)))
    }

    /// Ensure a php-fpm master is running for `php_bin` at `mode` (with SPX loaded
    /// when `profile`, and `opcache.preload` set when `preload` is given),
    /// returning its port.
    pub fn ensure(
        &mut self,
        php_bin: &Path,
        mode: &Mode,
        profile: bool,
        preload: Option<&Path>,
    ) -> Result<u16> {
        // Pre-per-site GUI wrote zz-dpl-xdebug.ini into Homebrew's system conf.d.
        // Menu-bar "Debugging" is FPM-only and never affected CLI `php` (Pest TIA,
        // artisan, …) — scrub the leftover so it can't keep confusing that.
        let _ = xdebug::cleanup_legacy_system_loader(php_bin);

        let key: MasterKey = (php_bin.to_path_buf(), mode.clone(), profile, preload.map(Path::to_path_buf));
        if let Some(m) = self.masters.get_mut(&key) {
            // Reap if it died; otherwise reuse.
            if matches!(m.child.try_wait(), Ok(Some(_))) {
                self.masters.remove(&key);
            } else {
                return Ok(m.port);
            }
        }

        // A master that keeps dying gets the backoff ladder rather than a fresh
        // spawn on every request that asks for it.
        if !self.may_start(&key) {
            let (failures, _) = self.failed.get(&key).copied().unwrap_or((0, std::time::Instant::now()));
            anyhow::bail!(
                "php-fpm for {} ({mode}) has failed {failures} time(s); \
                 not restarting yet — see its log, then `dpl fpm restart`",
                php_bin.display()
            );
        }

        let fpm_bin = derive_fpm(php_bin)
            .with_context(|| format!("no php-fpm found next to {}", php_bin.display()))?;
        let port = free_port()?;
        let conf = write_conf(php_bin, mode, profile, preload, port, self.max_children)?;
        let log = log_file(php_bin, mode, profile, preload)?;

        let mut cmd = Command::new(&fpm_bin);
        cmd.arg("--nodaemonize").arg("--fpm-config").arg(&conf);
        for (key, value) in master_env(mode, crate::dumps::port()) {
            cmd.env(key, value);
        }
        // XDEBUG_CONFIG (e.g. `idekey=VSCODE`, commonly exported by dotfiles) would
        // override the ini so `dpl xdebug ide`/`port` do nothing — a removal, not a
        // value, so it can't live in `master_env`.
        cmd.env_remove("XDEBUG_CONFIG");

        // Load Xdebug and/or SPX only into the masters that want them, each via its
        // own dpl scan dir added to PHP_INI_SCAN_DIR (leading `:` keeps the system
        // conf.d). A plain master loads neither.
        let mut scan_dirs: Vec<PathBuf> = Vec::new();
        if !mode.is_off() {
            match xdebug::write_loader(None, php_bin, &self.xdebug_settings) {
                Ok(Some(dir)) => scan_dirs.push(dir),
                Ok(None) => tracing::warn!(
                    php = %php_bin.display(), %mode,
                    "Xdebug requested but not installed for this PHP; serving without it \
                     (try `dpl php ext-install <version> xdebug`)"
                ),
                Err(e) => tracing::warn!(error = %e, "writing the Xdebug loader ini failed"),
            }
        }
        if profile {
            match dpl_core::spx::write_loader(None, php_bin) {
                Ok(Some(dir)) => scan_dirs.push(dir),
                Ok(None) => tracing::warn!(
                    php = %php_bin.display(),
                    "profiler requested but SPX isn't installed for this PHP; serving without it \
                     (try `dpl php ext-install <version> spx`)"
                ),
                Err(e) => tracing::warn!(error = %e, "writing the SPX loader ini failed"),
            }
        }
        if !scan_dirs.is_empty() {
            cmd.env("PHP_INI_SCAN_DIR", scan_dir_env(&scan_dirs));
        }

        let child = cmd
            .stdout(std::process::Stdio::from(log.try_clone().unwrap_or_else(|_| log_null())))
            .stderr(std::process::Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {}", fpm_bin.display()))?;

        tracing::info!(fpm = %fpm_bin.display(), port, %mode, "php-fpm master started");
        // Carry any prior failure count onto the new master so a pool that dies
        // repeatedly climbs the ladder instead of resetting on each respawn.
        let failures = self.failed.remove(&key).map(|(n, _)| n).unwrap_or(0);
        self.masters.insert(
            key,
            Master { port, child, stats: None, failures, retry_at: None, detail: None },
        );
        // Prime the first worker in the background so a real request doesn't wait
        // on the fork + PHP startup this master just triggered.
        warm(port, master_dir(php_bin, mode, profile, preload)?);
        Ok(port)
    }

    /// Gracefully reload every pool: SIGUSR2 tells php-fpm to replace its
    /// workers while keeping the listening socket open, so requests queue for a
    /// moment rather than being refused. This is the right tool for picking up
    /// changed PHP ini or a new opcache preload; it will *not* change anything
    /// the master read at startup, such as `pm.max_children` or `XDEBUG_MODE`.
    pub fn reload_all(&self) -> usize {
        let mut n = 0;
        for m in self.masters.values() {
            if let Some(pid) = m.child.id() {
                signal(pid, "-USR2");
                n += 1;
            }
        }
        tracing::info!(pools = n, "php-fpm pools reloaded");
        n
    }

    /// Stop every master. The next reconcile rebuilds them from config, which is
    /// what makes this the escape hatch for a pool that is wedged, or one whose
    /// startup-only settings changed.
    pub fn restart_all(&mut self) -> usize {
        let keys: Vec<MasterKey> = self.masters.keys().cloned().collect();
        let n = keys.len();
        for k in keys {
            if let Some(m) = self.masters.remove(&k) {
                stop_master(m);
            }
        }
        tracing::info!(pools = n, "php-fpm pools stopped for restart");
        n
    }

    /// Every pool dpl supervises, as reported to the CLI/GUI.
    ///
    /// `site_counts` maps a master key to how many sites route to it; the pool
    /// itself has no idea, since the proxy picks a pool per request.
    pub fn pools(
        &self,
        site_counts: &BTreeMap<MasterKey, u32>,
    ) -> Vec<dpl_core::ipc::FpmPoolInfo> {
        let mut out: Vec<dpl_core::ipc::FpmPoolInfo> = Vec::new();
        for (k, m) in &self.masters {
            let dir = master_dir(&k.0, &k.1, k.2, k.3.as_deref()).ok();
            out.push(dpl_core::ipc::FpmPoolInfo {
                php: php_label(&k.0),
                mode: k.1.to_string(),
                profile: k.2,
                preload: k.3.as_ref().map(|p| p.display().to_string()),
                port: m.port,
                pid: m.child.id(),
                running: true,
                sites: site_counts.get(k).copied().unwrap_or(0),
                stats: m.stats.clone(),
                detail: m.detail.clone(),
                log: dir
                    .as_ref()
                    .map(|d| d.join("fpm.log").display().to_string())
                    .unwrap_or_default(),
                slowlog: dir.as_ref().map(|d| d.join("slow.log").display().to_string()),
            });
        }
        // Masters that died and are waiting out their backoff still belong in
        // the listing — a pool that is missing is the thing you most want to
        // see, and omitting it makes `dpl fpm status` quietly wrong.
        for (k, (failures, since)) in &self.failed {
            let dir = master_dir(&k.0, &k.1, k.2, k.3.as_deref()).ok();
            let detail = if *failures >= MAX_FAILURES {
                format!("gave up after {failures} failed starts — `dpl fpm restart` to retry")
            } else {
                let wait = backoff_for(*failures).saturating_sub(since.elapsed());
                format!("exited {failures} time(s); retrying in {}s", wait.as_secs())
            };
            out.push(dpl_core::ipc::FpmPoolInfo {
                php: php_label(&k.0),
                mode: k.1.to_string(),
                profile: k.2,
                preload: k.3.as_ref().map(|p| p.display().to_string()),
                port: 0,
                pid: None,
                running: false,
                sites: site_counts.get(k).copied().unwrap_or(0),
                stats: None,
                detail: Some(detail),
                log: dir
                    .as_ref()
                    .map(|d| d.join("fpm.log").display().to_string())
                    .unwrap_or_default(),
                slowlog: dir.as_ref().map(|d| d.join("slow.log").display().to_string()),
            });
        }
        out
    }

    /// Ports of every live master, for the supervision tick to scrape.
    ///
    /// Returned as plain data so the scrape can happen without the registry
    /// lock held — a scrape can block for [`STATUS_TIMEOUT`], and holding the
    /// lock that long would stall every request the proxy is trying to route.
    pub fn live_ports(&self) -> Vec<u16> {
        self.masters.values().map(|m| m.port).collect()
    }

    /// Fold scraped status back in, keyed by port.
    pub fn apply_stats(&mut self, scraped: &[(u16, Option<dpl_core::ipc::FpmPoolStats>)]) {
        // Publish to the lock-free side cache first, so the proxy's error path
        // sees fresh numbers even while the registry is busy.
        if let Ok(mut cache) = POOL_STATS.write() {
            let map = cache.get_or_insert_with(BTreeMap::new);
            for (port, stats) in scraped {
                if let Some(s) = stats {
                    map.insert(*port, s.clone());
                }
            }
            // Drop pools that no longer exist, so a recycled port can't inherit
            // a dead pool's numbers and mislabel a future failure.
            let live: BTreeSet<u16> = self.masters.values().map(|m| m.port).collect();
            map.retain(|port, _| live.contains(port));
        }
        MAX_CHILDREN.store(self.max_children, std::sync::atomic::Ordering::Relaxed);

        for (port, stats) in scraped {
            if let Some(m) = self.masters.values_mut().find(|m| m.port == *port) {
                if let Some(s) = stats {
                    m.stats = Some(s.clone());
                    // A pool that answered is healthy by definition, whatever it
                    // failed at before.
                    m.failures = 0;
                    m.retry_at = None;
                    m.detail = None;
                } else if m.stats.is_some() {
                    // Keep the last good numbers, but say they're stale: this is
                    // the saturation case, and the old figures are the evidence.
                    m.detail = Some("not answering its status page (pool busy?)".to_string());
                }
            }
        }
    }

    /// Notice masters that have died, and rate-limit how fast they come back.
    ///
    /// `ensure()` already reaps a dead master, but only when a request happens
    /// to ask for its address — so a pool that dies at 3am stays dead-and-silent
    /// until someone browses to it. This runs on a timer instead, and applies
    /// the same backoff ladder Octane servers get: a master that dies instantly
    /// is a boot error (a broken ini, a missing extension), and respawning it in
    /// a tight loop helps nobody.
    ///
    /// Returns true when something changed, so the caller can reconcile.
    pub fn supervise(&mut self) -> bool {
        let mut dead: Vec<MasterKey> = Vec::new();
        for (k, m) in self.masters.iter_mut() {
            if matches!(m.child.try_wait(), Ok(Some(_))) {
                dead.push(k.clone());
            }
        }
        if dead.is_empty() {
            return false;
        }
        for k in dead {
            if let Some(m) = self.masters.remove(&k) {
                let failures = m.failures + 1;
                tracing::warn!(
                    php = %k.0.display(), mode = %k.1, failures,
                    "php-fpm master exited; will respawn"
                );
                // Carry the failure count forward on the *key*, so the respawn
                // in `ensure` starts from what we already know rather than from
                // zero. Without this the ladder resets every tick and a master
                // that dies on boot is restarted forever at full speed.
                self.failed.insert(k, (failures, std::time::Instant::now()));
            }
        }
        // A master that *crashed* — or was SIGKILLed by anything other than
        // `stop_master` — never ran its shutdown handler, so its workers are
        // now orphans still listening on the pool's port. Left alone they hold
        // that port for the daemon's lifetime and, worse, keep accepting: a
        // request can be served by a worker belonging to a pool that no longer
        // exists, which looks like the site hanging rather than a crash.
        // `stop_master` avoids this by signalling politely; nothing can make a
        // crash polite, so sweep after the fact.
        kill_orphaned_workers();
        true
    }

    /// Bring back masters that died, for those whose backoff has elapsed.
    ///
    /// Targeted on purpose. The obvious implementation is to call `reconcile()`
    /// and let it rebuild whatever is missing, but reconcile re-detects PHP —
    /// one spawned process per installed version — and runs with the registry
    /// mutex held, which every inbound request also needs. Doing that on a
    /// crash turned a dead pool into a multi-second stall across all 100-odd
    /// sites, measured at 27s on this machine. A dead master's key already says
    /// exactly what to restart, so restart precisely that.
    pub fn respawn_failed(&mut self) -> usize {
        let candidates: Vec<MasterKey> = self.failed.keys().cloned().collect();
        let mut started = 0;
        for k in candidates {
            if !self.may_start(&k) {
                continue;
            }
            match self.ensure(&k.0, &k.1, k.2, k.3.as_deref()) {
                Ok(_) => started += 1,
                Err(e) => tracing::warn!(php = %k.0.display(), error = %e, "php-fpm respawn failed"),
            }
        }
        started
    }

    /// Whether a master may be (re)spawned yet, given its failure history.
    fn may_start(&self, key: &MasterKey) -> bool {
        match self.failed.get(key) {
            None => true,
            Some((failures, since)) => {
                if *failures >= MAX_FAILURES {
                    return false;
                }
                since.elapsed() >= backoff_for(*failures)
            }
        }
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
            if let Some(m) = self.masters.remove(&k) {
                stop_master(m);
                tracing::info!(php = %k.0.display(), mode = %k.1, "php-fpm master stopped");
            }
        }
    }
}

/// Last known counters per pool port, readable without the registry lock.
///
/// The proxy needs these to explain a 502, and that is the one moment it must
/// not queue behind the registry mutex — a reconcile can hold it for a while,
/// and blocking there would turn "this pool is busy" into "the whole daemon is
/// stalled". Written by the supervision tick, read on the error path only.
static POOL_STATS: std::sync::RwLock<Option<BTreeMap<u16, dpl_core::ipc::FpmPoolStats>>> =
    std::sync::RwLock::new(None);

/// Cached counters for the pool listening on `port`, if it has ever answered.
pub fn cached_stats(port: u16) -> Option<dpl_core::ipc::FpmPoolStats> {
    POOL_STATS.read().ok()?.as_ref()?.get(&port).cloned()
}

/// The per-pool worker ceiling, published for the proxy's error path so it can
/// say "40/40" without reaching into the registry.
static MAX_CHILDREN: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(dpl_core::config::DEFAULT_FPM_MAX_CHILDREN);

pub fn cached_max_children() -> u32 {
    MAX_CHILDREN.load(std::sync::atomic::Ordering::Relaxed)
}

/// How often to scrape every pool and check for dead masters.
///
/// The scrape is a FastCGI round trip per pool against php-fpm's own handler —
/// it never enters PHP — so this is cheap, but it is not free, and the numbers
/// it collects only need to be fresh enough to explain a failure that just
/// happened.
const SUPERVISE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Keep php-fpm pools observed and alive for the daemon's lifetime.
///
/// Two jobs on one timer. It scrapes each pool's status page so a 502 can be
/// explained with real numbers — see [`Master::stats`] for why that has to be
/// cached ahead of time rather than fetched when the 502 happens — and it
/// notices masters that have exited, which previously went unseen until a
/// request happened to ask for the pool's address.
///
/// The lock is taken three times rather than held across the scrape: a pool
/// that has stopped answering takes [`STATUS_TIMEOUT`] to fail, and holding the
/// registry for that long would stall every request the proxy is routing —
/// turning one sick pool into a site-wide stall.
pub async fn watch(state: crate::server::DaemonState) {
    let mut tick = tokio::time::interval(SUPERVISE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;

        let ports = state.registry.lock().await.fpm_live_ports();

        // Scrape every pool with the lock *released*: a pool that has stopped
        // answering takes STATUS_TIMEOUT to fail, and the registry mutex is on
        // the path of every inbound request.
        let mut scraped: Vec<(u16, Option<dpl_core::ipc::FpmPoolStats>)> =
            Vec::with_capacity(ports.len());
        for port in ports {
            scraped.push((port, scrape(port).await.ok()));
        }

        let mut reg = state.registry.lock().await;
        reg.fpm_apply_stats(&scraped);
        reg.fpm_supervise();
        // Every tick, not only the one where a master was seen to die. The
        // first respawn attempt lands inside the backoff window and is refused
        // by design, so gating this on a fresh death means the retry never
        // comes and the pool stays down for good — sites 404 with a daemon that
        // believes it is supervising them. Restarts just the pools that died;
        // see `respawn_failed` for why this is not a reconcile.
        reg.fpm_respawn_failed();
    }
}

/// A short version label for a PHP binary, read from its path.
///
/// Deliberately not `php::version_of`, which execs the binary: this runs for
/// every pool inside the reporting path, with the registry lock held, and
/// spawning a PHP process per pool there would stall the proxy. Homebrew's
/// layout puts the version in the path (`/opt/homebrew/opt/php@8.4/bin/php`),
/// so parsing is both free and accurate for the binaries dpl manages; anything
/// unrecognised falls back to the path, which is at least unambiguous.
fn php_label(bin: &Path) -> String {
    for comp in bin.components() {
        let s = comp.as_os_str().to_string_lossy();
        if let Some(v) = s.strip_prefix("php@") {
            return v.to_string();
        }
    }
    bin.display().to_string()
}

/// Ask a pool for its own status over FastCGI.
///
/// php-fpm answers `pm.status_path` from the master's own handler, so the reply
/// describes the pool rather than running any of the user's code. `?json` is
/// asked for because the plain-text format is meant for humans and has changed
/// shape between releases; the JSON keys have not.
///
/// A failure here is information, not an error to bubble: the usual cause is
/// that every worker is busy, which is precisely the condition the caller wants
/// to know about.
pub async fn scrape(port: u16) -> Result<dpl_core::ipc::FpmPoolStats> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let params: Vec<(String, String)> = [
        ("GATEWAY_INTERFACE", "CGI/1.1"),
        ("REQUEST_METHOD", "GET"),
        ("SCRIPT_NAME", STATUS_PATH),
        ("SCRIPT_FILENAME", STATUS_PATH),
        ("REQUEST_URI", STATUS_PATH),
        ("QUERY_STRING", "json"),
        ("SERVER_PROTOCOL", "HTTP/1.1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();

    let resp = tokio::time::timeout(STATUS_TIMEOUT, crate::fastcgi::request(addr, &params, &[]))
        .await
        .map_err(|_| anyhow::anyhow!("status scrape timed out (pool busy?)"))??;

    parse_status(&resp.stdout)
}

/// Pull the JSON body out of a FastCGI status reply and map php-fpm's key names
/// onto [`dpl_core::ipc::FpmPoolStats`].
///
/// Split out from [`scrape`] so it can be tested against a captured payload
/// without a live pool — the key names are php-fpm's, and a rename upstream
/// would otherwise only surface as silently-zero counters in production.
fn parse_status(stdout: &[u8]) -> Result<dpl_core::ipc::FpmPoolStats> {
    let text = String::from_utf8_lossy(stdout);
    // CGI replies are headers, a blank line, then the body.
    let body = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map(|(_, b)| b)
        .unwrap_or(&text);
    let v: serde_json::Value =
        serde_json::from_str(body.trim()).context("parsing php-fpm status JSON")?;

    let u32_at = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
    let u64_at = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);

    Ok(dpl_core::ipc::FpmPoolStats {
        active: u32_at("active processes"),
        idle: u32_at("idle processes"),
        total: u32_at("total processes"),
        listen_queue: u32_at("listen queue"),
        max_listen_queue: u32_at("max listen queue"),
        max_children_reached: u32_at("max children reached"),
        slow_requests: u32_at("slow requests"),
        accepted_conn: u64_at("accepted conn"),
        uptime: u64_at("start since"),
    })
}

/// Retire one master, pool and all.
///
/// SIGTERM rather than SIGKILL, because php-fpm terminates its workers from its
/// own signal handler — a SIGKILLed master never runs it, and the workers live
/// on as orphans still holding the pool's listening socket. That is not merely a
/// leak: the port outlives the master, so once we hand it to a fresh master the
/// stale workers keep accepting on it too, serving requests from a pool nobody
/// supervises and whose Xdebug mode we already decided against.
///
/// The `Child` has to outlive the signal. `kill_on_drop` means dropping it here
/// would SIGKILL the master before it could reap anything, so ownership moves to
/// a task that waits for it to leave, with SIGKILL as the backstop if it won't.
fn stop_master(mut m: Master) {
    let Some(pid) = m.child.id() else {
        let _ = m.child.start_kill();
        return;
    };
    signal(pid, "-TERM");
    tokio::spawn(async move {
        let exited =
            tokio::time::timeout(std::time::Duration::from_secs(10), m.child.wait()).await;
        if exited.is_err() {
            tracing::warn!(pid, "php-fpm master ignored SIGTERM; killing");
            let _ = m.child.start_kill();
            let _ = m.child.wait().await;
        }
    });
}

/// Reap php-fpm *workers* whose master is gone.
///
/// The `pkill -f` in [`FpmManager::kill_orphans`] only ever matches masters: a
/// worker rewrites its argv to `php-fpm: pool www`, so the config path we match
/// on isn't there to find. Orphaned workers therefore survived every daemon
/// restart, accumulating indefinitely while holding dead pools' sockets open.
///
/// A worker's parent is always its master, so a parent of init means the master
/// is gone and the worker is garbage by definition. Matching the worker argv
/// exactly is what keeps this off healthy php-fpm installs, ours or anyone
/// else's — a master started by launchd also has init for a parent, but reads as
/// `master process`, never as a pool.
fn kill_orphaned_workers() {
    let Ok(out) = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,comm="]).output()
    else {
        return;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if !line.contains("php-fpm: pool ") {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(pid), Some("1")) = (fields.next(), fields.next()) else { continue };
        match pid.parse::<u32>() {
            Ok(pid) if pid > 1 => {
                signal(pid, "-TERM");
                tracing::info!(pid, "reaped orphaned php-fpm worker");
            }
            _ => {}
        }
    }
}

/// Send one signal to one pid. Never a pid of 0 or negative — that would mean
/// "the whole process group" to `kill(1)`, i.e. the daemon itself.
fn signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .arg(sig)
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// The environment every php-fpm master carries, independent of the request.
///
/// Returned as data, not applied inline, so a test can assert the fork-safety
/// variable survives. Its absence is a segfault that only surfaces on macOS under
/// a long-lived master serving pgsql — never on a fresh dev restart, never in CI —
/// so a silent drop in a future refactor would ship unnoticed. The pool runs with
/// `clear_env = no`, so these reach the PHP workers.
fn master_env(mode: &Mode, dumps_port: u16) -> Vec<(&'static str, String)> {
    vec![
        // Zero-config `dumps()` in any served site.
        ("DUMPS_HOST", "127.0.0.1".to_string()),
        ("DUMPS_PORT", dumps_port.to_string()),
        ("DUMPS_ENABLED", "1".to_string()),
        // Pinned for *every* master, `off` included: an inherited XDEBUG_MODE=debug
        // would otherwise switch on an "off" master that loads Xdebug via conf.d.
        ("XDEBUG_MODE", mode.as_str().to_string()),
        // libpq negotiates GSSAPI before anything else (`gssencmode` defaults to
        // `prefer`), reaching Kerberos → CFPreferences → libdispatch — none of them
        // fork-safe. php-fpm workers are forks of this master, so the first pgsql
        // connect in a worker lands in libdispatch with invalid post-fork state and
        // segfaults; the site 502s with no PHP error to show for it. Nothing served
        // locally authenticates to Postgres with Kerberos, so disabling it is free.
        ("PGGSSENCMODE", "disable".to_string()),
    ]
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
/// by Xdebug mode, whether SPX is loaded, and their preload script, so all three
/// are part of the path. Preloaded sites each get their own master, so the
/// script's absolute path is hashed into the slug to keep two preloaded sites on
/// the same PHP+mode from colliding on one directory.
fn master_dir(php_bin: &Path, mode: &Mode, profile: bool, preload: Option<&Path>) -> Result<PathBuf> {
    let mut slug = mode.slug();
    if profile {
        slug = format!("{slug}-spx");
    }
    if let Some(script) = preload {
        slug = format!("{slug}-preload-{:016x}", path_hash(script));
    }
    Ok(dpl_core::paths::php_dir(None, php_bin)?.join(format!("fpm-{slug}")))
}

/// A stable (within and across runs) 64-bit FNV-1a hash of a path's bytes, used
/// only to give each preload master a unique, collision-free directory slug.
fn path_hash(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `PHP_INI_SCAN_DIR` covering `dirs`, with a leading empty entry so PHP still
/// reads its compiled-in system conf.d first — then each dpl dir in order. An
/// empty `dirs` should never reach here (the caller guards), but yields `:`,
/// which is just "the system dir", harmlessly.
fn scan_dir_env(dirs: &[PathBuf]) -> String {
    let mut out = String::new();
    for dir in dirs {
        out.push(':');
        out.push_str(&dir.to_string_lossy());
    }
    out
}

/// Write (or overwrite) a minimal php-fpm config for this master and return its
/// path. `ondemand` keeps idle footprint near zero — which is what makes a
/// second, Xdebug- or SPX-enabled master cheap to leave running. The 5-minute
/// idle window is a deliberate cold-start trade: the first request to an idle
/// worker recompiles the app into opcache (the slow part), so letting a warm
/// worker survive normal dev pauses — a rebuild, a coffee — keeps opcache primed
/// and subsequent requests fast, at the cost of one lingering worker per master.
///
/// When `preload` is set, the pool gets `opcache.preload` pointed at that script
/// (with headroom for the compiled code), so php-fpm runs it once at master start
/// and holds the compiled classes in shared memory for every worker — turning the
/// first real request warm. Only preload masters carry it; the shared master does
/// not, so ordinary sites pay nothing.
fn write_conf(
    php_bin: &Path,
    mode: &Mode,
    profile: bool,
    preload: Option<&Path>,
    port: u16,
    max_children: u32,
) -> Result<PathBuf> {
    let dir = master_dir(php_bin, mode, profile, preload)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let log = dir.join("fpm.log");
    let slow = dir.join("slow.log");
    let conf = dir.join("php-fpm.conf");
    // Preloaded pools need a bigger opcache segment (the whole preload script is
    // resident) and the directive itself.
    let preload_conf = match preload {
        Some(script) => format!(
            "php_admin_value[opcache.memory_consumption] = 256\n\
             php_admin_value[opcache.preload] = {}\n",
            script.display(),
        ),
        None => String::new(),
    };
    let body = format!(
        "[global]\n\
         error_log = {log}\n\
         daemonize = no\n\
         \n\
         [www]\n\
         listen = 127.0.0.1:{port}\n\
         listen.backlog = 256\n\
         pm = ondemand\n\
         pm.max_children = {max_children}\n\
         pm.process_idle_timeout = 300s\n\
         pm.max_requests = 2000\n\
         pm.status_path = {STATUS_PATH}\n\
         ping.path = {PING_PATH}\n\
         ping.response = pong\n\
         request_terminate_timeout = 90s\n\
         request_slowlog_timeout = {SLOWLOG_TIMEOUT}s\n\
         slowlog = {slow}\n\
         catch_workers_output = yes\n\
         clear_env = no\n\
         {preload_conf}",
        log = log.display(),
        slow = slow.display(),
    );
    std::fs::write(&conf, body).with_context(|| format!("writing {}", conf.display()))?;
    // A side-effect-free target for the post-spawn warm-up hit (see `warm`).
    std::fs::write(dir.join("warm.php"), "<?php echo 'ok';\n").ok();
    Ok(conf)
}

/// Fire one throwaway FastCGI request at a freshly-spawned master so php-fpm
/// forks its first worker and PHP fully initializes — extensions' MINIT, the
/// shared opcache segment — *before* a real request arrives. It runs a trivial
/// dpl-owned script, never the user's app, so it has no side effects; the only
/// thing it buys is moving the one-time fork + interpreter startup off the
/// first real request's critical path. Fire-and-forget: any error is ignored,
/// since a failed warm-up just means the first real request pays as it did before.
fn warm(port: u16, dir: PathBuf) {
    tokio::spawn(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let script = dir.join("warm.php");
        let params = [
            ("GATEWAY_INTERFACE", "CGI/1.1"),
            ("REQUEST_METHOD", "GET"),
            ("SCRIPT_NAME", "/warm.php"),
            ("REQUEST_URI", "/warm.php"),
            ("SERVER_PROTOCOL", "HTTP/1.1"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .chain([
            ("SCRIPT_FILENAME".to_string(), script.to_string_lossy().into_owned()),
            ("DOCUMENT_ROOT".to_string(), dir.to_string_lossy().into_owned()),
        ])
        .collect::<Vec<_>>();
        let _ = crate::fastcgi::request(addr, &params, &[]).await;
    });
}

fn log_file(php_bin: &Path, mode: &Mode, profile: bool, preload: Option<&Path>) -> Result<std::fs::File> {
    let dir = master_dir(php_bin, mode, profile, preload)?;
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

    /// A real php-fpm 8.x `?json` status body, headers and all. Pinned as a
    /// fixture because the mapping in `parse_status` depends on php-fpm's exact
    /// key names — a rename upstream would otherwise surface only as counters
    /// that are silently zero, which reads as "pool idle" rather than "broken".
    const STATUS_JSON: &[u8] = b"Content-Type: application/json\r\n\r\n\
{\"pool\":\"www\",\"process manager\":\"ondemand\",\"start time\":1754600000,\
\"start since\":3600,\"accepted conn\":1234,\"listen queue\":7,\"max listen queue\":19,\
\"listen queue len\":256,\"idle processes\":0,\"active processes\":40,\
\"total processes\":40,\"max active processes\":40,\"max children reached\":3,\
\"slow requests\":11}";

    #[test]
    fn status_json_maps_onto_the_counters_we_report() {
        let s = parse_status(STATUS_JSON).unwrap();
        assert_eq!(s.active, 40);
        assert_eq!(s.idle, 0);
        assert_eq!(s.total, 40);
        assert_eq!(s.listen_queue, 7);
        assert_eq!(s.max_listen_queue, 19);
        assert_eq!(s.max_children_reached, 3);
        assert_eq!(s.slow_requests, 11);
        assert_eq!(s.accepted_conn, 1234);
        assert_eq!(s.uptime, 3600);
    }

    #[test]
    fn a_pool_with_every_worker_busy_and_callers_waiting_is_saturated() {
        let s = parse_status(STATUS_JSON).unwrap();
        assert!(s.saturated(40), "40/40 busy with 7 queued is the 502 case");
    }

    #[test]
    fn an_idle_pool_is_not_saturated_however_big_the_queue_field_reads() {
        // Guards the shape of the check: a pool with spare workers is never
        // saturated, so a stale queue reading can't produce a false diagnosis.
        let mut s = parse_status(STATUS_JSON).unwrap();
        s.idle = 5;
        s.total = 6;
        s.active = 1;
        assert!(!s.saturated(40));
    }

    #[test]
    fn a_body_without_cgi_headers_still_parses() {
        // php-fpm's reply has headers; a fixture or a future version might not.
        let s = parse_status(b"{\"active processes\":2,\"idle processes\":1}").unwrap();
        assert_eq!(s.active, 2);
        assert_eq!(s.idle, 1);
    }

    #[test]
    fn php_version_comes_from_the_path_without_execing_anything() {
        assert_eq!(php_label(Path::new("/opt/homebrew/opt/php@8.4/bin/php")), "8.4");
        // Unrecognised layouts fall back to something unambiguous rather than
        // guessing a version.
        assert_eq!(php_label(Path::new("/usr/bin/php")), "/usr/bin/php");
    }

    #[test]
    fn a_just_failed_master_waits_but_becomes_eligible_once_the_backoff_passes() {
        // The bug this guards: the first respawn attempt lands inside the
        // backoff and is refused, so anything that only retries on the tick
        // where the death was *noticed* never retries at all — the pool stays
        // down for good and its sites 404 while the daemon reports supervising
        // them. Eligibility therefore has to be a function of elapsed time, not
        // of having just observed the death.
        let mut m = FpmManager::new();
        let key: MasterKey =
            (PathBuf::from("/opt/homebrew/opt/php@8.4/bin/php"), Mode::off(), false, None);

        m.failed.insert(key.clone(), (1, std::time::Instant::now()));
        assert!(!m.may_start(&key), "a master that died a moment ago waits");

        // Rewind the clock past the first rung of the ladder.
        m.failed.insert(key.clone(), (1, std::time::Instant::now() - BACKOFF[0] * 2));
        assert!(m.may_start(&key), "once the backoff has elapsed it must be retried");
    }

    #[test]
    fn a_master_that_keeps_dying_is_eventually_left_alone() {
        let mut m = FpmManager::new();
        let key: MasterKey =
            (PathBuf::from("/opt/homebrew/opt/php@8.4/bin/php"), Mode::off(), false, None);
        // However long ago it failed, past the cap it stays down until a human
        // intervenes — a boot error is not fixed by restarting it forever.
        m.failed.insert(
            key.clone(),
            (MAX_FAILURES, std::time::Instant::now() - std::time::Duration::from_secs(3600)),
        );
        assert!(!m.may_start(&key));
    }

    #[test]
    fn an_unknown_master_starts_immediately() {
        let m = FpmManager::new();
        let key: MasterKey =
            (PathBuf::from("/opt/homebrew/opt/php@8.4/bin/php"), Mode::off(), false, None);
        assert!(m.may_start(&key), "no failure history means no delay");
    }

    #[test]
    fn the_backoff_ladder_is_bounded_at_both_ends() {
        assert_eq!(backoff_for(0), BACKOFF[0], "a zeroth failure still waits");
        assert_eq!(backoff_for(1), BACKOFF[0]);
        assert_eq!(backoff_for(99), BACKOFF[BACKOFF.len() - 1], "never indexes past the ladder");
    }

    #[test]
    fn the_pool_config_carries_the_status_and_slowlog_directives() {
        // These are what the whole feature rests on: without them php-fpm
        // reports nothing and writes no backtraces, and every scrape 404s.
        let dir = std::env::temp_dir().join("dpl-fpm-conf-test");
        std::fs::create_dir_all(&dir).ok();
        let php = Path::new("/opt/homebrew/opt/php@8.4/bin/php");
        let conf = write_conf(php, &Mode::off(), false, None, 9999, 25).unwrap();
        let body = std::fs::read_to_string(&conf).unwrap();
        assert!(body.contains(&format!("pm.status_path = {STATUS_PATH}")));
        assert!(body.contains(&format!("ping.path = {PING_PATH}")));
        assert!(body.contains("slowlog = "));
        assert!(body.contains(&format!("request_slowlog_timeout = {SLOWLOG_TIMEOUT}s")));
        assert!(body.contains("pm.max_children = 25"), "the ceiling is configurable");
    }

    #[test]
    fn masters_for_different_modes_get_different_dirs() {
        let php = Path::new("/opt/homebrew/opt/php@8.3/bin/php");
        let off = master_dir(php, &Mode::off(), false, None).unwrap();
        let dbg = master_dir(php, &Mode::parse("debug").unwrap(), false, None).unwrap();
        assert_ne!(off, dbg);
        assert!(off.ends_with("fpm-off"));
        assert!(dbg.ends_with("fpm-debug"));
        // Both hang off the same per-binary directory, which also holds the
        // shared conf.d the loader ini is written into.
        assert_eq!(off.parent(), dbg.parent());
    }

    /// A profiled site gets a distinct master from the plain one at the same mode,
    /// so a plain request never loads SPX.
    #[test]
    fn profiler_masters_are_distinct_from_plain_ones() {
        let php = Path::new("/opt/homebrew/opt/php@8.3/bin/php");
        let plain = master_dir(php, &Mode::off(), false, None).unwrap();
        let profiled = master_dir(php, &Mode::off(), true, None).unwrap();
        assert_ne!(plain, profiled);
        assert!(plain.ends_with("fpm-off"));
        assert!(profiled.ends_with("fpm-off-spx"));
    }

    /// A preloaded site gets its own master directory, distinct from the shared
    /// one at the same mode, and two different preload scripts never collide.
    #[test]
    fn preload_masters_are_distinct_and_per_script() {
        let php = Path::new("/opt/homebrew/opt/php@8.3/bin/php");
        let shared = master_dir(php, &Mode::off(), false, None).unwrap();
        let a = master_dir(php, &Mode::off(), false, Some(Path::new("/a/preload.php"))).unwrap();
        let b = master_dir(php, &Mode::off(), false, Some(Path::new("/b/preload.php"))).unwrap();
        assert_ne!(shared, a);
        assert_ne!(a, b, "different preload scripts must not share a master dir");
        assert!(a.file_name().unwrap().to_str().unwrap().starts_with("fpm-off-preload-"));
        // Stable: the same script resolves to the same dir across calls.
        assert_eq!(a, master_dir(php, &Mode::off(), false, Some(Path::new("/a/preload.php"))).unwrap());
    }

    #[test]
    fn equivalent_mode_spellings_share_one_master_dir() {
        let php = Path::new("/opt/homebrew/opt/php@8.3/bin/php");
        let a = master_dir(php, &Mode::parse("debug,develop").unwrap(), false, None).unwrap();
        let b = master_dir(php, &Mode::parse("develop,debug").unwrap(), false, None).unwrap();
        assert_eq!(a, b);
    }

    /// The one that matters: dropping this reintroduces a macOS segfault that no
    /// other test can catch (it needs a real forked worker under an aged master).
    #[test]
    fn every_master_disables_fork_unsafe_gssapi() {
        for mode in [Mode::off(), Mode::parse("debug").unwrap()] {
            let env = master_env(&mode, 9912);
            assert!(
                env.iter().any(|(k, v)| *k == "PGGSSENCMODE" && v == "disable"),
                "PGGSSENCMODE=disable missing for mode {mode:?}: {env:?}"
            );
        }
    }

    #[test]
    fn master_env_pins_xdebug_mode_even_when_off() {
        let env = master_env(&Mode::off(), 9912);
        let xdebug = env.iter().find(|(k, _)| *k == "XDEBUG_MODE");
        assert_eq!(xdebug.map(|(_, v)| v.as_str()), Some("off"));
    }

    #[test]
    fn master_env_carries_the_dumps_receiver_port() {
        let env = master_env(&Mode::off(), 4321);
        assert!(env.iter().any(|(k, v)| *k == "DUMPS_PORT" && v == "4321"));
    }
}
