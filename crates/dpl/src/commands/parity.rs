//! `dpl parity` — diff a local `.test` site against the dply site it deploys to.
//!
//! The two halves of "it works on my machine" live in the same tool, so we can
//! answer the question nobody else can: *will this deploy behave like it does
//! here?* We compare the branch and repo, the PHP version and the extensions
//! the project demands, the environment **key names** (never their values), the
//! document root, and the databases the site expects.
//!
//! Values are deliberately never read or printed on either side — a parity
//! report is safe to paste into an issue.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dpl_dply::{endpoints as ep, models, DplyClient};
use serde_json::Value;

use crate::checks::{Checks, Report, Status};
use crate::daemon;
use dpl_core::ipc::{Request, Response};

const CATEGORIES: [&str; 4] = ["Project", "PHP", "Environment", "Services"];

/// Compare `site` (a linked local site) with its dply counterpart.
pub fn run(
    home: Option<&str>,
    host: Option<&str>,
    site: Option<String>,
    remote: Option<String>,
    json: bool,
) -> Result<()> {
    let local = resolve_local(home, site)?;
    let remote_name = remote.unwrap_or_else(|| local.name.clone());

    let client = DplyClient::for_active(host, home.map(str::to_string))
        .context("resolving dply client (are you logged in? `dpl login`)")?;

    let remote_site = ep::sites::show(&client, &remote_name).with_context(|| {
        format!("no dply site named `{remote_name}` (list them with `dpl dply sites:list`, or pass --remote)")
    })?;

    let report = build(&client, &local, &remote_name, &remote_site);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    report.render();
    Ok(())
}

/// A linked local site: where it lives and what PHP it's served with.
pub struct LocalSite {
    pub name: String,
    pub path: PathBuf,
    pub docroot: PathBuf,
    pub php: Option<String>,
}

/// Find the local site by name, else by the current directory's project path.
fn resolve_local(home: Option<&str>, site: Option<String>) -> Result<LocalSite> {
    let cfg_path = dpl_core::paths::local_config(home)?;
    let cfg = dpl_core::config::LocalConfig::load(&cfg_path)?;
    let default_php = cfg.default_php.clone().or_else(dpl_core::php::default_version);

    let name = match site {
        Some(n) => n.to_lowercase(),
        None => {
            let cwd = std::env::current_dir()?;
            cfg.links
                .iter()
                .find(|(_, link)| link.path == cwd)
                .map(|(name, _)| name.clone())
                .with_context(|| {
                    format!("{} isn't a linked site — pass a site name, e.g. `dpl parity my-app`", cwd.display())
                })?
        }
    };

    let link = cfg
        .links
        .get(&name)
        .with_context(|| format!("no linked site named `{name}` (parity compares linked projects)"))?;

    // Prefer the daemon's view of the docroot; fall back to the usual layout.
    let docroot = daemon_docroot(home, &name).unwrap_or_else(|| {
        let public = link.path.join("public");
        if public.is_dir() { public } else { link.path.clone() }
    });

    Ok(LocalSite {
        name,
        path: link.path.clone(),
        docroot,
        php: link.php.clone().or(default_php),
    })
}

fn daemon_docroot(home: Option<&str>, name: &str) -> Option<PathBuf> {
    match daemon::call(Request::ListSites, home) {
        Ok(Response::Sites { sites, .. }) => sites
            .iter()
            .find(|s| s.name == name)
            .filter(|s| !s.docroot.is_empty())
            .map(|s| PathBuf::from(&s.docroot)),
        _ => None,
    }
}

/// Assemble every parity check.
fn build(client: &DplyClient, local: &LocalSite, remote_name: &str, remote: &Value) -> Report {
    let mut c = Checks::default();

    check_project(&mut c, local, remote_name, remote);
    check_php(&mut c, local, remote);
    check_env(&mut c, client, local, remote_name);
    check_services(&mut c, client, remote_name);

    let subtitle = format!(
        "local {}.test  ↔  dply {remote_name} on {}",
        local.name,
        models::cell_of(remote, &["server_name", "server_id"])
    );
    Report::new(&format!("dpl parity {}", local.name), Some(subtitle), &CATEGORIES, c.0)
}

// ---- Project ----------------------------------------------------------------

fn check_project(c: &mut Checks, local: &LocalSite, remote_name: &str, remote: &Value) {
    let cat = "Project";

    c.add(
        "parity.remote",
        cat,
        "Remote site",
        Status::Info,
        format!(
            "{remote_name} ({}) — {}",
            models::cell_of(remote, &["type", "runtime"]),
            models::cell_of(remote, &["status"])
        ),
    );

    // The repo the server pulls from should be the repo you're standing in.
    let remote_repo = models::cell_of(remote, &["git_repository_url", "source.repo"]);
    match git(&local.path, &["remote", "get-url", "origin"]) {
        Some(origin) if !remote_repo.is_empty() => {
            if same_repo(&origin, &remote_repo) {
                c.add("parity.repo", cat, "Repository", Status::Pass, remote_repo);
            } else {
                c.add("parity.repo", cat, "Repository", Status::Fail, format!("local {origin} → remote {remote_repo}"));
                c.hint("This local project isn't the repo that site deploys from — you're comparing two different apps. Pass --remote to pick the right dply site.");
            }
        }
        Some(origin) => c.add("parity.repo", cat, "Repository", Status::Info, format!("local {origin}; remote doesn't report one")),
        None => c.add("parity.repo", cat, "Repository", Status::Info, "local project isn't a git checkout"),
    }

    // Deploying from `main` while you develop on a feature branch is the most
    // common "why isn't my change live" moment.
    let remote_branch = models::cell_of(remote, &["git_branch", "source.branch"]);
    match (git(&local.path, &["rev-parse", "--abbrev-ref", "HEAD"]), remote_branch.is_empty()) {
        (Some(branch), false) if branch == remote_branch => {
            c.add("parity.branch", cat, "Branch", Status::Pass, format!("both on {branch}"));
        }
        (Some(branch), false) => {
            c.add("parity.branch", cat, "Branch", Status::Warn, format!("local on {branch}, deploys from {remote_branch}"));
            c.hint("Work you do here won't ship until it lands on the deploy branch.");
        }
        (Some(branch), true) => c.add("parity.branch", cat, "Branch", Status::Info, format!("local on {branch}")),
        (None, _) => {}
    }

    // A mismatched docroot means the server serves a different entrypoint.
    let remote_root = models::cell_of(remote, &["document_root"]);
    if !remote_root.is_empty() {
        let local_leaf = leaf(&local.docroot);
        let remote_leaf = leaf(Path::new(&remote_root));
        if local_leaf == remote_leaf {
            c.add("parity.docroot", cat, "Document root", Status::Pass, format!("both serve {local_leaf}/"));
        } else {
            c.add("parity.docroot", cat, "Document root", Status::Warn, format!("local {local_leaf}/, remote {remote_leaf}/"));
            c.hint("The server serves a different directory than you do locally, so routing and asset paths can differ.");
        }
    }

    let deployed = models::cell_of(remote, &["last_deploy_at"]);
    if !deployed.is_empty() {
        c.add("parity.last_deploy", cat, "Last deploy", Status::Info, deployed);
    }
}

// ---- PHP --------------------------------------------------------------------

fn check_php(c: &mut Checks, local: &LocalSite, remote: &Value) {
    let cat = "PHP";
    let local_php = local.php.clone();
    let remote_php = {
        let v = models::cell_of(remote, &["runtime_version", "php_version"]);
        (!v.is_empty()).then_some(v)
    };

    match (&local_php, &remote_php) {
        (Some(l), Some(r)) if version(l) == version(r) => {
            c.add("parity.php_version", cat, "PHP version", Status::Pass, format!("both {r}"));
        }
        (Some(l), Some(r)) => {
            c.add("parity.php_version", cat, "PHP version", Status::Fail, format!("local {l}, remote {r}"));
            c.hint("You're developing on a different PHP than you ship on — syntax and behaviour differences surface only after deploy.");
            c.fix("Match the remote locally", &format!("dpl use {r} {}", local.name));
        }
        (Some(l), None) => {
            c.add("parity.php_version", cat, "PHP version", Status::Info, format!("local {l}; the remote doesn't report its version"));
            c.hint("dply returned no runtime_version for this site, so the versions can't be compared automatically.");
        }
        (None, Some(r)) => c.add("parity.php_version", cat, "PHP version", Status::Warn, format!("remote {r}; no local PHP resolved")),
        (None, None) => {}
    }

    // composer.json is the project's own statement of what it needs.
    let composer = read_json(&local.path.join("composer.json"));
    let Some(composer) = composer else { return };
    let require = composer.get("require").and_then(Value::as_object);
    let Some(require) = require else { return };

    if let Some(constraint) = require.get("php").and_then(Value::as_str) {
        let needs = min_version(constraint);
        let judge = |label: &str, have: &Option<String>| -> Option<(Status, String)> {
            let have = have.as_deref()?;
            let (hv, need) = (version(have)?, needs?);
            Some(if hv >= need {
                (Status::Pass, format!("{label} {have} satisfies {constraint}"))
            } else {
                (Status::Fail, format!("{label} {have} is below the required {constraint}"))
            })
        };
        if let Some((status, detail)) = judge("remote", &remote_php).or_else(|| judge("local", &local_php)) {
            c.add("parity.composer_php", cat, "composer.json php", status, detail);
            if status == Status::Fail {
                c.hint("The environment can't run this project's own declared minimum — Composer will refuse, or the app will fatal.");
            }
        }
    }

    // Extensions the project requires, versus what your local PHP actually loads.
    // We can't introspect the server's modules over the API, so this is a local
    // check with a remote caveat.
    let required: Vec<String> = require
        .keys()
        .filter_map(|k| k.strip_prefix("ext-"))
        .map(|e| e.to_lowercase())
        .collect();
    if required.is_empty() {
        return;
    }
    let loaded = loaded_extensions(local.php.as_deref());
    let missing: Vec<&String> = required.iter().filter(|e| !loaded.contains(*e)).collect();
    if missing.is_empty() {
        c.add(
            "parity.extensions",
            cat,
            "Required extensions",
            Status::Pass,
            format!("all {} loaded locally: {}", required.len(), required.join(", ")),
        );
        c.hint("dply doesn't expose the server's module list, so this confirms the local side only.");
    } else {
        let names: Vec<String> = missing.iter().map(|s| s.to_string()).collect();
        c.add(
            "parity.extensions",
            cat,
            "Required extensions",
            Status::Fail,
            format!("not loaded locally: {}", names.join(", ")),
        );
        c.hint("composer.json requires these, so the app can fatal locally even though it runs on the server.");
        if let Some(v) = &local.php {
            c.fix("Install the first one", &format!("dpl php ext-install {v} {}", names[0]));
        }
    }
}

// ---- Environment ------------------------------------------------------------

/// Compare `.env` **key names** against the remote's. Values are never read.
fn check_env(c: &mut Checks, client: &DplyClient, local: &LocalSite, remote_name: &str) {
    let cat = "Environment";

    let local_keys = env_file_keys(&local.path.join(".env"));
    let remote_keys = match ep::site::env_list(client, remote_name) {
        Ok(v) => remote_env_keys(&v),
        Err(e) => {
            c.add("parity.env", cat, "Environment", Status::Info, format!("couldn't read the remote env: {e}"));
            return;
        }
    };

    if local_keys.is_empty() {
        c.add("parity.env", cat, "Local .env", Status::Info, format!("no .env in {}", local.path.display()));
        return;
    }
    c.add(
        "parity.env",
        cat,
        "Keys compared",
        Status::Info,
        format!("{} local, {} remote (names only — no values are read)", local_keys.len(), remote_keys.len()),
    );

    // Keys you rely on locally that the server has never heard of: the classic
    // "works here, 500s in production".
    let missing: Vec<&String> = local_keys.difference(&remote_keys).collect();
    if missing.is_empty() {
        c.add("parity.env_missing", cat, "Missing on the remote", Status::Pass, "none — every local key exists remotely");
    } else {
        let names: Vec<String> = missing.iter().map(|s| s.to_string()).collect();
        c.add("parity.env_missing", cat, "Missing on the remote", Status::Fail, names.join(", "));
        c.hint("Your app reads these locally; on the server they resolve to null. Anything gated on them silently changes behaviour.");
        // A placeholder command — the GUI copies it rather than running it, so
        // we can never write a literal `<value>` into someone's production env.
        c.fix(
            "Copy the set command",
            &format!("dpl dply site:env {remote_name} --set {}=<value>", names[0]),
        );
    }

    // The reverse: configuration the server has that you've never exercised.
    let extra: Vec<&String> = remote_keys.difference(&local_keys).collect();
    if !extra.is_empty() {
        let names: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        c.add("parity.env_extra", cat, "Only on the remote", Status::Warn, names.join(", "));
        c.hint("Code paths driven by these keys never run on your machine. Add them to .env (any value) to exercise them.");
    }
}

// ---- Services ---------------------------------------------------------------

fn check_services(c: &mut Checks, client: &DplyClient, remote_name: &str) {
    let cat = "Services";
    let Ok(value) = ep::sites::databases(client, remote_name) else { return };
    let rows = models::rows(&value);
    if rows.is_empty() {
        return;
    }
    let names: Vec<String> = rows.iter().map(|r| models::cell_of(r, &["name", "database"])).filter(|n| !n.is_empty()).collect();
    c.add(
        "parity.databases",
        cat,
        "Remote databases",
        Status::Info,
        if names.is_empty() { format!("{} attached", rows.len()) } else { names.join(", ") },
    );
    c.hint("Compare against your local Services panel — a database the app expects but you don't run locally hides migration bugs.");
}

// ---- Helpers ----------------------------------------------------------------

/// `KEY` names from a dotenv file. Values are parsed past, never retained.
fn env_file_keys(path: &Path) -> BTreeSet<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeSet::new() };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(key, _value)| key.trim().trim_start_matches("export ").trim().to_string())
        .filter(|k| !k.is_empty())
        .collect()
}

/// Key names from dply's env payload (`[{key, value, scope}]` or a map).
fn remote_env_keys(value: &Value) -> BTreeSet<String> {
    if let Some(map) = value.as_object() {
        // A bare `{KEY: value}` map — take the keys, ignore the values.
        if !map.contains_key("key") {
            return map.keys().cloned().collect();
        }
    }
    models::rows(value)
        .iter()
        .map(|row| models::cell_of(row, &["key", "name"]))
        .filter(|k| !k.is_empty())
        .collect()
}

/// Extensions the given PHP loads, lowercased (`php -m`).
fn loaded_extensions(version: Option<&str>) -> BTreeSet<String> {
    let binary = version
        .and_then(dpl_core::php::resolve)
        .unwrap_or_else(dpl_core::php::default_binary);
    let Ok(out) = std::process::Command::new(binary).arg("-m").output() else { return BTreeSet::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Do two git URLs name the same repo? Ignores scheme, `.git`, and SSH form.
fn same_repo(a: &str, b: &str) -> bool {
    normalize_repo(a) == normalize_repo(b)
}

fn normalize_repo(url: &str) -> String {
    let u = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let u = u.split("://").last().unwrap_or(u);
    let u = u.rsplit('@').next().unwrap_or(u); // git@github.com:x/y → github.com:x/y
    u.replace(':', "/").to_lowercase()
}

/// The last path component (`/home/dply/app/public` → `public`).
fn leaf(path: &Path) -> String {
    path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// `(major, minor)` of a version string.
fn version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().trim_start_matches("php").trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|p| p.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// The minimum `(major, minor)` a Composer constraint accepts (`^8.2` → 8.2).
fn min_version(constraint: &str) -> Option<(u32, u32)> {
    let digits: String = constraint
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    version(&digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_env_key_names_without_values() {
        let dir = std::env::temp_dir().join("dpl-parity-env-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, "# comment\nAPP_KEY=base64:secret\n\nexport DB_PASSWORD = hunter2\nBAD_LINE\n").unwrap();
        let keys = env_file_keys(&path);
        assert!(keys.contains("APP_KEY"));
        assert!(keys.contains("DB_PASSWORD"));
        assert!(!keys.contains("BAD_LINE"));
        assert_eq!(keys.len(), 2);
        // The values never leave the parser.
        assert!(!keys.iter().any(|k| k.contains("hunter2") || k.contains("secret")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remote_env_keys_from_either_shape() {
        let list = serde_json::json!([{"key": "APP_ENV", "value": "production"}, {"key": "APP_DEBUG"}]);
        let keys = remote_env_keys(&list);
        assert!(keys.contains("APP_ENV") && keys.contains("APP_DEBUG") && keys.len() == 2);

        let map = serde_json::json!({"APP_ENV": "production", "QUEUE_CONNECTION": "redis"});
        let keys = remote_env_keys(&map);
        assert!(keys.contains("APP_ENV") && keys.contains("QUEUE_CONNECTION"));
    }

    #[test]
    fn compares_repo_urls_across_forms() {
        assert!(same_repo("git@github.com:shaferllc/lookout.git", "https://github.com/shaferllc/lookout.git"));
        assert!(same_repo("https://github.com/a/b", "https://github.com/a/b.git"));
        assert!(!same_repo("git@github.com:a/b.git", "https://github.com/a/c.git"));
    }

    #[test]
    fn parses_versions_and_constraints() {
        assert_eq!(version("8.3"), Some((8, 3)));
        assert_eq!(version("php8.2"), Some((8, 2)));
        assert_eq!(version("8.4.1"), Some((8, 4)));
        assert_eq!(min_version("^8.2"), Some((8, 2)));
        assert_eq!(min_version(">=7.4.0"), Some((7, 4)));
    }
}
