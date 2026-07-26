//! `dpl tags` — free-form labels on local sites, for slicing a large fleet.
//!
//! Framework and project kind are *detected*; tags are the axis detection can't
//! reach — which client a site belongs to, what's archived, what's mid-rewrite.
//! Tags are normalised on the way in, so a fleet doesn't accumulate `Client X`,
//! `client x` and `client-x` as three separate groupings.

use anyhow::Result;
use dpl_core::ipc::{Request, Response, SiteInfo};

use crate::daemon;

/// `dpl tags` — every tag in use, with the sites carrying it.
pub fn list(home: Option<&str>, json: bool) -> Result<()> {
    let sites = taggable_sites(home)?;
    let mut counts: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for site in &sites {
        for tag in &site.tags {
            counts.entry(tag.clone()).or_default().push(site.name.clone());
        }
    }

    if json {
        let rows: Vec<serde_json::Value> = counts
            .iter()
            .map(|(tag, sites)| serde_json::json!({ "tag": tag, "count": sites.len(), "sites": sites }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "tags": rows }))?);
        return Ok(());
    }

    if counts.is_empty() {
        println!("No tags yet. Add one with `dpl tags add <site> <tag>…`.");
        println!("Tags group sites the way frameworks can't — by client, status, or whatever you need.");
        return Ok(());
    }

    let width = counts.keys().map(|t| t.len()).max().unwrap_or(3).max(3);
    println!("{:<width$}  {:>5}  SITES", "TAG", "COUNT", width = width);
    // Commonest first: the tags that actually organise the fleet lead.
    let mut rows: Vec<(&String, &Vec<String>)> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (tag, sites) in rows {
        println!("{:<width$}  {:>5}  {}", tag, sites.len(), sites.join(", "), width = width);
    }
    let untagged = sites.iter().filter(|s| s.tags.is_empty()).count();
    if untagged > 0 {
        println!("\n{untagged} site(s) have no tags.");
    }
    Ok(())
}

/// `dpl tags show <site>`.
pub fn show(home: Option<&str>, site: String) -> Result<()> {
    let site = find(home, &site)?;
    if site.tags.is_empty() {
        println!("`{}` has no tags.", site.name);
    } else {
        println!("{}", site.tags.join(" "));
    }
    Ok(())
}

/// `dpl tags add <site> <tag>…` — union with what's already there.
pub fn add(home: Option<&str>, site: String, tags: Vec<String>) -> Result<()> {
    let current = find(home, &site)?;
    let mut merged = current.tags.clone();
    merged.extend(tags);
    set_tags(home, current.name, merged)
}

/// `dpl tags rm <site> <tag>…`.
pub fn remove(home: Option<&str>, site: String, tags: Vec<String>) -> Result<()> {
    let current = find(home, &site)?;
    // Normalise what's being removed so `dpl tags rm x "Client X"` works on the
    // stored `client-x` — the user shouldn't have to know the stored spelling.
    let drop = dpl_core::config::normalize_tags(tags);
    let kept: Vec<String> =
        current.tags.iter().filter(|t| !drop.contains(t)).cloned().collect();
    set_tags(home, current.name, kept)
}

/// `dpl tags set <site> [tag]…` — replace outright (no tags clears).
pub fn set(home: Option<&str>, site: String, tags: Vec<String>) -> Result<()> {
    let current = find(home, &site)?;
    set_tags(home, current.name, tags)
}

fn set_tags(home: Option<&str>, site: String, tags: Vec<String>) -> Result<()> {
    match daemon::call(Request::SetTags { site, tags }, home)? {
        Response::Message { text } => println!("✓ {text}"),
        Response::Ok => println!("✓ Done."),
        Response::Error { message } => anyhow::bail!("{message}"),
        other => return crate::commands::unexpected(other),
    }
    Ok(())
}

fn find(home: Option<&str>, site: &str) -> Result<SiteInfo> {
    let name = site.to_lowercase();
    taggable_sites(home)?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("no local site named `{name}`. See `dpl sites`."))
}

/// Every site that can carry a tag — parked as well as linked. Proxies are
/// excluded: they point at another service and have no project of their own.
fn taggable_sites(home: Option<&str>) -> Result<Vec<SiteInfo>> {
    let Response::Sites { sites, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    Ok(sites.into_iter().filter(|s| s.source != "proxy").collect())
}
