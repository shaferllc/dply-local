//! `dpl node` — per-project Node version management, delegated to fnm/nvm.
//!
//! dpl writes each repo's `.nvmrc` (the pin fnm and nvm auto-switch on) and reads
//! a project's desired version back from `.nvmrc`/`.node-version`/`package.json`.
//! Installing a version is handed to whichever manager is present. dpl never runs
//! `node` itself, so the actual per-`cd` switch is the manager's job.
//!
//! It does run your *package manager*, though: `install`, `run`, and `exec` fan
//! one command out across every site that has a `package.json`, each in its own
//! directory, under its own Node pin, through its own agent (npm/pnpm/yarn/bun).

use std::path::{Path, PathBuf};

use anyhow::Result;
use dpl_core::ipc::{Request, Response, SiteInfo};
use dpl_core::node::{self, Agent, AgentChoice, Manager, Pin};

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
    println!("Run npm across every site with `dpl node npm <args…>`, e.g. `dpl node npm ci`.");
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

/// Which sites a fan-out touches and how it handles failure — shared by
/// `deps`, `run`, `exec`, and `npm`.
pub struct Fan {
    /// Limit to one linked site; `None` fans out across all of them.
    pub site: Option<String>,
    /// Force a package manager instead of detecting one per site.
    pub agent: Option<String>,
    /// Stop at the first site that fails.
    pub fail_fast: bool,
}

impl From<crate::cli::FanArgs> for Fan {
    fn from(args: crate::cli::FanArgs) -> Self {
        Fan { site: args.site, agent: args.agent, fail_fast: args.fail_fast }
    }
}

/// What to run in each site. The words differ per agent, so the job is resolved
/// against each site's own agent rather than fixed up front.
pub enum Job {
    /// Install dependencies from the lockfile (`frozen`: refuse to update it).
    Install { frozen: bool },
    /// Run a package.json script, plus any arguments for the script itself.
    Script { name: String, extra: Vec<String> },
    /// Hand these words to the agent verbatim.
    Verbatim(Vec<String>),
}

impl Job {
    /// The agent's arguments for this job.
    fn args(&self, choice: &AgentChoice) -> Vec<String> {
        match self {
            Job::Install { frozen } => node::install_args(choice, *frozen),
            Job::Script { name, extra } => node::run_args(name, extra),
            Job::Verbatim(args) => args.clone(),
        }
    }

    /// How the job reads in the header, before any site-specific agent is known.
    fn label(&self) -> String {
        match self {
            Job::Install { frozen: false } => "install".into(),
            Job::Install { frozen: true } => "install (frozen lockfile)".into(),
            Job::Script { name, extra } if extra.is_empty() => format!("run {name}"),
            Job::Script { name, extra } => format!("run {name} {}", extra.join(" ")),
            Job::Verbatim(args) => args.join(" "),
        }
    }
}

/// `dpl node deps` / `run` / `exec` / `npm` — run one job in every linked
/// site that has a `package.json`.
///
/// The point is the fan-out: `dpl node deps` after a pull, `dpl node run
/// build` before a demo, without walking the tree by hand. Each site runs in its
/// own directory, under its own pinned Node version, through the package manager
/// that site actually uses — a pnpm repo and a bun repo in the same fleet each
/// get the right tool, because `npm install` in a pnpm project is a mess to
/// undo. Sites run one at a time: package managers are heavy on disk and
/// network, and interleaved output from twenty installs is unreadable.
pub fn fan_out(home: Option<&str>, fan: Fan, job: Job) -> Result<()> {
    let forced = match fan.agent.as_deref() {
        None => None,
        Some(name) => Some(Agent::parse(name).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown package manager `{name}` — expected one of {}.",
                Agent::ALL.map(|a| a.as_str()).join(", ")
            )
        })?),
    };

    let named = fan.site.clone();
    let (targets, skipped) = npm_targets(home, fan.site)?;
    if targets.is_empty() {
        match named {
            Some(name) => anyhow::bail!("{name} has no package.json — nothing to run."),
            None => anyhow::bail!(
                "no linked site has a package.json ({skipped} site{} checked).",
                if skipped == 1 { "" } else { "s" }
            ),
        }
    }

    let manager = node::detect_manager();
    let nvm_script = node::nvm_script();
    let label = job.label();

    // Resolve every site's agent up front so the header can say what the run is
    // about to touch — "12 sites" reads very differently once you know three of
    // them are pnpm.
    let plan: Vec<(String, PathBuf, AgentChoice)> = targets
        .into_iter()
        .map(|(name, dir)| {
            let choice = match forced {
                // A forced agent still needs the repo's yarn flavour, since
                // `--agent yarn` says nothing about berry vs classic flags.
                Some(agent) => AgentChoice {
                    agent,
                    reason: dpl_core::node::AgentReason::Forced,
                    berry: node::detect_agent(&dir).berry,
                },
                None => node::detect_agent(&dir),
            };
            (name, dir, choice)
        })
        .collect();

    println!(
        "{label} — {} site{}{}{}\n",
        plan.len(),
        if plan.len() == 1 { "" } else { "s" },
        agent_tally(&plan),
        if skipped > 0 { format!(", {skipped} without a package.json skipped") } else { String::new() },
    );

    let mut failed: Vec<String> = Vec::new();
    for (name, dir, choice) in &plan {
        let pin = node::read_pin(dir);
        println!(
            "── {name}  {} ({})  {}  [{}]",
            choice.agent.as_str(),
            choice.reason.as_str(),
            pin_label(pin.as_ref()),
            dir.display(),
        );

        let args = job.args(choice);
        let inv = node::pinned_invocation(
            manager,
            nvm_script.as_deref(),
            pin.as_ref(),
            choice.agent.as_str(),
            &args,
        );
        let status = std::process::Command::new(&inv.program)
            .args(&inv.args)
            .current_dir(dir)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                println!("   ✗ {name}: {} exited {}", choice.agent.as_str(), s.code().unwrap_or(-1));
                failed.push(name.clone());
            }
            // Worth naming outright: a package manager missing from a bare PATH
            // will fail identically for every remaining site, and "pnpm isn't
            // installed" is a different problem from "the build broke".
            Err(e) => {
                println!("   ✗ {name}: could not run {}: {e}", inv.program);
                failed.push(name.clone());
            }
        }
        println!();

        if fan.fail_fast && !failed.is_empty() {
            anyhow::bail!("stopped at {name} (--fail-fast).");
        }
    }

    if failed.is_empty() {
        match plan.len() {
            1 => println!("✓ {label} succeeded in {}.", plan[0].0),
            n => println!("✓ {label} succeeded in all {n} sites."),
        }
        return Ok(());
    }
    anyhow::bail!("{} of {} sites failed: {}", failed.len(), plan.len(), failed.join(", "));
}

/// `dpl node scripts [site]` — the package.json scripts each site can run.
pub fn scripts(home: Option<&str>, site: Option<String>, json: bool) -> Result<()> {
    let (targets, _) = npm_targets(home, site)?;

    if json {
        let rows: Vec<serde_json::Value> = targets
            .iter()
            .map(|(name, dir)| {
                let choice = node::detect_agent(dir);
                serde_json::json!({
                    "name": name,
                    "path": dir,
                    "agent": choice.agent.as_str(),
                    "agent_source": choice.reason.as_str(),
                    "scripts": node::read_scripts(dir),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "sites": rows }))?);
        return Ok(());
    }

    if targets.is_empty() {
        println!("No linked site has a package.json.");
        return Ok(());
    }
    for (name, dir) in &targets {
        let choice = node::detect_agent(dir);
        let scripts = node::read_scripts(dir);
        println!("{name}  ({})", choice.agent.as_str());
        if scripts.is_empty() {
            println!("    (no scripts)");
        } else {
            println!("    {}", scripts.join("  "));
        }
    }
    println!("\nRun one everywhere with `dpl node run <script>`.");
    Ok(())
}

/// A parenthesised count per agent (`, npm 8 · pnpm 3`), or nothing when the
/// whole fleet agrees — a tally of one tells you nothing.
fn agent_tally(plan: &[(String, PathBuf, AgentChoice)]) -> String {
    let mut counts: Vec<(Agent, usize)> = Vec::new();
    for (_, _, choice) in plan {
        match counts.iter_mut().find(|(a, _)| *a == choice.agent) {
            Some((_, n)) => *n += 1,
            None => counts.push((choice.agent, 1)),
        }
    }
    if counts.len() < 2 {
        return counts.first().map(|(a, _)| format!(", {}", a.as_str())).unwrap_or_default();
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.as_str().cmp(b.0.as_str())));
    let parts: Vec<String> =
        counts.iter().map(|(a, n)| format!("{} {n}", a.as_str())).collect();
    format!(", {}", parts.join(" · "))
}

/// The sites a fan-out will run in, plus how many were passed over for having no
/// `package.json` — worth reporting so a surprisingly short run doesn't look
/// like a lookup bug.
fn npm_targets(
    home: Option<&str>,
    site: Option<String>,
) -> Result<(Vec<(String, PathBuf)>, usize)> {
    let candidates: Vec<SiteInfo> = match site {
        Some(name) => {
            let name = name.to_lowercase();
            vec![list_sites(home)?
                .into_iter()
                .find(|s| s.name == name)
                .ok_or_else(|| anyhow::anyhow!("no local site named {name}. See `dpl node`."))?]
        }
        // Proxies point at something dpl doesn't own, so there is no repo to
        // run a package manager in.
        None => list_sites(home)?.into_iter().filter(|s| s.source != "proxy").collect(),
    };

    let total = candidates.len();
    let targets: Vec<(String, PathBuf)> = candidates
        .into_iter()
        .map(|s| (s.name, PathBuf::from(s.path)))
        .filter(|(_, dir)| dir.join("package.json").is_file())
        .collect();
    let skipped = total - targets.len();
    Ok((targets, skipped))
}

fn pin_label(pin: Option<&Pin>) -> String {
    match pin {
        Some(Pin { version, source }) => format!("Node {version} ({})", source.as_str()),
        None => "Node (unpinned)".to_string(),
    }
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
