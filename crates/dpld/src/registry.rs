//! The daemon's live view of local sites: the persisted [`LocalConfig`], the
//! php-fpm masters, and the mutations the CLI drives (park / link / unpark /
//! unlink / secure / use-php / reload).
//!
//! Requests are served via php-fpm (see [`crate::fpm`]) + FastCGI rather than
//! `php -S`, so a warm worker pool handles each request. One master per PHP
//! binary serves every site on that version; [`Registry::resolve_request`]
//! gives the proxy the document root + FastCGI address for a host.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use dpl_core::config::LocalConfig;
use dpl_core::ipc::SiteInfo;
use dpl_core::sites::{self, ResolvedSite};

use crate::fpm::FpmManager;

/// Everything the proxy needs to serve one request for a site.
pub struct SiteRoute {
    pub name: String,
    pub docroot: PathBuf,
    pub fpm_addr: SocketAddr,
    pub secure: bool,
}

/// Per-site routing data computed at reconcile time.
struct RouteInfo {
    docroot: PathBuf,
    php_bin: PathBuf,
    secure: bool,
}

pub struct Registry {
    config: LocalConfig,
    config_path: PathBuf,
    fpm: FpmManager,
    routes: BTreeMap<String, RouteInfo>,
    /// Configured TLDs (cached from config for the request hot path).
    tlds: Vec<String>,
}

impl Registry {
    pub fn load() -> Result<Self> {
        let config_path = dpl_core::paths::local_config(None)?;
        let config = LocalConfig::load(&config_path)?;
        let tlds = config.tlds();
        Ok(Registry {
            config,
            config_path,
            fpm: FpmManager::new(),
            routes: BTreeMap::new(),
            tlds,
        })
    }

    /// The primary TLD (for building canonical URLs).
    pub fn primary_tld(&self) -> String {
        self.config.primary_tld()
    }

    /// If `host` (already lowercased, port-stripped) ends with a configured
    /// TLD, return the left-most label as the site name.
    pub fn site_for_host(&self, host: &str) -> Option<String> {
        for tld in &self.tlds {
            let suffix = format!(".{tld}");
            if let Some(prefix) = host.strip_suffix(&suffix) {
                if !prefix.is_empty() {
                    return Some(prefix.split('.').next().unwrap_or(prefix).to_string());
                }
            }
        }
        None
    }

    fn save(&self) -> Result<()> {
        self.config.save(&self.config_path).context("saving local config")
    }

    /// Give the proxy the route for a host's site, if it's currently servable.
    pub fn resolve_request(&self, site_name: &str) -> Option<SiteRoute> {
        let route = self.routes.get(site_name)?;
        let fpm_addr = self.fpm.addr_for(&route.php_bin)?;
        Some(SiteRoute {
            name: site_name.to_string(),
            docroot: route.docroot.clone(),
            fpm_addr,
            secure: route.secure,
        })
    }

    pub fn site_infos(&self) -> Vec<SiteInfo> {
        sites::resolve(&self.config)
            .into_iter()
            .map(|s| {
                let serving = self.fpm.addr_for(&php_bin_for(&s)).is_some();
                SiteInfo {
                    host: s.host(),
                    url: s.url(),
                    name: s.name,
                    path: s.path.to_string_lossy().into_owned(),
                    docroot: s.docroot.to_string_lossy().into_owned(),
                    source: s.source.as_str().to_string(),
                    php: s.php,
                    secure: s.secure,
                    serving,
                }
            })
            .collect()
    }

    /// Names of sites that want HTTPS — used by the TLS listener to know which
    /// certs to have ready.
    pub fn secure_site_names(&self) -> Vec<String> {
        self.routes
            .iter()
            .filter(|(_, r)| r.secure)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Start php-fpm masters for every PHP version in use, stop unused ones,
    /// and rebuild the routing table. Returns the number of servable sites.
    pub fn reconcile(&mut self) -> usize {
        self.tlds = self.config.tlds();
        let resolved = sites::resolve(&self.config);

        // Ensure a master per distinct PHP binary; drop the rest.
        let mut needed: BTreeSet<PathBuf> = BTreeSet::new();
        for site in &resolved {
            let bin = php_bin_for(site);
            if self.fpm.ensure(&bin).is_ok() {
                needed.insert(bin);
            }
        }
        self.fpm.retain(&needed);

        // Rebuild routes.
        self.routes.clear();
        for site in &resolved {
            self.routes.insert(
                site.name.clone(),
                RouteInfo {
                    docroot: site.docroot.clone(),
                    php_bin: php_bin_for(site),
                    secure: site.secure,
                },
            );
        }

        self.routes
            .values()
            .filter(|r| self.fpm.addr_for(&r.php_bin).is_some())
            .count()
    }

    // ---- mutations (each persists + reconciles) ----

    pub fn park(&mut self, path: &str) -> Result<String> {
        let path = canonicalize(path)?;
        if self.config.parked.contains(&path) {
            return Ok(format!("{} is already parked.", path.display()));
        }
        self.config.parked.push(path.clone());
        self.save()?;
        let n = self.reconcile();
        Ok(format!("Parked {}. Serving {n} site(s).", path.display()))
    }

    pub fn unpark(&mut self, path: &str) -> Result<String> {
        let path = canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
        let before = self.config.parked.len();
        self.config.parked.retain(|p| p != &path);
        if self.config.parked.len() == before {
            return Ok(format!("{} was not parked.", path.display()));
        }
        self.save()?;
        self.reconcile();
        Ok(format!("Unparked {}.", path.display()))
    }

    pub fn link(&mut self, name: Option<&str>, path: &str) -> Result<String> {
        let path = canonicalize(path)?;
        let name = match name {
            Some(n) => n.to_lowercase(),
            None => sites::name_for(&path).context("could not derive a site name from the path")?,
        };
        self.config.links.insert(
            name.clone(),
            dpl_core::config::Link { path: path.clone(), php: None, secure: false },
        );
        self.save()?;
        self.reconcile();
        Ok(format!("Linked {} → {}.test", path.display(), name))
    }

    pub fn unlink(&mut self, name: &str) -> Result<String> {
        let name = name.to_lowercase();
        if self.config.links.remove(&name).is_none() {
            return Ok(format!("No linked site named {name}."));
        }
        self.save()?;
        self.reconcile();
        Ok(format!("Unlinked {name}."))
    }

    pub fn use_php(&mut self, version: &str, site: Option<&str>) -> Result<String> {
        if dpl_core::php::resolve(version).is_none() {
            let have: Vec<String> = dpl_core::php::detect().into_iter().map(|p| p.version).collect();
            anyhow::bail!(
                "PHP {version} not found. Installed: {}",
                if have.is_empty() { "none".into() } else { have.join(", ") }
            );
        }
        match site {
            Some(name) => {
                let name = name.to_lowercase();
                let link = self
                    .config
                    .links
                    .get_mut(&name)
                    .with_context(|| format!("{name} is not linked (php pinning applies to linked sites)"))?;
                link.php = Some(version.to_string());
                self.save()?;
                self.reconcile();
                Ok(format!("{name}.test now uses PHP {version}."))
            }
            None => {
                self.config.default_php = Some(version.to_string());
                self.save()?;
                self.reconcile();
                Ok(format!("Default PHP set to {version}."))
            }
        }
    }

    pub fn reload(&mut self) -> Result<String> {
        self.config = LocalConfig::load(&self.config_path)?;
        let n = self.reconcile();
        Ok(format!("Reloaded. Serving {n} site(s)."))
    }

    /// Current TLDs (primary first).
    pub fn tld_list(&self) -> Vec<String> {
        self.config.tlds()
    }

    /// Add a TLD. Reminds the caller it needs the resolver installed.
    pub fn tld_add(&mut self, tld: &str) -> Result<String> {
        let tld = normalize_tld(tld)?;
        let mut tlds = self.config.tlds();
        if tlds.iter().any(|t| t == &tld) {
            return Ok(format!(".{tld} is already configured."));
        }
        tlds.push(tld.clone());
        self.config.tlds = tlds;
        self.save()?;
        self.reconcile();
        Ok(format!(
            "Added .{tld}. Run `dpl setup` (or `sudo dpl-helper install-resolver {tld} 5333`) so it resolves."
        ))
    }

    /// Remove a TLD (can't remove the last one).
    pub fn tld_remove(&mut self, tld: &str) -> Result<String> {
        let tld = normalize_tld(tld)?;
        let mut tlds = self.config.tlds();
        if tlds.len() == 1 {
            anyhow::bail!("can't remove the only TLD (.{}).", tlds[0]);
        }
        let before = tlds.len();
        tlds.retain(|t| t != &tld);
        if tlds.len() == before {
            return Ok(format!(".{tld} was not configured."));
        }
        self.config.tlds = tlds;
        self.save()?;
        self.reconcile();
        Ok(format!("Removed .{tld}."))
    }

    pub fn set_secure(&mut self, name: &str, secure: bool) -> Result<String> {
        let name = name.to_lowercase();
        match self.config.links.get_mut(&name) {
            Some(link) => {
                link.secure = secure;
                self.save()?;
                self.reconcile();
                Ok(format!(
                    "{name}.test is now served over {}.",
                    if secure { "HTTPS" } else { "HTTP" }
                ))
            }
            None => Ok(format!(
                "secure currently applies to linked sites only; {name} is not linked."
            )),
        }
    }
}

/// Normalize a TLD: strip a leading dot, lowercase, validate it's a simple
/// label so it can't break `/etc/resolver/<tld>`.
fn normalize_tld(tld: &str) -> Result<String> {
    let t = tld.trim().trim_start_matches('.').to_lowercase();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(t)
    } else {
        anyhow::bail!("invalid TLD: {tld}")
    }
}

/// Which PHP binary a site should run under.
fn php_bin_for(site: &ResolvedSite) -> PathBuf {
    match &site.php {
        Some(v) => dpl_core::php::resolve(v).unwrap_or_else(dpl_core::php::default_binary),
        None => dpl_core::php::default_binary(),
    }
}

/// Resolve a user-supplied path to an absolute, canonical form.
fn canonicalize(path: &str) -> Result<PathBuf> {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        dpl_core::paths::home(None)?.join(rest)
    } else {
        PathBuf::from(path)
    };
    std::fs::canonicalize(&expanded)
        .with_context(|| format!("no such directory: {}", expanded.display()))
}
