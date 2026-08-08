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
    let meta = detect_project(project);
    (meta.framework, meta.requires_php)
}

/// The coarse bucket a project falls into — what it *is*, as distinct from which
/// framework it uses. This is the axis worth grouping a site list by: "show me
/// the Node projects" is a question a fleet of a hundred sites makes you ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectKind {
    /// Served by PHP — the composer projects and the bare-`index.php` ones.
    Php,
    /// A JavaScript project: `package.json` and no PHP behind it.
    Node,
    /// Plain files, no runtime.
    Static,
    /// A project in some other language. Not servable as a `.test` site, but
    /// worth naming: a parked folder full of these is a fleet you can prune,
    /// and "unknown" tells you nothing about which ones those are.
    Other,
    /// Nothing recognisable — an empty folder, a monorepo parent, a dead path.
    #[default]
    Unknown,
}

impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectKind::Php => "php",
            ProjectKind::Node => "node",
            ProjectKind::Static => "static",
            ProjectKind::Other => "other",
            ProjectKind::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<ProjectKind> {
        match s.trim().to_lowercase().as_str() {
            "php" => Some(ProjectKind::Php),
            "node" => Some(ProjectKind::Node),
            "static" => Some(ProjectKind::Static),
            "other" => Some(ProjectKind::Other),
            "unknown" => Some(ProjectKind::Unknown),
            _ => None,
        }
    }
}

/// Everything detection can say about a project from its manifests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectMeta {
    /// The headline label — what the site *is*, e.g. `Laravel (^13.0)`,
    /// `Next.js (^15.0)`, `WordPress`.
    pub framework: Option<String>,
    pub kind: ProjectKind,
    /// composer.json `require.php`.
    pub requires_php: Option<String>,
    /// The JavaScript framework, when it isn't the headline — a Laravel app with
    /// a Vue front end is a Laravel site *and* a Vue codebase, and hiding the
    /// second fact makes the fleet look more uniform than it is.
    pub node_framework: Option<String>,
    /// The PHP-side stack built on top of the framework: Filament, Inertia,
    /// Livewire, Nova. Which one a project uses changes how you work in it far
    /// more than the Laravel version does, yet a fleet of 56 Laravel apps
    /// reports 56 identical labels without it.
    pub stack: Option<String>,
}

/// Identify a project from its manifests.
///
/// PHP wins the headline whenever a `composer.json` is present: that's what
/// actually serves the site here, and a Laravel app's `package.json` describes
/// its asset pipeline, not its identity. The JavaScript side is still reported,
/// just not as the primary label.
pub fn detect_project(project: &Path) -> ProjectMeta {
    let composer = std::fs::read_to_string(project.join("composer.json")).ok();
    let package = std::fs::read_to_string(project.join("package.json")).ok();
    let node_framework = package.as_deref().and_then(node_framework_from_package);

    if let Some(text) = composer {
        return ProjectMeta {
            framework: framework_from_composer(&text),
            kind: ProjectKind::Php,
            requires_php: required_php_from_composer(&text),
            node_framework,
            stack: php_stack(&text),
        };
    }

    // No composer.json. A PHP entry point still means a PHP site (WordPress
    // installs and hand-rolled projects both land here).
    if let Some(php) = framework_without_composer(project) {
        return ProjectMeta {
            framework: Some(php),
            kind: ProjectKind::Php,
            requires_php: None,
            node_framework,
            stack: None,
        };
    }

    if package.is_some() {
        return ProjectMeta {
            // A package.json with nothing recognisable in it is still a Node
            // project — say so rather than shrugging.
            framework: node_framework.clone().or_else(|| Some("Node".into())),
            kind: ProjectKind::Node,
            requires_php: None,
            node_framework,
            stack: None,
        };
    }

    if project.join("index.html").is_file() {
        return ProjectMeta {
            framework: Some("Static".into()),
            kind: ProjectKind::Static,
            ..Default::default()
        };
    }

    if let Some(label) = other_language(project) {
        return ProjectMeta {
            framework: Some(label.into()),
            kind: ProjectKind::Other,
            ..Default::default()
        };
    }

    ProjectMeta::default()
}

/// A package's version constraint from already-read `composer.json` text.
///
/// A lightweight, dependency-free scan: find `"pkg"`, then the quoted value
/// after the following colon. It reads `require` and `require-dev` alike, since
/// it doesn't distinguish sections — which is what we want here, as Breeze and
/// friends are dev dependencies that still shape the whole app.
fn composer_version(text: &str, pkg: &str) -> Option<String> {
    let key = format!("\"{pkg}\"");
    let after = &text[text.find(&key)? + key.len()..];
    let rest = &after[after.find(':')? + 1..];
    let q1 = rest.find('"')?;
    let q2 = rest[q1 + 1..].find('"')?;
    Some(rest[q1 + 1..=q1 + q2].to_string())
}

/// PHP stacks layered on a framework, most determining first.
///
/// Order encodes what actually shapes day-to-day work. Filament is built *on*
/// Livewire and Jetstream ships *either* Livewire or Inertia, so the more
/// specific choice has to win or every Filament panel reports as "Livewire".
/// Below that, the frontend architecture (Inertia vs Livewire) tells you more
/// about how a page is built than the auth scaffolding does.
const PHP_STACKS: &[(&str, &str)] = &[
    ("filament/filament", "Filament"),
    ("laravel/nova", "Nova"),
    ("inertiajs/inertia-laravel", "Inertia"),
    ("livewire/volt", "Volt"),
    ("livewire/livewire", "Livewire"),
    ("laravel/jetstream", "Jetstream"),
    ("laravel/breeze", "Breeze"),
];

/// The stack a composer project layers on its framework, with its version.
/// Scans `require` and `require-dev` alike — Breeze is a dev dependency, and
/// missing it would hide the scaffolding the whole app is shaped by.
fn php_stack(text: &str) -> Option<String> {
    PHP_STACKS.iter().find_map(|(pkg, label)| {
        composer_version(text, pkg)
            .map(|v| if v.is_empty() { label.to_string() } else { format!("{label} ({v})") })
    })
}

/// Manifests of languages dpl doesn't serve. Checked last, so a Laravel app that
/// happens to vendor a Go tool is still Laravel.
const OTHER_MANIFESTS: &[(&str, &str)] = &[
    ("Cargo.toml", "Rust"),
    ("go.mod", "Go"),
    ("Gemfile", "Ruby"),
    ("pyproject.toml", "Python"),
    ("requirements.txt", "Python"),
    ("Package.swift", "Swift"),
    ("pubspec.yaml", "Dart"),
    ("mix.exs", "Elixir"),
];

fn other_language(project: &Path) -> Option<&'static str> {
    OTHER_MANIFESTS
        .iter()
        .find(|(file, _)| project.join(file).is_file())
        .map(|(_, label)| *label)
}

/// JavaScript frameworks, most specific first.
///
/// Order is the whole design. Every Next.js app depends on `react`, every Nuxt
/// app on `vue`, and a SvelteKit app on `svelte` — so the meta-framework has to
/// be checked before the view library, and the view library before the build
/// tool, or every site in the fleet reports as "React".
const NODE_FRAMEWORKS: &[(&str, &str)] = &[
    // Meta-frameworks (they pull in a view library of their own).
    ("next", "Next.js"),
    ("nuxt", "Nuxt"),
    ("@sveltejs/kit", "SvelteKit"),
    ("astro", "Astro"),
    ("@remix-run/react", "Remix"),
    ("gatsby", "Gatsby"),
    ("@angular/core", "Angular"),
    ("@docusaurus/core", "Docusaurus"),
    ("vitepress", "VitePress"),
    ("vuepress", "VuePress"),
    ("@11ty/eleventy", "Eleventy"),
    ("@builder.io/qwik", "Qwik"),
    // Application platforms.
    ("@nestjs/core", "NestJS"),
    ("expo", "Expo"),
    ("react-native", "React Native"),
    ("electron", "Electron"),
    // Servers.
    ("fastify", "Fastify"),
    ("hono", "Hono"),
    ("koa", "Koa"),
    ("express", "Express"),
    // View libraries.
    ("svelte", "Svelte"),
    ("solid-js", "Solid"),
    ("vue", "Vue"),
    ("react", "React"),
    // Build tooling — the weakest signal, so it goes last.
    ("vite", "Vite"),
    ("laravel-mix", "Mix"),
    ("webpack", "Webpack"),
];

/// The JavaScript framework a `package.json` describes, with its version
/// constraint. Looks in `dependencies` and `devDependencies` alike: a Vite or
/// Astro project keeps its framework in devDependencies, and insisting on the
/// runtime section would miss exactly the front-end projects this is for.
fn node_framework_from_package(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let sections = ["dependencies", "devDependencies"];
    let version_of = |pkg: &str| -> Option<String> {
        sections.iter().find_map(|section| {
            value
                .get(section)?
                .get(pkg)?
                .as_str()
                .map(|v| v.to_string())
        })
    };
    let (label, version) = NODE_FRAMEWORKS
        .iter()
        .find_map(|(pkg, label)| version_of(pkg).map(|v| (*label, v)))?;
    Some(if version.is_empty() { label.to_string() } else { format!("{label} ({version})") })
}

/// Parse a framework label out of already-read `composer.json` text. A composer
/// project with no framework we recognise is still `PHP (Composer)`.
fn framework_from_composer(text: &str) -> Option<String> {
    let ver = |pkg: &str| composer_version(text, pkg);
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
    /// Whether the daemon reloads this site's Octane workers when its sources
    /// change. Defaults to on; meaningless without an Octane `runtime`.
    pub watch: bool,
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
    /// package.json script the daemon supervises as this site's dev server.
    /// `None` = none; parked sites never have one (there is no link to hold it).
    pub dev: Option<String>,
    /// User-assigned tags. Parked sites have none — tags live on the link.
    pub tags: Vec<String>,
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
        let tags = config.tags.get(&name).cloned().unwrap_or_default();
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
            watch: link.watch.unwrap_or(true),
            dev: link.dev.clone(),
            tags,
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
            let tags = config.tags.get(&name).cloned().unwrap_or_default();
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
                watch: true,
                dev: None,
                tags,
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
    let tags = config.tags.get(&name).cloned().unwrap_or_default();
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
            watch: link.watch.unwrap_or(true),
            dev: link.dev.clone(),
            tags,
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
                watch: true,
                dev: None,
                tags,
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

    /// A project dir with a package.json (and optionally a composer.json).
    fn project_with(files: &[(&str, &str)], tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dpl-kind-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        dir
    }

    fn pkg(deps: &str) -> String {
        format!(r#"{{"name":"x","dependencies":{deps}}}"#)
    }

    #[test]
    fn detects_node_frameworks_with_their_versions() {
        let cases = [
            (pkg(r#"{"next":"^15.0.1","react":"^19.0.0"}"#), "Next.js (^15.0.1)"),
            (pkg(r#"{"nuxt":"^3.12.0","vue":"^3.4.0"}"#), "Nuxt (^3.12.0)"),
            (pkg(r#"{"@sveltejs/kit":"^2.5.0","svelte":"^4.2.0"}"#), "SvelteKit (^2.5.0)"),
            (pkg(r#"{"@nestjs/core":"^10.0.0"}"#), "NestJS (^10.0.0)"),
            (pkg(r#"{"express":"^4.19.2"}"#), "Express (^4.19.2)"),
            (pkg(r#"{"vue":"^3.4.0"}"#), "Vue (^3.4.0)"),
            (pkg(r#"{"react":"^19.0.0"}"#), "React (^19.0.0)"),
        ];
        for (body, expected) in cases {
            let dir = project_with(&[("package.json", &body)], "fw");
            let meta = detect_project(&dir);
            assert_eq!(meta.framework.as_deref(), Some(expected));
            assert_eq!(meta.kind, ProjectKind::Node);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The ordering invariant: every Next app depends on React and every Nuxt app
    /// on Vue, so a naive scan reports the whole fleet as React/Vue.
    #[test]
    fn the_meta_framework_beats_the_view_library_it_bundles() {
        let dir = project_with(&[("package.json", &pkg(r#"{"react":"^19.0.0","next":"^15.0.0"}"#))], "order");
        assert_eq!(detect_project(&dir).framework.as_deref(), Some("Next.js (^15.0.0)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Astro, Vite and friends live in devDependencies — insisting on runtime
    /// deps would miss exactly the front-end projects this is for.
    #[test]
    fn dev_dependencies_count_too() {
        let body = r#"{"name":"x","devDependencies":{"astro":"^4.5.0"}}"#;
        let dir = project_with(&[("package.json", body)], "dev");
        assert_eq!(detect_project(&dir).framework.as_deref(), Some("Astro (^4.5.0)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Laravel app with a Vue front end is a Laravel *site* — PHP serves it —
    /// but the Vue is still worth reporting.
    #[test]
    fn php_wins_the_headline_and_node_rides_along() {
        let dir = project_with(
            &[
                ("composer.json", r#"{"require":{"php":"^8.3","laravel/framework":"^12.0"}}"#),
                ("package.json", &pkg(r#"{"vue":"^3.4.0","vite":"^5.0.0"}"#)),
            ],
            "both",
        );
        let meta = detect_project(&dir);
        assert_eq!(meta.framework.as_deref(), Some("Laravel (^12.0)"));
        assert_eq!(meta.kind, ProjectKind::Php);
        assert_eq!(meta.requires_php.as_deref(), Some("^8.3"));
        assert_eq!(meta.node_framework.as_deref(), Some("Vue (^3.4.0)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_package_json_with_nothing_known_is_still_node() {
        let dir = project_with(&[("package.json", r#"{"name":"x"}"#)], "bare");
        let meta = detect_project(&dir);
        assert_eq!(meta.kind, ProjectKind::Node);
        assert_eq!(meta.framework.as_deref(), Some("Node"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_of_files_is_static_and_an_empty_one_is_unknown() {
        let dir = project_with(&[("index.html", "<h1>hi</h1>")], "static");
        assert_eq!(detect_project(&dir).kind, ProjectKind::Static);
        let _ = std::fs::remove_dir_all(&dir);

        let dir = project_with(&[("README.md", "nothing here")], "empty");
        let meta = detect_project(&dir);
        assert_eq!(meta.kind, ProjectKind::Unknown);
        assert_eq!(meta.framework, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A parked folder full of Rust and Swift checkouts should say so rather
    /// than showing two dozen identical "unknown" rows.
    #[test]
    fn projects_in_other_languages_are_named() {
        for (file, label) in [("Cargo.toml", "Rust"), ("go.mod", "Go"), ("Package.swift", "Swift")] {
            let dir = project_with(&[(file, "")], &format!("other-{label}"));
            let meta = detect_project(&dir);
            assert_eq!(meta.kind, ProjectKind::Other);
            assert_eq!(meta.framework.as_deref(), Some(label));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn detects_the_laravel_stack_with_its_version() {
        let cases = [
            (r#"{"require":{"laravel/framework":"^12.0","filament/filament":"^3.2"}}"#, "Filament (^3.2)"),
            (r#"{"require":{"laravel/framework":"^12.0","inertiajs/inertia-laravel":"^1.0"}}"#, "Inertia (^1.0)"),
            (r#"{"require":{"laravel/framework":"^12.0","livewire/livewire":"^3.5"}}"#, "Livewire (^3.5)"),
            (r#"{"require":{"laravel/nova":"^4.0"}}"#, "Nova (^4.0)"),
        ];
        for (body, expected) in cases {
            let dir = project_with_composer(body);
            let meta = detect_project(&dir);
            assert_eq!(meta.stack.as_deref(), Some(expected), "for {body}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Filament is built *on* Livewire, so a naive scan labels every Filament
    /// panel "Livewire" and the distinction that matters disappears.
    #[test]
    fn the_more_specific_stack_wins() {
        let dir = project_with_composer(
            r#"{"require":{"laravel/framework":"^12.0","livewire/livewire":"^3.5","filament/filament":"^3.2"}}"#,
        );
        assert_eq!(detect_project(&dir).stack.as_deref(), Some("Filament (^3.2)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Breeze is a dev dependency but shapes the whole app.
    #[test]
    fn a_dev_dependency_stack_still_counts() {
        let dir = project_with_composer(
            r#"{"require":{"laravel/framework":"^12.0"},"require-dev":{"laravel/breeze":"^2.0"}}"#,
        );
        assert_eq!(detect_project(&dir).stack.as_deref(), Some("Breeze (^2.0)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_laravel_app_has_no_stack() {
        let dir = project_with_composer(r#"{"require":{"laravel/framework":"^12.0"}}"#);
        let meta = detect_project(&dir);
        assert_eq!(meta.framework.as_deref(), Some("Laravel (^12.0)"));
        assert_eq!(meta.stack, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PHP app that vendors a Go tool is still a PHP app.
    #[test]
    fn a_web_manifest_beats_another_languages() {
        let dir = project_with(
            &[("composer.json", r#"{"require":{"laravel/framework":"^12.0"}}"#), ("go.mod", "module x")],
            "mixed",
        );
        assert_eq!(detect_project(&dir).kind, ProjectKind::Php);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// WordPress has no composer.json but is emphatically a PHP site.
    #[test]
    fn wordpress_is_php_even_without_composer() {
        let dir = project_with(&[("wp-config.php", "<?php")], "wp");
        let meta = detect_project(&dir);
        assert_eq!(meta.framework.as_deref(), Some("WordPress"));
        assert_eq!(meta.kind, ProjectKind::Php);
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
