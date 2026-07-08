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
            });
        }
    }

    sites.sort_by(|a, b| a.name.cmp(&b.name));
    sites
}
