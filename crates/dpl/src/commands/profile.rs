//! `dpl profile` — per-site SPX flame-graph profiler control.
//!
//! Mutations go through the daemon, which owns the config and the php-fpm
//! masters. Turning the profiler on for a site whose PHP lacks SPX installs it
//! first (via the same Homebrew tap `dpl php ext-install` uses), so one command
//! gets from nothing to captured flame graphs.

use anyhow::Result;
use dpl_core::ipc::{Request, Response, SiteInfo};

use crate::daemon;

/// `dpl profile` / `dpl profile status` — one row per site.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let sites = list_sites(home)?;
    let sites: Vec<&SiteInfo> = sites.iter().filter(|s| s.source != "proxy").collect();

    if json {
        let rows: Vec<serde_json::Value> = sites
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "php": s.php,
                    "profiling": s.profile,
                    "installed": s.profile_installed,
                    "url": profiler_url(s),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "sites": rows }))?);
        return Ok(());
    }

    if sites.is_empty() {
        println!("No local sites yet. Link a project with `dpl link .`.");
        return Ok(());
    }

    let width = sites.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    println!("{:<width$}  {:<8}  {:<9}  {}", "SITE", "PHP", "PROFILER", "", width = width);
    for s in &sites {
        let state = if s.profile { "on" } else { "off" };
        let note = if s.profile && !s.profile_installed { "SPX not installed for this PHP" } else { "" };
        println!(
            "{:<width$}  {:<8}  {:<9}  {}",
            s.name,
            s.php.as_deref().unwrap_or("default"),
            state,
            note,
            width = width
        );
    }
    if sites.iter().any(|s| s.profile) {
        println!("\nOpen a site's flame graphs with `dpl profile open <site>`.");
    }
    Ok(())
}

/// `dpl profile on|off <site>`. Enabling installs SPX for the site's PHP first
/// when it's missing.
pub fn set(home: Option<&str>, site: String, on: bool) -> Result<()> {
    let site = site.to_lowercase();
    if on {
        if let Some(info) = find_site(home, &site)? {
            if !info.profile_installed {
                let version = info.php.clone().unwrap_or_else(|| "default".into());
                println!("SPX isn't installed for PHP {version} yet — installing it…\n");
                crate::commands::local::php_ext_install(&version, "spx")?;
                println!();
            }
        }
    }
    send(home, Request::SetProfile { site, on })
}

/// `dpl profile open <site>` — launch the same-origin flame-graph UI.
pub fn open(home: Option<&str>, site: String) -> Result<()> {
    let site = site.to_lowercase();
    let Some(info) = find_site(home, &site)? else {
        anyhow::bail!("no local site named {site}. See `dpl profile`.");
    };
    if !info.profile {
        anyhow::bail!("the profiler is off for {site}. Turn it on with `dpl profile on {site}`.");
    }
    let url = profiler_url(&info);
    println!("Opening {url}");
    open_in_browser(&url)
}

fn profiler_url(s: &SiteInfo) -> String {
    // SPX serves its UI at any path with these query params; `/` is its home.
    format!("{}/?SPX_UI_URI=/&SPX_KEY={}", s.url.trim_end_matches('/'), dpl_core::spx::KEY)
}

#[cfg(target_os = "macos")]
fn open_in_browser(url: &str) -> Result<()> {
    std::process::Command::new("open").arg(url).status().map(|_| ()).map_err(Into::into)
}

#[cfg(not(target_os = "macos"))]
fn open_in_browser(url: &str) -> Result<()> {
    std::process::Command::new("xdg-open").arg(url).status().map(|_| ()).map_err(Into::into)
}

fn list_sites(home: Option<&str>) -> Result<Vec<SiteInfo>> {
    let Response::Sites { sites, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    Ok(sites)
}

fn find_site(home: Option<&str>, name: &str) -> Result<Option<SiteInfo>> {
    Ok(list_sites(home)?.into_iter().find(|s| s.name == name))
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
