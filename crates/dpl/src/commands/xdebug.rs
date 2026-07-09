//! `dpl xdebug` — per-site Xdebug control.
//!
//! Mutations go through the daemon, which owns the config and the php-fpm
//! masters. Status is assembled here: the site list comes from the daemon, and
//! the IDE settings are read straight from `~/.dpl/config.toml`.

use anyhow::Result;
use dpl_core::ipc::{Request, Response, SiteInfo};

use crate::daemon;

/// `dpl xdebug status` — one row per site, plus the shared IDE settings.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let Response::Sites { sites, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    let settings = load_settings(home)?;

    // Reverse proxies run no PHP of ours, so they have no Xdebug mode to show.
    let sites: Vec<&SiteInfo> = sites.iter().filter(|s| s.source != "proxy").collect();

    if json {
        let rows: Vec<serde_json::Value> = sites
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "php": s.php,
                    "mode": s.xdebug.as_deref().unwrap_or("off"),
                    "installed": s.xdebug_installed,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "client_port": settings.client_port,
                "ide_key": settings.ide_key,
                "sites": rows,
            }))?
        );
        return Ok(());
    }

    if sites.is_empty() {
        println!("No local sites yet. Link a project with `dpl link .`.");
        return Ok(());
    }

    println!("IDE  127.0.0.1:{}  (idekey {})\n", settings.client_port, settings.ide_key);
    let width = sites.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    println!("{:<width$}  {:<8}  {:<16}  {}", "SITE", "PHP", "XDEBUG", "", width = width);
    for s in &sites {
        let mode = s.xdebug.as_deref().unwrap_or("off");
        // "installed" only matters when a site actually wants Xdebug.
        let note = if mode != "off" && !s.xdebug_installed {
            "not installed for this PHP"
        } else {
            ""
        };
        println!(
            "{:<width$}  {:<8}  {:<16}  {}",
            s.name,
            s.php.as_deref().unwrap_or("default"),
            mode,
            note,
            width = width
        );
    }

    if sites.iter().any(|s| s.xdebug.as_deref().unwrap_or("off") != "off") {
        println!(
            "\nStep debugging: point your IDE at 127.0.0.1:{}, set a breakpoint, reload the site.",
            settings.client_port
        );
    }
    Ok(())
}

/// `dpl xdebug on|off|mode <m> [site]`.
pub fn set_mode(home: Option<&str>, mode: &str, site: Option<String>) -> Result<()> {
    // Validate before the round-trip so a typo fails fast with the full list of
    // valid modes rather than as a daemon error.
    dpl_core::xdebug::Mode::parse(mode)?;
    send(
        home,
        Request::SetXdebug {
            mode: Some(mode.to_string()),
            site: site.map(|s| s.to_lowercase()),
            port: None,
            ide_key: None,
        },
    )
}

/// `dpl xdebug port <n>`.
pub fn set_port(home: Option<&str>, port: u16) -> Result<()> {
    if port == 0 {
        anyhow::bail!("port must be between 1 and 65535");
    }
    send(home, Request::SetXdebug { mode: None, site: None, port: Some(port), ide_key: None })
}

/// `dpl xdebug ide <key>`.
pub fn set_ide(home: Option<&str>, key: String) -> Result<()> {
    send(home, Request::SetXdebug { mode: None, site: None, port: None, ide_key: Some(key) })
}

fn send(home: Option<&str>, request: Request) -> Result<()> {
    match daemon::call(request, home)? {
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        Response::Ok => Ok(()),
        other => crate::commands::unexpected(other),
    }
}

/// The shared IDE settings, straight from the config file (the daemon has no
/// verb to read them back, and they're not secret).
fn load_settings(home: Option<&str>) -> Result<dpl_core::xdebug::Settings> {
    let path = dpl_core::paths::local_config(home)?;
    Ok(dpl_core::config::LocalConfig::load(&path)?.xdebug)
}
