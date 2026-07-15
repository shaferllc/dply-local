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
    match std::fs::read_to_string(project.join("composer.json")) {
        Ok(text) => framework_from_composer(&text),
        Err(_) => framework_without_composer(project),
    }
}

/// The PHP version constraint a project requires (composer.json `require.php`,
/// e.g. `"^8.3"`) — used to flag PHP compatibility / suggest per-site isolation.
pub fn detect_required_php(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("composer.json")).ok()?;
    required_php_from_composer(&text)
}

/// Framework label **and** required-PHP constraint from a single read of
/// `composer.json` (the framework still falls back to well-known files when
/// there is no composer.json). `detect_framework` + `detect_required_php` read
/// the same file twice; the daemon's hot `dpl sites` path uses this instead so
/// each project's composer.json is read once per reconcile, not twice per call.
pub fn detect_meta(project: &Path) -> (Option<String>, Option<String>) {
    match std::fs::read_to_string(project.join("composer.json")) {
        Ok(text) => (framework_from_composer(&text), required_php_from_composer(&text)),
        Err(_) => (framework_without_composer(project), None),
    }
}

/// Parse a framework label out of already-read `composer.json` text. A composer
/// project with no framework we recognise is still `PHP (Composer)`.
fn framework_from_composer(text: &str) -> Option<String> {
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
    Some("PHP (Composer)".into())
}

/// Framework detection for a project without a readable `composer.json`.
fn framework_without_composer(project: &Path) -> Option<String> {
    if project.join("wp-config.php").is_file() || project.join("wp-load.php").is_file() {
        return Some("WordPress".into());
    }
    if project.join("index.php").is_file() {
        return Some("PHP".into());
    }
    None
}

/// Parse the `require.php` constraint out of already-read `composer.json` text.
fn required_php_from_composer(text: &str) -> Option<String> {
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
    /// Effective Xdebug mode: the site's own setting, else the config default.
    /// An unparsable stored value degrades to `off` rather than failing the
    /// whole reconcile over one bad site.
    pub xdebug: crate::xdebug::Mode,
    /// Whether the SPX profiler is on for this site.
    pub profile: bool,
    /// Absolute path to this site's opcache preload script, if configured. A
    /// preloaded site gets its own php-fpm master. Resolved from `Link.preload`
    /// against the project root; parked sites are always `None`.
    pub preload: Option<PathBuf>,
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

    let mode_of = |raw: Option<&String>| -> crate::xdebug::Mode {
        raw.or(config.default_xdebug.as_ref())
            .map(|m| crate::xdebug::Mode::parse(m).unwrap_or_default())
            .unwrap_or_default()
    };

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
            xdebug: mode_of(link.xdebug.as_ref()),
            profile: link.profile,
            preload: link.preload.as_ref().map(|p| link.path.join(p)),
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
                xdebug: mode_of(None),
                profile: false,
                preload: None,
            });
        }
    }

    sites.sort_by(|a, b| a.name.cmp(&b.name));
    sites
}

/// Resolve a single site by name without scanning every parked directory —
/// the incremental-reconcile fast path. Mirrors [`resolve`]'s precedence
/// (links win over parked dirs; first parked dir wins) but touches only the
/// named site: a map lookup for links, a `join(name).is_dir()` stat per
/// parked root otherwise. `None` = no such site anymore.
pub fn resolve_one(config: &LocalConfig, name: &str) -> Option<ResolvedSite> {
    let name = name.to_lowercase();
    if name.is_empty() || name.starts_with('.') {
        return None;
    }
    let tld = config.primary_tld();
    let mode_of = |raw: Option<&String>| -> crate::xdebug::Mode {
        raw.or(config.default_xdebug.as_ref())
            .map(|m| crate::xdebug::Mode::parse(m).unwrap_or_default())
            .unwrap_or_default()
    };

    if let Some(link) = config.links.get(&name) {
        return Some(ResolvedSite {
            name,
            docroot: docroot_for(&link.path),
            path: link.path.clone(),
            php: link.php.clone().or_else(|| config.default_php.clone()),
            secure: link.secure,
            source: SiteSource::Linked,
            tld,
            runtime: link.runtime.clone(),
            xdebug: mode_of(link.xdebug.as_ref()),
            profile: link.profile,
            preload: link.preload.as_ref().map(|p| link.path.join(p)),
        });
    }

    for parked in &config.parked {
        let path = parked.join(&name);
        if path.is_dir() {
            return Some(ResolvedSite {
                name,
                docroot: docroot_for(&path),
                path,
                php: config.default_php.clone(),
                secure: false,
                source: SiteSource::Parked,
                tld,
                runtime: None,
                xdebug: mode_of(None),
                profile: false,
                preload: None,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a composer.json into a fresh temp project dir and return the dir.
    fn project_with_composer(body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dpl-sites-test-{}", uniq()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("composer.json"), body).unwrap();
        dir
    }

    // No Date/rand in tests either; a process-unique counter is enough.
    fn uniq() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::process::id() as u64 * 1_000_000 + N.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn detects_laravel_with_version() {
        let dir = project_with_composer(
            r#"{"require":{"php":"^8.3","laravel/framework":"^11.0"}}"#,
        );
        assert_eq!(detect_framework(&dir), Some("Laravel (^11.0)".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn composer_without_known_framework_is_php_composer() {
        let dir = project_with_composer(r#"{"require":{"php":">=8.1"}}"#);
        assert_eq!(detect_framework(&dir), Some("PHP (Composer)".into()));
        assert_eq!(detect_required_php(&dir), Some(">=8.1".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn required_php_absent_is_none() {
        let dir = project_with_composer(r#"{"require":{"laravel/framework":"^11.0"}}"#);
        assert_eq!(detect_required_php(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_composer_falls_back_to_well_known_files() {
        let dir = std::env::temp_dir().join(format!("dpl-sites-test-{}", uniq()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wp-config.php"), "<?php").unwrap();
        assert_eq!(detect_framework(&dir), Some("WordPress".into()));
        assert_eq!(detect_required_php(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The single-read `detect_meta` must agree with the two separate readers it
    /// replaces on the hot path — this is the invariant the caching relies on.
    #[test]
    fn detect_meta_matches_the_two_separate_readers() {
        let cases = [
            r#"{"require":{"php":"^8.2","laravel/framework":"^12.0"}}"#,
            r#"{"require":{"symfony/framework-bundle":"^7.0"}}"#,
            r#"{"require":{"php":"~8.1"}}"#,
            r#"{}"#,
        ];
        for body in cases {
            let dir = project_with_composer(body);
            let (fw, php) = detect_meta(&dir);
            assert_eq!(fw, detect_framework(&dir), "framework mismatch for {body}");
            assert_eq!(php, detect_required_php(&dir), "required_php mismatch for {body}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
