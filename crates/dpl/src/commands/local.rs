//! Local `.test` site management: `sites`, `park`, `link`, `secure`, `open`,
//! and friends. These talk to the `dpld` daemon over the control socket; the
//! daemon owns the registry and the running PHP backends.

use anyhow::{Context, Result};
use dpl_core::ipc::{Request, Response, SiteInfo};

use crate::daemon;

/// Directory argument default: the given path, or the current directory.
fn resolve_path(arg: Option<String>) -> Result<String> {
    match arg {
        Some(p) => Ok(p),
        None => Ok(std::env::current_dir()
            .context("resolving current directory")?
            .to_string_lossy()
            .into_owned()),
    }
}

/// Site-name default: the given name, or the current directory's base name.
fn resolve_name(arg: Option<String>) -> Result<String> {
    match arg {
        Some(n) => Ok(n.to_lowercase()),
        None => std::env::current_dir()
            .context("resolving current directory")?
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .context("could not derive a site name from the current directory"),
    }
}

pub fn sites(home: Option<&str>, json: bool) -> Result<()> {
    match daemon::call(Request::ListSites, home)? {
        Response::Sites { sites, http_port, tld } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&sites)?);
                return Ok(());
            }
            render_sites(&sites, http_port, &tld);
            Ok(())
        }
        other => crate::commands::unexpected(other),
    }
}

pub fn proxy_set(home: Option<&str>, name: String, target: String) -> Result<()> {
    send_message(home, Request::Proxy { action: "set".into(), name, target: Some(target) })
}

pub fn proxy_remove(home: Option<&str>, name: String) -> Result<()> {
    send_message(home, Request::Proxy { action: "remove".into(), name, target: None })
}

/// List reverse proxies (the `proxy`-source entries from the site list).
pub fn proxies(home: Option<&str>, json: bool) -> Result<()> {
    match daemon::call(Request::ListSites, home)? {
        Response::Sites { sites, .. } => {
            let proxies: Vec<&SiteInfo> = sites.iter().filter(|s| s.source == "proxy").collect();
            if json {
                let arr: Vec<_> = proxies.iter().map(|s| serde_json::json!({ "host": s.host, "target": s.path })).collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
                return Ok(());
            }
            if proxies.is_empty() {
                println!("No proxies. Add one: `dpl proxy blog http://localhost:3000`.");
                return Ok(());
            }
            let w = proxies.iter().map(|s| s.host.len()).max().unwrap_or(4);
            for s in proxies {
                println!("{:<w$}  →  {}", s.host, s.path, w = w);
            }
            Ok(())
        }
        other => crate::commands::unexpected(other),
    }
}

pub fn park(home: Option<&str>, path: Option<String>) -> Result<()> {
    send_message(home, Request::Park { path: resolve_path(path)? })
}

pub fn unpark(home: Option<&str>, path: Option<String>) -> Result<()> {
    send_message(home, Request::Unpark { path: resolve_path(path)? })
}

pub fn link(home: Option<&str>, path: Option<String>, name: Option<String>) -> Result<()> {
    send_message(home, Request::Link { name, path: resolve_path(path)? })
}

pub fn unlink(home: Option<&str>, name: Option<String>) -> Result<()> {
    send_message(home, Request::Unlink { name: resolve_name(name)? })
}

pub fn secure(home: Option<&str>, name: Option<String>, secure: bool) -> Result<()> {
    send_message(home, Request::Secure { name: resolve_name(name)?, secure })
}

/// Open a site in the default browser, resolving its URL from the daemon so the
/// port is correct even when the proxy fell back to :8080.
pub fn open(home: Option<&str>, name: Option<String>) -> Result<()> {
    let name = resolve_name(name)?;
    let Response::Sites { sites, http_port, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    let site = sites
        .iter()
        .find(|s| s.name == name)
        .with_context(|| format!("no local site named `{name}` (try `dpl link .`)"))?;
    let url = browser_url(site, http_port);
    println!("Opening {url}");
    open_in_browser(&url)
}

/// List installed PHP versions (no daemon needed). Marks the current default
/// (the config's `default_php`, else the `php` on PATH) — used by the menu bar.
pub fn php_list(json: bool) -> Result<()> {
    php_list_home(None, json)
}

pub fn php_list_home(home: Option<&str>, json: bool) -> Result<()> {
    let versions = dpl_core::php::detect();
    let default_version = current_default_php(home, &versions);
    let is_default = |v: &str| Some(v) == default_version.as_deref();

    if json {
        let arr: Vec<_> = versions
            .iter()
            .map(|v| serde_json::json!({
                "version": v.version, "binary": v.binary, "source": v.source,
                "default": is_default(&v.version),
            }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    if versions.is_empty() {
        println!("No PHP found. Install one (e.g. `brew install php`).");
        return Ok(());
    }
    println!("{:<8}  {:<10}  {:<8}  BINARY", "VERSION", "SOURCE", "DEFAULT");
    for v in &versions {
        println!("{:<8}  {:<10}  {:<8}  {}", v.version, v.source,
            if is_default(&v.version) { "✓" } else { "" }, v.binary.display());
    }
    Ok(())
}

/// Identify a foreign web server holding a local port, or `None` if the port is
/// free. For :80 (plaintext) we read the HTTP `Server:` header (e.g. `Apache`,
/// `nginx`); for others we just report that something is listening.
pub(crate) fn port_server(port: u16) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(600)));

    if port == 80 {
        let _ = stream.write_all(b"HEAD / HTTP/1.0\r\nHost: dpl.probe\r\nConnection: close\r\n\r\n");
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).unwrap_or(0);
        let text = String::from_utf8_lossy(&buf[..n]);
        for line in text.lines() {
            if let Some(rest) = line.to_ascii_lowercase().strip_prefix("server:") {
                let name = rest.trim();
                let short = name.split(['/', ' ']).next().unwrap_or(name);
                return Some(if short.is_empty() { "another server".into() } else { short.to_string() });
            }
        }
        return Some("another web server".into());
    }
    // Something accepted the connection (e.g. nginx/Valet on TLS :443).
    Some("another server".into())
}

/// The default PHP version: config's `default_php`, else the version of the
/// `php` binary on PATH.
fn current_default_php(home: Option<&str>, versions: &[dpl_core::php::PhpVersion]) -> Option<String> {
    let path = dpl_core::paths::local_config(home).ok()?;
    if let Ok(cfg) = dpl_core::config::LocalConfig::load(&path) {
        if let Some(v) = cfg.default_php {
            return Some(v);
        }
    }
    let _ = versions;
    dpl_core::php::default_version()
}

/// Pin a PHP version for a site, or the global default with `--default`.
pub fn use_php(home: Option<&str>, version: String, name: Option<String>, default: bool) -> Result<()> {
    let site = if default { None } else { Some(resolve_name(name)?) };
    send_message(home, Request::UsePhp { version, site })
}

/// List every installable PHP version and its status — backs the GUI version
/// manager. No daemon or network needed; reads Homebrew's on-disk layout.
pub fn php_available(json: bool) -> Result<()> {
    let catalog = dpl_core::php::catalog();
    if json {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
        return Ok(());
    }
    println!("{:<8}  {:<10}  STATUS", "VERSION", "FORMULA");
    for e in &catalog {
        let status = if e.active {
            "active"
        } else if e.broken {
            "broken"
        } else if e.installed {
            "installed"
        } else {
            "not installed"
        };
        println!("{:<8}  {:<10}  {}", e.version, e.formula, status);
    }
    Ok(())
}

/// Normalize `8.3`, `php@8.3`, or `8.3.2` down to a `major.minor` line.
fn normalize_php_version(input: &str) -> Result<String> {
    let s = input.trim().trim_start_matches("php@").trim_start_matches("php");
    let parts: Vec<&str> = s.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        anyhow::bail!("expected a PHP version like 8.3 (got `{input}`)");
    }
    Ok(format!("{}.{}", parts[0], parts[1]))
}

/// Run `brew <args>` inheriting stdio so output streams live to the GUI pipe.
fn brew(args: &[&str]) -> Result<bool> {
    let status = std::process::Command::new("brew")
        .args(args)
        .status()
        .context("running brew")?;
    Ok(status.success())
}

fn ensure_brew() -> Result<()> {
    if which("brew").is_none() {
        anyhow::bail!("Homebrew not found. Install it from https://brew.sh, then re-run.");
    }
    Ok(())
}

/// Install a PHP line via Homebrew. Older lines that aren't in homebrew-core
/// live in the `shivammathur/php` tap, which we tap and retry against on demand.
pub fn php_install(version: &str) -> Result<()> {
    ensure_brew()?;
    let v = normalize_php_version(version)?;
    let formula = format!("php@{v}");
    println!("Installing {formula} via Homebrew — this may take several minutes…\n");
    if !brew(&["install", &formula])? {
        println!("\n{formula} isn't in homebrew-core; tapping shivammathur/php and retrying…\n");
        let _ = brew(&["tap", "shivammathur/php"])?;
        let tapped = format!("shivammathur/php/php@{v}");
        if !brew(&["install", &tapped])? {
            anyhow::bail!("`brew install {formula}` failed.");
        }
    }
    println!("\n✓ PHP {v} installed. Activate it with `dpl use {v} --default`.");
    Ok(())
}

/// Upgrade a PHP line to its newest patch release via Homebrew.
pub fn php_upgrade(version: &str) -> Result<()> {
    ensure_brew()?;
    let v = normalize_php_version(version)?;
    let formula = format!("php@{v}");
    println!("Upgrading {formula} via Homebrew…\n");
    if !brew(&["upgrade", &formula])? {
        anyhow::bail!("`brew upgrade {formula}` failed (it may already be up to date).");
    }
    println!("\n✓ PHP {v} upgraded.");
    Ok(())
}

/// Uninstall a PHP line via Homebrew.
pub fn php_uninstall(version: &str) -> Result<()> {
    ensure_brew()?;
    let v = normalize_php_version(version)?;
    let formula = format!("php@{v}");
    println!("Uninstalling {formula} via Homebrew…\n");
    if !brew(&["uninstall", "--ignore-dependencies", &formula])? {
        anyhow::bail!("`brew uninstall {formula}` failed.");
    }
    println!("\n✓ PHP {v} uninstalled.");
    Ok(())
}

/// Repair a broken PHP install. A structurally-missing keg (no binary) is
/// reinstalled; then any conf.d extension that points at a missing `.so` is
/// commented out so the version starts cleanly.
pub fn php_repair(version: &str) -> Result<()> {
    let v = normalize_php_version(version)?;
    let formula = format!("php@{v}");

    // Structural: no runnable binary at all → reinstall the keg.
    if dpl_core::php::resolve(&v).is_none() {
        ensure_brew()?;
        println!("Reinstalling {formula} — this may take several minutes…\n");
        if !brew(&["reinstall", &formula])? {
            anyhow::bail!("`brew reinstall {formula}` failed.");
        }
    }

    // Extension-level: disable conf.d entries that load a missing library.
    println!("Checking extensions for PHP {v}…");
    let disabled = php_fix_extensions(&v)?;
    if disabled.is_empty() {
        println!("  No broken extensions found.");
    } else {
        println!("  Disabled {} broken extension(s):", disabled.len());
        for d in &disabled {
            println!("    • {d}");
        }
        println!("  Reinstall an extension formula to restore it, e.g. `brew reinstall php@{v}-imap`.");
    }
    println!("\n✓ PHP {v} repaired.");
    Ok(())
}

/// Comment out any `extension=`/`zend_extension=` directive in a PHP line's
/// conf.d whose target `.so` is missing — the cause of "Unable to load dynamic
/// library" startup warnings. Returns `file → directive` for each one disabled.
fn php_fix_extensions(version: &str) -> Result<Vec<String>> {
    let bin = dpl_core::php::resolve(version)
        .with_context(|| format!("PHP {version} isn't installed"))?;

    // The additional-.ini scan directory. Keep the ini loaded (so the scan path
    // is reported) but route any broken-extension warning to stderr, leaving
    // stdout clean to parse.
    let ini_out = std::process::Command::new(&bin)
        .arg("-d").arg("display_errors=stderr")
        .arg("--ini")
        .output()
        .context("running php --ini")?;
    let ini_text = String::from_utf8_lossy(&ini_out.stdout);
    let Some(scan_dir) = ini_text
        .lines()
        .find(|l| l.contains("Scan for additional"))
        .and_then(|l| l.split_once(": "))
        .map(|(_, p)| p.trim().to_string())
        .filter(|p| !p.is_empty())
    else {
        return Ok(Vec::new());
    };

    // extension_dir (configured), for resolving bare extension names to a `.so`.
    let ext_dir = std::process::Command::new(&bin)
        .arg("-d").arg("display_errors=stderr")
        .arg("-r").arg("echo ini_get('extension_dir');")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    let mut disabled = Vec::new();
    let Ok(entries) = std::fs::read_dir(&scan_dir) else { return Ok(disabled) };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ini") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let mut changed = false;
        let rebuilt: Vec<String> = content
            .lines()
            .map(|line| {
                let t = line.trim_start();
                if t.starts_with(';') || t.starts_with('#') {
                    return line.to_string();
                }
                let directive = t
                    .strip_prefix("zend_extension=")
                    .or_else(|| t.strip_prefix("extension="));
                let Some(rest) = directive else { return line.to_string() };
                let val = rest.split(';').next().unwrap_or("").trim().trim_matches('"');
                if val.is_empty() {
                    return line.to_string();
                }
                let so = if val.contains('/') {
                    std::path::PathBuf::from(val)
                } else {
                    let name = if val.ends_with(".so") { val.to_string() } else { format!("{val}.so") };
                    match &ext_dir {
                        Some(d) => std::path::PathBuf::from(d).join(name),
                        None => std::path::PathBuf::from(name),
                    }
                };
                if so.exists() {
                    return line.to_string();
                }
                changed = true;
                let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                disabled.push(format!("{file} → {val}"));
                format!("; disabled by dpl (missing {}): {line}", so.display())
            })
            .collect();
        if changed {
            let mut out = rebuilt.join("\n");
            if content.ends_with('\n') {
                out.push('\n');
            }
            std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(disabled)
}

/// The conf.d scan directory for a PHP version.
fn php_conf_dir(version: &str) -> Option<std::path::PathBuf> {
    let bin = dpl_core::php::resolve(version)?;
    let out = std::process::Command::new(&bin)
        .arg("-d").arg("display_errors=stderr").arg("--ini").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|l| l.contains("Scan for additional"))
        .and_then(|l| l.split_once(": "))
        .map(|(_, p)| std::path::PathBuf::from(p.trim().trim_matches('"')))
}

/// Extension names already present in a version's conf.d (enabled or disabled).
fn installed_ext_names(version: &str) -> std::collections::BTreeSet<String> {
    let mut set = std::collections::BTreeSet::new();
    let Some(dir) = php_conf_dir(version) else { return set };
    let Ok(entries) = std::fs::read_dir(&dir) else { return set };
    for e in entries.flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if !f.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let base = f.trim_end_matches(".disabled").trim_end_matches(".ini");
        let name = match base.split_once('-') {
            Some((num, rest)) if num.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => base,
        };
        set.insert(name.to_lowercase());
    }
    set
}

/// Every extension the `shivammathur/extensions` tap offers for a PHP version.
fn brew_ext_catalog(version: &str) -> Vec<String> {
    let Ok(out) = std::process::Command::new("brew")
        .arg("search").arg("shivammathur/extensions/").output()
    else {
        return vec![];
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let suffix = format!("@{version}");
    let mut names: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("shivammathur/extensions/").map(str::to_string))
        .filter_map(|s| s.strip_suffix(&suffix).map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// List extensions installable for a version that aren't already present.
pub fn php_ext_available(version: &str, json: bool) -> Result<()> {
    let v = normalize_php_version(version)?;
    let installed = installed_ext_names(&v);
    let catalog = brew_ext_catalog(&v);
    let available: Vec<&String> = catalog.iter().filter(|n| !installed.contains(&n.to_lowercase())).collect();

    if json {
        let arr: Vec<_> = available.iter().map(|n| serde_json::json!({ "name": n })).collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    if catalog.is_empty() {
        println!("No extension catalog found. Tap it with: brew tap shivammathur/extensions");
        return Ok(());
    }
    println!("{} extension(s) installable for PHP {v}:", available.len());
    for n in &available {
        println!("  {n}");
    }
    Ok(())
}

/// Install a PECL/Homebrew extension for a version via the shivammathur tap.
pub fn php_ext_install(version: &str, name: &str) -> Result<()> {
    ensure_brew()?;
    let v = normalize_php_version(version)?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        anyhow::bail!("invalid extension name: {name}");
    }
    let formula = format!("shivammathur/extensions/{name}@{v}");
    println!("Installing {formula} via Homebrew — this may take a minute…\n");
    let _ = brew(&["tap", "shivammathur/extensions"])?;
    if !brew(&["install", &formula])? {
        anyhow::bail!("`brew install {formula}` failed.");
    }
    println!("\n✓ {name} installed for PHP {v}. Restart php-fpm (dpl restart) to load it.");
    Ok(())
}

/// Uninstall an extension for a version (brew uninstall the tap formula).
pub fn php_ext_uninstall(version: &str, name: &str) -> Result<()> {
    ensure_brew()?;
    let v = normalize_php_version(version)?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        anyhow::bail!("invalid extension name: {name}");
    }
    let formula = format!("shivammathur/extensions/{name}@{v}");
    println!("Uninstalling {formula} via Homebrew…\n");
    if !brew(&["uninstall", "--ignore-dependencies", &formula])? {
        anyhow::bail!("`brew uninstall {formula}` failed.");
    }
    println!("\n✓ {name} uninstalled for PHP {v}.");
    Ok(())
}

/// Fix broken extensions for a PHP line without a full reinstall.
pub fn php_fix(version: &str) -> Result<()> {
    let v = normalize_php_version(version)?;
    let disabled = php_fix_extensions(&v)?;
    if disabled.is_empty() {
        println!("No broken extensions found for PHP {v}.");
    } else {
        println!("Disabled {} broken extension(s) for PHP {v}:", disabled.len());
        for d in &disabled {
            println!("  • {d}");
        }
        println!("\nReinstall an extension formula to restore it, e.g. `brew reinstall php@{v}-imap`.");
    }
    Ok(())
}

/// Show (and optionally follow) a local site's request log.
pub fn logs(home: Option<&str>, name: Option<String>, lines: usize, follow: bool) -> Result<()> {
    let name = resolve_name(name)?;
    let path = dpl_core::paths::logs_dir(home)?.join(format!("{name}.log"));
    if !path.exists() {
        anyhow::bail!("no log for `{name}` yet (is the site being served? run `dpl sites`).");
    }
    // Print the last `lines` lines.
    let content = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(lines);
    for line in &all[start..] {
        println!("{line}");
    }
    if !follow {
        return Ok(());
    }

    // Follow: poll for appended bytes.
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(&path)?;
    let mut pos = file.seek(SeekFrom::End(0))?;
    println!("— following {name}.log (Ctrl-C to stop) —");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(pos);
        if len < pos {
            // File was truncated (backend restarted) — re-read from the top.
            pos = 0;
        }
        if len > pos {
            file.seek(SeekFrom::Start(pos))?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            print!("{buf}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            pos = len;
        }
    }
}

/// Publicly share a local site via a Cloudflare quick tunnel (`cloudflared`).
pub fn share(home: Option<&str>, name: Option<String>) -> Result<()> {
    let name = resolve_name(name)?;
    let Response::Sites { sites, http_port, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    let site = sites
        .iter()
        .find(|s| s.name == name)
        .with_context(|| format!("no local site named `{name}`"))?;

    if which("cloudflared").is_none() {
        anyhow::bail!(
            "cloudflared is not installed. Install it with `brew install cloudflared`, then retry."
        );
    }
    if !site.serving {
        anyhow::bail!("`{name}` is not being served, so there's nothing to share.");
    }

    println!("Starting a public Cloudflare tunnel for {}.test …", name);
    println!("(Ctrl-C to stop sharing.)\n");
    // cloudflared forwards to our proxy; --http-host-header makes the proxy
    // route the request to the right site.
    let status = std::process::Command::new("cloudflared")
        .arg("tunnel")
        .arg("--url")
        .arg(format!("http://127.0.0.1:{http_port}"))
        .arg("--http-host-header")
        .arg(format!("{name}.{}", "test"))
        .status()
        .context("running cloudflared")?;
    if !status.success() {
        anyhow::bail!("cloudflared exited with an error.");
    }
    Ok(())
}

/// Reload the daemon's config from disk and restart backends.
pub fn restart(home: Option<&str>) -> Result<()> {
    send_message(home, Request::Reload)
}

/// Hard-reset all backends (stop + reap php-fpm/Octane, then rebuild).
pub fn repair_backends(home: Option<&str>) -> Result<()> {
    send_message(home, Request::RepairBackends)
}

/// Show where the tool keeps its files.
pub fn paths(home: Option<&str>) -> Result<()> {
    let p = |label: &str, path: std::path::PathBuf| println!("{label:<14}{}", path.display());
    p("config", dpl_core::paths::local_config(home)?);
    p("dply auth", dpl_core::paths::dply_config(home)?);
    p("socket", dpl_core::paths::daemon_socket(home)?);
    p("certs", dpl_core::paths::certs_dir(home)?);
    p("logs", dpl_core::paths::logs_dir(home)?);
    Ok(())
}

/// List/add/remove the TLDs sites answer on.
pub fn tld(home: Option<&str>, action: String, name: Option<String>, json: bool) -> Result<()> {
    match daemon::call(Request::Tld { action: action.clone(), name }, home)? {
        Response::Lines { lines } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&lines)?);
            } else {
                for (i, t) in lines.iter().enumerate() {
                    println!(".{t}{}", if i == 0 { "  (primary)" } else { "" });
                }
            }
            Ok(())
        }
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        other => crate::commands::unexpected(other),
    }
}

/// List database/cache services and instances.
pub fn services(home: Option<&str>, json: bool) -> Result<()> {
    let req = Request::Service { action: "list".into(), name: None, engine: None, version: None, port: None };
    match daemon::call(req, home)? {
        Response::ServiceList { services } => {
            if json {
                let arr: Vec<_> = services.iter().map(|s| serde_json::json!({
                    "name": s.name, "engine": s.engine, "port": s.port, "version": s.version,
                    "installed": s.installed, "running": s.running, "external": s.external,
                })).collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
                return Ok(());
            }
            if services.is_empty() {
                println!("No services. Create one: `dpl service create mypg --engine postgres`.");
                return Ok(());
            }
            println!("{:<14}  {:<9}  {:<8}  {:<6}  {}", "NAME", "ENGINE", "VERSION", "PORT", "STATE");
            for s in &services {
                let state = if s.external { "running (external)" }
                    else if s.running { "running" } else { "stopped" };
                println!("{:<14}  {:<9}  {:<8}  {:<6}  {}", s.name, s.engine,
                    if s.version.is_empty() { "-" } else { &s.version }, s.port, state);
            }
            Ok(())
        }
        other => crate::commands::unexpected(other),
    }
}

/// Manage services + multi-version instances.
pub fn service(
    home: Option<&str>, action: String, name: Option<String>,
    engine: Option<String>, version: Option<String>, port: Option<u16>, json: bool,
) -> Result<()> {
    if action == "list" {
        return services(home, json);
    }
    if action == "info" {
        let target = name.clone().context("usage: dpl service info <name>")?;
        return service_info(home, &target);
    }
    if action == "install" {
        let spec = name.clone().or(engine.clone())
            .context("usage: dpl service install <engine>[@version], e.g. postgresql@17")?;
        return service_install(&spec);
    }
    let req = Request::Service {
        action: action.clone(),
        name: name.or_else(|| if action == "versions" { engine.clone() } else { None }),
        engine, version, port,
    };
    match daemon::call(req, home)? {
        Response::Versions { versions } => {
            if json {
                let arr: Vec<_> = versions.iter().map(|v| serde_json::json!({
                    "engine": v.engine, "version": v.version, "source": v.source })).collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
                return Ok(());
            }
            if versions.is_empty() {
                println!("No engine versions found (install via DBngin or Homebrew).");
                return Ok(());
            }
            println!("{:<10}  {:<8}  SOURCE", "ENGINE", "VERSION");
            for v in &versions {
                println!("{:<10}  {:<8}  {}", v.engine, v.version, v.source);
            }
            Ok(())
        }
        Response::Message { text } => { println!("{text}"); Ok(()) }
        Response::Ok => Ok(()),
        other => crate::commands::unexpected(other),
    }
}

/// Install a database engine via Homebrew so a fresh machine needs neither
/// DBngin nor a manual `brew install`. `spec` is `engine[@version]`.
fn service_install(spec: &str) -> Result<()> {
    if which("brew").is_none() {
        anyhow::bail!("Homebrew not found. Install it from https://brew.sh, then re-run.");
    }
    let (engine, version) = match spec.split_once('@') {
        Some((e, v)) => (e.to_lowercase(), Some(v.to_string())),
        None => (spec.to_lowercase(), None),
    };
    // (formula, optional tap that provides it).
    let (formula, tap): (String, Option<&str>) = match engine.as_str() {
        "postgres" | "postgresql" | "pg" => (format!("postgresql@{}", version.as_deref().unwrap_or("17")), None),
        "mysql" => (version.map(|v| format!("mysql@{v}")).unwrap_or_else(|| "mysql".into()), None),
        "mariadb" => (version.map(|v| format!("mariadb@{v}")).unwrap_or_else(|| "mariadb".into()), None),
        "redis" => (version.map(|v| format!("redis@{v}")).unwrap_or_else(|| "redis".into()), None),
        "meilisearch" | "meili" => ("meilisearch".into(), None),
        "mongodb" | "mongo" => ("mongodb-community".into(), Some("mongodb/brew")),
        "minio" | "s3" | "rustfs" => ("minio".into(), None),
        "stripe-mock" | "stripemock" | "stripe" => ("stripe-mock".into(), Some("stripe/stripe-mock")),
        other => anyhow::bail!("unknown service `{other}` (postgres|mysql|mariadb|redis|meilisearch|mongodb|minio|stripe-mock)"),
    };

    if let Some(t) = tap {
        println!("Tapping {t}…");
        let _ = std::process::Command::new("brew").args(["tap", t]).status();
    }
    println!("Installing {formula} via Homebrew — this may take a minute…\n");
    let status = std::process::Command::new("brew")
        .arg("install")
        .arg(&formula)
        .status()
        .context("running brew")?;
    if !status.success() {
        anyhow::bail!("`brew install {formula}` failed.");
    }
    // Normalize the engine name back for the create hint.
    let create_engine = match engine.as_str() {
        "postgresql" | "pg" => "postgres",
        e => e,
    };
    println!("\n✓ Installed {formula}.");
    println!("  Create an instance: dpl service create my{create_engine} --engine {create_engine}");
    Ok(())
}

/// Print connection details for a service/instance (host/port/user + a URL and
/// a client command to copy).
fn service_info(home: Option<&str>, target: &str) -> Result<()> {
    let req = Request::Service { action: "list".into(), name: None, engine: None, version: None, port: None };
    let Response::ServiceList { services } = daemon::call(req, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    let s = services
        .iter()
        .find(|s| s.name == target)
        .with_context(|| format!("no service named `{target}`"))?;
    let (url, cmd) = connection_strings(&s.engine, s.port);
    println!("{}  ({}{})", s.name, s.engine, if s.version.is_empty() { String::new() } else { format!(" {}", s.version) });
    println!("  host      127.0.0.1");
    println!("  port      {}", s.port);
    println!("  user      {}", default_user(&s.engine));
    println!("  password  (none)");
    println!("  url       {url}");
    println!("  connect   {cmd}");
    Ok(())
}

fn default_user(engine: &str) -> &'static str {
    match engine {
        "postgres" => "postgres",
        "mysql" | "mariadb" => "root",
        _ => "-",
    }
}

/// (connection URL, client command) for an engine on a port.
fn connection_strings(engine: &str, port: u16) -> (String, String) {
    match engine {
        "postgres" => (
            format!("postgresql://postgres@127.0.0.1:{port}/postgres"),
            format!("psql -h 127.0.0.1 -p {port} -U postgres"),
        ),
        "mysql" | "mariadb" => (
            format!("mysql://root@127.0.0.1:{port}"),
            format!("mysql -h 127.0.0.1 -P {port} -u root"),
        ),
        "redis" => (
            format!("redis://127.0.0.1:{port}"),
            format!("redis-cli -p {port}"),
        ),
        _ => (format!("127.0.0.1:{port}"), String::new()),
    }
}

/// Run a `dpl db` operation (optionally against a specific instance port).
pub fn db(home: Option<&str>, action: String, engine: String, name: Option<String>, port: Option<u16>, file: Option<String>) -> Result<()> {
    match daemon::call(Request::Db { action: action.clone(), engine, name, port, file }, home)? {
        Response::Lines { lines } => {
            if lines.is_empty() {
                println!("(no databases)");
            } else {
                for l in lines {
                    println!("{l}");
                }
            }
            Ok(())
        }
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        other => crate::commands::unexpected(other),
    }
}

/// Inspect captured mail (filesystem-based; no daemon call).
pub fn mail(home: Option<&str>, action: String, id: Option<String>, json: bool) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    match action.as_str() {
        "list" => {
            let mut files = eml_files(&dir)?;
            files.sort();
            files.reverse(); // newest first
            if json {
                let arr: Vec<_> = files.iter().map(|f| {
                    let id = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    let (from, to, subject) = mail_headers(f);
                    serde_json::json!({ "id": id, "from": from, "to": to, "subject": subject })
                }).collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
                return Ok(());
            }
            if files.is_empty() {
                println!("No captured mail. Point your app's SMTP at 127.0.0.1:1025.");
                return Ok(());
            }
            println!("{:<24}  {:<26}  SUBJECT", "ID", "TO");
            for f in files {
                let id = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let (_from, to, subject) = mail_headers(&f);
                println!("{id:<24}  {:<26}  {subject}", truncate(&to, 26));
            }
            Ok(())
        }
        "show" => {
            let id = id.context("usage: dpl mail show <id>")?;
            let path = dir.join(format!("{id}.eml"));
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("no message {id}"))?;
            println!("{body}");
            Ok(())
        }
        "clear" => {
            let files = eml_files(&dir)?;
            let n = files.len();
            for f in files {
                let _ = std::fs::remove_file(f);
            }
            println!("Cleared {n} message(s).");
            Ok(())
        }
        other => anyhow::bail!("unknown mail action: {other} (list|show|clear)"),
    }
}

fn eml_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("eml") {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Pull From + To + Subject from an .eml for the list view.
fn mail_headers(path: &std::path::Path) -> (String, String, String) {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut from = String::new();
    let mut to = String::new();
    let mut subject = String::new();
    for line in content.lines() {
        if line.is_empty() {
            break; // end of headers
        }
        let lower = line.to_lowercase();
        let value = || line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
        if from.is_empty() && (lower.starts_with("from:") || lower.starts_with("x-envelope-from:")) {
            from = value();
        } else if to.is_empty() && (lower.starts_with("to:") || lower.starts_with("x-envelope-to:")) {
            to = value();
        } else if subject.is_empty() && lower.starts_with("subject:") {
            subject = value();
        }
    }
    (from, to, subject)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

/// One-time privileged setup: route `.test`, trust the CA, and (optionally)
/// redirect :80/:443 so sites are reachable at clean `http://<name>.test`.
pub fn setup(home: Option<&str>, ports: bool) -> Result<()> {
    let helper = helper_path()?;
    let ca = dpl_core::paths::certs_dir(home)?.join("ca.pem");
    if !ca.exists() {
        anyhow::bail!("CA not found at {}. Start the daemon once (`dpld`) so it can generate the CA, then re-run.", ca.display());
    }
    let tlds = configured_tlds(home)?;
    println!("This runs the privileged helper via sudo — you may be prompted for your password.\n");

    for tld in &tlds {
        sudo(&helper, &["install-resolver", tld, "5333"])?;
    }
    sudo(&helper, &["trust-ca", &ca.to_string_lossy()])?;
    if ports {
        sudo(&helper, &["install-portmap", "8080", "8443"])?;
        println!("\n✓ Setup complete. Restart the daemon with the redirect ports:");
        println!("    DPL_HTTP_PORT=8080 DPL_HTTPS_PORT=8443 dpld");
        println!("  then browse to http://<name>.test (and https://<name>.test).");
    } else {
        println!("\n✓ Setup complete (no port redirect). Sites are at http://<name>.test:8080 and https://<name>.test:8443.");
    }
    Ok(())
}

/// Take over local `.test` serving from Valet/Apache: stop them so dpl can own
/// ports 80/443, install dpl's resolver + trusted HTTPS + port redirect, and
/// restart the daemon. Interactive (uses sudo) — run in a Terminal.
pub fn takeover(home: Option<&str>) -> Result<()> {
    use std::process::Command as Cmd;
    let step = |desc: &str, cmd: &str, args: &[&str]| {
        println!("• {desc}");
        let _ = Cmd::new(cmd).args(args).status();
    };

    println!("Take over ports 80/443 from Valet/Apache\n");
    println!("Valet's nginx and Apache are holding :80/:443, so the browser hits them");
    println!("instead of dpl. This stops and DISABLES them so dpl serves your sites.");
    println!("You'll be prompted for your password (sudo).\n");

    // 1) Valet's own stop (handles its nginx/dnsmasq/php-fpm cleanly).
    if which("valet").is_some() {
        step("Stopping Valet…", "valet", &["stop"]);
    }

    // 2) Disable the root-level brew services the Valet/Apache stack uses so
    //    they don't respawn on boot (this is what `valet stop` alone misses).
    if which("brew").is_some() {
        for svc in ["nginx", "httpd", "dnsmasq"] {
            step(&format!("Disabling brew service {svc}…"), "sudo", &["brew", "services", "stop", svc]);
        }
    }

    // 3) macOS built-in Apache (the "It works!" server), stopped + kept down.
    step("Stopping Apache…", "sudo", &["apachectl", "stop"]);
    step(
        "Disabling Apache autostart…",
        "sudo",
        &["launchctl", "unload", "-w", "/System/Library/LaunchDaemons/org.apache.httpd.plist"],
    );

    // 4) Force-kill any stragglers still bound to the ports.
    step("Clearing leftover nginx…", "sudo", &["pkill", "-x", "nginx"]);
    step("Clearing leftover httpd…", "sudo", &["pkill", "-x", "httpd"]);

    // 5) Install dpl's resolver, trust the CA, redirect :80/:443.
    println!("\n• Configuring dpl (resolver, trusted HTTPS, :80/:443 redirect)…");
    setup(home, true)?;

    // 6) Restart the daemon so it rebinds cleanly.
    println!("\n• Restarting the dpl daemon…");
    let _ = crate::commands::daemon::manage(home, "restart".to_string());

    // 7) Verify the ports are actually free now.
    std::thread::sleep(std::time::Duration::from_millis(800));
    println!();
    let foreign = |s: &str| {
        let l = s.to_lowercase();
        l.contains("nginx") || l.contains("apache") || l.contains("valet")
    };
    match port_server(80) {
        Some(s) if foreign(&s) => {
            println!("⚠ Port 80 is STILL held by {s}.");
            println!("  Something is respawning it. Find it with:");
            println!("      sudo lsof -nP -iTCP:80 -sTCP:LISTEN");
            println!("  then stop that service (e.g. `sudo brew services stop <name>`).");
        }
        _ => {
            println!("✓ Ports are clear — dpl now serves http://<name>.test (and https://).");
            println!("  Reload a site in your browser.");
        }
    }
    Ok(())
}

/// Set a linked site's runtime (assumes the server is already installed).
pub fn set_runtime(home: Option<&str>, site: String, runtime: String) -> Result<()> {
    send_message(home, Request::SetRuntime { site, runtime })
}

/// Install Laravel Octane in a site's project and switch it to that server.
/// Runs `composer require laravel/octane` + `php artisan octane:install` in the
/// project (this modifies the app), installing Swoole/FrankenPHP/RoadRunner as
/// needed, then flips the site's runtime.
pub fn octane_setup(home: Option<&str>, site: &str, server: &str) -> Result<()> {
    use std::process::Command as Cmd;
    let server = server.to_lowercase();
    if !["swoole", "roadrunner", "frankenphp"].contains(&server.as_str()) {
        anyhow::bail!("unknown server `{server}` (swoole | roadrunner | frankenphp)");
    }

    // Locate the site's project + PHP version from the config.
    let cfg_path = dpl_core::paths::local_config(home)?;
    let cfg = dpl_core::config::LocalConfig::load(&cfg_path)?;
    let link = cfg
        .links
        .get(&site.to_lowercase())
        .with_context(|| format!("no linked site named `{site}` (Octane applies to linked Laravel apps)"))?;
    let project = link.path.clone();
    if !project.join("artisan").is_file() {
        anyhow::bail!("`{site}` isn't a Laravel app (no artisan at {}).", project.display());
    }
    let version = link.php.clone().or(cfg.default_php.clone()).or_else(dpl_core::php::default_version);
    let php = link
        .php
        .as_deref()
        .and_then(dpl_core::php::resolve)
        .unwrap_or_else(dpl_core::php::default_binary);

    println!("Setting up Laravel Octane ({server}) for `{site}` in {}…\n", project.display());

    // Swoole needs its PHP extension; FrankenPHP/RoadRunner binaries are
    // fetched by octane:install.
    if server == "swoole" {
        if let Some(v) = &version {
            println!("• Installing the swoole extension for PHP {v}…");
            let _ = php_ext_install(v, "swoole");
        }
    }

    // composer require laravel/octane (in the project).
    println!("\n• composer require laravel/octane …");
    if which("composer").is_none() {
        anyhow::bail!("Composer not found — install it (brew install composer) and re-run.");
    }
    let ok = Cmd::new("composer")
        .arg("require").arg("laravel/octane").arg("--no-interaction")
        .current_dir(&project).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        anyhow::bail!("`composer require laravel/octane` failed.");
    }

    // php artisan octane:install --server=<server>.
    println!("\n• php artisan octane:install --server={server} …");
    let ok = Cmd::new(&php)
        .arg("artisan").arg("octane:install")
        .arg(format!("--server={server}"))
        .arg("--no-interaction")
        .current_dir(&project).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        anyhow::bail!("`artisan octane:install` failed.");
    }

    // Flip the runtime (daemon starts the server + proxies to it).
    println!("\n• Switching `{site}` to octane-{server}…");
    send_message(home, Request::SetRuntime { site: site.to_string(), runtime: format!("octane-{server}") })?;
    println!("\n✓ `{site}` now runs on Laravel Octane ({server}). Reload it in your browser.");
    Ok(())
}

/// Switch how `.test` resolves: `hosts` (per-site /etc/hosts entries — keeps
/// iCloud Private Relay working) or `resolver` (wildcard DNS). Installs/removes
/// the NOPASSWD sudoers rule that lets the daemon keep /etc/hosts in sync.
pub fn resolution(home: Option<&str>, mode: Option<String>) -> Result<()> {
    let mode = mode.unwrap_or_else(|| "status".into());
    let helper = helper_path()?;
    let helper_str = helper.to_string_lossy().into_owned();
    let user = std::env::var("USER").unwrap_or_default();
    let tlds = configured_tlds(home)?;

    match mode.as_str() {
        "hosts" => {
            println!("Switching .test resolution to /etc/hosts (keeps iCloud Private Relay on).");
            println!("Installs a sudoers rule so dpl can update /etc/hosts without a password.\n");
            // Let the daemon update /etc/hosts silently from now on.
            sudo(&helper, &["install-sudoers", &user, &helper_str])?;
            // Drop the DNS resolver files (the Private Relay trigger).
            for tld in &tlds {
                let _ = sudo(&helper, &["remove-resolver", tld]);
            }
            // Daemon writes the current sites into /etc/hosts.
            match daemon::call(Request::SetResolution { mode: "hosts".into() }, home)? {
                Response::Message { text } => println!("\n✓ {text}"),
                other => return crate::commands::unexpected(other),
            }
            println!("  Private Relay should re-enable within a minute.");
        }
        "resolver" => {
            // Clear /etc/hosts first (while the sudoers rule still permits it).
            let _ = daemon::call(Request::SetResolution { mode: "resolver".into() }, home)?;
            let _ = sudo(&helper, &["remove-sudoers"]);
            for tld in &tlds {
                sudo(&helper, &["install-resolver", tld, "5333"])?;
            }
            println!("✓ Back to wildcard DNS resolution for .test.");
        }
        _ => {
            let current = current_resolution(home);
            println!("Resolution mode: {current}");
            println!("  dpl resolution hosts     — per-site /etc/hosts (Private Relay stays on)");
            println!("  dpl resolution resolver  — wildcard DNS (default)");
        }
    }
    Ok(())
}

/// Read the configured resolution mode from the local config.
fn current_resolution(home: Option<&str>) -> String {
    dpl_core::paths::local_config(home)
        .ok()
        .and_then(|p| dpl_core::config::LocalConfig::load(&p).ok())
        .and_then(|c| c.resolution)
        .unwrap_or_else(|| "resolver".into())
}

/// Reverse [`takeover`]: give ports 80/443 back to Valet. Removes dpl's port
/// redirect + resolver, re-enables Valet's nginx/dnsmasq daemons, and starts
/// Valet. dpl keeps working on :8080/:8443. Interactive (sudo).
pub fn untakeover(home: Option<&str>) -> Result<()> {
    use std::process::Command as Cmd;
    let step = |desc: &str, cmd: &str, args: &[&str]| {
        println!("• {desc}");
        let _ = Cmd::new(cmd).args(args).status();
    };

    println!("Restore Valet — hand ports 80/443 back\n");
    println!("Removes dpl's port redirect and re-enables Valet's nginx/dnsmasq.");
    println!("dpl stays available on :8080/:8443. You'll be prompted for your password.\n");

    // 1) Remove dpl's :80/:443 redirect + resolver + CA trust.
    println!("• Removing dpl's port redirect and resolver…");
    let _ = unsetup(home);

    // 2) Re-enable Valet's root daemons.
    if which("brew").is_some() {
        for svc in ["nginx", "dnsmasq"] {
            step(&format!("Re-enabling brew service {svc}…"), "sudo", &["brew", "services", "start", svc]);
        }
    }

    // 3) Let Valet reconfigure itself (resolver, nginx config, php).
    if which("valet").is_some() {
        step("Starting Valet…", "valet", &["start"]);
    }

    println!("\n✓ Valet restored. dpl sites remain at http://<name>.test:8080 (and :8443 for HTTPS).");
    Ok(())
}

/// Undo `dpl setup`.
pub fn unsetup(home: Option<&str>) -> Result<()> {
    let helper = helper_path()?;
    let ca = dpl_core::paths::certs_dir(home)?.join("ca.pem");
    sudo(&helper, &["remove-portmap"])?;
    for tld in configured_tlds(home)? {
        sudo(&helper, &["remove-resolver", &tld])?;
    }
    if ca.exists() {
        sudo(&helper, &["untrust-ca", &ca.to_string_lossy()])?;
    }
    println!("\n✓ Reverted setup.");
    Ok(())
}

/// The configured TLDs, read straight from the local config file.
fn configured_tlds(home: Option<&str>) -> Result<Vec<String>> {
    let path = dpl_core::paths::local_config(home)?;
    Ok(dpl_core::config::LocalConfig::load(&path)?.tlds())
}

/// Trust (or untrust) the local CA in the system keychain.
pub fn trust(home: Option<&str>, add: bool) -> Result<()> {
    let helper = helper_path()?;
    let ca = dpl_core::paths::certs_dir(home)?.join("ca.pem");
    if !ca.exists() {
        anyhow::bail!("CA not found at {}. Start the daemon once (`dpld`) to generate it.", ca.display());
    }
    let op = if add { "trust-ca" } else { "untrust-ca" };
    sudo(&helper, &[op, &ca.to_string_lossy()])?;
    Ok(())
}

/// Run the helper under sudo, inheriting the terminal for the password prompt.
fn sudo(helper: &std::path::Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("sudo")
        .arg(helper)
        .args(args)
        .status()
        .context("running sudo (is it installed?)")?;
    if !status.success() {
        anyhow::bail!("privileged step `{}` failed.", args.first().copied().unwrap_or("?"));
    }
    Ok(())
}

/// Find the `dpl-helper` binary: next to this executable, else on `$PATH`.
fn helper_path() -> Result<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let h = dir.join("dpl-helper");
            if h.is_file() {
                return Ok(h);
            }
        }
    }
    if let Some(p) = which("dpl-helper") {
        return Ok(std::path::PathBuf::from(p));
    }
    anyhow::bail!("dpl-helper not found next to `dpl` or on your PATH.")
}

/// `which <name>` → path, or None.
pub(crate) fn which(name: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/env").arg("which").arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then_some(p)
}

fn send_message(home: Option<&str>, request: Request) -> Result<()> {
    match daemon::call(request, home)? {
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        Response::Ok => Ok(()),
        other => crate::commands::unexpected(other),
    }
}

/// A site's URL including the proxy port when it isn't the default 80/443.
fn browser_url(site: &SiteInfo, http_port: u16) -> String {
    if site.secure || http_port == 80 {
        site.url.clone()
    } else {
        format!("{}:{http_port}", site.url)
    }
}

fn render_sites(sites: &[SiteInfo], http_port: u16, tld: &str) {
    if sites.is_empty() {
        println!("No local sites yet. Park a folder (`dpl park ~/Sites`) or link a project (`dpl link .`).");
        return;
    }
    let port_note = if http_port == 80 {
        String::new()
    } else {
        format!(":{http_port}")
    };
    println!("Serving .{tld} sites on port {http_port}\n");

    let name_w = sites.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    let url_w = sites
        .iter()
        .map(|s| s.host.len() + port_note.len())
        .max()
        .unwrap_or(3)
        .max(3);
    println!(
        "{:<width_n$}  {:<width_u$}  {:<7}  {:<6}  PATH",
        "NAME", "URL", "SOURCE", "SERVING",
        width_n = name_w,
        width_u = url_w,
    );
    for s in sites {
        let url = format!("{}{}", s.host, port_note);
        println!(
            "{:<width_n$}  {:<width_u$}  {:<7}  {:<6}  {}",
            s.name,
            url,
            s.source,
            if s.serving { "yes" } else { "no" },
            s.path,
            width_n = name_w,
            width_u = url_w,
        );
    }
}

/// Open a URL in the default browser (macOS `open`, Linux `xdg-open`).
fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .with_context(|| format!("launching {program}"))?;
    Ok(())
}
