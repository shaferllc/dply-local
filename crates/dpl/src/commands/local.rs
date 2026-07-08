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

/// List installed PHP versions (no daemon needed).
pub fn php_list(json: bool) -> Result<()> {
    let versions = dpl_core::php::detect();
    if json {
        let arr: Vec<_> = versions
            .iter()
            .map(|v| serde_json::json!({ "version": v.version, "binary": v.binary, "source": v.source }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    if versions.is_empty() {
        println!("No PHP found. Install one (e.g. `brew install php`).");
        return Ok(());
    }
    println!("{:<8}  {:<10}  BINARY", "VERSION", "SOURCE");
    for v in &versions {
        println!("{:<8}  {:<10}  {}", v.version, v.source, v.binary.display());
    }
    Ok(())
}

/// Pin a PHP version for a site, or the global default with `--default`.
pub fn use_php(home: Option<&str>, version: String, name: Option<String>, default: bool) -> Result<()> {
    let site = if default { None } else { Some(resolve_name(name)?) };
    send_message(home, Request::UsePhp { version, site })
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

/// Environment health check.
pub fn doctor(home: Option<&str>) -> Result<()> {
    let ok = "\u{2713}";
    let bad = "\u{2717}";
    let warn = "\u{26a0}";
    println!("dpl doctor\n");

    // PHP.
    let phps = dpl_core::php::detect();
    if phps.is_empty() {
        println!("{bad} PHP: none found — install with `brew install php`");
    } else {
        let list: Vec<String> = phps.iter().map(|p| p.version.clone()).collect();
        println!("{ok} PHP: {}", list.join(", "));
    }

    // Daemon + proxy.
    match daemon::call(Request::ListSites, home) {
        Ok(Response::Sites { sites, http_port, .. }) => {
            println!("{ok} daemon: running");
            let serving = sites.iter().filter(|s| s.serving).count();
            println!("{ok} proxy: port {http_port}");
            if http_port != 80 {
                println!("{warn} sites on :{http_port} — run `dpl setup` (sudo) for clean http://<name>.test on :80");
            }
            println!("{ok} sites: {} registered, {serving} serving", sites.len());
        }
        _ => println!("{bad} daemon: not running — start it with `dpld`"),
    }

    // Database/cache services (incl. externally-managed ones like DBngin).
    if std::path::Path::new("/Applications/DBngin.app").exists() {
        println!("{ok} DBngin: detected — using its running engines");
    }
    let svc_req = Request::Service { action: "list".into(), name: None, engine: None, version: None, port: None };
    if let Ok(Response::ServiceList { services }) = daemon::call(svc_req, home) {
        for s in &services {
            let mark = if s.running { ok } else if s.installed { warn } else { warn };
            let state = if s.external {
                "running (external, e.g. DBngin/Postgres.app)".to_string()
            } else if s.running {
                "running (managed by dpl)".to_string()
            } else if s.installed {
                "installed, stopped".to_string()
            } else {
                "not installed".to_string()
            };
            println!("{mark} {}: {state} on port {}", s.name, s.port);
        }
    }

    // Optional tooling.
    match which("cloudflared") {
        Some(_) => println!("{ok} cloudflared: installed (dpl share available)"),
        None => println!("{warn} cloudflared: not installed (dpl share needs it)"),
    }
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
    let formula = match engine.as_str() {
        "postgres" | "postgresql" | "pg" => format!("postgresql@{}", version.as_deref().unwrap_or("17")),
        "mysql" => version.map(|v| format!("mysql@{v}")).unwrap_or_else(|| "mysql".into()),
        "mariadb" => version.map(|v| format!("mariadb@{v}")).unwrap_or_else(|| "mariadb".into()),
        "redis" => version.map(|v| format!("redis@{v}")).unwrap_or_else(|| "redis".into()),
        other => anyhow::bail!("unknown engine `{other}` (postgres|mysql|mariadb|redis)"),
    };

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
fn which(name: &str) -> Option<String> {
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
