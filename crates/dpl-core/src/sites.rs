//! Resolving the [`LocalConfig`](crate::config::LocalConfig) into the concrete
//! set of sites the daemon should serve.
//!
//! Two sources feed the list, yerd-style:
//! - **parked** directories: every immediate subdirectory becomes a site named
//!   after the folder (`~/Sites/blog` → `blog.test`);
//! - **linked** projects: an explicit name → path mapping.
//!
//! Links win over a parked directory of the same name. The document root is the
//! project's `public/` subdirectory when present (Laravel/Symfony/most modern
//! PHP apps), else the project root.

use std::path::{Path, PathBuf};

use crate::config::LocalConfig;

/// Detect a project's framework/type from its `composer.json` (or well-known
/// files) — e.g. `Laravel (^12)`, `Symfony`, `WordPress`, `Drupal`. Returns a
/// display string, or `None` if it doesn't look like a PHP project.
pub fn detect_framework(project: &Path) -> Option<String> {
    let composer = project.join("composer.json");
    if let Ok(text) = std::fs::read_to_string(&composer) {
        // Lightweight, dependency-free scan: find `"pkg"` then the quoted value
        // after the following colon (its version constraint).
        let ver = |pkg: &str| -> Option<String> {
            let key = format!("\"{pkg}\"");
            let after = &text[text.find(&key)? + key.len()..];
            let rest = &after[after.find(':')? + 1..];
            let q1 = rest.find('"')?;
            let q2 = rest[q1 + 1..].find('"')?;
            Some(rest[q1 + 1..=q1 + q2].to_string())
        };
        if let Some(v) = ver("laravel/framework") {
            return Some(format!("Laravel ({v})"));
        }
        if let Some(v) = ver("symfony/framework-bundle").or_else(|| ver("symfony/symfony")) {
            return Some(format!("Symfony ({v})"));
        }
        if let Some(v) = ver("tempest/framework") {
            return Some(format!("Tempest ({v})"));
        }
        if ver("statamic/cms").is_some() {
            return Some("Statamic".into());
        }
        if ver("craftcms/cms").is_some() {
            return Some("Craft".into());
        }
        if ver("drupal/core").is_some() || ver("drupal/core-recommended").is_some() {
            return Some("Drupal".into());
        }
        if ver("slim/slim").is_some() {
            return Some("Slim".into());
        }
        if ver("cakephp/cakephp").is_some() {
            return Some("CakePHP".into());
        }
        return Some("PHP (Composer)".into());
    }
    if project.join("wp-config.php").is_file() || project.join("wp-load.php").is_file() {
        return Some("WordPress".into());
    }
    if project.join("index.php").is_file() {
        return Some("PHP".into());
    }
    None
}

/// The PHP version constraint a project requires (composer.json `require.php`,
/// e.g. `"^8.3"`) — used to flag PHP compatibility / suggest per-site isolation.
pub fn detect_required_php(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("composer.json")).ok()?;
    // Find the `require` block, then the `php` constraint within it.
    let after = &text[text.find("\"require\"")?..];
    let rest = &after[after.find("\"php\"")? + 5..];
    let val = &rest[rest.find(':')? + 1..];
    let q1 = val.find('"')?;
    let q2 = val[q1 + 1..].find('"')?;
    Some(val[q1 + 1..=q1 + q2].to_string())
}

/// A fully-resolved site ready to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSite {
    /// Site name — the `<name>` in `<name>.test`.
    pub name: String,
    /// Project root on disk.
    pub path: PathBuf,
    /// Directory to serve from (`public/` if it exists, else `path`).
    pub docroot: PathBuf,
    /// Pinned PHP version, if any.
    pub php: Option<String>,
    /// Whether this site should be served over HTTPS.
    pub secure: bool,
    /// Where the site came from.
    pub source: SiteSource,
    /// Primary TLD for this site's canonical host/URL.
    pub tld: String,
    /// Application-server runtime (`None`/`"fpm"` = php-fpm; else an Octane
    /// server the daemon supervises + proxies).
    pub runtime: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteSource {
    Parked,
    Linked,
}

impl SiteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SiteSource::Parked => "parked",
            SiteSource::Linked => "linked",
        }
    }
}

impl ResolvedSite {
    /// The hostname this site answers on, e.g. `blog.test`.
    pub fn host(&self) -> String {
        format!("{}.{}", self.name, self.tld)
    }

    /// The browser URL, honouring the secure flag.
    pub fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", self.host())
    }
}

/// Choose a site's document root: `public/` when it exists, else the root.
pub fn docroot_for(path: &Path) -> PathBuf {
    let public = path.join("public");
    if public.is_dir() {
        public
    } else {
        path.to_path_buf()
    }
}

/// Derive a site name from a directory path (lowercased basename).
pub fn name_for(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
}

/// Expand a config into the ordered, de-duplicated list of sites to serve.
/// Linked sites take precedence over a parked directory of the same name.
pub fn resolve(config: &LocalConfig) -> Vec<ResolvedSite> {
    let mut sites: Vec<ResolvedSite> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    let tld = config.primary_tld();

    // Links first so they win on name collisions.
    for (name, link) in &config.links {
        let name = name.to_lowercase();
        if !seen.insert(name.clone()) {
            continue;
        }
        sites.push(ResolvedSite {
            name,
            docroot: docroot_for(&link.path),
            path: link.path.clone(),
            php: link.php.clone().or_else(|| config.default_php.clone()),
            secure: link.secure,
            source: SiteSource::Linked,
            tld: tld.clone(),
            runtime: link.runtime.clone(),
        });
    }

    // Then each parked directory's immediate subdirectories.
    for parked in &config.parked {
        let Ok(entries) = std::fs::read_dir(parked) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = name_for(&path) else { continue };
            if name.starts_with('.') || !seen.insert(name.clone()) {
                continue;
            }
            sites.push(ResolvedSite {
                name,
                docroot: docroot_for(&path),
                path,
                php: config.default_php.clone(),
                secure: false,
                source: SiteSource::Parked,
                tld: tld.clone(),
                runtime: None,
            });
        }
    }

    sites.sort_by(|a, b| a.name.cmp(&b.name));
    sites
}
