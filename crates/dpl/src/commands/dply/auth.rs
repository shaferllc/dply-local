//! `login` / `logout` / `whoami` — device-flow authentication against the
//! shared `~/.dply/config.json`. Mirrors dply-cli's Auth commands.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use dpl_dply::{ConfigStore, DeviceFlow, DeviceStatus, DplyClient};
use serde_json::Value;

/// Run the device-authorization flow and store the resulting token.
pub fn login(
    host_flag: Option<&str>,
    home: Option<String>,
    no_browser: bool,
) -> anyhow::Result<()> {
    let store = ConfigStore::new(home.clone());
    let host = store.resolve_host(host_flag)?;
    let flow = DeviceFlow::new()?;

    let code = flow.start(&host).context("starting device-flow login")?;

    println!("To authorize dpl on {host}:");
    println!();
    println!("  1. Visit: {}", code.verification_uri);
    println!("  2. Enter code: {}", code.user_code);
    println!();
    println!("Or open the direct link:\n  {}", code.verification_uri_complete);
    println!();

    if !no_browser {
        let _ = open_in_browser(&code.verification_uri_complete);
    }

    print!("Waiting for approval");
    let _ = std::io::stdout().flush();

    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let interval = Duration::from_secs(code.interval);

    let token = loop {
        if Instant::now() >= deadline {
            println!();
            bail!("login timed out — the code expired. Run `dpl login` again.");
        }
        std::thread::sleep(interval);
        print!(".");
        let _ = std::io::stdout().flush();

        let outcome = flow.poll(&host, &code.device_code)?;
        match outcome.status {
            DeviceStatus::Authorized => {
                let token = outcome
                    .token
                    .context("server authorized the request but returned no token")?;
                break token;
            }
            DeviceStatus::Pending => continue,
            DeviceStatus::Denied => {
                println!();
                bail!("login was denied in the browser.");
            }
            DeviceStatus::Expired => {
                println!();
                bail!("the login code expired. Run `dpl login` again.");
            }
        }
    };
    println!();

    // Sniff the profile so `whoami` has something to show. Best-effort.
    let user = fetch_profile(&host, &token).unwrap_or(Value::Null);
    store.set_host(&host, &token, user.clone(), true)?;

    println!("✓ Logged in to {host}");
    if let Some(name) = user.pointer("/operator/name").and_then(Value::as_str) {
        println!("  as {name}");
    }
    Ok(())
}

/// Forget the token for the active host.
pub fn logout(host_flag: Option<&str>, home: Option<String>) -> anyhow::Result<()> {
    let store = ConfigStore::new(home);
    let host = store.resolve_host(host_flag)?;
    if store.token_for(&host)?.is_none() {
        println!("Not logged in to {host}.");
        return Ok(());
    }
    store.forget_host(&host)?;
    println!("✓ Logged out of {host}");
    Ok(())
}

/// Show the active host, a masked token, and the cached profile.
pub fn whoami(host_flag: Option<&str>, home: Option<String>, json: bool) -> anyhow::Result<()> {
    let store = ConfigStore::new(home);
    let host = store.resolve_host(host_flag)?;
    let entry = store.host_entry(&host)?;

    if json {
        let payload = serde_json::json!({
            "host": host,
            "authenticated": entry.is_some(),
            "profile": entry.as_ref().map(|e| e.user.clone()),
            "updated_at": entry.as_ref().map(|e| e.updated_at.clone()),
        });
        crate::output::json(&payload);
        return Ok(());
    }

    println!("Host:  {host}");
    match entry {
        None => println!("Status: not logged in (run `dpl login`)"),
        Some(e) => {
            println!("Token: {}", mask(&e.token));
            if let Some(name) = e.user.pointer("/operator/name").and_then(Value::as_str) {
                println!("User:  {name}");
            }
            if let Some(email) = e.user.pointer("/operator/email").and_then(Value::as_str) {
                println!("Email: {email}");
            }
            if let Some(org) = e.user.pointer("/organization/name").and_then(Value::as_str) {
                println!("Org:   {org}");
            }
            if !e.updated_at.is_empty() {
                println!("Since: {}", e.updated_at);
            }
        }
    }
    Ok(())
}

/// GET /api/v1/operator/summary with a freshly-minted token.
fn fetch_profile(host: &str, token: &str) -> anyhow::Result<Value> {
    let client = DplyClient::new(host, Some(token.to_string()))?;
    Ok(dpl_dply::endpoints::operator::summary(&client)?)
}

fn mask(token: &str) -> String {
    let n = token.chars().count();
    if n <= 8 {
        return "•".repeat(n);
    }
    let head: String = token.chars().take(4).collect();
    let tail: String = token.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// Open a URL in the default browser (macOS `open`, Linux `xdg-open`).
fn open_in_browser(url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
