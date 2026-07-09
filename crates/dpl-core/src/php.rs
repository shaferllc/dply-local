//! Detecting and resolving PHP versions, shared by the CLI (`dpl php`) and the
//! daemon (which spawns each site's backend with the right binary).
//!
//! We look in the usual macOS/Linux places: Homebrew's `php` and versioned
//! `php@8.x` formulae, and any `php`/`php8.x` on `PATH`. Version pinning stores
//! just the `major.minor` string (e.g. `"8.3"`); [`resolve`] maps that back to
//! a concrete binary.

use std::path::PathBuf;
use std::process::Command;

/// A discovered PHP install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpVersion {
    /// `major.minor`, e.g. `"8.3"`.
    pub version: String,
    /// Absolute path to the `php` binary.
    pub binary: PathBuf,
    /// Where it was found (`homebrew`, `path`).
    pub source: String,
}

/// Common Homebrew prefixes (Apple silicon, Intel) plus a Linuxbrew default.
pub const BREW_PREFIXES: &[&str] = &["/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"];

/// All PHP installs we can find, de-duplicated by version (Homebrew wins over a
/// bare `PATH` entry), sorted newest-first.
pub fn detect() -> Vec<PhpVersion> {
    let mut found: Vec<PhpVersion> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = Default::default();

    let mut push = |bin: PathBuf, source: &str| {
        if let Some(ver) = version_of(&bin) {
            if seen.insert(ver.clone()) {
                found.push(PhpVersion { version: ver, binary: bin, source: source.into() });
            }
        }
    };

    // Homebrew: opt/php and opt/php@8.x/bin/php.
    for prefix in BREW_PREFIXES {
        let opt = PathBuf::from(prefix).join("opt");
        let Ok(entries) = std::fs::read_dir(&opt) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "php" || name.starts_with("php@") {
                let bin = entry.path().join("bin/php");
                if bin.is_file() {
                    push(bin, "homebrew");
                }
            }
        }
    }

    // PATH: php, php8, php8.x.
    for candidate in ["php", "php8", "php8.4", "php8.3", "php8.2", "php8.1", "php8.0"] {
        if let Some(bin) = which(candidate) {
            push(bin, "path");
        }
    }

    found.sort_by(|a, b| b.version.cmp(&a.version));
    found
}

/// Every PHP line we know how to install via Homebrew, newest-first. Anything
/// older than core carries lives in the `shivammathur/php` tap, which the
/// installer taps on demand.
pub const KNOWN_VERSIONS: &[&str] =
    &["8.5", "8.4", "8.3", "8.2", "8.1", "8.0", "7.4", "7.3", "7.2", "7.1", "7.0", "5.6"];

/// A PHP line's install status, for the version manager. Covers both installed
/// versions and ones you *could* install.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogEntry {
    /// `major.minor`, e.g. `"8.3"`.
    pub version: String,
    /// Homebrew formula, e.g. `"php@8.3"`.
    pub formula: String,
    /// A keg exists on disk (installed, though possibly broken).
    pub installed: bool,
    /// Installed but its `opt` symlink / `php` binary is missing — needs repair.
    pub broken: bool,
    /// This is the active default version.
    pub active: bool,
    /// Full installed version (e.g. `"8.5.8"`), when a keg is present.
    pub installed_version: Option<String>,
}

/// The full catalog of installable PHP versions with per-version status —
/// backs the GUI "PHP Version Manager". Combines the on-disk keg scan with the
/// known-versions list so uninstalled lines still show up as installable.
pub fn catalog() -> Vec<CatalogEntry> {
    let prefix = brew_prefix();
    let default = default_version();

    KNOWN_VERSIONS
        .iter()
        .map(|v| {
            let version = (*v).to_string();
            let formula = format!("php@{version}");
            // A keg is present if its Cellar dir exists at all, or (for the
            // current line) the unversioned `php` formula resolves to this
            // version. An *empty* Cellar dir counts as installed-but-broken —
            // Homebrew only creates it during an install, so its emptiness
            // signals a half-installed keg to repair, not a missing one.
            let keg = prefix
                .as_ref()
                .map(|p| p.join("Cellar").join(&formula).is_dir() || cellar_php_is(p, &version))
                .unwrap_or(false);
            // The runnable binary at this version's opt path, if any.
            let binary = prefix.as_ref().and_then(|p| {
                let versioned = p.join("opt").join(&formula).join("bin/php");
                if versioned.is_file() {
                    return Some(versioned);
                }
                let unversioned = p.join("opt/php/bin/php");
                (cellar_php_is(p, &version) && unversioned.is_file()).then_some(unversioned)
            });
            let opt_ok = binary.is_some();
            // A working binary is still "broken" if it can't load a configured
            // extension at startup (missing .so) — repairable without a full
            // reinstall.
            let ext_broken = binary
                .as_deref()
                .map(|b| !startup_load_errors(b).is_empty())
                .unwrap_or(false);
            let installed_version = if keg {
                prefix.as_ref().and_then(|p| cellar_keg_version(p, &version))
            } else {
                None
            };
            CatalogEntry {
                version: version.clone(),
                formula,
                installed: keg,
                broken: keg && (!opt_ok || ext_broken),
                active: default.as_deref() == Some(version.as_str()),
                installed_version,
            }
        })
        .collect()
}

/// The Homebrew prefix in use (first that exists on disk).
fn brew_prefix() -> Option<PathBuf> {
    BREW_PREFIXES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.join("Cellar").is_dir() || p.join("opt").is_dir())
}

/// The full installed version for `major.minor` (e.g. `8.5.8`), from the Cellar
/// keg directory name — checks `php@X.Y` then the unversioned `php` keg.
fn cellar_keg_version(prefix: &std::path::Path, version: &str) -> Option<String> {
    let newest = |dir: PathBuf| -> Option<String> {
        std::fs::read_dir(&dir).ok()?.flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with(char::is_numeric).then_some(n)
            })
            .max()
    };
    newest(prefix.join("Cellar").join(format!("php@{version}")))
        .or_else(|| {
            newest(prefix.join("Cellar").join("php"))
                .filter(|v| v.starts_with(&format!("{version}.")))
        })
}

/// Does the unversioned `php` keg (Cellar/php/<v>.*) match `major.minor`?
fn cellar_php_is(prefix: &std::path::Path, version: &str) -> bool {
    let dir = prefix.join("Cellar").join("php");
    let Ok(entries) = std::fs::read_dir(&dir) else { return false };
    entries.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(&format!("{version}."))
    })
}

/// Resolve a pinned `major.minor` version to a binary, or `None` if absent.
pub fn resolve(version: &str) -> Option<PathBuf> {
    detect()
        .into_iter()
        .find(|p| p.version == version)
        .map(|p| p.binary)
}

/// The default `php` on `PATH`, if any.
pub fn default_binary() -> PathBuf {
    which("php").unwrap_or_else(|| PathBuf::from("php"))
}

/// `major.minor` of the default `php` on `PATH` (what unpinned sites run).
pub fn default_version() -> Option<String> {
    version_of(&default_binary())
}

/// Ask a php binary for its `major.minor` version. Runs with `-n` (no php.ini)
/// so a broken extension's startup warning can't pollute the output — the
/// version is a compile-time constant and needs no ini.
pub fn version_of(bin: &std::path::Path) -> Option<String> {
    let out = Command::new(bin)
        .arg("-n")
        .arg("-r")
        .arg("echo PHP_MAJOR_VERSION.'.'.PHP_MINOR_VERSION;")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() || !v.contains('.') {
        None
    } else {
        Some(v)
    }
}

/// Missing shared libraries a php binary fails to load at startup — the paths
/// inside "Unable to load dynamic library '…'" warnings. Empty when the version
/// starts cleanly. Used to flag otherwise-installed versions as needing repair.
pub fn startup_load_errors(bin: &std::path::Path) -> Vec<String> {
    let Ok(out) = Command::new(bin)
        .arg("-d").arg("display_errors=stderr")
        .arg("-d").arg("error_reporting=E_ALL")
        .arg("-r").arg("exit(0);")
        .output()
    else {
        return Vec::new();
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr
        .lines()
        .filter(|l| l.contains("Unable to load dynamic library"))
        .filter_map(|l| {
            let start = l.find('\'')?;
            let rest = &l[start + 1..];
            let end = rest.find('\'')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// `which <name>` → absolute path.
fn which(name: &str) -> Option<PathBuf> {
    let out = Command::new("/usr/bin/env").arg("which").arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}
