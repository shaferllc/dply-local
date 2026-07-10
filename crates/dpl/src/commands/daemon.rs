//! Managing the background daemon as a login service so it starts on boot and
//! stays up — a macOS launchd **LaunchAgent** (runs as the user, no root) or a
//! systemd **user unit** on Linux.

use anyhow::{bail, Context, Result};

const LABEL: &str = "com.tomshafer.dpld";

pub fn manage(_home: Option<&str>, action: String) -> Result<()> {
    match action.as_str() {
        "install" => install(),
        "uninstall" => uninstall(),
        "start" => start(),
        "stop" => stop(),
        "restart" => {
            let _ = stop();
            std::thread::sleep(std::time::Duration::from_millis(400));
            start()
        }
        "status" => status(),
        other => bail!("unknown daemon action: {other} (install|uninstall|start|stop|restart|status)"),
    }
}

/// Absolute path to the `dpld` binary next to this `dpl`.
fn dpld_path() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("locating dpl")?;
    let dir = exe.parent().context("dpl has no parent dir")?;
    let dpld = dir.join("dpld");
    if dpld.is_file() {
        Ok(dpld)
    } else if let Some(p) = which("dpld") {
        Ok(p.into())
    } else {
        bail!("dpld not found next to `dpl` or on PATH")
    }
}

#[cfg(target_os = "macos")]
fn plist_path() -> Result<std::path::PathBuf> {
    let home = dpl_core::paths::home(None)?;
    Ok(home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn install() -> Result<()> {
    let dpld = dpld_path()?;
    let logs = dpl_core::paths::logs_dir(None)?;
    std::fs::create_dir_all(&logs).ok();
    let path = plist_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    // Redirect :80/:443 requires the pf setup from `dpl setup`; the agent runs
    // dpld on the fallback ports which that redirect targets.
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array><string>{dpld}</string></array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>DPL_HTTP_PORT</key><string>8080</string>
        <key>DPL_HTTPS_PORT</key><string>8443</string>
        <!-- launchd hands us a minimal PATH; add Homebrew so php/php-fpm and
             the database engines are discoverable. -->
        <key>PATH</key><string>/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/local/sbin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>{out}</string>
    <key>StandardErrorPath</key><string>{err}</string>
</dict>
</plist>
"#,
        dpld = dpld.display(),
        out = logs.join("dpld.out.log").display(),
        err = logs.join("dpld.err.log").display(),
    );
    std::fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))?;
    // (Re)load the agent.
    let _ = run("launchctl", &["unload", &path.to_string_lossy()]);
    run("launchctl", &["load", &path.to_string_lossy()])?;
    println!("✓ Installed and started the dpld LaunchAgent — it now runs on login.");
    println!("  Ports: http 8080 / https 8443 (pair with `dpl setup` for clean :80/:443).");
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall() -> Result<()> {
    let path = plist_path()?;
    let _ = run("launchctl", &["unload", &path.to_string_lossy()]);
    let _ = std::fs::remove_file(&path);
    println!("✓ Removed the dpld LaunchAgent.");
    Ok(())
}

/// After `dpl setup`, dpld runs as a system LaunchDaemon so launchd can bind
/// :80/:443 for it. That job lives in the system domain, so driving it needs
/// root — hence `sudo launchctl` rather than the plain user-domain calls the
/// LaunchAgent took.
#[cfg(target_os = "macos")]
const SOCKETD_LABEL: &str = "com.dply.dpld";
#[cfg(target_os = "macos")]
const SOCKETD_PLIST: &str = "/Library/LaunchDaemons/com.dply.dpld.plist";

#[cfg(target_os = "macos")]
fn socketd_installed() -> bool {
    std::path::Path::new(SOCKETD_PLIST).exists()
}

#[cfg(target_os = "macos")]
fn start() -> Result<()> {
    if socketd_installed() {
        let target = format!("system/{SOCKETD_LABEL}");
        // Already bootstrapped? Then kickstart it instead of erroring.
        if run_interactive("sudo", &["launchctl", "bootstrap", "system", SOCKETD_PLIST]).is_err() {
            run_interactive("sudo", &["launchctl", "kickstart", &target])?;
        }
        println!("✓ Started dpld (LaunchDaemon, owns :80/:443).");
        return Ok(());
    }
    let path = plist_path()?;
    // Load the agent (KeepAlive + RunAtLoad start it); if it's already loaded,
    // kick it with `start`.
    if run("launchctl", &["load", &path.to_string_lossy()]).is_err() {
        let _ = run("launchctl", &["start", LABEL]);
    }
    println!("✓ Started dpld.");
    Ok(())
}

#[cfg(target_os = "macos")]
fn stop() -> Result<()> {
    if socketd_installed() {
        // KeepAlive is on, so `stop` would just respawn it — boot it out of the
        // system domain to actually stop it. It comes back on reboot or `start`.
        run_interactive("sudo", &["launchctl", "bootout", &format!("system/{SOCKETD_LABEL}")])?;
        println!("✓ Stopped dpld (LaunchDaemon).");
        return Ok(());
    }
    // The agent sets KeepAlive, so `launchctl stop` just triggers a respawn —
    // unload the agent to actually stop it (it reloads on next login / `start`).
    let path = plist_path()?;
    run("launchctl", &["unload", &path.to_string_lossy()])?;
    println!("✓ Stopped dpld.");
    Ok(())
}

#[cfg(target_os = "macos")]
fn status() -> Result<()> {
    if socketd_installed() {
        // `launchctl list` only shows the caller's own domain, so a user can't see
        // a system job here. Report what we can check without sudo, and point at
        // the command that actually probes the running daemon.
        println!("dpld LaunchDaemon: installed (owns :80/:443)");
        println!("  Run `dpl status` to see whether it's up.");
        return Ok(());
    }
    let out = std::process::Command::new("launchctl").arg("list").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let listed = text.lines().any(|l| l.contains(LABEL));
    println!("dpld LaunchAgent: {}", if listed { "loaded" } else { "not installed" });
    Ok(())
}

// ---- Linux (systemd --user) ----

#[cfg(target_os = "linux")]
fn unit_path() -> Result<std::path::PathBuf> {
    let home = dpl_core::paths::home(None)?;
    Ok(home.join(".config/systemd/user/dpld.service"))
}

#[cfg(target_os = "linux")]
fn install() -> Result<()> {
    let dpld = dpld_path()?;
    let path = unit_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let unit = format!(
        "[Unit]\nDescription=dpl local dev daemon\n\n[Service]\nExecStart={dpld}\n\
         Environment=DPL_HTTP_PORT=8080\nEnvironment=DPL_HTTPS_PORT=8443\nRestart=always\n\n\
         [Install]\nWantedBy=default.target\n",
        dpld = dpld.display(),
    );
    std::fs::write(&path, unit).with_context(|| format!("writing {}", path.display()))?;
    run("systemctl", &["--user", "daemon-reload"])?;
    run("systemctl", &["--user", "enable", "--now", "dpld.service"])?;
    println!("✓ Installed and started the dpld systemd user unit.");
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall() -> Result<()> {
    let _ = run("systemctl", &["--user", "disable", "--now", "dpld.service"]);
    let _ = std::fs::remove_file(unit_path()?);
    let _ = run("systemctl", &["--user", "daemon-reload"]);
    println!("✓ Removed the dpld systemd user unit.");
    Ok(())
}

#[cfg(target_os = "linux")]
fn start() -> Result<()> {
    run("systemctl", &["--user", "start", "dpld.service"])?;
    println!("✓ Started dpld.");
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop() -> Result<()> {
    run("systemctl", &["--user", "stop", "dpld.service"])?;
    println!("✓ Stopped dpld.");
    Ok(())
}

#[cfg(target_os = "linux")]
fn status() -> Result<()> {
    let _ = run("systemctl", &["--user", "status", "dpld.service"]);
    Ok(())
}

// ---- helpers ----

fn run(program: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(program).args(args).output()
        .with_context(|| format!("running {program}"))?;
    if out.status.success() {
        Ok(())
    } else {
        bail!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim())
    }
}

/// Like `run`, but leaves stdin/stdout attached to the terminal. `run` captures
/// output, which nulls stdin — `sudo` then has nowhere to prompt for a password
/// and fails with "a terminal is required".
#[cfg(target_os = "macos")]
fn run_interactive(program: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(program).args(args).status()
        .with_context(|| format!("running {program}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{program} {} failed", args.join(" "))
    }
}

fn which(name: &str) -> Option<String> {
    dpl_core::tools::which(name).map(|p| p.to_string_lossy().into_owned())
}
