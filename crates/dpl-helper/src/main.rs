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
//!   install-portmap <http> <https> redirect :80→http and :443→https (macOS pf)
//!   remove-portmap                undo it
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

// ---- port 80/443 redirect (macOS pf) ----

#[cfg(target_os = "macos")]
fn install_portmap(http: &str, https: &str) -> Result<String, String> {
    let http = valid_port(http)?;
    let https = valid_port(https)?;
    let anchor = format!(
        "rdr pass inet proto tcp from any to any port 80 -> 127.0.0.1 port {http}\n\
         rdr pass inet proto tcp from any to any port 443 -> 127.0.0.1 port {https}\n"
    );
    std::fs::write("/etc/pf.anchors/dpl", anchor).map_err(|e| format!("writing pf anchor: {e}"))?;
    let conf = "rdr-anchor \"dpl\"\nload anchor \"dpl\" from \"/etc/pf.anchors/dpl\"\n";
    std::fs::write("/etc/pf-dpl.conf", conf).map_err(|e| format!("writing pf conf: {e}"))?;
    // Load our ruleset and enable pf (already-enabled is fine).
    run("pfctl", &["-Ef", "/etc/pf-dpl.conf"])
        .or_else(|_| run("pfctl", &["-f", "/etc/pf-dpl.conf"]))?;
    Ok(format!(
        ":80 → 127.0.0.1:{http} and :443 → 127.0.0.1:{https} redirect installed. \
         Run dpld with DPL_HTTP_PORT={http} DPL_HTTPS_PORT={https}."
    ))
}

#[cfg(target_os = "macos")]
fn remove_portmap() -> Result<String, String> {
    let _ = std::fs::remove_file("/etc/pf.anchors/dpl");
    let _ = std::fs::remove_file("/etc/pf-dpl.conf");
    let _ = run("pfctl", &["-a", "dpl", "-F", "all"]);
    Ok("Port redirect removed.".into())
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
