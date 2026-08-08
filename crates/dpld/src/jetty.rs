//! Supervises Jetty tunnels, so an opted-in site has a public URL that is
//! always live.
//!
//! A tunnel is `jetty share` holding a WebSocket to the Jetty edge: public
//! requests arrive there and are forwarded back down to us. Because the agent
//! is a long-lived process that can drop, die, or be killed, it needs the same
//! supervision a dev server gets — start, restart, back off when it fails
//! repeatedly, and stop the whole process tree rather than the launcher.
//!
//! Two details make this work rather than merely run:
//!
//! **The tunnel points at the site's own hostname, not at an IP.** dpl serves
//! every site from one port and routes on the `Host` header, so what the agent
//! puts there decides which site a public request reaches. The agent sets `Host`
//! from `--host`, so pointing it at `<site>.<tld>` makes tunnel traffic arrive
//! looking exactly like local traffic and take exactly the same route.
//!
//! Getting this wrong is quiet rather than loud: with `--host=127.0.0.1` the
//! tunnel connects, the agent forwards, dpl answers — with the site-index page,
//! because `127.0.0.1` matches no site. A working tunnel serving the wrong page.
//!
//! As a fallback the registry also treats a live tunnel's public hostname as an
//! alias for its site, so traffic still routes if it ever arrives with the
//! public `Host` instead (`Registry::site_for_host`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

/// How long to wait after each successive failed start. A tunnel that dies
/// instantly is usually a bad token, a label the team doesn't hold, or no
/// network; retrying hard fixes none of those and rate-limits the API.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
const MAX_FAILURES: u32 = 5;

/// A tunnel that stays up this long counts as healthy, clearing the backoff.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);

/// Where Jetty publishes tunnels. Only used to recognise our own hostnames and
/// to predict the URL before the agent has printed one.
const TUNNEL_SUFFIX: &str = "tunnels.usejetty.online";

struct Tunnel {
    /// Reserved subdomain label — the thing that makes the URL permanent.
    label: String,
    /// The site's canonical local hostname, e.g. `placeholder.test`. This is
    /// what the agent sends as `Host`, and therefore what dpl routes on.
    host: String,
    /// dpl's http port, which the agent forwards to.
    port: u16,
    child: Option<Child>,
    /// The agent's process group, so stopping it takes the whole tree.
    pgid: Option<u32>,
    started: Instant,
    failures: u32,
    retry_at: Option<Instant>,
    /// The public URL, once the agent has announced it.
    url: Option<String>,
    detail: Option<String>,
    log: PathBuf,
}

/// What `dpl share` and the GUI display for one site.
#[derive(Clone, Debug)]
pub struct ShareInfo {
    pub site: String,
    pub label: String,
    pub running: bool,
    pub pgid: Option<u32>,
    pub url: Option<String>,
    pub detail: Option<String>,
    pub log: String,
}

pub struct Tunnels {
    tunnels: BTreeMap<String, Tunnel>,
}

impl Tunnels {
    pub fn new() -> Self {
        Tunnels { tunnels: BTreeMap::new() }
    }

    /// Ensure `site` has a tunnel on `label`, starting one if it isn't running.
    /// Idempotent — called from every reconcile and every supervise tick.
    pub fn ensure(&mut self, site: &str, label: &str, host: &str, port: u16) {
        if let Some(t) = self.tunnels.get_mut(site) {
            if t.label == label && t.port == port && t.host == host {
                if t.is_alive() {
                    return;
                }
            } else {
                // A new label is a new URL, so the old agent is serving the
                // wrong name and has to go.
                t.stop();
                self.tunnels.remove(site);
            }
        }

        let entry = self.tunnels.entry(site.to_string()).or_insert_with(|| Tunnel {
            label: label.to_string(),
            host: host.to_string(),
            port,
            child: None,
            pgid: None,
            started: Instant::now(),
            failures: 0,
            retry_at: None,
            url: None,
            detail: None,
            log: log_path(site),
        });
        entry.label = label.to_string();
        entry.host = host.to_string();
        entry.port = port;

        if entry.failures >= MAX_FAILURES {
            return;
        }
        if entry.retry_at.map(|t| Instant::now() < t).unwrap_or(false) {
            return;
        }
        entry.start(site);
    }

    /// Kill tunnel agents left behind by a previous daemon.
    ///
    /// `kill_on_drop` doesn't fire when launchd SIGKILLs us, so agents outlive
    /// the daemon that started them. That is worse than an idle stray process:
    /// the agent still holds its reserved label, and Jetty refuses to create a
    /// second tunnel on a label already in use — so the *new* daemon's agent
    /// fails with "that label is already in use" and the site has no public URL
    /// until someone finds the stray by hand. Worse still, the orphan is running
    /// the arguments it was started with, so a settings change appears to have
    /// no effect while an old tunnel quietly keeps serving.
    ///
    /// A parent of init is the discriminator, exactly as for orphaned php-fpm
    /// workers: an agent dpl started has dpld for a parent, and a `jetty share`
    /// you ran yourself in a terminal has your shell. Only a dead supervisor
    /// leaves one parented to init.
    pub fn kill_orphans() {
        let Ok(out) =
            std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,command="]).output()
        else {
            return;
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if !line.contains("jetty") || !line.contains(" share ") {
                continue;
            }
            let mut fields = line.split_whitespace();
            let (Some(pid), Some("1")) = (fields.next(), fields.next()) else { continue };
            let Ok(pid) = pid.parse::<u32>() else { continue };
            if pid <= 1 {
                continue;
            }
            let _ = std::process::Command::new("/bin/kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            tracing::info!(pid, "reaped orphaned jetty tunnel agent");
        }
    }

    /// Stop tunnels for sites that no longer want one.
    pub fn retain(&mut self, keep: &BTreeSet<String>) {
        let stale: Vec<String> =
            self.tunnels.keys().filter(|k| !keep.contains(*k)).cloned().collect();
        for site in stale {
            if let Some(mut t) = self.tunnels.remove(&site) {
                t.stop();
                tracing::info!(site, "tunnel stopped");
            }
        }
    }

    /// One supervision pass: reap the dead, restart what should be up, and pick
    /// up a URL the agent has announced since we last looked.
    pub fn supervise(&mut self) {
        let names: Vec<String> = self.tunnels.keys().cloned().collect();
        for site in names {
            let Some(t) = self.tunnels.get_mut(&site) else { continue };
            if t.is_alive() {
                if t.url.is_none() {
                    t.url = scrape_url(&t.log).or_else(|| Some(predicted_url(&t.label)));
                }
                if t.failures > 0 && t.started.elapsed() >= HEALTHY_AFTER {
                    t.failures = 0;
                    t.detail = None;
                }
                continue;
            }
            if t.failures >= MAX_FAILURES {
                continue;
            }
            if t.retry_at.map(|x| Instant::now() < x).unwrap_or(false) {
                continue;
            }
            tracing::info!(site = %site, "tunnel restarting");
            t.start(&site);
        }
    }

    /// Clear a site's failure history and start it again — `dpl share restart`.
    /// The reset matters: a tunnel that gave up (an expired token, say) must be
    /// recoverable without toggling sharing off and on.
    pub fn restart(&mut self, site: &str) -> bool {
        let Some(t) = self.tunnels.get_mut(site) else { return false };
        t.stop();
        t.failures = 0;
        t.retry_at = None;
        t.detail = None;
        t.url = None;
        t.start(site);
        true
    }

    pub fn statuses(&self) -> Vec<ShareInfo> {
        self.tunnels
            .iter()
            .map(|(site, t)| ShareInfo {
                site: site.clone(),
                label: t.label.clone(),
                running: t.child.is_some(),
                pgid: t.pgid,
                url: t.url.clone(),
                detail: t.detail.clone(),
                log: t.log.to_string_lossy().into_owned(),
            })
            .collect()
    }

    /// Map a public tunnel hostname back to the site it belongs to.
    ///
    /// This is what lets a request arriving from the edge — whose `Host` is the
    /// tunnel name, not a `.test` name — reach the right site. Matched against
    /// configured labels rather than only running tunnels, so a request landing
    /// while the agent is mid-restart still routes instead of falling through to
    /// the index page.
    pub fn site_for_tunnel_host(&self, host: &str) -> Option<String> {
        let bare = host.split(':').next().unwrap_or(host);
        let label = bare.strip_suffix(&format!(".{TUNNEL_SUFFIX}"))?;
        self.tunnels
            .iter()
            .find(|(_, t)| t.label.eq_ignore_ascii_case(label))
            .map(|(site, _)| site.clone())
    }
}

impl Tunnel {
    fn is_alive(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else { return false };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                let long_enough = self.started.elapsed() >= HEALTHY_AFTER;
                self.child = None;
                self.pgid = None;
                self.url = None;
                if long_enough {
                    self.failures = 1;
                } else {
                    self.failures += 1;
                }
                self.detail = Some(if self.failures >= MAX_FAILURES {
                    format!(
                        "exited ({status}) {} times — giving up. Check `jetty doctor`, \
                         then `dpl share restart`.",
                        self.failures
                    )
                } else {
                    format!("exited ({status}) — reconnecting")
                });
                self.retry_at = Some(Instant::now() + backoff_for(self.failures));
                false
            }
            Err(_) => {
                self.child = None;
                self.pgid = None;
                false
            }
        }
    }

    fn start(&mut self, site: &str) {
        // `--host` is the site's own `.test` name rather than 127.0.0.1, because
        // the agent copies it into the `Host` header of everything it forwards
        // and that header is how dpl picks the site. An IP here connects fine
        // and then serves the wrong page — see the module docs.
        //
        let args = [
            "share".to_string(),
            self.port.to_string(),
            format!("--host={}", self.host),
            format!("--subdomain={}", self.label),
            // Deliberately *not* --no-delete-on-exit. Permanence comes from the
            // reserved label, which the team owns and which resolves to the
            // same URL every time; the tunnel record is per-run. Leaving the
            // record registered instead means a stopped agent still holds the
            // label, and the next start is refused it ("that label is already
            // in use by an active tunnel") — the site loses the very URL the
            // flag was meant to preserve.
            "--delete-on-exit".to_string(),
            // Health-probing the proxy would only prove the proxy is up, which
            // we already know — and a site that is briefly 502ing must not stop
            // its tunnel from existing.
            "--no-health-check".to_string(),
        ];

        let Some((bin, prefix)) = jetty_launcher() else {
            self.failures += 1;
            self.detail = Some(
                "can't find the `jetty` CLI, or `cpx` to run it with. The daemon runs under \
                 launchd's minimal PATH, so a Composer-global install isn't visible to it — \
                 install cpx (`composer global require cpx/cpx`) or set JETTY_BIN, then \
                 `dpl share restart`."
                    .to_string(),
            );
            self.retry_at = Some(Instant::now() + backoff_for(self.failures));
            tracing::warn!(site, "jetty CLI not found");
            return;
        };

        let mut cmd = Command::new(&bin);
        cmd.args(&prefix)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(open_log(&self.log)))
            .stderr(std::process::Stdio::from(open_log(&self.log)))
            .process_group(0)
            .kill_on_drop(true);

        match cmd.spawn() {
            Ok(child) => {
                self.pgid = child.id();
                self.child = Some(child);
                self.started = Instant::now();
                self.url = None;
                self.detail = None;
                tracing::info!(site, label = %self.label, "tunnel started");
            }
            Err(e) => {
                self.failures += 1;
                self.detail = Some(format!("could not start {}: {e}", bin.display()));
                self.retry_at = Some(Instant::now() + backoff_for(self.failures));
                tracing::warn!(site, error = %e, "tunnel failed to start");
            }
        }
    }

    fn stop(&mut self) {
        if let Some(pgid) = self.pgid.take() {
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
        self.url = None;
    }
}

/// Find the `jetty` binary.
///
/// `PATH` alone is not enough here. The daemon is started by launchd with a
/// deliberately minimal environment — the plist lists the Homebrew and system
/// bin directories and nothing else — while `jetty` installs by default into
/// Composer's global bin dir, which is in *your* shell's PATH and not in the
/// daemon's. Relying on `PATH` therefore works when you run things by hand and
/// fails once launchd owns the process, which is the only way it actually runs.
///
/// `JETTY_BIN` wins when set, so an unusual install is one env var away rather
/// than a code change.
fn jetty_bin() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("JETTY_BIN") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    // Order matters, and not for the usual reason. Jetty can be installed more
    // than once on a machine — its own `doctor` warns about exactly this — and
    // the copies drift: here a Composer-global v0.1.60 sat alongside a v0.1.61
    // phar, and the older one crashed resuming a tunnel
    // (`attachTunnel(): $tunnelId must be of type string, int given`), which
    // presents as a tunnel that connects and then dies in a loop. So prefer the
    // installer's own location over Composer's, which is where a stale copy
    // tends to linger after switching install methods.
    let home = dpl_core::paths::home(None).ok();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(h) = &home {
        candidates.push(h.join(".local/bin/jetty"));
    }
    if let Some(found) = dpl_core::tools::which("jetty") {
        candidates.push(found);
    }
    if let Some(h) = &home {
        candidates.push(h.join(".composer/vendor/bin/jetty"));
        candidates.push(h.join(".config/composer/vendor/bin/jetty"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/jetty"));
    candidates.push(PathBuf::from("/usr/local/bin/jetty"));
    candidates.into_iter().find(|p| p.is_file())
}

/// The Composer package behind the `jetty` CLI, for running it through cpx.
const JETTY_PACKAGE: &str = "jetty/client";

/// How to invoke the Jetty CLI: a program plus any arguments that must precede
/// the ones we want to pass.
///
/// Two shapes, in preference order. An installed binary is used directly. If
/// there is none, the tool is run through [cpx](https://cpx.dev) — `npx` for
/// Composer packages — which fetches and caches `jetty/client` on demand.
///
/// The cpx path is worth having for more than convenience. Every Jetty failure
/// this integration has hit was an *installation* problem rather than a tunnel
/// problem: a Composer-global install invisible to the daemon's launchd PATH,
/// and two installs drifting to different versions where the older one crashed
/// resuming a tunnel. cpx keeps one cached copy it updates itself, so neither
/// can happen.
fn jetty_launcher() -> Option<(PathBuf, Vec<String>)> {
    if let Some(bin) = jetty_bin() {
        return Some((bin, Vec::new()));
    }
    // Same search as the CLI itself: cpx is a Composer-global install, so the
    // daemon's minimal PATH usually can't see it either.
    let home = dpl_core::paths::home(None).ok();
    let cpx = dpl_core::tools::which("cpx").or_else(|| {
        home.as_ref().and_then(|h| {
            [h.join(".composer/vendor/bin/cpx"), h.join(".config/composer/vendor/bin/cpx")]
                .into_iter()
                .find(|p| p.is_file())
        })
    })?;
    // `-n` so a fetch never stops on a prompt no one is there to answer.
    Some((cpx, vec![JETTY_PACKAGE.to_string(), "-n".to_string()]))
}

/// The URL a reserved label resolves to. Used before the agent has printed one,
/// so the UI can show where a site *will* appear rather than a blank while the
/// tunnel is connecting.
fn predicted_url(label: &str) -> String {
    format!("https://{label}.{TUNNEL_SUFFIX}")
}

/// Pull the public URL out of the agent's own output.
///
/// Preferred over [`predicted_url`] because it is ground truth: if Jetty hands
/// out a different name than the label asked for — a reservation that lapsed,
/// a fallback to a random subdomain — the printed URL is the one that works.
fn scrape_url(log: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    // Scan newest-first: a reconnect appends a fresh URL and the old one is
    // stale the moment it does.
    text.lines().rev().find_map(|line| {
        let idx = line.find("https://")?;
        let url: String = line[idx..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
            .collect();
        url.contains(TUNNEL_SUFFIX).then_some(url)
    })
}

fn backoff_for(failures: u32) -> Duration {
    let idx = (failures.max(1) as usize - 1).min(BACKOFF.len() - 1);
    BACKOFF[idx]
}

fn log_path(site: &str) -> PathBuf {
    let dir = dpl_core::paths::logs_dir(None).unwrap_or_else(|_| PathBuf::from("/tmp"));
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("{site}-share.log"))
}

fn open_log(path: &std::path::Path) -> std::fs::File {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|_| std::fs::File::create("/dev/null").expect("/dev/null"))
}

/// Keep opted-in sites' tunnels alive for the daemon's lifetime.
pub async fn watch(state: crate::server::DaemonState) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        state.registry.lock().await.supervise_tunnels();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tunnel_hostname_maps_back_to_its_site() {
        // The whole point: a request from the edge carries the public hostname,
        // and without this it matches no `.test` name and lands on the index.
        let mut t = Tunnels::new();
        t.tunnels.insert(
            "dply".to_string(),
            Tunnel {
                label: "dply-dev".into(),
                host: "dply.test".into(),
                port: 80,
                child: None,
                pgid: None,
                started: Instant::now(),
                failures: 0,
                retry_at: None,
                url: None,
                detail: None,
                log: PathBuf::from("/dev/null"),
            },
        );

        assert_eq!(
            t.site_for_tunnel_host("dply-dev.tunnels.usejetty.online"),
            Some("dply".to_string())
        );
        // Ports appear on the Host header when a client sends one.
        assert_eq!(
            t.site_for_tunnel_host("dply-dev.tunnels.usejetty.online:443"),
            Some("dply".to_string())
        );
        // A label we don't hold must not be claimed, or one dpl would answer
        // for another user's tunnel.
        assert_eq!(t.site_for_tunnel_host("someone-else.tunnels.usejetty.online"), None);
        // Ordinary local traffic is none of this code's business.
        assert_eq!(t.site_for_tunnel_host("dply.test"), None);
    }

    #[test]
    fn the_announced_url_wins_over_the_predicted_one() {
        let dir = std::env::temp_dir().join("dpl-jetty-scrape-test");
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("share.log");

        // Jetty fell back to a different name than we asked for; the printed one
        // is the only one that actually resolves.
        std::fs::write(
            &log,
            "connecting…\nTunnel live at https://fallback-xyz.tunnels.usejetty.online\n",
        )
        .unwrap();
        assert_eq!(
            scrape_url(&log).as_deref(),
            Some("https://fallback-xyz.tunnels.usejetty.online")
        );

        // A reconnect appends a newer URL, which supersedes the earlier one.
        std::fs::write(
            &log,
            "live at https://old.tunnels.usejetty.online\nreconnected: https://new.tunnels.usejetty.online\n",
        )
        .unwrap();
        assert_eq!(scrape_url(&log).as_deref(), Some("https://new.tunnels.usejetty.online"));

        // Unrelated URLs in the output are not tunnel URLs.
        std::fs::write(&log, "see https://usejetty.online/docs for help\n").unwrap();
        assert_eq!(scrape_url(&log), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_backoff_ladder_is_bounded() {
        assert_eq!(backoff_for(0), BACKOFF[0]);
        assert_eq!(backoff_for(99), BACKOFF[BACKOFF.len() - 1]);
    }
}
