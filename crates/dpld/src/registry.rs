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
    /// If set, the site runs on an Octane server at this loopback port — the
    /// proxy forwards to it instead of serving over FastCGI.
    pub upstream: Option<u16>,
}

/// Per-site routing data computed at reconcile time.
struct RouteInfo {
    docroot: PathBuf,
    php_bin: PathBuf,
    secure: bool,
    upstream: Option<u16>,
}

pub struct Registry {
    config: LocalConfig,
    config_path: PathBuf,
    fpm: FpmManager,
    appservers: crate::appserver::AppServers,
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
            appservers: crate::appserver::AppServers::new(),
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

    /// The reverse-proxy target for a site name, if one is configured.
    pub fn proxy_target(&self, site_name: &str) -> Option<String> {
        self.config.proxies.get(site_name).cloned()
    }

    /// Give the proxy the route for a host's site, if it's currently servable.
    pub fn resolve_request(&self, site_name: &str) -> Option<SiteRoute> {
        let route = self.routes.get(site_name)?;
        // Octane sites are served by proxying to their upstream port; a dummy
        // fpm_addr is fine since the proxy checks `upstream` first.
        let fpm_addr = match self.fpm.addr_for(&route.php_bin) {
            Some(a) => a,
            None if route.upstream.is_some() => SocketAddr::from(([127, 0, 0, 1], 0)),
            None => return None,
        };
        Some(SiteRoute {
            name: site_name.to_string(),
            docroot: route.docroot.clone(),
            fpm_addr,
            secure: route.secure,
            upstream: route.upstream,
        })
    }

    pub fn site_infos(&self) -> Vec<SiteInfo> {
        let mut infos: Vec<SiteInfo> = sites::resolve(&self.config)
            .into_iter()
            .map(|s| {
                // Use the cached route's binary — never re-resolve PHP here.
                // `php_bin_for` spawns `which`/`php` processes, and doing that
                // per-site under the registry lock deadlocked the daemon.
                // Octane sites are "serving" when their upstream is up.
                let serving = self
                    .routes
                    .get(&s.name)
                    .map(|r| r.upstream.is_some() || self.fpm.addr_for(&r.php_bin).is_some())
                    .unwrap_or(false);
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
                    runtime: s.runtime,
                    framework: sites::detect_framework(&s.path),
                    requires_php: sites::detect_required_php(&s.path),
                }
            })
            .collect();

        // Reverse proxies present as sites whose "path" is the target URL.
        let tld = self.config.primary_tld();
        for (name, target) in &self.config.proxies {
            infos.push(SiteInfo {
                host: format!("{name}.{tld}"),
                url: format!("http://{name}.{tld}"),
                name: name.clone(),
                path: target.clone(),
                docroot: target.clone(),
                source: "proxy".into(),
                php: None,
                secure: false,
                serving: true,
                runtime: None,
                framework: None,
                requires_php: None,
            });
        }
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
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

        // Detect PHP ONCE per reconcile — `php::detect()` spawns a process per
        // installed version, so resolving it per-site (previously twice each)
        // made every mutation take tens of seconds. Cache it here.
        let versions = dpl_core::php::detect();
        let default_bin = dpl_core::php::default_binary();
        let bin_for = |site: &ResolvedSite| -> PathBuf {
            match &site.php {
                Some(v) => versions
                    .iter()
                    .find(|p| &p.version == v)
                    .map(|p| p.binary.clone())
                    .unwrap_or_else(|| default_bin.clone()),
                None => default_bin.clone(),
            }
        };

        // Ensure a master per distinct PHP binary; drop the rest. Remember each
        // site's binary so we don't resolve it again for the routes.
        let mut needed: BTreeSet<PathBuf> = BTreeSet::new();
        let site_bins: Vec<PathBuf> = resolved.iter().map(&bin_for).collect();
        for bin in &site_bins {
            if self.fpm.ensure(bin).is_ok() {
                needed.insert(bin.clone());
            }
        }
        self.fpm.retain(&needed);

        // Start/stop Octane servers for sites on a non-fpm runtime, and record
        // each one's upstream port.
        let mut octane_sites: BTreeSet<String> = BTreeSet::new();
        let mut upstreams: BTreeMap<String, u16> = BTreeMap::new();
        for (site, bin) in resolved.iter().zip(&site_bins) {
            let runtime = site.runtime.as_deref().unwrap_or("fpm");
            if runtime != "fpm" && !runtime.is_empty() {
                if let Some(port) = self.appservers.ensure(&site.name, runtime, &site.path, bin) {
                    octane_sites.insert(site.name.clone());
                    upstreams.insert(site.name.clone(), port);
                }
            }
        }
        self.appservers.retain(&octane_sites);

        // Rebuild routes.
        self.routes.clear();
        for (site, bin) in resolved.iter().zip(site_bins) {
            self.routes.insert(
                site.name.clone(),
                RouteInfo {
                    docroot: site.docroot.clone(),
                    php_bin: bin,
                    secure: site.secure,
                    upstream: upstreams.get(&site.name).copied(),
                },
            );
        }

        // In hosts-file mode, keep /etc/hosts in sync with the current sites so
        // `.test` resolves without a local DNS resolver (keeps Private Relay on).
        if self.config.uses_hosts() {
            self.sync_hosts_file(&resolved);
        }

        self.routes
            .values()
            .filter(|r| self.fpm.addr_for(&r.php_bin).is_some())
            .count()
    }

    /// Rewrite the dpl-managed block of `/etc/hosts` (via the privileged helper,
    /// which a NOPASSWD sudoers rule lets us run silently) to list every site on
    /// every configured TLD. Best-effort: if the helper or sudoers rule is
    /// missing, we just skip it.
    fn sync_hosts_file(&self, sites: &[ResolvedSite]) {
        let Some(helper) = helper_path() else { return };
        let mut hosts: Vec<String> = Vec::new();
        let names = sites.iter().map(|s| s.name.clone()).chain(self.config.proxies.keys().cloned());
        for name in names {
            for tld in &self.tlds {
                hosts.push(format!("{name}.{tld}"));
            }
        }
        hosts.sort();
        hosts.dedup();

        let mut args: Vec<String> =
            vec!["-n".into(), helper.to_string_lossy().into_owned(), "sync-hosts".into()];
        args.extend(hosts);
        if let Err(e) = std::process::Command::new("sudo").args(&args).output() {
            tracing::warn!(error = %e, "hosts sync failed (is the sudoers rule installed?)");
        }
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
            dpl_core::config::Link { path: path.clone(), php: None, secure: false, runtime: None },
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

    /// Bulk import: park many directories and link many named projects, then
    /// save + reconcile ONCE. Used by `dpl valet import` so pulling in dozens of
    /// sites costs one reconcile instead of one per site.
    pub fn import_sites(&mut self, parked: &[String], links: &[(String, String)]) -> Result<String> {
        let mut parked_ok = 0;
        for p in parked {
            if let Ok(path) = canonicalize(p) {
                if !self.config.parked.contains(&path) {
                    self.config.parked.push(path);
                    parked_ok += 1;
                }
            }
        }
        let mut linked_ok = 0;
        for (name, path) in links {
            let Ok(path) = canonicalize(path) else { continue };
            self.config.links.insert(
                name.to_lowercase(),
                dpl_core::config::Link { path, php: None, secure: false, runtime: None },
            );
            linked_ok += 1;
        }
        self.save()?;
        let n = self.reconcile();
        Ok(format!(
            "Imported {parked_ok} parked dir(s) and {linked_ok} linked site(s). Serving {n} site(s)."
        ))
    }

    /// Bulk remove: unpark directories and unlink sites (by name), saving +
    /// reconciling once. The reverse of [`import_sites`].
    pub fn remove_sites(&mut self, parked: &[String], link_names: &[String]) -> Result<String> {
        let mut unparked = 0;
        for p in parked {
            let path = canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
            let before = self.config.parked.len();
            self.config.parked.retain(|x| x != &path);
            if self.config.parked.len() != before {
                unparked += 1;
            }
        }
        let mut unlinked = 0;
        for name in link_names {
            if self.config.links.remove(&name.to_lowercase()).is_some() {
                unlinked += 1;
            }
        }
        self.save()?;
        let n = self.reconcile();
        Ok(format!(
            "Removed {unparked} parked dir(s) and {unlinked} linked site(s). Serving {n} site(s)."
        ))
    }

    /// Set a linked site's application-server runtime.
    pub fn set_runtime(&mut self, site: &str, runtime: &str) -> Result<String> {
        let site = site.to_lowercase();
        let runtime = runtime.to_lowercase();
        const VALID: [&str; 4] = ["fpm", "octane-swoole", "octane-roadrunner", "octane-frankenphp"];
        if !VALID.contains(&runtime.as_str()) {
            anyhow::bail!("unknown runtime `{runtime}` (fpm | octane-swoole | octane-roadrunner | octane-frankenphp)");
        }
        let link = self
            .config
            .links
            .get_mut(&site)
            .with_context(|| format!("no linked site named `{site}` (runtimes apply to linked sites)"))?;
        link.runtime = if runtime == "fpm" { None } else { Some(runtime.clone()) };
        self.save()?;
        let n = self.reconcile();
        Ok(format!("`{site}` now runs on {runtime}. Serving {n} site(s)."))
    }

    /// Switch how `.test` names resolve: `hosts` (per-site /etc/hosts entries,
    /// Private-Relay-safe) or `resolver` (wildcard DNS). Writes or clears the
    /// /etc/hosts block accordingly.
    pub fn set_resolution(&mut self, mode: &str) -> Result<String> {
        let mode = if mode == "hosts" { "hosts" } else { "resolver" };
        self.config.resolution = Some(mode.to_string());
        self.save()?;
        if mode == "hosts" {
            self.reconcile(); // writes /etc/hosts for the current sites
            Ok("Now resolving .test via /etc/hosts — iCloud Private Relay stays on.".into())
        } else {
            self.clear_hosts_file(); // remove our block while sudoers still allows it
            self.reconcile();
            Ok("Now resolving .test via the local DNS resolver.".into())
        }
    }

    /// Remove the dpl-managed block from /etc/hosts (via the helper).
    fn clear_hosts_file(&self) {
        let Some(helper) = helper_path() else { return };
        let _ = std::process::Command::new("sudo")
            .args(["-n", &helper.to_string_lossy(), "clear-hosts"])
            .output();
    }

    /// Point a `.test` host at another local service.
    pub fn proxy_set(&mut self, name: &str, target: &str) -> Result<String> {
        let name = name.to_lowercase();
        let name = name.strip_suffix(&format!(".{}", self.config.primary_tld())).unwrap_or(&name).to_string();
        let target = normalize_target(target)?;
        self.config.proxies.insert(name.clone(), target.clone());
        self.save()?;
        self.reconcile();
        Ok(format!("Proxying {name}.{} → {target}", self.config.primary_tld()))
    }

    pub fn proxy_remove(&mut self, name: &str) -> Result<String> {
        let name = name.to_lowercase();
        let name = name.strip_suffix(&format!(".{}", self.config.primary_tld())).unwrap_or(&name).to_string();
        if self.config.proxies.remove(&name).is_none() {
            return Ok(format!("No proxy named {name}."));
        }
        self.save()?;
        Ok(format!("Removed proxy {name}."))
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
        // Force the php-fpm pools to restart so php.ini / extension changes
        // take effect (stop all, then reconcile respawns what's needed).
        self.fpm.retain(&BTreeSet::new());
        let n = self.reconcile();
        Ok(format!("Reloaded. Serving {n} site(s)."))
    }

    /// Hard-reset every backend: stop all php-fpm masters + Octane servers,
    /// reap orphaned masters left by crashed daemons, then rebuild from scratch.
    /// The escape hatch when the machine gets into a churned/wedged state.
    pub fn repair_backends(&mut self) -> Result<String> {
        self.config = LocalConfig::load(&self.config_path)?;
        self.fpm.retain(&BTreeSet::new());
        self.appservers.retain(&BTreeSet::new());
        // Kill stray php-fpm masters from previously-crashed daemons.
        crate::fpm::FpmManager::kill_orphans();
        let n = self.reconcile();
        Ok(format!("Repaired backends — restarted php-fpm{}. Serving {n} site(s).",
            if self.appservers_active() { " + Octane servers" } else { "" }))
    }

    /// Whether any Octane server is currently supervised.
    fn appservers_active(&self) -> bool {
        self.routes.values().any(|r| r.upstream.is_some())
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

    /// Make an already-configured TLD the primary (first) one — the canonical
    /// domain used to build each site's URL. Sites still answer on every TLD.
    pub fn tld_primary(&mut self, tld: &str) -> Result<String> {
        let tld = normalize_tld(tld)?;
        let mut tlds = self.config.tlds();
        if !tlds.iter().any(|t| t == &tld) {
            anyhow::bail!(".{tld} isn't configured — add it first with `dpl tld add {tld}`.");
        }
        tlds.retain(|t| t != &tld);
        tlds.insert(0, tld.clone());
        self.config.tlds = tlds;
        self.save()?;
        self.reconcile();
        Ok(format!("Primary TLD is now .{tld} — sites are canonically <name>.{tld}."))
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

/// Normalize a proxy target: add `http://` if no scheme; validate it parses.
fn normalize_target(target: &str) -> Result<String> {
    let t = target.trim();
    let t = if t.contains("://") { t.to_string() } else { format!("http://{t}") };
    // Basic sanity: must have a host after the scheme.
    if t.split("://").nth(1).map(|h| !h.is_empty()).unwrap_or(false) {
        Ok(t.trim_end_matches('/').to_string())
    } else {
        anyhow::bail!("invalid proxy target: {target}")
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
/// Locate the `dpl-helper` binary — next to the running daemon, else on PATH.
fn helper_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("dpl-helper");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    let out = std::process::Command::new("/usr/bin/env").args(["which", "dpl-helper"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

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
