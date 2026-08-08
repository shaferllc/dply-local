//! `dpl share` — give a site a permanent public URL through a Jetty tunnel.
//!
//! Sharing is per-site and opt-in. A tunnel is a long-lived agent holding a
//! WebSocket to the Jetty edge, so turning it on for a whole fleet would mean a
//! fleet of connections; you name the sites that need one.
//!
//! The label is what makes the URL *permanent*. It is a subdomain your team has
//! reserved on Jetty (`jetty domains` lists them), and reusing it across
//! restarts is what keeps `https://<label>.tunnels.usejetty.online` pointing at
//! the same site instead of handing out a fresh random name each time.

use anyhow::Result;
use dpl_core::ipc::{Request, Response, ShareStatusInfo};

use crate::daemon;

/// `dpl share` / `dpl share status` — one row per shared site.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let shares = list(home)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "shares": shares }))?);
        return Ok(());
    }

    if shares.is_empty() {
        println!("No sites are shared.");
        println!("Share one with:  dpl share on <site> <label>");
        println!("Your reserved labels:  jetty domains");
        return Ok(());
    }

    println!("{:<24} {:<16} {:<9}  {}", "SITE", "LABEL", "STATE", "URL");
    for s in &shares {
        let state = if s.running { "live" } else { "down" };
        let url = s.url.clone().unwrap_or_else(|| "—".to_string());
        println!("{:<24} {:<16} {:<9}  {}", s.site, s.label, state, url);
        if let Some(d) = &s.detail {
            println!("{:<24} {:<16} {:<9}  {}", "", "", "", d);
        }
    }
    Ok(())
}

/// Every supervised tunnel.
pub fn list(home: Option<&str>) -> Result<Vec<ShareStatusInfo>> {
    match daemon::call(Request::ShareStatus, home)? {
        Response::Shares { shares } => Ok(shares),
        Response::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("unexpected response from the daemon"),
    }
}

/// `dpl share on <site> <label>` — reserve this site's public URL.
pub fn on(home: Option<&str>, site: &str, label: &str) -> Result<()> {
    say(daemon::call(
        Request::SetShare { site: site.to_string(), label: Some(label.to_string()) },
        home,
    )?)
}

/// `dpl share off <site>` — stop sharing. The label stays reserved on Jetty, so
/// turning it back on later gets the same URL.
pub fn off(home: Option<&str>, site: &str) -> Result<()> {
    say(daemon::call(Request::SetShare { site: site.to_string(), label: None }, home)?)
}

/// `dpl share restart <site>` — reconnect, clearing any give-up state.
pub fn restart(home: Option<&str>, site: &str) -> Result<()> {
    say(daemon::call(Request::RestartShare { site: site.to_string() }, home)?)
}

/// `dpl share logs <site>` — the tunnel agent's own output.
pub fn logs(home: Option<&str>, site: &str, lines: usize, follow: bool) -> Result<()> {
    let shares = list(home)?;
    let Some(s) = shares.iter().find(|s| s.site == site) else {
        anyhow::bail!("`{site}` isn't shared — `dpl share on {site} <label>` first")
    };
    let path = std::path::Path::new(&s.log);
    if !path.exists() {
        println!("No log yet for `{site}` — the tunnel hasn't started.");
        return Ok(());
    }
    crate::commands::tail_log(path, lines, follow, &format!("{site} tunnel"))
}

fn say(resp: Response) -> Result<()> {
    match resp {
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        Response::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("unexpected response from the daemon"),
    }
}
