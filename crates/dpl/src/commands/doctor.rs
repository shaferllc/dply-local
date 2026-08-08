//! `dpl doctor` — a structured health report for the local environment.
//!
//! Every probe produces a [`Check`] with a stable `id`, a severity, a
//! human-readable detail, and (where we know one) the exact command that fixes
//! it. The CLI renders the checks as marked lines; the GUI consumes the same
//! report as JSON (`dpl doctor --json`) and renders each check as a row with a
//! working "fix" button — so the two never drift.

use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::checks::{Checks, Report, Status};
use crate::commands::local::{port_server, which};
use crate::daemon;
use dpl_core::ipc::{Request, Response, SiteInfo};

/// Categories in the order they're rendered.
const CATEGORIES: [&str; 8] =
    ["System", "Daemon", "Networking", "TLS", "PHP", "Sites", "Services", "Tooling"];

/// Run every probe and print the report (JSON with `--json`).
pub fn run(home: Option<&str>, json: bool) -> Result<()> {
    let report = build(home);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    report.render();
    Ok(())
}

/// Collect the full report.
pub fn build(home: Option<&str>) -> Report {
    let mut c = Checks::default();

    // The daemon snapshot is the input to several categories, so take it once.
    let sites = match daemon::call(Request::ListSites, home) {
        Ok(Response::Sites { sites, http_port, tld }) => Some((sites, http_port, tld)),
        _ => None,
    };

    check_system(&mut c, home);
    check_daemon(&mut c, home, sites.is_some());
    check_networking(&mut c, home, sites.as_ref().map(|(_, p, _)| *p));
    check_tls(&mut c, home);
    check_php(&mut c, home);
    check_sites(&mut c, home, sites.as_ref().map(|(s, _, _)| s.as_slice()));
    check_services(&mut c, home);
    check_tooling(&mut c);

    Report::new("dpl doctor", None, &CATEGORIES, c.0)
}

// ---- System -----------------------------------------------------------------

fn check_system(c: &mut Checks, home: Option<&str>) {
    let cat = "System";

    c.add(
        "system.platform",
        cat,
        "Platform",
        Status::Info,
        format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH),
    );

    c.add("system.cli_version", cat, "dpl version", Status::Info, env!("CARGO_PKG_VERSION"));

    // Load average: a saturated CPU starves php-fpm of workers, which reads to
    // the user as "dpl is hanging". Worth calling out explicitly.
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    // Load is transient and has no fix action, so a saturated CPU is a warning,
    // never a failure — otherwise every `cargo build` reports a broken install.
    match load_average() {
        Some(load) => {
            let ratio = load / cores as f64;
            let status = if ratio >= 0.9 { Status::Warn } else { Status::Pass };
            let state = if ratio >= 1.5 { "saturated" } else { "busy" };
            let detail = format!("{load:.2} across {cores} cores");
            let detail = if status == Status::Pass { detail } else { format!("{detail} — {state}") };
            c.add("system.load", cat, "System load", status, detail);
            if status != Status::Pass {
                c.hint(
                    "The CPU is oversubscribed, so php-fpm can't spawn workers fast enough and \
                     sites may hang. This isn't dpl — reduce background load (idle queue workers, \
                     builds, containers).",
                );
            }
        }
        None => c.add("system.load", cat, "System load", Status::Info, format!("{cores} cores")),
    }

    // Free space where certs, logs, and database volumes live.
    if let Ok(dir) = dpl_core::paths::dpl_dir(home) {
        if let Some(gb) = disk_free_gb(&dir) {
            let status = if gb < 1.0 {
                Status::Fail
            } else if gb < 5.0 {
                Status::Warn
            } else {
                Status::Pass
            };
            c.add("system.disk", cat, "Disk space", status, format!("{gb:.1} GB free on {}", dir.display()));
            if status != Status::Pass {
                c.hint("Databases and logs write here; MySQL/Postgres refuse to start when the volume fills.");
            }
        }
    }

    // Config file: a parse error silently strands every site.
    match dpl_core::paths::local_config(home) {
        Ok(path) if !path.exists() => {
            c.add("system.config", cat, "Config file", Status::Info, format!("not created yet ({})", path.display()));
        }
        Ok(path) => match dpl_core::config::LocalConfig::load(&path) {
            Ok(cfg) => {
                c.add(
                    "system.config",
                    cat,
                    "Config file",
                    Status::Pass,
                    format!(
                        "{} — {} link(s), {} parked, {} proxy(s), TLD(s): {}",
                        path.display(),
                        cfg.links.len(),
                        cfg.parked.len(),
                        cfg.proxies.len(),
                        cfg.tlds().join(", ")
                    ),
                );
            }
            Err(e) => {
                c.add("system.config", cat, "Config file", Status::Fail, format!("{} is unreadable: {e}", path.display()));
                c.hint("The daemon can't load your sites until this parses. Fix or delete the file.");
            }
        },
        Err(e) => c.add("system.config", cat, "Config file", Status::Fail, e.to_string()),
    }

    // The privileged helper backs setup/trust/resolution.
    match which("dpl-helper").or_else(helper_next_to_exe) {
        Some(p) => c.add("system.helper", cat, "Privileged helper", Status::Pass, p),
        None => {
            c.add("system.helper", cat, "Privileged helper", Status::Warn, "dpl-helper not found");
            c.hint("Needed for `dpl setup`, CA trust, and the :80/:443 redirect. Build it (`cargo build`) or install it beside `dpl`.");
        }
    }

    // Log growth is invisible until a disk fills.
    if let Ok(logs) = dpl_core::paths::logs_dir(home) {
        if logs.exists() {
            let mb = dir_size_bytes(&logs) as f64 / 1_048_576.0;
            let status = if mb > 512.0 { Status::Warn } else { Status::Info };
            c.add("system.logs", cat, "Logs", status, format!("{mb:.0} MB in {}", logs.display()));
            if status == Status::Warn {
                c.hint("Request logs have grown large. Delete old files in the logs directory.");
            }
        }
    }
}

// ---- Daemon -----------------------------------------------------------------

fn check_daemon(c: &mut Checks, home: Option<&str>, reachable: bool) {
    let cat = "Daemon";

    if !reachable {
        c.add("daemon.running", cat, "Daemon", Status::Fail, "not running — nothing is serving .test sites");
        c.fix("Start the daemon", "dpl start");
        return;
    }
    c.add("daemon.running", cat, "Daemon", Status::Pass, "running");

    // A stale dpld against a new dpl is a classic source of "unknown op" errors.
    if let Ok(Response::Version { version }) = daemon::call(Request::Version, home) {
        let cli = env!("CARGO_PKG_VERSION");
        if version == cli {
            c.add("daemon.version", cat, "Version", Status::Pass, format!("dpl and dpld both {version}"));
        } else {
            c.add("daemon.version", cat, "Version", Status::Warn, format!("dpl {cli}, dpld {version}"));
            c.hint("The running daemon is a different build than this CLI. Restart it to pick up the new binary.");
            c.fix("Restart the daemon", "dpl restart");
        }
    }

    if let Ok(Response::Status { status }) = daemon::call(Request::Status, home) {
        c.add("daemon.uptime", cat, "Uptime", Status::Info, humanize_secs(status.uptime_secs));
    }

    if let Ok(socket) = dpl_core::paths::daemon_socket(home) {
        c.add("daemon.socket", cat, "Control socket", Status::Info, socket.display().to_string());
    }

    // Autostart: without it, a reboot leaves every site dead.
    match autostart_installed() {
        true => c.add("daemon.autostart", cat, "Start on login", Status::Pass, "LaunchAgent installed"),
        false => {
            c.add("daemon.autostart", cat, "Start on login", Status::Warn, "not installed");
            c.hint("The daemon won't come back after a reboot until you start it by hand.");
            c.fix("Install autostart", "dpl daemon install");
        }
    }
}

// ---- Networking -------------------------------------------------------------

fn check_networking(c: &mut Checks, home: Option<&str>, http_port: Option<u16>) {
    let cat = "Networking";

    let Some(http_port) = http_port else {
        c.add("net.proxy", cat, "HTTP proxy", Status::Fail, "not listening (daemon is down)");
        c.fix("Start the daemon", "dpl start");
        return;
    };
    c.add("net.proxy", cat, "HTTP proxy", Status::Pass, format!("listening on port {http_port}"));

    // Ports 80/443: another web server here means the browser never reaches dpl.
    if http_port != 80 {
        let foreign = |s: &str| {
            let l = s.to_lowercase();
            l.contains("apache") || l.contains("nginx") || l.contains("valet") || l.contains("caddy")
        };
        match port_server(80) {
            Some(server) if foreign(&server) => {
                c.add("net.port_80", cat, "Port 80", Status::Fail, format!("held by {server}"));
                c.hint("The browser hits that server instead of dpl (e.g. Apache's “It works!” page).");
                c.fix("Take over ports 80/443", "dpl takeover");
                if port_server(443).is_some() {
                    c.add("net.port_443", cat, "Port 443", Status::Warn, format!("also held by {server}"));
                    c.fix("Take over ports 80/443", "dpl takeover");
                }
            }
            Some(_) => c.add("net.port_80", cat, "Port 80/443", Status::Pass, "served by dpl"),
            // On macOS the "Privileged ports" check below owns this story; saying
            // it twice just makes the report look like two problems.
            None if cfg!(target_os = "macos") => {}
            None => {
                c.add("net.port_80", cat, "Port 80", Status::Warn, "nothing is listening");
                c.hint(format!("Sites work at http://<name>.test:{http_port}, but not on the clean port-less URL.").as_str());
                c.fix("Run privileged setup", "sudo dpl setup");
            }
        }
    }

    // Who owns :80/:443. When the LaunchDaemon is installed, launchd binds them and
    // hands the descriptors to dpld — so `http_port == 80` is itself the proof it
    // worked, and a daemon on 8080 means the plist is there but didn't take.
    if cfg!(target_os = "macos") {
        let installed = Path::new("/Library/LaunchDaemons/com.dply.dpld.plist").exists();
        match (installed, http_port == 80) {
            (true, true) => {
                c.add("net.privports", cat, "Privileged ports", Status::Pass, ":80/:443 handed over by launchd");
            }
            (true, false) => {
                c.add("net.privports", cat, "Privileged ports", Status::Warn, format!("LaunchDaemon installed, but dpld is on :{http_port}"));
                c.hint("The daemon didn't get the launchd sockets — a stray LaunchAgent copy of dpld may have started first.");
                c.fix("Reinstall the port daemon", "sudo dpl setup");
            }
            (false, false) => {
                c.add("net.privports", cat, "Privileged ports", Status::Warn, "not installed");
                c.hint(format!("Sites work at http://<name>.test:{http_port}, but not on the clean port-less URL.").as_str());
                c.fix("Run privileged setup", "sudo dpl setup");
            }
            (false, true) => {
                c.add("net.privports", cat, "Privileged ports", Status::Pass, "dpld is serving :80 directly");
            }
        }

        // An anchor left over from the pf era is dead weight, and misleads anyone
        // debugging why :80 behaves oddly.
        if Path::new("/etc/pf.anchors/dpl").exists() {
            c.add("net.pf_legacy", cat, "Legacy pf redirect", Status::Warn, "/etc/pf.anchors/dpl is still present");
            c.hint("dpl no longer uses pf. The anchor does nothing now, but a stale rdr rule can still capture :80.");
            c.fix("Clean it up", "sudo dpl setup");
        }
    }

    // How .test names resolve, and whether that mode's plumbing is actually in place.
    let cfg = dpl_core::paths::local_config(home)
        .ok()
        .and_then(|p| dpl_core::config::LocalConfig::load(&p).ok())
        .unwrap_or_default();
    let tlds = cfg.tlds();

    if cfg.uses_hosts() {
        c.add("net.resolution", cat, "Resolution mode", Status::Info, "hosts-file (iCloud Private Relay keeps working)");
        let hosts = std::fs::read_to_string("/etc/hosts").unwrap_or_default();
        if hosts.contains("# >>> dpl managed") {
            c.add("net.hosts", cat, "/etc/hosts entries", Status::Pass, "dpl-managed block present");
        } else {
            c.add("net.hosts", cat, "/etc/hosts entries", Status::Warn, "no dpl-managed block");
            c.hint("Sites won't resolve until the daemon writes its block. Reload the daemon.");
            c.fix("Reload the daemon", "dpl reload");
        }
        if Path::new("/etc/sudoers.d/dpl").exists() {
            c.add("net.sudoers", cat, "Sudoers rule", Status::Pass, "/etc/sudoers.d/dpl installed");
        } else {
            c.add("net.sudoers", cat, "Sudoers rule", Status::Warn, "not installed");
            c.hint("Without it the daemon can't keep /etc/hosts in sync as you add sites.");
            c.fix("Re-apply hosts mode", "dpl resolution hosts");
        }
    } else {
        c.add("net.resolution", cat, "Resolution mode", Status::Info, "wildcard DNS resolver");
        c.hint("A local DNS resolver disables iCloud Private Relay. Switch to hosts mode to keep it on.");
        for tld in &tlds {
            // .localhost is resolved to loopback by the OS — no /etc/resolver needed.
            if dpl_core::config::is_native_tld(tld) {
                c.add(&format!("net.resolver.{tld}"), cat, &format!("Resolver .{tld}"), Status::Pass, "native — resolves to loopback, no resolver needed");
                continue;
            }
            let path = format!("/etc/resolver/{tld}");
            if Path::new(&path).exists() {
                c.add(&format!("net.resolver.{tld}"), cat, &format!("Resolver .{tld}"), Status::Pass, path);
            } else {
                c.add(&format!("net.resolver.{tld}"), cat, &format!("Resolver .{tld}"), Status::Fail, format!("{path} missing"));
                c.hint("Nothing routes this TLD to dpl, so the browser can't find your sites.");
                c.fix("Run privileged setup", "sudo dpl setup");
            }
        }
    }

    // Ask dpl's own DNS responder to resolve a name — the end-to-end proof that
    // the resolver, not just its config file, works.
    let probe = format!("dpl-doctor.{}", cfg.primary_tld());
    match dns_probe(5333, &probe) {
        Some(ip) if ip == Ipv4Addr::LOCALHOST => {
            c.add("net.dns", cat, "DNS responder", Status::Pass, format!("*.{} → 127.0.0.1 (:5333)", cfg.primary_tld()))
        }
        Some(ip) => c.add("net.dns", cat, "DNS responder", Status::Warn, format!("answered {ip}, expected 127.0.0.1")),
        None => {
            c.add("net.dns", cat, "DNS responder", Status::Warn, "no answer on :5333");
            if !cfg.uses_hosts() {
                c.hint("In resolver mode, .test lookups go here — sites won't resolve while it's silent.");
                c.fix("Restart the daemon", "dpl restart");
            }
        }
    }

    // The sinks sites talk to.
    let sink = |c: &mut Checks, id: &str, title: &str, port: u16, what: &str| {
        if tcp_listening(port) {
            c.add(id, cat, title, Status::Pass, format!("listening on :{port}"));
        } else {
            c.add(id, cat, title, Status::Warn, format!("not listening on :{port}"));
            c.hint(what);
        }
    };
    sink(c, "net.mail", "Mail sink", 1025, "Mail sent from sites won't be captured. Restart the daemon.");
    sink(c, "net.dumps", "Debug bridge", 9912, "`dump()` output won't reach the Dumps panel. Restart the daemon.");
}

// ---- TLS --------------------------------------------------------------------

fn check_tls(c: &mut Checks, home: Option<&str>) {
    let cat = "TLS";
    let Ok(certs) = dpl_core::paths::certs_dir(home) else { return };
    let ca = certs.join("ca.pem");

    if !ca.exists() {
        c.add("tls.ca", cat, "Local CA", Status::Fail, format!("missing at {}", ca.display()));
        c.hint("The daemon generates it on first start; HTTPS sites can't be issued certificates without it.");
        c.fix("Start the daemon", "dpl start");
        return;
    }
    c.add("tls.ca", cat, "Local CA", Status::Pass, ca.display().to_string());

    match ca_trusted(&ca) {
        Some(true) => c.add("tls.trust", cat, "CA trusted", Status::Pass, "trusted in the system keychain"),
        Some(false) => {
            c.add("tls.trust", cat, "CA trusted", Status::Warn, "not in the system keychain");
            c.hint("HTTPS sites will show a certificate warning until the CA is trusted.");
            c.fix("Trust the CA", "sudo dpl trust");
        }
        None => c.add("tls.trust", cat, "CA trusted", Status::Info, "trust state not checked on this platform"),
    }

    // Issued site certificates, and the soonest one to expire.
    let issued: Vec<_> = std::fs::read_dir(&certs)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "crt" || e == "pem"))
        .filter(|p| p.file_name().is_some_and(|n| n != "ca.pem"))
        .collect();
    c.add("tls.certs", cat, "Site certificates", Status::Info, format!("{} issued", issued.len()));

    if let Some((name, days)) = soonest_expiry(&issued) {
        let status = if days < 0 {
            Status::Fail
        } else if days < 14 {
            Status::Warn
        } else {
            Status::Pass
        };
        let detail = if days < 0 {
            format!("{name} expired {} day(s) ago", -days)
        } else {
            format!("{name} expires in {days} day(s)")
        };
        c.add("tls.expiry", cat, "Certificate expiry", status, detail);
        if status != Status::Pass {
            c.hint("Re-secure the site to reissue its certificate.");
        }
    }
}

// ---- PHP --------------------------------------------------------------------

fn check_php(c: &mut Checks, home: Option<&str>) {
    let cat = "PHP";
    let versions = dpl_core::php::detect();

    if versions.is_empty() {
        c.add("php.versions", cat, "PHP", Status::Fail, "no PHP found");
        c.hint("Sites can't be served without a PHP binary.");
        c.fix("Install PHP", "brew install php");
        return;
    }
    let list: Vec<String> = versions.iter().map(|v| v.version.clone()).collect();
    c.add("php.versions", cat, "Installed versions", Status::Pass, list.join(", "));

    let cfg = dpl_core::paths::local_config(home)
        .ok()
        .and_then(|p| dpl_core::config::LocalConfig::load(&p).ok())
        .unwrap_or_default();
    match cfg.default_php.clone().or_else(dpl_core::php::default_version) {
        Some(v) => c.add("php.default", cat, "Default version", Status::Pass, v),
        None => c.add("php.default", cat, "Default version", Status::Warn, "none set — using whatever `php` is on PATH"),
    }

    // A version that can't load its own extensions starts, but fatals at runtime
    // in ways that look like application bugs.
    for v in &versions {
        let errors = dpl_core::php::startup_load_errors(&v.binary);
        if errors.is_empty() {
            continue;
        }
        let libs: Vec<String> = errors
            .iter()
            .map(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone()))
            .collect();
        c.add(
            &format!("php.load_errors.{}", v.version),
            cat,
            &format!("PHP {} extensions", v.version),
            Status::Warn,
            format!("{} extension(s) fail to load: {}", libs.len(), libs.join(", ")),
        );
        c.hint("Usually a Homebrew upgrade moved the shared libraries these extensions link against.");
        c.fix("Repair this version", &format!("dpl php repair {}", v.version));
    }

    // Old GUI wrote zz-dpl-xdebug.ini into Homebrew's system conf.d. Site
    // debugging is FPM-only now; that leftover neither loads Xdebug for CLI nor
    // reflects the menu-bar toggle.
    let legacy: Vec<String> = versions
        .iter()
        .filter(|v| dpl_core::xdebug::legacy_system_loader_path(&v.binary).is_some())
        .map(|v| v.version.clone())
        .collect();
    if legacy.is_empty() {
        c.add(
            "php.xdebug_system_leftover",
            cat,
            "System Xdebug leftover",
            Status::Pass,
            "no pre-per-site zz-dpl-xdebug.ini in Homebrew conf.d",
        );
    } else {
        c.add(
            "php.xdebug_system_leftover",
            cat,
            "System Xdebug leftover",
            Status::Warn,
            format!(
                "legacy zz-dpl-xdebug.ini still in system conf.d for PHP {}",
                legacy.join(", ")
            ),
        );
        c.hint(
            "Menu-bar debugging loads Xdebug into php-fpm only. The old GUI file in \
             Homebrew's conf.d does not enable CLI Xdebug for Pest/artisan.",
        );
        c.fix("Scrub leftovers", &format!("dpl php repair {}", legacy[0]));
    }

    // Pest `--tia` / `--coverage` need a coverage driver on the *CLI* binary.
    // Per-site Xdebug (menu bar) never loads into `php` on PATH by design.
    let default_bin = cfg
        .default_php
        .as_deref()
        .and_then(dpl_core::php::resolve)
        .or_else(|| versions.first().map(|v| v.binary.clone()));
    if let Some(bin) = default_bin {
        let version = dpl_core::php::version_of(&bin).unwrap_or_else(|| "?".into());
        let coverage = cli_coverage_driver(&bin);
        match coverage.as_deref() {
            Some("pcov") => c.add(
                "php.cli_coverage",
                cat,
                "CLI coverage (Pest TIA)",
                Status::Pass,
                format!("pcov loaded on PHP {version}"),
            ),
            Some("xdebug") => {
                c.add(
                    "php.cli_coverage",
                    cat,
                    "CLI coverage (Pest TIA)",
                    Status::Pass,
                    format!("Xdebug loaded on PHP {version} CLI"),
                );
                let hint = format!(
                    "pcov is faster for Pest `--tia` / `--coverage`. Prefer \
                     `dpl php ext-install {version} pcov` and keep Xdebug FPM-only."
                );
                c.hint(&hint);
            }
            _ => {
                c.add(
                    "php.cli_coverage",
                    cat,
                    "CLI coverage (Pest TIA)",
                    Status::Warn,
                    format!("neither pcov nor Xdebug loaded on PHP {version} CLI"),
                );
                c.hint(
                    "Menu-bar Start Debugging only affects php-fpm for that site. Pest \
                     `--parallel --tia` needs pcov (preferred) or Xdebug on the CLI binary.",
                );
                c.fix("Install pcov for CLI", &format!("dpl php ext-install {version} pcov"));
            }
        }
    }
}

/// Which coverage driver the CLI `php` binary has loaded, if any.
fn cli_coverage_driver(php_bin: &Path) -> Option<&'static str> {
    let out = std::process::Command::new(php_bin)
        .args([
            "-d",
            "display_errors=stderr",
            "-r",
            "echo extension_loaded('pcov') ? 'pcov' : (extension_loaded('xdebug') ? 'xdebug' : '');",
        ])
        .env_remove("PHP_INI_SCAN_DIR")
        .output()
        .ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "pcov" => Some("pcov"),
        "xdebug" => Some("xdebug"),
        _ => None,
    }
}

// ---- Sites ------------------------------------------------------------------

fn check_sites(c: &mut Checks, home: Option<&str>, sites: Option<&[SiteInfo]>) {
    let cat = "Sites";
    let Some(sites) = sites else {
        c.add("sites.registry", cat, "Sites", Status::Fail, "unavailable (daemon is down)");
        return;
    };

    let serving = sites.iter().filter(|s| s.serving).count();
    let secure = sites.iter().filter(|s| s.secure).count();
    c.add(
        "sites.registry",
        cat,
        "Registered",
        Status::Pass,
        format!("{} site(s), {serving} serving, {secure} on HTTPS", sites.len()),
    );

    // A link whose folder was moved or deleted 404s with no explanation.
    let missing: Vec<&str> = sites
        .iter()
        .filter(|s| s.source != "proxy" && !s.path.is_empty() && !Path::new(&s.path).is_dir())
        .map(|s| s.name.as_str())
        .collect();
    if missing.is_empty() {
        c.add("sites.paths", cat, "Project folders", Status::Pass, "all present");
    } else {
        c.add("sites.paths", cat, "Project folders", Status::Fail, format!("missing: {}", missing.join(", ")));
        c.hint("These sites point at folders that no longer exist. Unlink them, or restore the folder.");
        // One command rather than an unlink per site: the fix runner executes a
        // single program with no shell, so a chain of `&&` would never run — and
        // a button that clears one of three names isn't a fix.
        c.fix("Unlink missing sites", "dpl unlink --missing");
    }

    // A missing docroot means the framework's front controller isn't where we look.
    let bad_docroot: Vec<&str> = sites
        .iter()
        .filter(|s| s.source != "proxy" && !s.docroot.is_empty() && Path::new(&s.path).is_dir() && !Path::new(&s.docroot).is_dir())
        .map(|s| s.name.as_str())
        .collect();
    if !bad_docroot.is_empty() {
        c.add("sites.docroots", cat, "Document roots", Status::Warn, format!("missing: {}", bad_docroot.join(", ")));
        c.hint("The site serves from a folder that doesn't exist yet (e.g. `public/` before install).");
    }

    // Serving a project on a PHP older than its composer.json demands produces
    // syntax errors that look like corrupt vendor code.
    let cfg = dpl_core::paths::local_config(home)
        .ok()
        .and_then(|p| dpl_core::config::LocalConfig::load(&p).ok())
        .unwrap_or_default();
    let default_php = cfg.default_php.clone().or_else(dpl_core::php::default_version);
    let mut mismatched = Vec::new();
    for s in sites {
        let (Some(req), Some(effective)) = (
            s.requires_php.as_deref().and_then(parse_version),
            s.php.clone().or_else(|| default_php.clone()).as_deref().and_then(parse_version),
        ) else {
            continue;
        };
        if effective < req {
            mismatched.push(format!("{} needs {}.{}, runs {}.{}", s.name, req.0, req.1, effective.0, effective.1));
        }
    }
    if !mismatched.is_empty() {
        c.add("sites.php_requirement", cat, "PHP requirements", Status::Warn, mismatched.join("; "));
        c.hint("Pin a newer PHP for these sites (right-click a site → Switch PHP Version).");
    }

    // Parked folders that vanished silently drop every site beneath them.
    let gone: Vec<String> = cfg
        .parked
        .iter()
        .filter(|p| !p.is_dir())
        .map(|p| p.display().to_string())
        .collect();
    if !gone.is_empty() {
        c.add("sites.parked", cat, "Parked folders", Status::Warn, format!("missing: {}", gone.join(", ")));
        c.hint("Every site under a parked folder disappears with it. Unpark it (`dpl unpark <path>`) or restore the folder.");
    }

    // A proxy whose upstream is down is the other half of "my site is blank".
    for (name, target) in &cfg.proxies {
        let Some((host, port)) = local_target(target) else { continue };
        if tcp_listening(port) {
            c.add(&format!("sites.proxy.{name}"), cat, &format!("Proxy {name}"), Status::Pass, format!("upstream {target} is up"));
        } else {
            c.add(&format!("sites.proxy.{name}"), cat, &format!("Proxy {name}"), Status::Warn, format!("nothing listening on {host}:{port}"));
            c.hint("Start the dev server this proxy forwards to (e.g. `npm run dev`).");
        }
    }
}

// ---- Services ---------------------------------------------------------------

fn check_services(c: &mut Checks, home: Option<&str>) {
    let cat = "Services";

    if Path::new("/Applications/DBngin.app").exists() {
        c.add("services.dbngin", cat, "DBngin", Status::Info, "detected — dpl uses its running engines");
    }

    let req = Request::Service { action: "list".into(), name: None, engine: None, version: None, port: None };
    let Ok(Response::ServiceList { services }) = daemon::call(req, home) else { return };

    if services.is_empty() {
        c.add("services.none", cat, "Databases & caches", Status::Info, "no services configured");
        return;
    }
    for s in &services {
        let (status, state) = if s.external {
            (Status::Pass, "running (external, e.g. DBngin/Postgres.app)".to_string())
        } else if s.running {
            (Status::Pass, "running (managed by dpl)".to_string())
        } else if s.installed {
            (Status::Warn, "installed, stopped".to_string())
        } else {
            (Status::Warn, "not installed".to_string())
        };
        c.add(
            &format!("services.{}", s.name),
            cat,
            &s.name,
            status,
            format!("{state} on port {}", s.port),
        );
        if !s.running && s.installed && !s.external {
            c.fix("Start it", &format!("dpl service start {}", s.name));
        }
    }
}

// ---- Tooling ----------------------------------------------------------------

fn check_tooling(c: &mut Checks) {
    let cat = "Tooling";
    let optional = |c: &mut Checks, id: &str, bin: &str, title: &str, why: &str, install: &str| {
        match which(bin) {
            Some(p) => c.add(id, cat, title, Status::Pass, p),
            None => {
                c.add(id, cat, title, Status::Warn, "not installed");
                c.hint(why);
                c.fix("Install it", install);
            }
        }
    };

    optional(c, "tool.brew", "brew", "Homebrew", "dpl installs and repairs PHP versions through Homebrew.", "");
    optional(c, "tool.composer", "composer", "Composer", "Needed to set up Laravel Octane for a site.", "brew install composer");
    // Two ways to expose a site, and they are alternatives rather than a stack:
    // `dpl share quick` runs a throwaway Cloudflare tunnel in the foreground,
    // `dpl share on` keeps a Jetty tunnel on a reserved name. Neither is
    // required, so a missing one is only worth mentioning against the command
    // that needs it.
    optional(
        c,
        "tool.cloudflared",
        "cloudflared",
        "cloudflared",
        "Needed by `dpl share quick` for a one-off public URL. Not needed for `dpl share`, which uses Jetty.",
        "brew install cloudflared",
    );
    check_jetty(c, cat);

    // Valet isn't a dependency — it's a conflict worth naming when present.
    if let Some(p) = which("valet") {
        c.add("tool.valet", cat, "Laravel Valet", Status::Info, format!("installed at {p}"));
        c.hint("Valet and dpl both want ports 80/443. `dpl takeover` hands them to dpl; `dpl untakeover` gives them back.");
    }
}

/// The Jetty CLI, which the daemon drives for permanent public URLs.
///
/// Reported in more detail than a plain `which`, because two things about a
/// Jetty install fail in ways that don't look like an install problem:
///
/// * **The daemon can't see a Composer-global install.** dpld runs under
///   launchd's minimal PATH, so a `jetty` that works in your shell can be
///   invisible to the process that actually needs it.
/// * **Several copies drift apart.** A stale build alongside a current one
///   presents as a tunnel that connects and then dies in a loop — here a
///   v0.1.60 crashed resuming a tunnel while a v0.1.61 sat beside it.
fn check_jetty(c: &mut Checks, cat: &str) {
    // The same places the daemon looks, in the same order.
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates: Vec<String> = [
        format!("{home}/.local/bin/jetty"),
        format!("{home}/.composer/vendor/bin/jetty"),
        format!("{home}/.config/composer/vendor/bin/jetty"),
        "/opt/homebrew/bin/jetty".to_string(),
        "/usr/local/bin/jetty".to_string(),
    ]
    .into_iter()
    .filter(|p| Path::new(p).is_file())
    .collect();

    if candidates.is_empty() && which("jetty").is_none() {
        c.add("tool.jetty", cat, "Jetty CLI", Status::Warn, "not installed");
        c.hint("Needed by `dpl share` to give a site a permanent public URL.");
        c.fix("Install it", "composer global require jetty/client");
        return;
    }

    let primary = candidates.first().cloned().or_else(|| which("jetty")).unwrap_or_default();
    if candidates.len() > 1 {
        c.add(
            "tool.jetty",
            cat,
            "Jetty CLI",
            Status::Warn,
            format!("{} installs: {}", candidates.len(), candidates.join(", ")),
        );
        c.hint(
            "Several copies can drift to different versions, and a stale one fails in a way that \
             looks like a broken tunnel rather than a broken install. dpl uses the first.",
        );
        c.fix("Clean them up", "jetty doctor");
    } else {
        c.add("tool.jetty", cat, "Jetty CLI", Status::Pass, primary);
    }
}

// ---- Probes -----------------------------------------------------------------

/// Is something accepting TCP connections on this local port?
fn tcp_listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(250)).is_ok()
}

/// Ask a DNS server on `port` to resolve `name`, returning the A record it
/// answers with. A hand-rolled query keeps `doctor` dependency-free.
fn dns_probe(port: u16, name: &str) -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind(("127.0.0.1", 0)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(500))).ok()?;

    // Header: id, RD flag, one question.
    let mut query: Vec<u8> = vec![0x7a, 0x1c, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        query.push(u8::try_from(label.len()).ok()?);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&[0, 1, 0, 1]); // QTYPE=A, QCLASS=IN

    sock.send_to(&query, ("127.0.0.1", port)).ok()?;
    let mut buf = [0u8; 512];
    let (n, _) = sock.recv_from(&mut buf).ok()?;
    let resp = &buf[..n];
    if n < 12 || u16::from_be_bytes([resp[6], resp[7]]) == 0 {
        return None; // no answers
    }

    // Skip the echoed question, then read the first answer's rdata.
    let mut i = 12;
    while i < n && resp[i] != 0 {
        i += 1 + resp[i] as usize;
    }
    i += 1 + 4;
    if i >= n {
        return None;
    }
    if resp[i] & 0xc0 == 0xc0 {
        i += 2; // compressed name pointer
    } else {
        while i < n && resp[i] != 0 {
            i += 1 + resp[i] as usize;
        }
        i += 1;
    }
    if i + 10 > n {
        return None;
    }
    let rtype = u16::from_be_bytes([resp[i], resp[i + 1]]);
    let rdlen = u16::from_be_bytes([resp[i + 8], resp[i + 9]]) as usize;
    i += 10;
    if rtype != 1 || rdlen != 4 || i + 4 > n {
        return None;
    }
    Some(Ipv4Addr::new(resp[i], resp[i + 1], resp[i + 2], resp[i + 3]))
}

/// The one-minute load average.
#[cfg(target_os = "linux")]
fn load_average() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next()?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn load_average() -> Option<f64> {
    // `vm.loadavg` prints `{ 1.79 2.02 2.11 }`.
    let out = cmd("sysctl", &["-n", "vm.loadavg"])?;
    out.split_whitespace().nth(1)?.parse().ok()
}

/// Free space (GB) on the volume holding `path`.
fn disk_free_gb(path: &Path) -> Option<f64> {
    let out = cmd("df", &["-k", &path.to_string_lossy()])?;
    let avail_kb: f64 = out.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb / 1_048_576.0)
}

/// Total bytes under `dir` (one level of recursion is enough for our layouts).
fn dir_size_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Is our CA in the system keychain? `None` where we can't tell.
#[cfg(target_os = "macos")]
fn ca_trusted(ca: &Path) -> Option<bool> {
    let printed = cmd("openssl", &["x509", "-in", &ca.to_string_lossy(), "-noout", "-fingerprint", "-sha1"])?;
    let fingerprint = printed.split('=').nth(1)?.trim().replace(':', "").to_uppercase();
    let keychain = cmd("security", &["find-certificate", "-a", "-Z", "/Library/Keychains/System.keychain"])?;
    Some(keychain.to_uppercase().contains(&fingerprint))
}

#[cfg(not(target_os = "macos"))]
fn ca_trusted(_ca: &Path) -> Option<bool> {
    None
}

/// The certificate expiring soonest: its file name and days remaining.
fn soonest_expiry(certs: &[std::path::PathBuf]) -> Option<(String, i64)> {
    certs
        .iter()
        .take(50)
        .filter_map(|p| {
            let out = cmd("openssl", &["x509", "-in", &p.to_string_lossy(), "-noout", "-enddate"])?;
            let end = out.split('=').nth(1)?.trim().to_string();
            // `date -j -f` (macOS) / `date -d` (GNU) → epoch seconds.
            let epoch = cmd("date", &["-j", "-f", "%b %e %H:%M:%S %Y %Z", &end, "+%s"])
                .or_else(|| cmd("date", &["-d", &end, "+%s"]))?;
            let epoch: i64 = epoch.trim().parse().ok()?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some((name, (epoch - now) / 86_400))
        })
        .min_by_key(|(_, days)| *days)
}

/// `host:port` when a proxy target points at this machine (the only case we can
/// probe), else `None`.
fn local_target(target: &str) -> Option<(String, u16)> {
    let rest = target.split("://").nth(1).unwrap_or(target);
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (authority.to_string(), if target.starts_with("https") { 443 } else { 80 }),
    };
    (host == "localhost" || host == "127.0.0.1").then_some((host, port))
}

/// Is the login autostart service installed?
#[cfg(target_os = "macos")]
fn autostart_installed() -> bool {
    dirs_home()
        .map(|h| h.join("Library/LaunchAgents/com.tomshafer.dpld.plist").exists())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn autostart_installed() -> bool {
    dirs_home()
        .map(|h| h.join(".config/systemd/user/dpld.service").exists())
        .unwrap_or(false)
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// `dpl-helper` sitting next to the running `dpl`.
fn helper_next_to_exe() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let helper = exe.parent()?.join("dpl-helper");
    helper.is_file().then(|| helper.display().to_string())
}

/// Run a command and return its trimmed stdout, or `None` if it failed.
fn cmd(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// First `major.minor` in a version or constraint string (`"^8.3"` → `(8, 3)`).
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let bytes: Vec<char> = text.chars().collect();
    for (i, ch) in bytes.iter().enumerate() {
        if !ch.is_ascii_digit() {
            continue;
        }
        let rest: String = bytes[i..].iter().take_while(|c| c.is_ascii_digit() || **c == '.').collect();
        let mut parts = rest.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        return Some((major, minor));
    }
    None
}

fn humanize_secs(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_and_constraints() {
        assert_eq!(parse_version("8.3"), Some((8, 3)));
        assert_eq!(parse_version("^8.2"), Some((8, 2)));
        assert_eq!(parse_version(">=7.4.0"), Some((7, 4)));
        assert_eq!(parse_version("8"), Some((8, 0)));
        assert_eq!(parse_version("none"), None);
    }

    #[test]
    fn only_local_proxy_targets_are_probeable() {
        assert_eq!(local_target("http://localhost:3000"), Some(("localhost".into(), 3000)));
        assert_eq!(local_target("http://127.0.0.1:8000/app"), Some(("127.0.0.1".into(), 8000)));
        assert_eq!(local_target("https://example.com"), None);
    }

    #[test]
    fn humanizes_uptime() {
        assert_eq!(humanize_secs(42), "42s");
        assert_eq!(humanize_secs(3 * 3600 + 120), "3h 2m");
        assert_eq!(humanize_secs(50 * 3600), "2d 2h");
    }
}
