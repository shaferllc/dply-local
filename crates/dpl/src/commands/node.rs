//! `dpl node` — per-project Node version management, delegated to fnm/nvm.
//!
//! dpl writes each repo's `.nvmrc` (the pin fnm and nvm auto-switch on) and reads
//! a project's desired version back from `.nvmrc`/`.node-version`/`package.json`.
//! Installing a version is handed to whichever manager is present. dpl never runs
//! `node` itself, so the actual per-`cd` switch is the manager's job.

use std::path::{Path, PathBuf};

use anyhow::Result;
use dpl_core::ipc::{Request, Response, SiteInfo};
use dpl_core::node::{self, Manager, Pin};

use crate::daemon;

/// `dpl node` / `dpl node status` — one row per site.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let manager = node::detect_manager();
    let sites: Vec<SiteInfo> =
        list_sites(home)?.into_iter().filter(|s| s.source != "proxy").collect();

    if json {
        let rows: Vec<serde_json::Value> = sites
            .iter()
            .map(|s| {
                let pin = node::read_pin(Path::new(&s.path));
                serde_json::json!({
                    "name": s.name,
                    "path": s.path,
                    "version": pin.as_ref().map(|p| p.version.clone()),
                    "source": pin.as_ref().map(|p| p.source.as_str()),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "manager": manager.map(|m| m.name()),
                "sites": rows,
            }))?
        );
        return Ok(());
    }

    match manager {
        Some(m) => println!("Manager: {} (auto-switches on `cd`)\n", m.name()),
        None => {
            println!("No Node manager found. Install one so `.nvmrc` pins auto-switch:");
            println!("    brew install fnm   # then add its shell hook (see `fnm env`)\n");
        }
    }

    if sites.is_empty() {
        println!("No local sites yet. Link a project with `dpl link .`.");
        return Ok(());
    }

    let width = sites.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    println!("{:<width$}  {:<12}  PINNED IN", "SITE", "NODE", width = width);
    for s in &sites {
        match node::read_pin(Path::new(&s.path)) {
            Some(pin) => println!("{:<width$}  {:<12}  {}", s.name, pin.version, pin.source.as_str(), width = width),
            None => println!("{:<width$}  {:<12}  (unpinned)", s.name, "—", width = width),
        }
    }
    println!("\nPin one with `dpl node use <version> <site>`, or `dpl node detect <site>` from package.json.");
    Ok(())
}

/// `dpl node use <version> [site]` — write the site's `.nvmrc`.
pub fn use_version(home: Option<&str>, version: String, site: Option<String>) -> Result<()> {
    let dir = project_dir(home, site)?;
    node::write_nvmrc(&dir, &version)?;
    println!("Pinned Node {version} for {} (.nvmrc).", dir.display());
    if node::detect_manager().is_some() {
        println!("`cd` into it (or run `nvm use` / `fnm use`) to switch.");
    } else {
        println!("Install fnm or nvm to auto-switch — see `dpl node`.");
    }
    if !node_version_installed(&version) {
        println!("\nNode {version} may not be installed yet — `dpl node install {version}`.");
    }
    Ok(())
}

/// `dpl node detect [site]` — pin from package.json's `engines.node`.
pub fn detect(home: Option<&str>, site: Option<String>) -> Result<()> {
    let dir = project_dir(home, site)?;
    match node::read_pin(&dir) {
        Some(Pin { version, source }) if source == dpl_core::node::PinSource::PackageJson => {
            let Some(v) = node::normalize_range(&version) else {
                anyhow::bail!(
                    "package.json wants Node \"{version}\", which isn't a plain version — \
                     pin it explicitly with `dpl node use <version>`."
                );
            };
            node::write_nvmrc(&dir, &v)?;
            println!("package.json wants Node \"{version}\" → pinned {v} (.nvmrc).");
            Ok(())
        }
        Some(Pin { version, source }) => {
            println!("Already pinned to {version} via {}. Nothing to detect.", source.as_str());
            Ok(())
        }
        None => anyhow::bail!("no .nvmrc, .node-version, or package.json engines.node in {}", dir.display()),
    }
}

/// `dpl node install <version>` — hand off to the detected manager.
pub fn install(version: &str) -> Result<()> {
    let Some(manager) = node::detect_manager() else {
        anyhow::bail!(
            "no Node manager found. Install fnm (`brew install fnm`) or nvm, then re-run."
        );
    };
    println!("Installing Node {version} via {}…\n", manager.name());
    let ok = match manager {
        Manager::Fnm => run("fnm", &["install", version]),
        Manager::Nvm => {
            // nvm is a shell function, so source it in a login shell first.
            let script = node::nvm_script()
                .ok_or_else(|| anyhow::anyhow!("nvm.sh not found"))?;
            run(
                "bash",
                &["-lc", &format!(". {} && nvm install {}", script.display(), version)],
            )
        }
    };
    if !ok {
        anyhow::bail!("installing Node {version} via {} failed.", manager.name());
    }
    println!("\n✓ Node {version} installed. Pin it for a site with `dpl node use {version} <site>`.");
    Ok(())
}

/// The project directory for a site name, or the current directory when omitted.
fn project_dir(home: Option<&str>, site: Option<String>) -> Result<PathBuf> {
    match site {
        Some(name) => {
            let name = name.to_lowercase();
            let s = list_sites(home)?
                .into_iter()
                .find(|s| s.name == name)
                .ok_or_else(|| anyhow::anyhow!("no local site named {name}. See `dpl node`."))?;
            Ok(PathBuf::from(s.path))
        }
        None => std::env::current_dir().map_err(Into::into),
    }
}

/// Best-effort check that a manager already has `version`. Only fnm is a plain
/// binary; for nvm (a shell function) we don't probe and just stay quiet.
fn node_version_installed(version: &str) -> bool {
    if dpl_core::tools::which("fnm").is_some() {
        if let Ok(out) = std::process::Command::new("fnm").arg("ls").output() {
            return String::from_utf8_lossy(&out.stdout).contains(version);
        }
    }
    true
}

fn list_sites(home: Option<&str>) -> Result<Vec<SiteInfo>> {
    let Response::Sites { sites, .. } = daemon::call(Request::ListSites, home)? else {
        anyhow::bail!("unexpected daemon response");
    };
    Ok(sites)
}

fn run(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
