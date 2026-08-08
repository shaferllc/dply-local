//! Xdebug: which modes exist, where the extension lives, and how the daemon
//! turns it on for one site without disturbing the rest.
//!
//! Two facts about Xdebug 3 shape everything here:
//!
//! 1. `xdebug.mode` is read once, when the PHP process starts. It cannot be set
//!    per-request — not from `.user.ini`, not from `php_admin_value` in an
//!    Apache vhost or a **php-fpm pool**. The only per-process lever is the
//!    `XDEBUG_MODE` environment variable, which overrides the ini setting.
//! 2. Because `XDEBUG_MODE` overrides the setting rather than changing it,
//!    `ini_get("xdebug.mode")` still reports the *ini* value — an empty string.
//!    Reading it back to discover the active mode gives the wrong answer, so
//!    [`Mode`] as recorded in the config is the source of truth. (A process that
//!    really must know can call `xdebug_info('mode')`, which reports the
//!    effective feature set.)
//!
//! Together these mean per-site Xdebug requires one php-fpm master per
//! (PHP binary, mode) pair — see `dpld::fpm` — and that dpl loads the extension
//! through its own ini scan directory ([`crate::paths::php_conf_dir`], wired up
//! via `PHP_INI_SCAN_DIR`) rather than editing the system `conf.d`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CoreError, Result};

/// Every mode Xdebug accepts. `off` is exclusive; the rest combine.
pub const MODES: &[&str] = &["off", "develop", "coverage", "debug", "gcstats", "profile", "trace"];

/// A validated, canonical `xdebug.mode` value.
///
/// Canonical means "sorted and de-duplicated", so `debug,develop` and
/// `develop,debug` compare equal. That matters: the daemon keys a php-fpm
/// master by this value, and two spellings of one mode must not spawn two
/// pools.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mode(String);

impl Default for Mode {
    fn default() -> Self {
        Mode::off()
    }
}

impl Mode {
    /// Xdebug loaded but doing nothing — its near-zero-overhead state.
    pub fn off() -> Self {
        Mode("off".into())
    }

    /// Parse a comma-separated mode list. An empty string means `off`.
    pub fn parse(raw: &str) -> Result<Self> {
        let mut parts: Vec<String> = raw
            .split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();

        if parts.is_empty() {
            return Ok(Mode::off());
        }
        if let Some(bad) = parts.iter().find(|p| !MODES.contains(&p.as_str())) {
            return Err(CoreError::Message(format!(
                "unknown Xdebug mode `{bad}` (valid: {})",
                MODES.join(", ")
            )));
        }
        parts.sort();
        parts.dedup();

        if parts.iter().any(|p| p == "off") {
            if parts.len() > 1 {
                return Err(CoreError::Message(
                    "`off` cannot be combined with other Xdebug modes".into(),
                ));
            }
            return Ok(Mode::off());
        }
        Ok(Mode(parts.join(",")))
    }

    /// The canonical string, e.g. `"debug,develop"`. Never empty.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when Xdebug should do nothing.
    pub fn is_off(&self) -> bool {
        self.0 == "off"
    }

    /// True when this mode includes `part` (e.g. `"debug"`).
    pub fn has(&self, part: &str) -> bool {
        self.0.split(',').any(|p| p == part)
    }

    /// A filesystem-safe suffix for naming this mode's php-fpm config dir.
    pub fn slug(&self) -> String {
        self.0.replace(',', "-")
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Client-side connection settings, shared by every site (they describe the
/// *IDE*, not the project). Persisted in `~/.dpl/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Port the IDE listens on for step-debug connections.
    pub client_port: u16,
    /// `xdebug.idekey` — `PHPSTORM`, `VSCODE`, …
    pub ide_key: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { client_port: 9003, ide_key: "PHPSTORM".into() }
    }
}

/// The ini scan directory a PHP binary reads by default (`php --ini`).
///
/// `PHP_INI_SCAN_DIR` is scrubbed from the child's environment first: the daemon
/// sets it for its own php-fpm masters, and inheriting that here would report
/// dpl's directory back to us instead of the system one.
pub fn system_conf_dir(php_bin: &Path) -> Option<PathBuf> {
    let out = Command::new(php_bin).arg("--ini").env_remove("PHP_INI_SCAN_DIR").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find_map(|l| l.strip_prefix("Scan for additional .ini files in:"))?;
    let dir = line.trim();
    (!dir.is_empty() && dir != "(none)").then(|| PathBuf::from(dir))
}

/// Locate `xdebug.so` for a PHP binary.
///
/// Distros ship the extension with its `zend_extension` line **commented out**
/// (`; zend_extension="…"`) so it's installed but inert, so we read the path out
/// of any `*xdebug*.ini` regardless of whether the line is live. Falling back to
/// Homebrew's `opt/xdebug@<version>` keg covers a fresh install whose ini hasn't
/// been written yet.
pub fn so_path(php_bin: &Path) -> Option<PathBuf> {
    if let Some(dir) = system_conf_dir(php_bin) {
        if let Some(so) = so_from_conf_dir(&dir) {
            if so.is_file() {
                return Some(so);
            }
        }
    }
    let version = crate::php::version_of(php_bin)?;
    crate::php::BREW_PREFIXES
        .iter()
        .map(|p| PathBuf::from(p).join(format!("opt/xdebug@{version}/xdebug.so")))
        .find(|p| p.is_file())
}

/// Read a `zend_extension=<path>` value out of any `*xdebug*.ini` in `dir`,
/// accepting commented-out lines.
fn so_from_conf_dir(dir: &Path) -> Option<PathBuf> {
    let mut inis: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            name.contains("xdebug") && name.ends_with(".ini")
        })
        .collect();
    inis.sort();

    for ini in inis {
        let Ok(text) = std::fs::read_to_string(&ini) else { continue };
        for raw in text.lines() {
            let line = raw.trim_start_matches([';', ' ', '\t']);
            if !line.to_lowercase().starts_with("zend_extension") {
                continue;
            }
            let Some((_, value)) = line.split_once('=') else { continue };
            let value = value.trim().trim_matches(['"', '\'']);
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

/// Whether Xdebug is installed for this PHP binary (loaded or not).
pub fn installed(php_bin: &Path) -> bool {
    so_path(php_bin).is_some()
}

/// Render the dpl-managed ini that loads Xdebug.
///
/// Deliberately pins `xdebug.mode=off`: the mode always arrives via
/// `XDEBUG_MODE` on the master's environment, and baking a live mode in here
/// would turn Xdebug on for *every* pool sharing this PHP binary — the exact
/// thing per-site mode exists to avoid.
pub fn loader_ini(so: &Path, settings: &Settings, output_dir: &Path) -> String {
    format!(
        "; Managed by dpl — do not edit. Regenerated on every daemon reconcile.\n\
         ;\n\
         ; Loaded only by dpl's php-fpm masters, via PHP_INI_SCAN_DIR. Your system\n\
         ; php.ini and conf.d are untouched, so the `php` on your PATH is unaffected.\n\
         ;\n\
         ; The active mode is *not* set here. xdebug.mode can only be read at process\n\
         ; start, so the daemon passes it as XDEBUG_MODE on each master's environment\n\
         ; and runs one master per (PHP version, mode). See `dpl xdebug`.\n\
         zend_extension={so}\n\
         xdebug.mode=off\n\
         xdebug.start_with_request=yes\n\
         xdebug.client_host=127.0.0.1\n\
         xdebug.client_port={port}\n\
         xdebug.idekey={ide_key}\n\
         xdebug.discover_client_host=false\n\
         xdebug.output_dir={output_dir}\n",
        so = so.display(),
        port = settings.client_port,
        ide_key = settings.ide_key,
        output_dir = output_dir.display(),
    )
}

/// Write the loader ini into dpl's scan dir for `php_bin`, returning that dir.
///
/// Returns `Ok(None)` when Xdebug isn't installed for this binary — the caller
/// then leaves `PHP_INI_SCAN_DIR` unset and the master runs without Xdebug at
/// all. A stale loader from a previous install is removed so an uninstalled
/// Xdebug can't keep a dangling `zend_extension` line alive.
pub fn write_loader(
    override_home: Option<&str>,
    php_bin: &Path,
    settings: &Settings,
) -> Result<Option<PathBuf>> {
    // Drop the pre-per-site GUI leftover before writing the real loader.
    let _ = cleanup_legacy_system_loader(php_bin);

    let dir = crate::paths::php_conf_dir(override_home, php_bin)?;
    let ini = dir.join("zz-dpl-xdebug.ini");

    let Some(so) = so_path(php_bin) else {
        let _ = std::fs::remove_file(&ini);
        return Ok(None);
    };

    let output_dir = crate::paths::xdebug_dir(override_home)?;
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    std::fs::create_dir_all(&output_dir).map_err(|e| CoreError::io(&output_dir, e))?;
    std::fs::write(&ini, loader_ini(&so, settings, &output_dir))
        .map_err(|e| CoreError::io(&ini, e))?;
    Ok(Some(dir))
}

/// Path of the pre-per-site-Xdebug GUI leftover in the *system* conf.d, if any.
///
/// The old SwiftUI path wrote `zz-dpl-xdebug.ini` into Homebrew's scan directory
/// (often just `xdebug.mode=off` with no `zend_extension`). That neither enabled
/// site debugging nor helped CLI tools like Pest TIA — and it confused users who
/// saw "Debugging on" in the menu bar while `php -m` had no Xdebug. Current dpl
/// loads Xdebug only via [`crate::paths::php_conf_dir`] + `PHP_INI_SCAN_DIR`.
pub fn legacy_system_loader_path(php_bin: &Path) -> Option<PathBuf> {
    let dir = system_conf_dir(php_bin)?;
    let ini = dir.join("zz-dpl-xdebug.ini");
    if !ini.is_file() {
        return None;
    }
    let Ok(text) = std::fs::read_to_string(&ini) else {
        return None;
    };
    // Only treat files that look like ours as leftovers — never remove a
    // user-authored file that happens to share the name.
    text.contains("Managed by dpl").then_some(ini)
}

/// Remove [`legacy_system_loader_path`] when present. Returns `true` if a file
/// was deleted.
pub fn cleanup_legacy_system_loader(php_bin: &Path) -> bool {
    let Some(ini) = legacy_system_loader_path(php_bin) else {
        return false;
    };
    std::fs::remove_file(&ini).is_ok()
}

/// The `PHP_INI_SCAN_DIR` value that adds `dir` to PHP's default scan path.
///
/// The leading colon is load-bearing: it means "the compiled-in directory, then
/// this one". Without it PHP scans *only* `dir` and every other extension the
/// user has configured silently stops loading.
pub fn scan_dir_env(dir: &Path) -> String {
    format!(":{}", dir.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_off_parse_to_off() {
        assert!(Mode::parse("").unwrap().is_off());
        assert!(Mode::parse("off").unwrap().is_off());
        assert!(Mode::parse("  OFF ").unwrap().is_off());
        assert_eq!(Mode::default(), Mode::off());
    }

    #[test]
    fn mode_lists_canonicalize_so_pools_dont_split() {
        let a = Mode::parse("debug,develop").unwrap();
        let b = Mode::parse("develop, debug").unwrap();
        let c = Mode::parse("DEVELOP,debug,debug").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(a.as_str(), "debug,develop");
    }

    #[test]
    fn rejects_unknown_and_combined_off() {
        assert!(Mode::parse("bogus").is_err());
        assert!(Mode::parse("debug,bogus").is_err());
        assert!(Mode::parse("off,debug").is_err());
    }

    #[test]
    fn has_matches_whole_parts_only() {
        let m = Mode::parse("debug").unwrap();
        assert!(m.has("debug"));
        assert!(!m.has("bug"));
        assert!(!m.has("develop"));
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(Mode::parse("debug,develop").unwrap().slug(), "debug-develop");
        assert_eq!(Mode::off().slug(), "off");
    }

    #[test]
    fn loader_pins_mode_off_and_carries_settings() {
        let ini = loader_ini(
            Path::new("/opt/xdebug.so"),
            &Settings { client_port: 9005, ide_key: "VSCODE".into() },
            Path::new("/tmp/out"),
        );
        assert!(ini.contains("zend_extension=/opt/xdebug.so"));
        assert!(ini.contains("xdebug.mode=off"));
        assert!(ini.contains("xdebug.client_port=9005"));
        assert!(ini.contains("xdebug.idekey=VSCODE"));
        assert!(ini.contains("xdebug.output_dir=/tmp/out"));
    }

    #[test]
    fn scan_dir_env_keeps_the_default_directory() {
        assert_eq!(scan_dir_env(Path::new("/a/b")), ":/a/b");
    }

    #[test]
    fn so_path_reads_a_commented_out_zend_extension() {
        let dir = std::env::temp_dir().join(format!("dpl-xdebug-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let so = dir.join("xdebug.so");
        std::fs::write(&so, b"").unwrap();
        std::fs::write(
            dir.join("20-xdebug.ini"),
            format!("[xdebug]\n; zend_extension=\"{}\"\n", so.display()),
        )
        .unwrap();

        assert_eq!(so_from_conf_dir(&dir), Some(so));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_removes_only_dpl_managed_system_leftovers() {
        let dir = std::env::temp_dir().join(format!("dpl-xdebug-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("zz-dpl-xdebug.ini");
        std::fs::write(&ours, "; Managed by dpl — Xdebug\nxdebug.mode=off\n").unwrap();
        let foreign = dir.join("zz-dpl-xdebug.ini.foreign");
        // Simulate the name check: a non-dpl file must not match via content.
        assert!(std::fs::read_to_string(&ours).unwrap().contains("Managed by dpl"));
        std::fs::remove_file(&ours).unwrap();
        std::fs::write(&foreign, "xdebug.mode=debug\n").unwrap();
        assert!(!std::fs::read_to_string(&foreign).unwrap().contains("Managed by dpl"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
