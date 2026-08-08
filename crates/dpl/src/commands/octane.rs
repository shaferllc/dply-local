//! `dpl octane` — Laravel Octane sites and the code they're holding in memory.
//!
//! php-fpm re-reads your application on every request; an Octane worker boots it
//! once and keeps it. That's the speed, and it's also the one thing that
//! surprises people: the class you just edited isn't the class being served. So
//! the daemon watches each Octane site's sources and reloads its workers when
//! they change, and this command is the front for seeing that, doing it by hand,
//! and turning it off for a project that would rather reload deliberately.

use anyhow::Result;
use dpl_core::ipc::{AppServerInfo, Request, Response, SiteInfo};

use crate::daemon;

/// `dpl octane` / `dpl octane status` — one row per supervised Octane server.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let servers = list(home)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "servers": servers }))?);
        return Ok(());
    }

    if servers.is_empty() {
        println!("No Octane sites. Install one with `dpl octane install <site>`,");
        println!("or point a site at a server it already has with `dpl runtime <site> octane-frankenphp`.");
        return Ok(());
    }

    let width = servers.iter().map(|s| s.site.len()).max().unwrap_or(4).max(4);
    println!(
        "{:<width$}  {:<12}  {:<9}  {:<8}  WHERE",
        "SITE",
        "SERVER",
        "STATE",
        "WATCHING",
        width = width
    );
    for s in &servers {
        let server = s.runtime.strip_prefix("octane-").unwrap_or(&s.runtime);
        let state = if s.running { "running" } else { "stopped" };
        let watching = if s.watch { "on" } else { "off" };
        let where_ = match s.running {
            true => format!("127.0.0.1:{}", s.port),
            false => s.detail.clone().unwrap_or_else(|| "not started".into()),
        };
        println!(
            "{:<width$}  {:<12}  {:<9}  {:<8}  {}",
            s.site,
            server,
            state,
            watching,
            where_,
            width = width
        );
    }
    let reloads: u32 = servers.iter().map(|s| s.reloads).sum();
    if reloads > 0 {
        println!("\n{reloads} worker reload(s) this session.");
    }
    println!("\nLogs: `dpl octane logs <site>`. Reload by hand with `dpl octane reload <site>`.");
    Ok(())
}

/// `dpl octane install <site> [--server frankenphp]` — composer + artisan in the
/// project, then flip the runtime. Lives in `local` because it shares that
/// module's PHP-extension and Composer plumbing.
pub fn install(home: Option<&str>, site: &str, server: &str) -> Result<()> {
    crate::commands::local::octane_setup(home, site, server)
}

/// `dpl octane reload <site>` — cycle the workers, keep the listener.
pub fn reload(home: Option<&str>, site: String) -> Result<()> {
    send(home, Request::ReloadOctane { site })
}

/// `dpl octane restart <site>` — stop and start the server itself.
pub fn restart(home: Option<&str>, site: String) -> Result<()> {
    send(home, Request::RestartOctane { site })
}

/// `dpl octane watch <site> [on|off]` — reload-on-save, or show it.
pub fn watch(home: Option<&str>, site: String, state: Option<String>) -> Result<()> {
    let on = match state.as_deref().map(str::to_lowercase).as_deref() {
        Some("on") | Some("true") | Some("yes") => true,
        Some("off") | Some("false") | Some("no") => false,
        Some(other) => anyhow::bail!("say `on` or `off`, not `{other}`."),
        None => return show_watch(home, &site),
    };
    send(home, Request::SetOctaneWatch { site, on })
}

/// `dpl octane logs <site> [--lines N] [--follow]` — what the server has
/// printed, including the reloads the watcher triggered.
pub fn logs(home: Option<&str>, site: String, lines: usize, follow: bool) -> Result<()> {
    let site = site.to_lowercase();
    let server = list(home)?
        .into_iter()
        .find(|s| s.site == site)
        .ok_or_else(|| anyhow::anyhow!("`{site}` has no Octane server. See `dpl octane`."))?;

    crate::commands::tail_log(
        std::path::Path::new(&server.log),
        lines,
        follow,
        &format!("{}'s Octane server", server.site),
    )
}

/// Report a site's watch setting without changing it. Read from the site list
/// rather than the server list so it also answers for a site that isn't on
/// Octane yet — the setting is stored either way.
fn show_watch(home: Option<&str>, site: &str) -> Result<()> {
    let site = site.to_lowercase();
    let Response::Sites { sites, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    let info: SiteInfo = sites
        .into_iter()
        .find(|s| s.name == site)
        .ok_or_else(|| anyhow::anyhow!("no site named `{site}`."))?;

    let runtime = info.runtime.as_deref().unwrap_or("fpm");
    if info.watch {
        println!("`{site}` reloads its Octane workers when you save.");
    } else {
        println!("`{site}` does not reload on save — `dpl octane reload {site}` does it by hand.");
    }
    if runtime == "fpm" || runtime.is_empty() {
        println!("It's on php-fpm, though, which re-reads your code every request — so this only");
        println!("matters once it moves to Octane (`dpl octane install {site}`).");
    }
    println!("\nChange it with `dpl octane watch {site} on|off`.");
    Ok(())
}

fn list(home: Option<&str>) -> Result<Vec<AppServerInfo>> {
    let Response::AppServers { servers } = daemon::call(Request::OctaneStatus, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    Ok(servers)
}

fn send(home: Option<&str>, request: Request) -> Result<()> {
    match daemon::call(request, home)? {
        Response::Message { text } => println!("✓ {text}"),
        Response::Ok => println!("✓ Done."),
        Response::Error { message } => anyhow::bail!("{message}"),
        other => return crate::commands::unexpected(other),
    }
    Ok(())
}
