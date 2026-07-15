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
use dpl_core::xdebug::Mode;

use crate::fpm::{FpmManager, MasterKey};

/// Everything the proxy needs to serve one request for a site.
pub struct SiteRoute {
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
    /// Which php-fpm master this site belongs to, together with `php_bin` and
    /// `profile`.
    xdebug: Mode,
    /// Whether this site's master has the SPX profiler loaded.
    profile: bool,
    /// This site's opcache preload script (absolute), if any. A preloaded site
    /// belongs to its own master, so this is part of the master identity too.
    preload: Option<PathBuf>,
    secure: bool,
    upstream: Option<u16>,
}

/// Repo-derived facts about a site — its framework label, required-PHP
/// constraint, and Node pin — cached at reconcile time. Reading these means
/// touching the project's `composer.json` (twice, historically) and up to three
/// node files; `site_infos()` behind the `dpl sites` the GUI polls constantly
/// did that for every site on every call. Refreshed once per reconcile, exactly
/// like `xdebug_installed`/`spx_installed`.
#[derive(Default, Clone)]
struct SiteMeta {
    framework: Option<String>,
    requires_php: Option<String>,
    node: Option<dpl_core::node::Pin>,
}

pub struct Registry {
    config: LocalConfig,
    config_path: PathBuf,
    fpm: FpmManager,
    appservers: crate::appserver::AppServers,
    routes: BTreeMap<String, RouteInfo>,
    /// Configured TLDs (cached from config for the request hot path).
    tlds: Vec<String>,
    /// Whether Xdebug is installed, per PHP binary. Probing it runs `php --ini`,
    /// so it is resolved once per reconcile and read from here afterwards —
    /// spawning a process per site under the registry lock deadlocks the daemon.
    xdebug_installed: BTreeMap<PathBuf, bool>,
    /// Whether SPX is installed, per PHP binary. Cached for the same reason.
    spx_installed: BTreeMap<PathBuf, bool>,
    /// Per-site repo metadata (framework / required PHP / node pin), keyed by
    /// site name. Populated by [`Registry::reconcile`]; read by [`Registry::site_infos`].
    site_meta: BTreeMap<String, SiteMeta>,
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
            xdebug_installed: BTreeMap::new(),
            spx_installed: BTreeMap::new(),
            site_meta: BTreeMap::new(),
        })
    }

    /// Cached answer to "is Xdebug installed for this PHP binary?".
    fn xdebug_installed(&self, php_bin: &std::path::Path) -> bool {
        self.xdebug_installed.get(php_bin).copied().unwrap_or(false)
    }

    /// Cached answer to "is SPX installed for this PHP binary?".
    fn spx_installed(&self, php_bin: &std::path::Path) -> bool {
        self.spx_installed.get(php_bin).copied().unwrap_or(false)
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
        let fpm_addr = match self.fpm.addr_for(&route.php_bin, &route.xdebug, route.profile, route.preload.as_deref()) {
            Some(a) => a,
            None if route.upstream.is_some() => SocketAddr::from(([127, 0, 0, 1], 0)),
            None => return None,
        };
        Some(SiteRoute {
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
                    .map(|r| {
                        r.upstream.is_some() || self.fpm.addr_for(&r.php_bin, &r.xdebug, r.profile, r.preload.as_deref()).is_some()
                    })
                    .unwrap_or(false);
                // Reuse the cached route's binary rather than re-resolving PHP.
                let (xdebug_installed, profile_installed) = self
                    .routes
                    .get(&s.name)
                    .map(|r| (self.xdebug_installed(&r.php_bin), self.spx_installed(&r.php_bin)))
                    .unwrap_or((false, false));
                // Framework, required-PHP, and the node pin are cached per
                // reconcile (see `site_meta`) so this read-only path never
                // touches composer.json or the node files. A site that appeared
                // since the last reconcile (not yet cached) falls back to
                // reading them directly — correct, just not yet cheap.
                let meta = self.site_meta.get(&s.name);
                let (framework, requires_php) = match meta {
                    Some(m) => (m.framework.clone(), m.requires_php.clone()),
                    None => sites::detect_meta(&s.path),
                };
                let node_pin = match meta {
                    Some(m) => m.node.clone(),
                    None => dpl_core::node::read_pin(&s.path),
                };
                // Show the preload script as configured (relative to the project).
                let preload = self
                    .config
                    .links
                    .get(&s.name)
                    .and_then(|l| l.preload.as_ref())
                    .map(|p| p.to_string_lossy().into_owned());
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
                    framework,
                    requires_php,
                    xdebug: Some(s.xdebug.to_string()),
                    xdebug_installed,
                    profile: s.profile,
                    profile_installed,
                    preload,
                    node: node_pin.as_ref().map(|p| p.version.clone()),
                    node_source: node_pin.as_ref().map(|p| p.source.as_str().to_string()),
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
                // A reverse proxy runs no PHP of ours, so Xdebug/SPX are meaningless.
                xdebug: None,
                xdebug_installed: false,
                profile: false,
                profile_installed: false,
                preload: None,
                node: None,
                node_source: None,
            });
        }
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// Number of currently-servable sites, counted straight from the routing
    /// table — the cheap equivalent of `site_infos().filter(|s| s.serving).count()`
    /// that `Request::Status` used to pay for, without building every `SiteInfo`
    /// (which reads each project's composer.json and node files). Counts live
    /// fpm/Octane routes plus the always-on reverse proxies, matching how
    /// `site_infos()` marks `serving`.
    pub fn serving_count(&self) -> usize {
        let routes = self
            .routes
            .values()
            .filter(|r| {
                r.upstream.is_some()
                    || self.fpm.addr_for(&r.php_bin, &r.xdebug, r.profile, r.preload.as_deref()).is_some()
            })
            .count();
        routes + self.config.proxies.len()
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

        // Push the current IDE settings down before spawning anything. If they
        // changed, every running master is holding a stale loader ini, so drop
        // them all and let the loop below respawn with the new port / IDE key.
        if self.fpm.set_xdebug_settings(self.config.xdebug.clone()) {
            self.fpm.retain(&BTreeSet::new());
        }

        // Ensure a master per distinct (PHP binary, Xdebug mode, profiler); drop the
        // rest. Remember each site's binary so we don't resolve it again for routes.
        let mut needed: BTreeSet<MasterKey> = BTreeSet::new();
        let site_bins: Vec<PathBuf> = resolved.iter().map(&bin_for).collect();
        for (site, bin) in resolved.iter().zip(&site_bins) {
            if self.fpm.ensure(bin, &site.xdebug, site.profile, site.preload.as_deref()).is_ok() {
                needed.insert((bin.clone(), site.xdebug.clone(), site.profile, site.preload.clone()));
            }
        }
        self.fpm.retain(&needed);

        // Refresh the extension-installed caches once per distinct binary.
        let distinct_bins: BTreeSet<&PathBuf> = site_bins.iter().collect();
        self.xdebug_installed =
            distinct_bins.iter().map(|b| ((*b).clone(), dpl_core::xdebug::installed(b))).collect();
        self.spx_installed =
            distinct_bins.iter().map(|b| ((*b).clone(), dpl_core::spx::installed(b))).collect();

        // Cache each site's repo metadata (framework / required PHP / node pin)
        // so the read-only `site_infos()` never re-reads composer.json or the
        // node files on the hot `dpl sites` path. Same rationale as the caches
        // above; refreshed here on every mutation.
        self.site_meta = resolved
            .iter()
            .map(|s| {
                let (framework, requires_php) = sites::detect_meta(&s.path);
                let node = dpl_core::node::read_pin(&s.path);
                (s.name.clone(), SiteMeta { framework, requires_php, node })
            })
            .collect();

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
                    xdebug: site.xdebug.clone(),
                    profile: site.profile,
                    preload: site.preload.clone(),
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
            .filter(|r| self.fpm.addr_for(&r.php_bin, &r.xdebug, r.profile, r.preload.as_deref()).is_some())
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
            dpl_core::config::Link {
                path: path.clone(),
                php: None,
                secure: false,
                runtime: None,
                xdebug: None,
                profile: false,
                preload: None,
                database: None,
                db_branch: None,
            },
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
                dpl_core::config::Link { path, php: None, secure: false, runtime: None, xdebug: None, profile: false, preload: None, database: None, db_branch: None },
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

    /// Set the Xdebug mode for one linked site, or the default for all sites
    /// when `site` is `None`. `port`/`ide_key` update the shared IDE settings.
    ///
    /// Changing a mode moves the site onto a different php-fpm master, which
    /// [`Registry::reconcile`] spawns on demand; `off` is the shared, no-Xdebug
    /// master, so the common case never pays for the feature.
    pub fn set_xdebug(
        &mut self,
        mode: Option<&str>,
        site: Option<&str>,
        port: Option<u16>,
        ide_key: Option<&str>,
    ) -> Result<String> {
        let mut messages: Vec<String> = Vec::new();

        if let Some(p) = port {
            self.config.xdebug.client_port = p;
            messages.push(format!("IDE port set to {p}."));
        }
        if let Some(k) = ide_key {
            let k = k.trim().to_uppercase();
            if k.is_empty() {
                anyhow::bail!("IDE key cannot be empty");
            }
            messages.push(format!("IDE key set to {k}."));
            self.config.xdebug.ide_key = k;
        }

        if let Some(raw) = mode {
            // Parse before touching config so an invalid mode changes nothing.
            let parsed = Mode::parse(raw)?;
            let stored = (!parsed.is_off()).then(|| parsed.to_string());

            match site {
                Some(name) => {
                    let name = name.to_lowercase();
                    let link = self.config.links.get_mut(&name).with_context(|| {
                        format!("{name} is not linked (Xdebug modes apply to linked sites; \
                                 set the default with `dpl xdebug mode {parsed}`)")
                    })?;
                    link.xdebug = stored;
                    messages.push(if parsed.is_off() {
                        format!("Xdebug off for {name}.")
                    } else {
                        format!("Xdebug `{parsed}` for {name}.")
                    });
                }
                None => {
                    self.config.default_xdebug = stored;
                    messages.push(if parsed.is_off() {
                        "Xdebug off by default.".into()
                    } else {
                        format!("Xdebug `{parsed}` by default.")
                    });
                }
            }

            // Warn rather than fail: the site still serves, just without Xdebug,
            // and the user may be about to install it.
            if !parsed.is_off() {
                let bin = self
                    .routes
                    .get(site.unwrap_or_default())
                    .map(|r| r.php_bin.clone())
                    .unwrap_or_else(dpl_core::php::default_binary);
                if !dpl_core::xdebug::installed(&bin) {
                    messages.push(format!(
                        "Warning: Xdebug is not installed for {} — install it with \
                         `dpl php ext-install <version> xdebug`.",
                        bin.display()
                    ));
                }
            }
        }

        if messages.is_empty() {
            return Ok("Nothing to change.".into());
        }
        self.save()?;
        let n = self.reconcile();
        messages.push(format!("Serving {n} site(s)."));
        Ok(messages.join(" "))
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

    /// Turn the SPX profiler on/off for a site. Only linked sites persist a
    /// profiler flag; a parked site would forget it on the next reconcile.
    pub fn set_profile(&mut self, name: &str, on: bool) -> Result<String> {
        let name = name.to_lowercase();
        match self.config.links.get_mut(&name) {
            Some(link) => {
                link.profile = on;
                self.save()?;
                self.reconcile();
                if on {
                    Ok(format!(
                        "Profiler on for {name}.test — every request is now captured. \
                         Open the flame graphs at http://{name}.test/?SPX_UI_URI=/&SPX_KEY={key}",
                        key = dpl_core::spx::KEY
                    ))
                } else {
                    Ok(format!("Profiler off for {name}.test."))
                }
            }
            None => Ok(format!("the profiler applies to linked sites only; {name} is not linked.")),
        }
    }

    /// Set or clear a site's opcache preload script (relative to the project
    /// root). Only linked sites can opt in — a parked site would forget it on the
    /// next reconcile. Enabling verifies the script exists so a typo fails loudly
    /// here rather than making php-fpm refuse to serve.
    pub fn set_preload(&mut self, name: &str, script: Option<String>) -> Result<String> {
        let name = name.to_lowercase();
        let Some(link) = self.config.links.get_mut(&name) else {
            return Ok(format!("preload applies to linked sites only; {name} is not linked."));
        };
        match script {
            Some(rel) => {
                let abs = link.path.join(&rel);
                if !abs.is_file() {
                    anyhow::bail!(
                        "no preload script at {}. Scaffold one with `dpl preload generate {name}`.",
                        abs.display()
                    );
                }
                link.preload = Some(rel.clone().into());
                self.save()?;
                self.reconcile();
                Ok(format!(
                    "Preload on for {name}.test using {rel} — its own php-fpm master will \
                     compile it into opcache at startup. Preloaded code is frozen until the \
                     master restarts, so keep the script to vendor/framework, not app code."
                ))
            }
            None => {
                link.preload = None;
                self.save()?;
                self.reconcile();
                Ok(format!("Preload off for {name}.test — it folds back into the shared master."))
            }
        }
    }

    /// A linked site's branch-DB state: (project path, base database, live branch).
    /// The database/branch are None until `dpl db attach`.
    pub fn branch_db_state(&self, site: &str) -> Result<(PathBuf, Option<String>, Option<String>)> {
        let site = site.to_lowercase();
        let link = self
            .config
            .links
            .get(&site)
            .with_context(|| format!("{site} is not linked (branch databases apply to linked sites)"))?;
        Ok((link.path.clone(), link.database.clone(), link.db_branch.clone()))
    }

    /// Persist a site's branch-DB config: the base database name and the branch
    /// currently live in it. `None`/`None` detaches. No reconcile — the database
    /// mapping doesn't affect how the site is served.
    pub fn set_branch_db(&mut self, site: &str, database: Option<String>, branch: Option<String>) -> Result<()> {
        let site = site.to_lowercase();
        let link = self
            .config
            .links
            .get_mut(&site)
            .with_context(|| format!("{site} is not linked"))?;
        link.database = database;
        link.db_branch = branch;
        self.save()
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
