//! Import sites from an existing Laravel Valet install. Valet stores its config
//! at `~/.config/valet/config.json` (older: `~/.valet/`): a `paths` array of
//! parked directories and a `Sites/` directory of symlinks for `valet link`ed
//! projects (the symlink name is the site name). Both map cleanly onto dpl's
//! `park` and `link` — so we can pull a whole Valet setup in with one command.

use anyhow::{Context, Result};
use dpl_core::ipc::{Request, Response};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::daemon;

/// A `valet link`ed site: an explicit name pointing at a project directory.
#[derive(Debug, Clone, Serialize)]
pub struct ValetLink {
    pub name: String,
    pub path: String,
    /// Whether the target directory still exists.
    pub exists: bool,
}

/// A snapshot of what Valet has configured.
#[derive(Debug, Clone, Serialize)]
pub struct ValetInfo {
    pub installed: bool,
    pub tld: String,
    /// Parked directories (each subfolder becomes a site), excluding Valet's
    /// own `Sites` link directory.
    pub parked: Vec<String>,
    /// Individually linked projects, name → path.
    pub linked: Vec<ValetLink>,
}

/// Find Valet's config directory (new XDG location, then the legacy one).
fn valet_dir() -> Option<PathBuf> {
    let home = dirs_home()?;
    let candidates = [home.join(".config/valet"), home.join(".valet")];
    candidates.into_iter().find(|d| d.join("config.json").is_file())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Read and interpret Valet's config + `Sites/` symlinks.
pub fn discover() -> ValetInfo {
    let Some(dir) = valet_dir() else {
        return ValetInfo { installed: false, tld: "test".into(), parked: vec![], linked: vec![] };
    };
    let sites_dir = dir.join("Sites");

    // config.json → tld + paths.
    let mut tld = "test".to_string();
    let mut paths: Vec<String> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("config.json")) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(t) = cfg.get("tld").and_then(|v| v.as_str()) {
                tld = t.to_string();
            }
            if let Some(arr) = cfg.get("paths").and_then(|v| v.as_array()) {
                paths = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            }
        }
    }

    // Parked dirs = configured paths minus Valet's own Sites directory.
    let parked: Vec<String> = paths
        .into_iter()
        .filter(|p| Path::new(p) != sites_dir && Path::new(p).is_dir())
        .collect();

    // Linked sites = symlinks under Sites/, name = link filename, path = target.
    let mut linked: Vec<ValetLink> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&sites_dir) {
        for entry in entries.flatten() {
            let link_path = entry.path();
            // Only real symlinks (Valet links); skip stray files.
            let Ok(meta) = std::fs::symlink_metadata(&link_path) else { continue };
            if !meta.file_type().is_symlink() {
                continue;
            }
            let Some(name) = link_path.file_name().map(|f| f.to_string_lossy().into_owned()) else { continue };
            // Resolve the target (canonicalize follows the symlink).
            let target = std::fs::read_link(&link_path).ok();
            let resolved = std::fs::canonicalize(&link_path).ok();
            let path = resolved
                .or(target)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let exists = !path.is_empty() && Path::new(&path).is_dir();
            linked.push(ValetLink { name, path, exists });
        }
    }
    linked.sort_by(|a, b| a.name.cmp(&b.name));

    ValetInfo { installed: true, tld, parked, linked }
}

/// List what Valet has (table or `--json`).
pub fn list(json: bool) -> Result<()> {
    let info = discover();
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    if !info.installed {
        println!("No Laravel Valet install found (looked in ~/.config/valet and ~/.valet).");
        return Ok(());
    }
    println!("Valet TLD: .{}", info.tld);
    println!("\nParked directories ({}):", info.parked.len());
    if info.parked.is_empty() {
        println!("  (none)");
    }
    for p in &info.parked {
        println!("  {p}");
    }
    println!("\nLinked sites ({}):", info.linked.len());
    for l in &info.linked {
        println!("  {:<28} {}{}", l.name, l.path, if l.exists { "" } else { "  (missing)" });
    }
    println!("\nImport them with: dpl valet import");
    Ok(())
}

/// Import Valet's parked dirs and/or linked sites into dpl in ONE bulk
/// operation (a single daemon reconcile — importing per-site would reconcile
/// once each and take minutes). `manifest`, if given, is a JSON file with an
/// explicit `{parked:[…], links:[{name,path}]}` selection (used by the GUI);
/// otherwise everything Valet has is imported, filtered by `parks`/`links`.
pub fn import(home: Option<&str>, parks: bool, links: bool, match_tld: bool, manifest: Option<&str>) -> Result<()> {
    let tld = discover().tld;

    let (parked, link_pairs): (Vec<String>, Vec<(String, String)>) = if let Some(path) = manifest {
        read_manifest(path)?
    } else {
        let info = discover();
        if !info.installed {
            anyhow::bail!("No Laravel Valet install found (looked in ~/.config/valet and ~/.valet).");
        }
        let parked = if parks { info.parked.clone() } else { vec![] };
        let link_pairs = if links {
            info.linked.iter().filter(|l| l.exists).map(|l| (l.name.clone(), l.path.clone())).collect()
        } else {
            vec![]
        };
        (parked, link_pairs)
    };

    // Optionally adopt Valet's TLD so hosts match (best-effort).
    if match_tld {
        let _ = daemon::call(Request::Tld { action: "add".into(), name: Some(tld.clone()) }, home);
        let _ = daemon::call(Request::Tld { action: "primary".into(), name: Some(tld.clone()) }, home);
    }

    match daemon::call(Request::ImportSites { parked, links: link_pairs }, home)? {
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        Response::Ok => Ok(()),
        other => crate::commands::unexpected(other),
    }
}

/// Remove sites (the reverse of [`import`]). With a `manifest`, removes exactly
/// that selection; otherwise removes everything Valet has (undo a full import).
/// One daemon reconcile.
pub fn remove(home: Option<&str>, manifest: Option<&str>) -> Result<()> {
    let (parked, link_pairs) = if let Some(path) = manifest {
        read_manifest(path)?
    } else {
        let info = discover();
        let links = info.linked.iter().map(|l| (l.name.clone(), l.path.clone())).collect();
        (info.parked, links)
    };
    let links: Vec<String> = link_pairs.into_iter().map(|(name, _)| name).collect();

    match daemon::call(Request::RemoveSites { parked, links }, home)? {
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        Response::Ok => Ok(()),
        other => crate::commands::unexpected(other),
    }
}

/// Parse a GUI manifest: `{"parked":[…], "links":[{"name","path"}]}`.
fn read_manifest(path: &str) -> Result<(Vec<String>, Vec<(String, String)>)> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest {path}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).context("parsing manifest JSON")?;
    let parked = v.get("parked").and_then(|p| p.as_array()).map(|a| {
        a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
    }).unwrap_or_default();
    let links = v.get("links").and_then(|l| l.as_array()).map(|a| {
        a.iter().filter_map(|x| {
            let name = x.get("name")?.as_str()?.to_string();
            let path = x.get("path")?.as_str()?.to_string();
            Some((name, path))
        }).collect()
    }).unwrap_or_default();
    Ok((parked, links))
}
