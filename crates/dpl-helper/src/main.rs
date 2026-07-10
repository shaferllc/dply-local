//! Privileged one-shot helper — the security boundary between rootless daily
//! use and the one-time privileged setup. It is invoked as root (via `sudo`)
//! by `dpl setup` / `dpl trust`, performs exactly one strictly-validated
//! operation, and exits.
//!
//! Operations:
//!   trust-ca <ca.pem>              add the local CA to the system trust store
//!   untrust-ca <ca.pem>           remove it
//!   install-resolver <tld> <port> route *.tld to the local DNS responder
//!   remove-resolver <tld>         undo it
//!   install-socketd <user> <home> <dpld>  LaunchDaemon owning :80/:443 (macOS)
//!   remove-socketd                undo it
//!   install-portmap <http> <https> redirect :80→http and :443→https (Linux)
//!   remove-portmap                undo it (also tears down a legacy pf anchor)
//!
//! macOS is fully implemented; Linux does the trust + a best-effort resolver.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let op = args.first().map(String::as_str);

    let result = match op {
        Some("--version") => {
            println!("dpl-helper {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("trust-ca") => arg(&args, 1).and_then(trust_ca),
        Some("untrust-ca") => arg(&args, 1).and_then(untrust_ca),
        Some("install-resolver") => match (arg(&args, 1), arg(&args, 2)) {
            (Ok(tld), Ok(port)) => install_resolver(&tld, &port),
            _ => Err("usage: install-resolver <tld> <port>".into()),
        },
        Some("remove-resolver") => arg(&args, 1).and_then(|t| remove_resolver(&t)),
        Some("install-portmap") => match (arg(&args, 1), arg(&args, 2)) {
            (Ok(http), Ok(https)) => install_portmap(&http, &https),
            _ => Err("usage: install-portmap <http_port> <https_port>".into()),
        },
        Some("remove-portmap") => remove_portmap(),
        Some("install-socketd") => match (arg(&args, 1), arg(&args, 2), arg(&args, 3)) {
            (Ok(user), Ok(home), Ok(dpld)) => install_socketd(&user, &home, &dpld),
            _ => Err("usage: install-socketd <user> <home> <dpld-path>".into()),
        },
        Some("remove-socketd") => remove_socketd(),
        Some("sync-hosts") => sync_hosts(&args[1..]),
        Some("clear-hosts") => sync_hosts(&[]),
        Some("install-sudoers") => match (arg(&args, 1), arg(&args, 2)) {
            (Ok(user), Ok(helper)) => install_sudoers(&user, &helper),
            _ => Err("usage: install-sudoers <user> <helper-path>".into()),
        },
        Some("remove-sudoers") => remove_sudoers(),
        Some(other) => Err(format!("unknown operation: {other}")),
        None => Err("expected an operation (this helper is invoked by `dpl`)".into()),
    };

    match result {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dpl-helper: {e}");
            ExitCode::FAILURE
        }
    }
}

fn arg(args: &[String], i: usize) -> Result<String, String> {
    args.get(i).cloned().ok_or_else(|| format!("missing argument {i}"))
}

/// Validate a TLD is a simple label (no path traversal into /etc/resolver).
fn valid_tld(tld: &str) -> Result<(), String> {
    if !tld.is_empty() && tld.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(())
    } else {
        Err(format!("invalid tld: {tld}"))
    }
}

fn valid_port(p: &str) -> Result<u16, String> {
    p.parse::<u16>().map_err(|_| format!("invalid port: {p}"))
}

// ---- CA trust ----

#[cfg(target_os = "macos")]
fn trust_ca(pem: String) -> Result<String, String> {
    run("security", &[
        "add-trusted-cert", "-d", "-r", "trustRoot",
        "-k", "/Library/Keychains/System.keychain", &pem,
    ])?;
    Ok("Local CA added to the System keychain — browsers now trust https://*.test.".into())
}

#[cfg(target_os = "macos")]
fn untrust_ca(pem: String) -> Result<String, String> {
    run("security", &["remove-trusted-cert", "-d", &pem])?;
    Ok("Local CA removed from the System keychain.".into())
}

#[cfg(target_os = "linux")]
fn trust_ca(pem: String) -> Result<String, String> {
    std::fs::copy(&pem, "/usr/local/share/ca-certificates/dpl-ca.crt")
        .map_err(|e| format!("copying CA: {e}"))?;
    run("update-ca-certificates", &[])?;
    Ok("Local CA installed into the system trust store.".into())
}

#[cfg(target_os = "linux")]
fn untrust_ca(_pem: String) -> Result<String, String> {
    let _ = std::fs::remove_file("/usr/local/share/ca-certificates/dpl-ca.crt");
    run("update-ca-certificates", &["--fresh"])?;
    Ok("Local CA removed from the system trust store.".into())
}

// ---- DNS resolver ----

#[cfg(target_os = "macos")]
fn install_resolver(tld: &str, port: &str) -> Result<String, String> {
    valid_tld(tld)?;
    let port = valid_port(port)?;
    std::fs::create_dir_all("/etc/resolver").map_err(|e| format!("mkdir /etc/resolver: {e}"))?;
    let path = format!("/etc/resolver/{tld}");
    std::fs::write(&path, format!("nameserver 127.0.0.1\nport {port}\n"))
        .map_err(|e| format!("writing {path}: {e}"))?;
    Ok(format!("Installed {path} — *.{tld} now resolves to 127.0.0.1."))
}

#[cfg(target_os = "macos")]
fn remove_resolver(tld: &str) -> Result<String, String> {
    valid_tld(tld)?;
    let path = format!("/etc/resolver/{tld}");
    let _ = std::fs::remove_file(&path);
    Ok(format!("Removed {path}."))
}

#[cfg(target_os = "linux")]
fn install_resolver(tld: &str, _port: &str) -> Result<String, String> {
    valid_tld(tld)?;
    Err(format!(
        "automatic .{tld} resolver setup isn't wired for this Linux yet. \
         Point *.{tld} at 127.0.0.1 via systemd-resolved/dnsmasq, or add hosts entries."
    ))
}

#[cfg(target_os = "linux")]
fn remove_resolver(_tld: &str) -> Result<String, String> {
    Ok("Nothing to remove.".into())
}

// ---- privileged ports :80/:443 (macOS: launchd socket activation) ----

#[cfg(target_os = "macos")]
const SOCKETD_LABEL: &str = "com.dply.dpld";
#[cfg(target_os = "macos")]
const SOCKETD_PLIST: &str = "/Library/LaunchDaemons/com.dply.dpld.plist";

/// Install the LaunchDaemon that owns :80 and :443.
///
/// launchd binds both sockets as root at boot, then spawns `dpld` as `user` and
/// hands over the descriptors. The daemon never runs as root, and there is no
/// system state left behind that another tool can flush — which is exactly what
/// went wrong with the pf `rdr` anchor this replaces (any `pfctl -f /etc/pf.conf`
/// from Docker, a VPN, or a reboot silently dropped the redirect). Any leftover
/// pf anchor from an older dpl is cleaned up here.
#[cfg(target_os = "macos")]
fn install_socketd(user: &str, home: &str, dpld: &str) -> Result<String, String> {
    valid_user(user)?;
    // dpld spawns php-fpm pools and writes ~/.dpl. As root it would run every
    // site's PHP as root and leave root-owned files behind. `dpl setup` run under
    // `sudo` reports USER=root, so this is a live mistake, not a theoretical one.
    if user == "root" {
        return Err("refusing to run dpld as root — pass the login user, not root".into());
    }
    let home_path = std::path::Path::new(home);
    if !home_path.is_absolute() || !home_path.is_dir() {
        return Err(format!("home must be an absolute existing directory: {home}"));
    }
    let dpld_path = std::path::Path::new(dpld);
    if !dpld_path.is_absolute() || !dpld_path.is_file() {
        return Err(format!("dpld path must be an absolute existing file: {dpld}"));
    }
    // launchd re-reads this plist as root forever; a newline or a stray quote in
    // a path is the difference between a config value and an injected key.
    for (what, s) in [("home", home), ("dpld path", dpld)] {
        if s.chars().any(|c| c.is_control()) {
            return Err(format!("{what} contains control characters"));
        }
    }

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array><string>{dpld}</string></array>
    <!-- launchd (root) binds the sockets, then drops to this user to run dpld. -->
    <key>UserName</key><string>{user}</string>
    <key>Sockets</key>
    <dict>
        <key>http</key>
        <dict>
            <key>SockNodeName</key><string>127.0.0.1</string>
            <key>SockServiceName</key><string>80</string>
            <key>SockType</key><string>stream</string>
            <key>SockFamily</key><string>IPv4</string>
        </dict>
        <key>https</key>
        <dict>
            <key>SockNodeName</key><string>127.0.0.1</string>
            <key>SockServiceName</key><string>443</string>
            <key>SockType</key><string>stream</string>
            <key>SockFamily</key><string>IPv4</string>
        </dict>
    </dict>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key><string>{home}</string>
        <!-- launchd hands us a minimal PATH; add Homebrew so php/php-fpm and
             the database engines are discoverable. -->
        <key>PATH</key><string>/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/local/sbin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <!-- Without this a background job is CPU-throttled; it serves web requests. -->
    <key>ProcessType</key><string>Interactive</string>
    <key>StandardOutPath</key><string>{home}/.dpl/logs/dpld.out.log</string>
    <key>StandardErrorPath</key><string>{home}/.dpl/logs/dpld.err.log</string>
</dict>
</plist>
"#,
        label = SOCKETD_LABEL,
        dpld = xml_escape(dpld),
        user = xml_escape(user),
        home = xml_escape(home),
    );

    std::fs::write(SOCKETD_PLIST, plist).map_err(|e| format!("writing {SOCKETD_PLIST}: {e}"))?;
    // launchd refuses a plist that is group- or world-writable.
    let _ = run("chown", &["root:wheel", SOCKETD_PLIST]);
    let _ = run("chmod", &["0644", SOCKETD_PLIST]);

    // Replace any running copy, then start it. `bootstrap` is the modern spelling;
    // fall back to `load -w` on older systems.
    let target = format!("system/{SOCKETD_LABEL}");
    let _ = run("launchctl", &["bootout", &target]);
    run("launchctl", &["bootstrap", "system", SOCKETD_PLIST])
        .or_else(|_| run("launchctl", &["load", "-w", SOCKETD_PLIST]))?;
    let _ = run("launchctl", &["enable", &target]);

    let pf = remove_portmap_files();
    Ok(format!(
        ":80 and :443 are now bound by launchd and handed to dpld as {user}.{pf} \
         Sites are reachable at http://<name>.test with no port."
    ))
}

#[cfg(target_os = "macos")]
fn remove_socketd() -> Result<String, String> {
    let target = format!("system/{SOCKETD_LABEL}");
    let _ = run("launchctl", &["bootout", &target]);
    let _ = run("launchctl", &["unload", "-w", SOCKETD_PLIST]);
    let _ = std::fs::remove_file(SOCKETD_PLIST);
    Ok("Removed the :80/:443 LaunchDaemon. Sites fall back to :8080/:8443.".into())
}

/// Tear down the pf redirect a pre-launchd dpl may have left behind. Reported so
/// an upgrading user can see the old mechanism go away.
#[cfg(target_os = "macos")]
fn remove_portmap_files() -> String {
    let had_anchor = std::path::Path::new("/etc/pf.anchors/dpl").exists();
    let _ = std::fs::remove_file("/etc/pf.anchors/dpl");
    let _ = std::fs::remove_file("/etc/pf-dpl.conf");
    let _ = run("pfctl", &["-a", "dpl", "-F", "all"]);
    if had_anchor { " The old pf redirect was removed." } else { "" }.to_string()
}

#[cfg(target_os = "macos")]
fn remove_portmap() -> Result<String, String> {
    remove_socketd()?;
    remove_portmap_files();
    Ok("Port redirect removed.".into())
}

/// Escape the five XML metacharacters so a path can't close a tag and inject keys.
#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// macOS gets :80/:443 from launchd, not from a redirect. Kept so an older `dpl`
/// invoking this helper fails loudly instead of silently doing nothing.
#[cfg(target_os = "macos")]
fn install_portmap(_http: &str, _https: &str) -> Result<String, String> {
    Err("this dpl binds :80/:443 via launchd; run `sudo dpl setup` instead".into())
}

#[cfg(target_os = "linux")]
fn install_socketd(_user: &str, _home: &str, _dpld: &str) -> Result<String, String> {
    Err("socket activation is macOS-only; use install-portmap".into())
}

#[cfg(target_os = "linux")]
fn remove_socketd() -> Result<String, String> {
    Ok("Nothing to remove.".into())
}

#[cfg(target_os = "linux")]
fn install_portmap(http: &str, https: &str) -> Result<String, String> {
    let http = valid_port(http)?;
    let https = valid_port(https)?;
    run("iptables", &["-t", "nat", "-A", "OUTPUT", "-o", "lo", "-p", "tcp", "--dport", "80", "-j", "REDIRECT", "--to-ports", &http.to_string()])?;
    run("iptables", &["-t", "nat", "-A", "OUTPUT", "-o", "lo", "-p", "tcp", "--dport", "443", "-j", "REDIRECT", "--to-ports", &https.to_string()])?;
    Ok(format!(":80→{http} and :443→{https} redirect installed (iptables)."))
}

#[cfg(target_os = "linux")]
fn remove_portmap() -> Result<String, String> {
    Ok("Remove the iptables OUTPUT nat rules manually if set.".into())
}

// ---- /etc/hosts management (Private-Relay-safe alternative to a resolver) ----

const HOSTS_BEGIN: &str = "# >>> dpl managed (do not edit) >>>";
const HOSTS_END: &str = "# <<< dpl managed <<<";
const HOSTS_PATH: &str = "/etc/hosts";

/// A hostname safe to write into /etc/hosts (letters/digits/dot/hyphen only).
fn valid_hostname(h: &str) -> Result<(), String> {
    let ok = !h.is_empty()
        && h.len() <= 253
        && !h.starts_with(['.', '-'])
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if ok { Ok(()) } else { Err(format!("invalid hostname: {h}")) }
}

/// Rewrite the dpl-managed block of /etc/hosts to map exactly `hosts` →
/// 127.0.0.1 (IPv4 only, so the browser reaches dpld's IPv4 listener). An empty
/// list clears the block. Everything outside the markers is preserved verbatim.
fn sync_hosts(hosts: &[String]) -> Result<String, String> {
    for h in hosts {
        valid_hostname(h)?;
    }
    let current = std::fs::read_to_string(HOSTS_PATH).map_err(|e| format!("reading {HOSTS_PATH}: {e}"))?;

    // Drop any existing managed block, keep the rest.
    let mut out = String::new();
    let mut in_block = false;
    for line in current.lines() {
        match line.trim() {
            HOSTS_BEGIN => in_block = true,
            HOSTS_END => in_block = false,
            _ if !in_block => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    let mut out = out.trim_end().to_string();
    out.push('\n');
    if !hosts.is_empty() {
        out.push_str(HOSTS_BEGIN);
        out.push('\n');
        for h in hosts {
            out.push_str("127.0.0.1\t");
            out.push_str(h);
            out.push('\n');
        }
        out.push_str(HOSTS_END);
        out.push('\n');
    }

    // Write via a temp file + rename so /etc/hosts is never half-written.
    let tmp = "/etc/hosts.dpl.tmp";
    std::fs::write(tmp, &out).map_err(|e| format!("writing {tmp}: {e}"))?;
    std::fs::rename(tmp, HOSTS_PATH).map_err(|e| format!("replacing {HOSTS_PATH}: {e}"))?;
    Ok(format!("Synced {} host entr{} to {HOSTS_PATH}.", hosts.len(), if hosts.len() == 1 { "y" } else { "ies" }))
}

// ---- sudoers (let dpl update /etc/hosts without a password prompt) ----

fn valid_user(u: &str) -> Result<(), String> {
    let ok = !u.is_empty()
        && u.len() <= 32
        && u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if ok { Ok(()) } else { Err(format!("invalid user: {u}")) }
}

fn install_sudoers(user: &str, helper: &str) -> Result<String, String> {
    valid_user(user)?;
    let path = std::path::Path::new(helper);
    if !path.is_absolute() || !path.is_file() {
        return Err(format!("helper path must be an absolute existing file: {helper}"));
    }
    if helper.contains([',', '\n', ' ', '"', '\\']) {
        return Err("helper path contains disallowed characters".into());
    }
    // Only the hosts-sync operations, nothing else.
    let content = format!(
        "# Installed by dpl — lets dpl keep /etc/hosts in sync without a password.\n\
         {user} ALL=(root) NOPASSWD: {helper} sync-hosts *, {helper} clear-hosts\n"
    );
    let dst = "/etc/sudoers.d/dpl";
    let tmp = "/etc/sudoers.d/.dpl.tmp";
    std::fs::write(tmp, &content).map_err(|e| format!("writing {tmp}: {e}"))?;
    let _ = run("chmod", &["0440", tmp]);
    // Validate before activating — a malformed sudoers file could lock out sudo.
    match std::process::Command::new("visudo").args(["-cf", tmp]).output() {
        Ok(o) if o.status.success() => {}
        _ => {
            let _ = std::fs::remove_file(tmp);
            return Err("sudoers validation failed — not installed".into());
        }
    }
    std::fs::rename(tmp, dst).map_err(|e| format!("installing {dst}: {e}"))?;
    Ok(format!("Installed {dst} — dpl can update /etc/hosts without a password."))
}

fn remove_sudoers() -> Result<String, String> {
    let _ = std::fs::remove_file("/etc/sudoers.d/dpl");
    Ok("Removed /etc/sudoers.d/dpl.".into())
}

// ---- helper ----

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("running {program}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}
