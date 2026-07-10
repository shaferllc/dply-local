//! Finding the tools dpl shells out to (`php`, `brew`, `composer`, `mysqld`…).
//!
//! `PATH` alone is not enough. A GUI launched from Finder, a LaunchAgent, and a
//! cron job all inherit launchd's bare `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin`,
//! with no Homebrew. Probing with `PATH` only, dpl reports Homebrew and Composer
//! "not installed" on a machine that plainly has them, and then offers to install
//! them with the `brew` it just claimed was missing.
//!
//! So: search `PATH` first (the user's choice wins — a pinned PHP in `~/.local/bin`
//! must beat Homebrew's), then fall back to the standard prefixes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where tools live when `PATH` doesn't say. Apple Silicon Homebrew, Intel
/// Homebrew, then the system directories.
const FALLBACK_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// The absolute path to `name`, or `None` if nothing executable by that name is
/// reachable. Never shells out — `env which` would itself depend on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, std::env::var_os("PATH"))
}

/// `which`, against an explicit `PATH`. Split out so tests can vary the search
/// path without mutating the process environment, which every other test shares.
fn which_in(name: &str, path: Option<OsString>) -> Option<PathBuf> {
    // An absolute or relative path is not a name to look up.
    if name.contains('/') {
        let direct = PathBuf::from(name);
        return is_executable(&direct).then_some(direct);
    }

    if let Some(path) = path {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }

    FALLBACK_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| is_executable(candidate))
}

/// A regular file we may execute. A directory named `php` is not `php`.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    #[test]
    fn finds_a_tool_on_the_path() {
        assert_eq!(which_in("sh", path("/bin")), Some(PathBuf::from("/bin/sh")));
    }

    /// The whole point: launchd's bare PATH, or none at all, must not mean
    /// "nothing is installed".
    #[test]
    fn falls_back_when_the_path_cannot_find_it() {
        assert_eq!(which_in("sh", path("/nonexistent")), Some(PathBuf::from("/bin/sh")));
        assert_eq!(which_in("sh", None), Some(PathBuf::from("/bin/sh")));
    }

    /// A tool the user put earlier on PATH must beat the Homebrew fallback.
    #[test]
    fn path_wins_over_the_fallback_dirs() {
        assert_eq!(which_in("env", path("/usr/bin")), Some(PathBuf::from("/usr/bin/env")));
    }

    #[test]
    fn missing_tools_are_none() {
        assert!(which_in("dpl-definitely-not-a-real-binary", path("/usr/bin")).is_none());
    }

    #[test]
    fn an_explicit_path_is_used_as_given() {
        assert_eq!(which_in("/bin/sh", None), Some(PathBuf::from("/bin/sh")));
        assert!(which_in("/bin/nope-not-here", None).is_none());
    }

    #[test]
    fn a_directory_is_not_an_executable() {
        assert!(!is_executable(Path::new("/usr")));
        assert!(which_in("bin", path("/usr")).is_none());
    }
}
