//! SPX flame-graph profiler integration, shared by the CLI (`dpl profile`) and
//! the daemon (which runs a site's php-fpm pool with SPX loaded).
//!
//! SPX (`php-spx`) is a sampling profiler with a same-origin web UI. dpl manages
//! it the way it manages Xdebug: the extension is loaded *only* into the php-fpm
//! masters that want it, via `PHP_INI_SCAN_DIR`, so the `php` on your PATH and
//! every non-profiled site stay untouched. When profiling is on for a site, its
//! master loads SPX with HTTP profiling auto-started, so **every** request is
//! captured — no cookie, no per-request trigger — and the reports are browsable
//! at the site's own origin under `?SPX_UI_URI=/`.
//!
//! Homebrew's SPX formula loads the extension from the *system* conf.d, so it's
//! present (dormant, `http_enabled=0`) in every pool. dpl doesn't re-load it —
//! our loader is directives-only, activating HTTP profiling in a profiler pool
//! by setting `spx.http_*`. Loading it a second time would emit a `Module "SPX"
//! is already loaded` warning to stdout that can corrupt a response, and every
//! other pool already has it inert, so a plain site never profiles.

use std::path::{Path, PathBuf};

/// Gate for SPX's web UI. The real boundary is `http_ip_whitelist=127.0.0.1`
/// below — only localhost can reach the UI at all — so this is a formality that
/// keeps SPX happy, not a secret. `dpl profile open` passes it in the URL.
pub const KEY: &str = "dpl-local";

/// Locate `spx.so` for a PHP binary: the Homebrew keg first (where our own
/// installer puts it), then any `*spx*.ini` in the system conf.d — including a
/// `.disabled` one we parked, so detection survives our own isolation step.
pub fn so_path(php_bin: &Path) -> Option<PathBuf> {
    if let Some(version) = crate::php::version_of(php_bin) {
        if let Some(so) = crate::php::BREW_PREFIXES
            .iter()
            .map(|p| PathBuf::from(p).join(format!("opt/spx@{version}/spx.so")))
            .find(|p| p.is_file())
        {
            return Some(so);
        }
    }
    crate::xdebug::system_conf_dir(php_bin).and_then(|dir| so_from_conf_dir(&dir))
}

/// Read an `extension=<path>` value out of any `*spx*.ini` (enabled or
/// `.disabled`) in `dir`.
fn so_from_conf_dir(dir: &Path) -> Option<PathBuf> {
    let mut inis: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            name.contains("spx") && (name.ends_with(".ini") || name.ends_with(".ini.disabled"))
        })
        .collect();
    inis.sort();
    for ini in inis {
        let Ok(text) = std::fs::read_to_string(&ini) else { continue };
        for raw in text.lines() {
            let line = raw.trim_start_matches([';', ' ', '\t']);
            if !line.to_lowercase().starts_with("extension") {
                continue;
            }
            let Some((_, value)) = line.split_once('=') else { continue };
            let value = value.trim().trim_matches(['"', '\'']);
            if !value.is_empty() && value.to_lowercase().ends_with("spx.so") {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

/// Whether SPX is installed for this PHP binary.
pub fn installed(php_bin: &Path) -> bool {
    so_path(php_bin).is_some()
}

/// The dpl-managed ini that activates SPX for a profiler pool.
///
/// Directives only — SPX is already loaded by the system conf.d, and re-loading
/// it warns to stdout. `http_profiling_auto_start` captures every request; the
/// flame graphs are browsable same-origin at `http://<site>/?SPX_UI_URI=/`.
pub fn loader_ini(data_dir: &Path) -> String {
    format!(
        "; Managed by dpl — do not edit. Regenerated on every daemon reconcile.\n\
         ;\n\
         ; Added to PHP_INI_SCAN_DIR only for a profiled site's php-fpm master. SPX\n\
         ; itself is loaded by the system conf.d; here we just switch on HTTP\n\
         ; profiling so every request is captured.\n\
         spx.data_dir={data_dir}\n\
         spx.http_enabled=1\n\
         spx.http_key={key}\n\
         spx.http_ip_whitelist=127.0.0.1\n\
         spx.http_profiling_enabled=1\n\
         spx.http_profiling_auto_start=1\n",
        data_dir = data_dir.display(),
        key = KEY,
    )
}

/// Write the SPX loader into dpl's dedicated scan dir for `php_bin`, returning
/// that dir (to add to `PHP_INI_SCAN_DIR`).
///
/// `Ok(None)` when SPX isn't installed — the caller then leaves it out of the
/// scan path and the master serves without profiling. A stale loader is removed
/// so an uninstalled SPX can't keep a dangling `extension` line alive.
pub fn write_loader(override_home: Option<&str>, php_bin: &Path) -> Result<Option<PathBuf>, crate::error::CoreError> {
    let dir = crate::paths::spx_conf_dir(override_home, php_bin)?;
    let ini = dir.join("zz-dpl-spx.ini");

    // No SPX for this PHP means nothing to configure — the system conf.d isn't
    // loading it, so our directives would apply to an absent extension.
    if !installed(php_bin) {
        let _ = std::fs::remove_file(&ini);
        return Ok(None);
    }

    let data_dir = crate::paths::spx_data_dir(override_home, php_bin)?;
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::CoreError::io(&dir, e))?;
    std::fs::create_dir_all(&data_dir).map_err(|e| crate::error::CoreError::io(&data_dir, e))?;
    std::fs::write(&ini, loader_ini(&data_dir)).map_err(|e| crate::error::CoreError::io(&ini, e))?;
    Ok(Some(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_ini_auto_starts_profiling_without_reloading_the_extension() {
        let ini = loader_ini(Path::new("/data"));
        assert!(ini.contains("spx.http_profiling_auto_start=1"));
        assert!(ini.contains("spx.http_enabled=1"));
        assert!(ini.contains("spx.data_dir=/data"));
        assert!(ini.contains("spx.http_ip_whitelist=127.0.0.1"));
        // Re-loading SPX warns to stdout and can corrupt a response — the system
        // conf.d already loaded it.
        assert!(!ini.contains("extension="));
    }
}
