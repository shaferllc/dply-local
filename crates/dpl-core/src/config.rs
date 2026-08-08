//! Local-dev configuration persisted at `~/.dpl/config.toml`.
//!
//! This is the daemon's source of truth for what to serve: which folders
//! are "parked" (every subdirectory auto-serves as `<dir>.test`), which
//! individual projects are "linked", and per-site overrides (PHP version,
//! HTTPS). Phase 0 only defines the shape and load/save; the daemon starts
//! consuming it in Phase 2.
//!
//! Writes are atomic (temp file + rename) and the file is created 0600 so
//! another local user can't read paths/secrets out of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// The default TLD local sites answer on when none is configured.
pub const SITE_TLD: &str = "test";

/// TLDs the operating system already resolves to loopback (RFC 6761), so dpl
/// needs no `/etc/resolver` entry and no sudo for them — the `*.localhost`
/// opt-out. `<name>.localhost` reaches dpl on :80 with zero DNS setup.
pub fn is_native_tld(tld: &str) -> bool {
    tld.eq_ignore_ascii_case("localhost")
}

/// Clean up user-supplied tags: lowercased, trimmed, spaces and underscores
/// folded to `-`, empties dropped, deduped, sorted.
///
/// Normalising is what makes tags a *grouping* rather than a pile of near-misses
/// — without it `Client X`, `client x` and `client-x` are three separate tags
/// and the fleet view is worse than no tags at all.
pub fn normalize_tags<I, S>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = tags
        .into_iter()
        .map(|t| {
            t.as_ref()
                .trim()
                .to_lowercase()
                .chars()
                .map(|c| if c.is_whitespace() || c == '_' { '-' } else { c })
                .collect::<String>()
        })
        // Collapse the runs of `-` that folding can produce ("client  x").
        .map(|t| {
            let mut s = String::with_capacity(t.len());
            let mut last_dash = false;
            for c in t.chars() {
                if c == '-' {
                    if !last_dash && !s.is_empty() {
                        s.push(c);
                    }
                    last_dash = true;
                } else {
                    s.push(c);
                    last_dash = false;
                }
            }
            s.trim_end_matches('-').to_string()
        })
        .filter(|t| !t.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalConfig {
    /// TLDs sites answer on (e.g. `["test", "localhost"]`). The first is
    /// "primary" and used to build a site's canonical URL; requests to any
    /// listed TLD are served. Empty means the built-in default (`test`).
    pub tlds: Vec<String>,
    /// Folders whose immediate subdirectories each become a `<name>.<tld>` site.
    pub parked: Vec<PathBuf>,
    /// Explicitly linked projects, keyed by site name.
    pub links: BTreeMap<String, Link>,
    /// Reverse proxies: site name → target base URL (e.g. `blog` →
    /// `http://localhost:3000`). Requests to `<name>.<tld>` are forwarded to
    /// the target instead of being served from disk.
    pub proxies: BTreeMap<String, String>,
    /// Default PHP version applied to sites with no explicit override
    /// (e.g. `"8.4"`). `None` means "use whatever `php` is on PATH".
    pub default_php: Option<String>,
    /// How `.test` names resolve: `"resolver"` (default — a wildcard
    /// `/etc/resolver` entry pointing at dpl's DNS) or `"hosts"` (per-site
    /// `/etc/hosts` entries, which keep iCloud Private Relay working since no
    /// local DNS proxy is involved).
    pub resolution: Option<String>,
    /// Xdebug mode applied to sites with no explicit override. `None` means
    /// `off`, so Xdebug costs nothing until a site asks for it.
    pub default_xdebug: Option<String>,
    /// IDE-side Xdebug connection settings, shared by every site.
    pub xdebug: crate::xdebug::Settings,
    /// Free-form labels per site name — `client-x`, `archive`, `wip`.
    ///
    /// Keyed by site *name* rather than stored on the link, because roughly half
    /// a typical fleet is parked rather than linked, and a parked site has no
    /// link to hang metadata on. Tags describe the site, not how it was
    /// registered. Normalised on the way in (see [`normalize_tags`]).
    pub tags: BTreeMap<String, Vec<String>>,
    /// Ceiling on worker processes per php-fpm pool. `None` means
    /// [`DEFAULT_FPM_MAX_CHILDREN`].
    ///
    /// Worth tuning because it is a *memory* ceiling, not a concurrency one, and
    /// it applies per pool rather than per machine: dpl runs one master per
    /// (PHP version, Xdebug mode, profiler, preload), so several pools can be
    /// live at once and each may grow to this many workers. At the ~100 MB a
    /// Laravel worker typically resides at, the default is already ~4 GB per
    /// pool if anything ever saturates it.
    pub fpm_max_children: Option<u32>,
}

/// Workers per pool when the config says nothing. Enough that a browser opening
/// many parallel requests to one site doesn't queue, low enough that a runaway
/// app can't exhaust memory before `pm.process_idle_timeout` reaps the workers.
pub const DEFAULT_FPM_MAX_CHILDREN: u32 = 40;

impl LocalConfig {
    /// Workers allowed per php-fpm pool. Clamped to at least 1, since a pool
    /// with no workers accepts connections and then never answers them — a
    /// hang that presents as a gateway timeout rather than a config error.
    pub fn fpm_max_children(&self) -> u32 {
        self.fpm_max_children.unwrap_or(DEFAULT_FPM_MAX_CHILDREN).max(1)
    }
}

impl LocalConfig {
    /// Configured TLDs, falling back to the built-in default.
    pub fn tlds(&self) -> Vec<String> {
        if self.tlds.is_empty() {
            vec![SITE_TLD.to_string()]
        } else {
            self.tlds.clone()
        }
    }

    /// The primary TLD used to build canonical site URLs.
    pub fn primary_tld(&self) -> String {
        self.tlds.first().cloned().unwrap_or_else(|| SITE_TLD.to_string())
    }

    /// True when `.test` names are resolved via `/etc/hosts` entries rather than
    /// a local DNS resolver (keeps iCloud Private Relay working).
    pub fn uses_hosts(&self) -> bool {
        self.resolution.as_deref() == Some("hosts")
    }
}

/// A user-created database/cache instance: a specific engine version on its own
/// port with an isolated data directory. Persisted in `~/.dpl/services.toml`
/// so it survives restarts and can be supervised on boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInstance {
    /// Unique instance name (also the data-dir folder).
    pub name: String,
    /// Engine: `postgres` | `mysql` | `mariadb` | `redis`.
    pub engine: String,
    /// Version label (e.g. `17.0`), matched against discovered binaries.
    pub version: String,
    /// Loopback port this instance listens on.
    pub port: u16,
}

/// The service-instance registry (`~/.dpl/services.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesConfig {
    pub instances: Vec<ServiceInstance>,
}

impl ServicesConfig {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) if raw.trim().is_empty() => Ok(Self::default()),
            Ok(raw) => toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
            }
        }
        let body = toml::to_string_pretty(self)?;
        atomic_write(path, body.as_bytes())
    }
}

/// A single linked project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Absolute path to the project root.
    pub path: PathBuf,
    /// Pinned PHP version for this site, if any (overrides `default_php`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    /// Whether this site is served over HTTPS (Phase 4).
    #[serde(default)]
    pub secure: bool,
    /// Application-server runtime: `None`/`"fpm"` = php-fpm (default), or a
    /// Laravel Octane server: `"octane-swoole"`, `"octane-roadrunner"`,
    /// `"octane-frankenphp"`. Non-fpm runtimes are supervised by the daemon and
    /// reverse-proxied instead of served over FastCGI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Whether the daemon watches this site's PHP sources and reloads its Octane
    /// workers when they change. `None` = the default, which is on: an Octane
    /// worker holds the application in memory, so a site that doesn't reload is
    /// a site where the edit you just made hasn't happened. Only meaningful
    /// alongside an Octane `runtime`. See `dpl octane watch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<bool>,
    /// package.json script the daemon runs as this site's dev server (`"dev"`,
    /// `"watch"`, …). None = off, which is the default and stays the default:
    /// starting a dev server per site is opt-in because a fleet of them is a
    /// fleet of long-lived Node processes. See `dpl dev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
    /// Xdebug mode for this site (overrides `default_xdebug`). A site with a
    /// mode of its own gets its own php-fpm master.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xdebug: Option<String>,
    /// Whether the SPX flame-graph profiler is on for this site. A profiled site
    /// gets its own php-fpm master with SPX loaded and auto-profiling every
    /// request. See `dpl profile`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub profile: bool,
    /// Opcache preload script for this site, relative to the project root. When
    /// set, the site gets its own php-fpm master with `opcache.preload` pointed
    /// at it, so the script's (typically vendor) code is compiled into shared
    /// memory once at master start — eliminating the first-request compile.
    /// None = share the common master. See `dpl preload`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload: Option<PathBuf>,
    /// Base database name for branch-aware databases (Postgres). When set, this
    /// database always holds the checked-out branch's data and other branches
    /// are parked as `<database>@<branch>`. None = feature off. See `dpl db attach`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Which git branch's data currently occupies `database`. Maintained by the
    /// daemon on every switch; meaningless when `database` is None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_branch: Option<String>,
    /// Postgres port the branch databases live on, when not the default 5432.
    /// Recorded at attach so the auto-switch watcher targets the right instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_port: Option<u16>,
}

impl LocalConfig {
    /// Load from `path`, returning defaults if the file doesn't exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) if raw.trim().is_empty() => Ok(Self::default()),
            Ok(raw) => toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    /// Serialize and atomically write to `path`, creating the parent
    /// directory (0700) and the file (0600) if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let body = toml::to_string_pretty(self)?;
        atomic_write(path, body.as_bytes())
    }
}

/// Create `dir` (and parents) with mode 0700 on Unix. A no-op if it exists.
fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
    set_mode(dir, 0o700)?;
    Ok(())
}

/// Write `bytes` to `path` via a temp file + rename so a reader never sees
/// a half-written config. The file ends up mode 0600.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| CoreError::io(&tmp, e))?;
    set_mode(&tmp, 0o600)?;
    std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, perms).map_err(|e| CoreError::io(path, e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tag_tests {
    use super::normalize_tags;

    /// The point of normalising: three spellings of one idea must collapse to
    /// one tag, or grouping by tag is worse than not having tags.
    #[test]
    fn spellings_of_one_tag_collapse() {
        assert_eq!(
            normalize_tags(["Client X", "client x", "client-x", "CLIENT_X"]),
            vec!["client-x"]
        );
    }

    #[test]
    fn empties_and_stray_dashes_are_dropped() {
        assert_eq!(normalize_tags(["  ", "", "-", "wip "]), vec!["wip"]);
        assert_eq!(normalize_tags(["client   x"]), vec!["client-x"]);
        assert_eq!(normalize_tags(["trailing-"]), vec!["trailing"]);
    }

    #[test]
    fn results_are_sorted_so_display_is_stable() {
        assert_eq!(normalize_tags(["wip", "archive", "client-x"]), vec!["archive", "client-x", "wip"]);
    }
}
