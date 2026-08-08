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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dpl_core::config::LocalConfig;
use dpl_core::ipc::SiteInfo;
use dpl_core::sites::{self, ResolvedSite};
use dpl_core::xdebug::Mode;

use crate::fpm::{FpmManager, MasterKey};

/// A linked site's branch-aware-database configuration, as stored on its Link.
#[derive(Clone)]
pub struct BranchDbState {
    pub path: PathBuf,
    pub database: Option<String>,
    pub db_branch: Option<String>,
    /// Postgres port when not the default 5432.
    pub port: Option<u16>,
}

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
    /// Framework, project kind, required PHP, and the JavaScript framework —
    /// everything read from the project's manifests in one pass.
    project: dpl_core::sites::ProjectMeta,
    node: Option<dpl_core::node::Pin>,
    /// The site's package manager, `None` when the repo has no `package.json`.
    /// Detected here rather than per request: it stats a handful of lockfiles
    /// and may read package.json, which is exactly the per-site file work this
    /// cache exists to keep off the `dpl sites` path.
    agent: Option<dpl_core::node::AgentChoice>,
}

/// Normalise a single tag, rejecting input that normalises to nothing — `dpl
/// tags rename "  " x` should fail rather than quietly matching no sites.
fn one_tag(raw: &str) -> Result<String> {
    dpl_core::config::normalize_tags([raw])
        .into_iter()
        .next()
        .with_context(|| format!("`{raw}` isn't a usable tag"))
}

/// The package manager a site's repo calls for, or `None` when it has no
/// `package.json` — the distinction the GUI needs to decide whether a site has
/// any Node actions to offer at all.
fn detect_site_agent(path: &Path) -> Option<dpl_core::node::AgentChoice> {
    path.join("package.json")
        .is_file()
        .then(|| dpl_core::node::detect_agent(path))
}

pub struct Registry {
    config: LocalConfig,
    config_path: PathBuf,
    fpm: FpmManager,
    appservers: crate::appserver::AppServers,
    /// Supervised Node dev servers, one per opted-in site (see `dpl dev`).
    devservers: crate::devserver::DevServers,
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
    /// Installed PHP versions + the default binary, detected once per full
    /// reconcile (each detect spawns a process per version). The incremental
    /// path reads these instead of re-probing the machine on every mutation.
    php_versions: Vec<dpl_core::php::PhpVersion>,
    default_php_bin: PathBuf,
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
            devservers: crate::devserver::DevServers::new(),
            routes: BTreeMap::new(),
            tlds,
            xdebug_installed: BTreeMap::new(),
            spx_installed: BTreeMap::new(),
            site_meta: BTreeMap::new(),
            php_versions: Vec::new(),
            default_php_bin: PathBuf::new(),
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
                let project = match meta {
                    Some(m) => m.project.clone(),
                    None => sites::detect_project(&s.path),
                };
                let node_pin = match meta {
                    Some(m) => m.node.clone(),
                    None => dpl_core::node::read_pin(&s.path),
                };
                let agent = match meta {
                    Some(m) => m.agent.clone(),
                    None => detect_site_agent(&s.path),
                };
                // Show the preload script as configured (relative to the project).
                let link = self.config.links.get(&s.name);
                let preload = link
                    .and_then(|l| l.preload.as_ref())
                    .map(|p| p.to_string_lossy().into_owned());
                let (database, db_branch) = link
                    .map(|l| (l.database.clone(), l.db_branch.clone()))
                    .unwrap_or((None, None));
                // Read before the struct literal moves `s.name`.
                let dev_running = self.devservers.is_running(&s.name);
                let dev_port = self.devservers.port(&s.name);
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
                    watch: s.watch,
                    framework: project.framework,
                    requires_php: project.requires_php,
                    kind: Some(project.kind.as_str().to_string()),
                    node_framework: project.node_framework,
                    stack: project.stack,
                    tags: s.tags,
                    xdebug: Some(s.xdebug.to_string()),
                    xdebug_installed,
                    profile: s.profile,
                    profile_installed,
                    preload,
                    node: node_pin.as_ref().map(|p| p.version.clone()),
                    node_source: node_pin.as_ref().map(|p| p.source.as_str().to_string()),
                    node_agent: agent.as_ref().map(|a| a.agent.as_str().to_string()),
                    node_agent_source: agent.as_ref().map(|a| a.reason.as_str().to_string()),
                    dev: s.dev,
                    dev_running,
                    dev_port,
                    database,
                    db_branch,
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
                watch: false,
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
                node_agent: None,
                node_agent_source: None,
                dev: None,
                dev_running: false,
                dev_port: None,
                kind: None,
                node_framework: None,
                stack: None,
                tags: Vec::new(),
                database: None,
                db_branch: None,
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
        // made every mutation take tens of seconds. Cached on self so the
        // incremental path (`reconcile_site`) never re-probes at all.
        let versions = dpl_core::php::detect();
        let default_bin = dpl_core::php::default_binary();
        self.php_versions = versions.clone();
        self.default_php_bin = default_bin.clone();
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
                let project = sites::detect_project(&s.path);
                let node = dpl_core::node::read_pin(&s.path);
                let agent = detect_site_agent(&s.path);
                (s.name.clone(), SiteMeta { project, node, agent })
            })
            .collect();

        // Start/stop Octane servers for sites on a non-fpm runtime, and record
        // each one's upstream port.
        let mut octane_sites: BTreeSet<String> = BTreeSet::new();
        let mut upstreams: BTreeMap<String, u16> = BTreeMap::new();
        for (site, bin) in resolved.iter().zip(&site_bins) {
            let runtime = site.runtime.as_deref().unwrap_or("fpm");
            if runtime != "fpm" && !runtime.is_empty() {
                if let Some(port) = self.appservers.ensure(&site.name, runtime, &site.path, bin, site.watch) {
                    octane_sites.insert(site.name.clone());
                    upstreams.insert(site.name.clone(), port);
                }
            }
        }
        self.appservers.retain(&octane_sites);

        // Start/stop supervised dev servers. Unlike Octane these never affect
        // routing — a dev server is a side-car, not the site's backend.
        let mut dev_sites: BTreeSet<String> = BTreeSet::new();
        for site in resolved.iter() {
            if let Some(script) = site.dev.as_deref().filter(|s| !s.is_empty()) {
                self.devservers.ensure(&site.name, script, &site.path);
                dev_sites.insert(site.name.clone());
            }
        }
        self.devservers.retain(&dev_sites);

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

    /// Reconcile ONE site — the fast path for single-site mutations (secure,
    /// PHP pin, runtime, Xdebug mode, profiler, preload, link/unlink).
    ///
    /// A full [`Registry::reconcile`] re-detects PHP (a process spawn per
    /// installed version) and re-reads every site's repo metadata; at 100+
    /// sites that made each mutation pay for the whole machine. This touches
    /// only the named site and derives the keep-alive sets for php-fpm masters
    /// and Octane servers from the in-memory routing table, so unused backends
    /// are still stopped without resolving anybody else.
    pub fn reconcile_site(&mut self, name: &str) {
        let name = name.to_lowercase();
        match sites::resolve_one(&self.config, &name) {
            Some(site) => {
                // Binary from the cached detection; a pin to a version that
                // appeared since the last full reconcile triggers ONE refresh.
                let mut bin = self.cached_bin_for(&site);
                if bin.is_none() {
                    self.php_versions = dpl_core::php::detect();
                    bin = self.cached_bin_for(&site);
                }
                let bin = bin.unwrap_or_else(|| self.default_php_bin.clone());

                // Extension caches for a binary we haven't met yet.
                if !self.xdebug_installed.contains_key(&bin) {
                    self.xdebug_installed.insert(bin.clone(), dpl_core::xdebug::installed(&bin));
                    self.spx_installed.insert(bin.clone(), dpl_core::spx::installed(&bin));
                }

                let _ = self.fpm.ensure(&bin, &site.xdebug, site.profile, site.preload.as_deref());

                // Octane runtime for this site, if any.
                let runtime = site.runtime.as_deref().unwrap_or("fpm");
                let upstream = if runtime != "fpm" && !runtime.is_empty() {
                    self.appservers.ensure(&site.name, runtime, &site.path, &bin, site.watch)
                } else {
                    None
                };

                match site.dev.as_deref().filter(|s| !s.is_empty()) {
                    Some(script) => self.devservers.ensure(&site.name, script, &site.path),
                    // Turned off (or the site was unlinked): stop this one
                    // without disturbing every other site's dev server.
                    None => {
                        let keep: BTreeSet<String> = self
                            .devservers
                            .statuses()
                            .into_iter()
                            .map(|d| d.site)
                            .filter(|s| s != &site.name)
                            .collect();
                        self.devservers.retain(&keep);
                    }
                }

                self.site_meta.insert(name.clone(), SiteMeta {
                    project: sites::detect_project(&site.path),
                    node: dpl_core::node::read_pin(&site.path),
                    agent: detect_site_agent(&site.path),
                });
                self.routes.insert(
                    name,
                    RouteInfo {
                        docroot: site.docroot,
                        php_bin: bin,
                        xdebug: site.xdebug,
                        profile: site.profile,
                        preload: site.preload,
                        secure: site.secure,
                        upstream,
                    },
                );
            }
            None => {
                self.routes.remove(&name);
                self.site_meta.remove(&name);
                let keep: BTreeSet<String> = self
                    .devservers
                    .statuses()
                    .into_iter()
                    .map(|d| d.site)
                    .filter(|s| s != &name)
                    .collect();
                self.devservers.retain(&keep);
            }
        }

        // Keep-alive sets straight from the routing table — no disk, no spawns.
        let needed: BTreeSet<MasterKey> = self
            .routes
            .values()
            .map(|r| (r.php_bin.clone(), r.xdebug.clone(), r.profile, r.preload.clone()))
            .collect();
        self.fpm.retain(&needed);
        let octane: BTreeSet<String> = self
            .routes
            .iter()
            .filter(|(_, r)| r.upstream.is_some())
            .map(|(n, _)| n.clone())
            .collect();
        self.appservers.retain(&octane);

        // Keep /etc/hosts current in hosts mode — the site set may have changed.
        if self.config.uses_hosts() {
            let resolved = sites::resolve(&self.config);
            self.sync_hosts_file(&resolved);
        }
    }

    /// The cached binary for a site's (possibly pinned) PHP. `None` = pinned
    /// to a version the cache doesn't know (caller refreshes once).
    fn cached_bin_for(&self, site: &ResolvedSite) -> Option<PathBuf> {
        match &site.php {
            Some(v) => self.php_versions.iter().find(|p| &p.version == v).map(|p| p.binary.clone()),
            None => Some(self.default_php_bin.clone()),
        }
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
                dev: None,
                path: path.clone(),
                php: None,
                secure: false,
                runtime: None,
                watch: None,
                xdebug: None,
                profile: false,
                preload: None,
                database: None,
                db_branch: None,
                db_port: None,
            },
        );
        self.save()?;
        self.reconcile_site(&name);
        Ok(format!("Linked {} → {}.test", path.display(), name))
    }

    /// Move a linked site to a different directory.
    ///
    /// Deliberately not "unlink then link": `link` writes a fresh `Link` with
    /// every setting at its default, so relinking through it would silently drop
    /// the site's PHP pin, HTTPS, runtime, Xdebug and database branch. A project
    /// that moves on disk is still the same site — only the path changes.
    pub fn relink(&mut self, name: &str, path: &str) -> Result<String> {
        let name = name.to_lowercase();
        let path = canonicalize(path)?;
        let link = self
            .config
            .links
            .get_mut(&name)
            .with_context(|| format!("no linked site named `{name}` (try `dpl link {}`)", path.display()))?;
        if link.path == path {
            return Ok(format!("{name} already points at {}.", path.display()));
        }
        let old = std::mem::replace(&mut link.path, path.clone());
        self.save()?;
        self.reconcile_site(&name);
        Ok(format!("{name} now points at {} (was {}).", path.display(), old.display()))
    }

    pub fn unlink(&mut self, name: &str) -> Result<String> {
        let name = name.to_lowercase();
        if self.config.links.remove(&name).is_none() {
            return Ok(format!("No linked site named {name}."));
        }
        self.save()?;
        self.reconcile_site(&name);
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
                dpl_core::config::Link { path, php: None, secure: false, runtime: None, watch: None, dev: None, xdebug: None, profile: false, preload: None, database: None, db_branch: None, db_port: None },
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
        self.reconcile_site(&site);
        Ok(format!("`{site}` now runs on {runtime}. Serving {} site(s).", self.serving_count()))
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
        // A mode change for one site moves one pool; the shared IDE settings
        // (port / key) touch every master, so those still take the full pass.
        match (&site, port, ide_key) {
            (Some(name), None, None) => self.reconcile_site(name),
            _ => {
                self.reconcile();
            }
        }
        messages.push(format!("Serving {} site(s).", self.serving_count()));
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
                self.reconcile_site(&name);
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
        self.devservers.retain(&BTreeSet::new());
        // Kill stray php-fpm masters from previously-crashed daemons.
        crate::fpm::FpmManager::kill_orphans();
        let n = self.reconcile();
        Ok(format!("Repaired backends — restarted php-fpm{}. Serving {n} site(s).",
            if self.appservers_active() { " + Octane servers" } else { "" }))
    }

    /// Turn a site's supervised dev server on (with a package.json script) or
    /// off. The script is checked against the project so a typo fails here,
    /// loudly, instead of becoming a background process that dies five times and
    /// gives up quietly.
    pub fn set_dev(&mut self, site: &str, script: Option<&str>) -> Result<String> {
        let site = site.to_lowercase();
        let link = self
            .config
            .links
            .get_mut(&site)
            .with_context(|| format!("no linked site named `{site}` (dev servers apply to linked sites)"))?;
        let project = link.path.clone();

        match script {
            Some(script) => {
                if !project.join("package.json").is_file() {
                    anyhow::bail!("`{site}` has no package.json — nothing to run a dev server from.");
                }
                let available = dpl_core::node::read_scripts(&project);
                if !available.iter().any(|s| s == script) {
                    anyhow::bail!(
                        "`{site}` has no `{script}` script. It defines: {}.",
                        if available.is_empty() { "(none)".to_string() } else { available.join(", ") }
                    );
                }
                link.dev = Some(script.to_string());
                self.save()?;
                self.reconcile_site(&site);
                let agent = dpl_core::node::detect_agent(&project).agent.as_str().to_string();
                Ok(format!(
                    "`{site}` dev server on — `{agent} run {script}`, supervised. \
                     It restarts if it dies; `dpl dev` shows where it's listening."
                ))
            }
            None => {
                link.dev = None;
                self.save()?;
                self.reconcile_site(&site);
                Ok(format!("`{site}` dev server off."))
            }
        }
    }

    /// Replace a linked site's tags. Tags are normalised here, at the one place
    /// they enter the config, so every reader downstream can assume they're
    /// already lowercase, deduped and sorted.
    pub fn set_tags(&mut self, site: &str, tags: &[String]) -> Result<String> {
        let site = site.to_lowercase();
        let tags = dpl_core::config::normalize_tags(tags);
        // Any served site can be tagged, parked ones included — tags describe
        // the site, not how it came to exist.
        if !sites::resolve(&self.config).iter().any(|s| s.name == site) {
            anyhow::bail!("no local site named `{site}`. See `dpl sites`.");
        }
        if tags.is_empty() {
            // Don't leave empty vectors behind: `dpl tags` and the config file
            // should show only sites that actually carry tags.
            self.config.tags.remove(&site);
        } else {
            self.config.tags.insert(site.clone(), tags.clone());
        }
        self.save()?;
        // No reconcile: `site_infos` resolves tags from the config on every
        // call, and tags change nothing the routing table or the backends care
        // about. Reconciling here would spawn work for a label change.
        Ok(if tags.is_empty() {
            format!("`{site}` has no tags now.")
        } else {
            format!("`{site}` tagged: {}.", tags.join(", "))
        })
    }

    /// Rename a tag everywhere it appears.
    ///
    /// Normalisation makes one idea one tag; this makes *changing* that idea a
    /// single operation. Without it a mistyped tag on twenty sites is twenty
    /// edits, and the tag people actually use ends up being whichever spelling
    /// was least effort to leave alone.
    pub fn rename_tag(&mut self, from: &str, to: &str) -> Result<String> {
        let from = one_tag(from)?;
        let to = one_tag(to)?;
        if from == to {
            anyhow::bail!("`{from}` and `{to}` are the same tag after normalising.");
        }

        let mut changed = 0usize;
        let mut merged = 0usize;
        for tags in self.config.tags.values_mut() {
            if !tags.iter().any(|t| t == &from) {
                continue;
            }
            // A site already carrying the target ends up with it once, not twice.
            if tags.iter().any(|t| t == &to) {
                merged += 1;
            }
            tags.retain(|t| t != &from);
            tags.push(to.clone());
            tags.sort();
            tags.dedup();
            changed += 1;
        }
        if changed == 0 {
            anyhow::bail!("no site carries the tag `{from}`. See `dpl tags`.");
        }
        self.save()?;
        Ok(format!(
            "Renamed `{from}` → `{to}` on {changed} site{}{}.",
            if changed == 1 { "" } else { "s" },
            if merged > 0 { format!(" ({merged} already had `{to}`)") } else { String::new() }
        ))
    }

    /// Remove a tag from every site that carries it.
    pub fn delete_tag(&mut self, tag: &str) -> Result<String> {
        let tag = one_tag(tag)?;
        let mut changed = 0usize;
        for tags in self.config.tags.values_mut() {
            let before = tags.len();
            tags.retain(|t| t != &tag);
            if tags.len() != before {
                changed += 1;
            }
        }
        if changed == 0 {
            anyhow::bail!("no site carries the tag `{tag}`. See `dpl tags`.");
        }
        // Sites left with nothing shouldn't linger as empty entries in the file.
        self.config.tags.retain(|_, tags| !tags.is_empty());
        self.save()?;
        Ok(format!(
            "Removed `{tag}` from {changed} site{}.",
            if changed == 1 { "" } else { "s" }
        ))
    }

    /// Restart one site's dev server, clearing any give-up state.
    pub fn restart_dev(&mut self, site: &str) -> Result<String> {
        let site = site.to_lowercase();
        if !self.devservers.restart(&site) {
            anyhow::bail!("`{site}` has no dev server. Turn one on with `dpl dev on {site}`.");
        }
        Ok(format!("`{site}` dev server restarted."))
    }

    /// Every supervised dev server's current state.
    pub fn dev_statuses(&self) -> Vec<crate::devserver::DevInfo> {
        self.devservers.statuses()
    }

    /// One supervision pass — reap, restart, and pick up announced ports. Driven
    /// by the daemon's watch loop, because a dev server dying is not a mutation
    /// and would otherwise go unnoticed until something else reconciled.
    pub fn supervise_dev(&mut self) {
        self.devservers.supervise();
    }

    /// Every supervised Octane server's current state.
    pub fn octane_statuses(&self) -> Vec<crate::appserver::AppServerInfo> {
        self.appservers.statuses()
    }

    /// The Octane sites to fingerprint this tick, with the project root to scan.
    /// Handed out so the caller can walk the tree without holding this registry.
    pub fn appserver_watch_targets(&self) -> Vec<(String, std::path::PathBuf)> {
        self.appservers.watch_targets()
    }

    /// One Octane supervision pass, given this tick's source fingerprints.
    /// Driven by the daemon's watch loop: neither a worker dying nor a file
    /// being saved is a mutation, so nothing else would notice either.
    pub fn supervise_appservers(&mut self, scans: &[(String, u64)]) {
        self.appservers.supervise(scans);
    }

    /// Gracefully reload one site's Octane workers — `dpl octane reload`.
    pub fn reload_octane(&mut self, site: &str) -> Result<String> {
        let site = site.to_lowercase();
        if !self.appservers.reload(&site) {
            anyhow::bail!(
                "`{site}` isn't running on Octane. Switch it with `dpl runtime {site} octane-frankenphp`."
            );
        }
        Ok(format!("`{site}` Octane workers reloading — new code, same listener."))
    }

    /// Bounce one site's Octane server outright — `dpl octane restart`.
    pub fn restart_octane(&mut self, site: &str) -> Result<String> {
        let site = site.to_lowercase();
        if !self.appservers.restart(&site) {
            anyhow::bail!(
                "`{site}` isn't running on Octane. Switch it with `dpl runtime {site} octane-frankenphp`."
            );
        }
        Ok(format!("`{site}` Octane server stopping — it comes back in a second or two."))
    }

    /// Turn source watching on or off for a site — `dpl octane watch`.
    pub fn set_octane_watch(&mut self, site: &str, on: bool) -> Result<String> {
        let site = site.to_lowercase();
        let link = self.config.links.get_mut(&site).with_context(|| {
            format!("no linked site named `{site}` (watching applies to linked sites)")
        })?;
        // `None` is the default (on), so only the off case needs storing.
        link.watch = if on { None } else { Some(false) };
        let runtime = link.runtime.clone();
        self.save()?;
        self.reconcile_site(&site);

        let note = match runtime.as_deref() {
            None | Some("fpm") | Some("") => {
                " (it takes effect when the site moves to an Octane runtime — php-fpm reads your code fresh on every request)"
            }
            _ => "",
        };
        Ok(if on {
            format!("`{site}` reloads its Octane workers when you save{note}.")
        } else {
            format!("`{site}` no longer reloads on save — use `dpl octane reload {site}`{note}.")
        })
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
                self.reconcile_site(&name);
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
                self.reconcile_site(&name);
                Ok(format!(
                    "Preload on for {name}.test using {rel} — its own php-fpm master will \
                     compile it into opcache at startup. Preloaded code is frozen until the \
                     master restarts, so keep the script to vendor/framework, not app code."
                ))
            }
            None => {
                link.preload = None;
                self.save()?;
                self.reconcile_site(&name);
                Ok(format!("Preload off for {name}.test — it folds back into the shared master."))
            }
        }
    }

    /// Apply a project's `dpl.toml` spec declaratively: the link ends up
    /// matching the spec exactly (absent keys = defaults), in one save + one
    /// reconcile. Returns (site name, warnings) — a bad preload path or an
    /// unparsable Xdebug mode degrades that one setting with a warning rather
    /// than failing the whole `dpl up`.
    pub fn apply_spec(&mut self, path: &str, spec: &dpl_core::spec::SiteSpec) -> Result<(String, Vec<String>)> {
        let path = canonicalize(path)?;
        let name = match &spec.name {
            Some(n) => n.to_lowercase(),
            None => sites::name_for(&path).context("could not derive a site name from the path")?,
        };
        let mut warnings: Vec<String> = Vec::new();

        // Validate the degradable settings up front.
        let xdebug = match &spec.xdebug {
            Some(m) => match Mode::parse(m) {
                Ok(parsed) if !parsed.is_off() => Some(parsed.to_string()),
                Ok(_) => None,
                Err(e) => {
                    warnings.push(format!("ignoring xdebug: {e:#}"));
                    None
                }
            },
            None => None,
        };
        let runtime = match spec.runtime.as_deref() {
            None | Some("fpm") | Some("") => None,
            Some(r @ ("octane-swoole" | "octane-roadrunner" | "octane-frankenphp")) => Some(r.to_string()),
            Some(other) => {
                warnings.push(format!("ignoring unknown runtime `{other}`"));
                None
            }
        };
        let preload = match &spec.preload {
            Some(rel) if path.join(rel).is_file() => Some(std::path::PathBuf::from(rel)),
            Some(rel) => {
                warnings.push(format!("ignoring preload: no script at {rel}"));
                None
            }
            None => None,
        };
        // A dev script that doesn't exist would become a supervised process that
        // dies five times and gives up quietly — degrade loudly instead, exactly
        // as `preload` does for a missing script.
        let dev = match &spec.dev {
            Some(script) if dpl_core::node::read_scripts(&path).iter().any(|s| s == script) => {
                Some(script.clone())
            }
            Some(script) if !path.join("package.json").is_file() => {
                warnings.push(format!("ignoring dev: no package.json to run `{script}` from"));
                None
            }
            Some(script) => {
                warnings.push(format!("ignoring dev: no `{script}` script in package.json"));
                None
            }
            None => None,
        };
        if let Some(v) = &spec.php {
            if dpl_core::php::resolve(v).is_none() {
                warnings.push(format!("PHP {v} isn't installed — the site will run on the default until it is (`dpl php install {v}`)"));
            }
        }

        // Keep the live-branch marker when the same database stays attached;
        // a new attachment starts tracking the checked-out branch.
        let db_branch = match &spec.database {
            Some(db) => match self.config.links.get(&name) {
                Some(l) if l.database.as_deref() == Some(db.as_str()) && l.db_branch.is_some() => l.db_branch.clone(),
                _ => dpl_core::branchdb::git_branch(&path),
            },
            None => None,
        };

        self.config.links.insert(
            name.clone(),
            dpl_core::config::Link {
                dev,
                path,
                php: spec.php.clone(),
                secure: spec.secure,
                runtime,
                watch: spec.watch,
                xdebug,
                profile: spec.profile,
                preload,
                database: spec.database.clone(),
                db_branch,
                db_port: spec.db_port.filter(|p| *p != 5432),
            },
        );
        self.save()?;
        self.reconcile_site(&name);
        Ok((name, warnings))
    }

    /// Capture the site linked at `path` as a `SiteSpec` (for `dpl up --save`).
    /// `services` is left empty — which engines a project needs is knowledge
    /// the machine doesn't have; the user adds them to the file.
    pub fn export_spec(&self, path: &str) -> Result<dpl_core::spec::SiteSpec> {
        let path = canonicalize(path)?;
        let (name, link) = self
            .config
            .links
            .iter()
            .find(|(_, l)| l.path == path)
            .with_context(|| format!("{} is not a linked site — `dpl link` it first", path.display()))?;
        Ok(dpl_core::spec::SiteSpec {
            // Only carry the name when it isn't just the folder name.
            name: (sites::name_for(&path).as_deref() != Some(name.as_str())).then(|| name.clone()),
            php: link.php.clone(),
            secure: link.secure,
            runtime: link.runtime.clone(),
            watch: link.watch,
            xdebug: link.xdebug.clone(),
            profile: link.profile,
            preload: link.preload.as_ref().map(|p| p.to_string_lossy().into_owned()),
            database: link.database.clone(),
            db_port: link.db_port,
            services: Vec::new(),
            dev: link.dev.clone(),
        })
    }

    /// A linked site's branch-DB state. `database`/`db_branch` are None until
    /// `dpl db attach`.
    pub fn branch_db_state(&self, site: &str) -> Result<BranchDbState> {
        let site = site.to_lowercase();
        let link = self
            .config
            .links
            .get(&site)
            .with_context(|| format!("{site} is not linked (branch databases apply to linked sites)"))?;
        Ok(BranchDbState {
            path: link.path.clone(),
            database: link.database.clone(),
            db_branch: link.db_branch.clone(),
            port: link.db_port,
        })
    }

    /// Every attached site, for the auto-switch watcher: (site, state).
    pub fn attached_branch_dbs(&self) -> Vec<(String, BranchDbState)> {
        self.config
            .links
            .iter()
            .filter(|(_, l)| l.database.is_some())
            .map(|(name, l)| {
                (name.clone(), BranchDbState {
                    path: l.path.clone(),
                    database: l.database.clone(),
                    db_branch: l.db_branch.clone(),
                    port: l.db_port,
                })
            })
            .collect()
    }

    /// Persist a site's branch-DB config: base database, instance port, and the
    /// branch currently live. All-None detaches. No reconcile — the database
    /// mapping doesn't affect how the site is served.
    pub fn set_branch_db(
        &mut self,
        site: &str,
        database: Option<String>,
        port: Option<u16>,
        branch: Option<String>,
    ) -> Result<()> {
        let site = site.to_lowercase();
        let link = self
            .config
            .links
            .get_mut(&site)
            .with_context(|| format!("{site} is not linked"))?;
        link.database = database;
        link.db_branch = branch;
        link.db_port = port;
        self.save()
    }

    pub fn set_secure(&mut self, name: &str, secure: bool) -> Result<String> {
        let name = name.to_lowercase();
        match self.config.links.get_mut(&name) {
            Some(link) => {
                link.secure = secure;
                self.save()?;
                self.reconcile_site(&name);
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
