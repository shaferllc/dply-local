//! `dpl dev` — supervised Node dev servers, one per opted-in site.
//!
//! The daemon runs the script and keeps it alive; this command is the front for
//! turning it on, seeing where it's listening, and reading what it printed. The
//! process outlives your terminal, which is the entire point — a dev server
//! should not be hostage to the tab it was started in.

use anyhow::Result;
use dpl_core::ipc::{DevServerInfo, Request, Response};

use crate::daemon;

/// `dpl dev` / `dpl dev status` — one row per supervised dev server.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let servers = list(home)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "servers": servers }))?);
        return Ok(());
    }

    if servers.is_empty() {
        println!("No dev servers. Turn one on with `dpl dev on <site>`.");
        println!("It runs that site's `dev` script under the site's own package manager and");
        println!("Node pin, and the daemon keeps it alive — no terminal tab required.");
        return Ok(());
    }

    let width = servers.iter().map(|s| s.site.len()).max().unwrap_or(4).max(4);
    println!("{:<width$}  {:<10}  {:<9}  WHERE", "SITE", "SCRIPT", "STATE", width = width);
    for s in &servers {
        let state = if s.running { "running" } else { "stopped" };
        let where_ = match (s.running, s.port) {
            (true, Some(port)) => format!("http://localhost:{port}"),
            // Running but silent: plenty of scripts (watchers, type-checkers)
            // never listen on anything, so this is normal, not a fault.
            (true, None) => "—".to_string(),
            (false, _) => s.detail.clone().unwrap_or_else(|| "not started".into()),
        };
        println!(
            "{:<width$}  {:<10}  {:<9}  {}",
            s.site,
            s.script,
            state,
            where_,
            width = width
        );
    }
    println!("\nLogs: `dpl dev logs <site>`. Restart one with `dpl dev restart <site>`.");
    Ok(())
}

/// `dpl dev on <site> [--script dev]` — enable and start.
pub fn on(home: Option<&str>, site: String, script: Option<String>) -> Result<()> {
    let script = script.unwrap_or_else(|| "dev".to_string());
    send(home, Request::SetDev { site, script: Some(script) })
}

/// `dpl dev off <site>` — stop and disable.
pub fn off(home: Option<&str>, site: String) -> Result<()> {
    send(home, Request::SetDev { site, script: None })
}

/// `dpl dev restart <site>` — bounce it, clearing any give-up state.
pub fn restart(home: Option<&str>, site: String) -> Result<()> {
    send(home, Request::RestartDev { site })
}

/// `dpl dev logs <site> [--lines N] [--follow]` — what the dev server has
/// printed. Following is the point for a dev server you're actively watching:
/// a build error appears as you save, not when you re-run the command.
pub fn logs(home: Option<&str>, site: String, lines: usize, follow: bool) -> Result<()> {
    let site = site.to_lowercase();
    let server = list(home)?
        .into_iter()
        .find(|s| s.site == site)
        .ok_or_else(|| anyhow::anyhow!("`{site}` has no dev server. See `dpl dev`."))?;

    crate::commands::tail_log(
        std::path::Path::new(&server.log),
        lines,
        follow,
        &format!("{}'s dev server", server.site),
    )
}

fn list(home: Option<&str>) -> Result<Vec<DevServerInfo>> {
    let Response::DevServers { servers } = daemon::call(Request::DevStatus, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    Ok(servers)
}

fn send(home: Option<&str>, request: Request) -> Result<()> {
    match daemon::call(request, home)? {
        Response::Message { text } => println!("✓ {text}"),
        Response::Ok => println!("✓ Done."),
        Response::Error { message } => anyhow::bail!("{message}"),
        _ => anyhow::bail!("unexpected daemon response"),
    }
    Ok(())
}
