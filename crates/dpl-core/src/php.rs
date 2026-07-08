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
const BREW_PREFIXES: &[&str] = &["/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"];

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

/// Ask a php binary for its `major.minor` version.
fn version_of(bin: &PathBuf) -> Option<String> {
    let out = Command::new(bin).arg("-r").arg("echo PHP_MAJOR_VERSION.'.'.PHP_MINOR_VERSION;").output().ok()?;
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

/// `which <name>` → absolute path.
fn which(name: &str) -> Option<PathBuf> {
    let out = Command::new("/usr/bin/env").arg("which").arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}
