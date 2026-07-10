//! Per-project Node version management, delegated to fnm/nvm.
//!
//! dpl doesn't run `node` (you do, in your shell), so it can't force a version
//! the way it does for php-fpm. Instead it manages the pieces a Node version
//! manager needs: it writes each repo's `.nvmrc` — the pin file both fnm and nvm
//! read and auto-switch on — and reads a project's desired version back out of
//! `.nvmrc`, `.node-version`, or `package.json` `engines.node`. The manager does
//! the actual switching when you `cd` in.

use std::path::Path;

/// A Node version manager dpl can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    /// fnm — a fast, single-binary manager (`brew install fnm`).
    Fnm,
    /// nvm — the shell-function manager, loaded from `~/.nvm/nvm.sh`.
    Nvm,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Fnm => "fnm",
            Manager::Nvm => "nvm",
        }
    }
}

/// The Node manager available on this machine, preferring fnm (a real binary we
/// can call directly) over nvm (a shell function we have to source).
pub fn detect_manager() -> Option<Manager> {
    if crate::tools::which("fnm").is_some() {
        return Some(Manager::Fnm);
    }
    if nvm_script().is_some() {
        return Some(Manager::Nvm);
    }
    None
}

/// Path to `nvm.sh`, honouring `$NVM_DIR`, else `~/.nvm`.
pub fn nvm_script() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("NVM_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".nvm")))?;
    let script = dir.join("nvm.sh");
    script.is_file().then_some(script)
}

/// Where a project's pinned Node version comes from, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The version string, as written for the manager (e.g. `"20"`, `"18.19.0"`,
    /// `"lts/*"`).
    pub version: String,
    /// Which file it was read from, for display.
    pub source: PinSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinSource {
    Nvmrc,
    NodeVersion,
    PackageJson,
}

impl PinSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PinSource::Nvmrc => ".nvmrc",
            PinSource::NodeVersion => ".node-version",
            PinSource::PackageJson => "package.json",
        }
    }
}

/// Read a project's pinned Node version: `.nvmrc` and `.node-version` win (they're
/// the manager's own pins), else `package.json` `engines.node` as a fallback
/// hint. `None` when the project says nothing about Node.
pub fn read_pin(project_dir: &Path) -> Option<Pin> {
    if let Some(v) = read_first_line(&project_dir.join(".nvmrc")) {
        return Some(Pin { version: v, source: PinSource::Nvmrc });
    }
    if let Some(v) = read_first_line(&project_dir.join(".node-version")) {
        return Some(Pin { version: v, source: PinSource::NodeVersion });
    }
    engines_node(project_dir).map(|v| Pin { version: v, source: PinSource::PackageJson })
}

/// Write `.nvmrc` in `project_dir`, the pin fnm and nvm both auto-switch on.
pub fn write_nvmrc(project_dir: &Path, version: &str) -> Result<(), crate::error::CoreError> {
    let path = project_dir.join(".nvmrc");
    let body = format!("{}\n", version.trim());
    std::fs::write(&path, body).map_err(|e| crate::error::CoreError::io(&path, e))
}

/// Reduce a `package.json` `engines.node` range to a version a manager accepts —
/// the first version number in the range (`"^20.11 || 18"` → `"20"`, `">=18"` →
/// `"18"`). A hint, not a resolution; the user confirms with `dpl node use`.
pub fn normalize_range(range: &str) -> Option<String> {
    let mut digits = String::new();
    for ch in range.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    (!digits.is_empty()).then_some(digits)
}

fn read_first_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Pull `engines.node` out of `package.json` without a JSON dependency — a small
/// scan for the key and its string value, enough for a version hint.
fn engines_node(project_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project_dir.join("package.json")).ok()?;
    let engines_at = text.find("\"engines\"")?;
    let rest = &text[engines_at..];
    let node_at = rest.find("\"node\"")?;
    let after = &rest[node_at + "\"node\"".len()..];
    let colon = after.find(':')?;
    let after_colon = &after[colon + 1..];
    let open = after_colon.find('"')?;
    let value = &after_colon[open + 1..];
    let close = value.find('"')?;
    let range = value[..close].trim();
    (!range.is_empty()).then(|| range.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_engine_ranges_to_a_major() {
        assert_eq!(normalize_range("^20.11.0"), Some("20".into()));
        assert_eq!(normalize_range(">=18"), Some("18".into()));
        assert_eq!(normalize_range("20.x"), Some("20".into()));
        assert_eq!(normalize_range("18 || 20"), Some("18".into()));
        assert_eq!(normalize_range("lts/*"), None);
    }

    #[test]
    fn reads_engines_node_from_package_json() {
        let dir = std::env::temp_dir().join(format!("dpl-node-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("package.json"),
            r#"{ "name": "x", "engines": { "node": ">=20.0.0" }, "scripts": {} }"#,
        )
        .unwrap();
        let pin = read_pin(&dir).unwrap();
        assert_eq!(pin.version, ">=20.0.0");
        assert_eq!(pin.source, PinSource::PackageJson);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nvmrc_wins_over_package_json() {
        let dir = std::env::temp_dir().join(format!("dpl-node-test2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("package.json"), r#"{ "engines": { "node": "18" } }"#).unwrap();
        write_nvmrc(&dir, "20").unwrap();
        let pin = read_pin(&dir).unwrap();
        assert_eq!(pin.version, "20");
        assert_eq!(pin.source, PinSource::Nvmrc);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
