//! Branch-aware databases: the pure helpers.
//!
//! A site with a `database` configured gets one database *per git branch*.
//! The base name (what Laravel's `.env` points at) always holds the checked-out
//! branch's data; every other branch's data is parked in a database named
//! `<base>@<branch>`. Switching branches is then two catalog renames (near
//! instant); only the first visit to a branch pays a template copy.
//!
//! This module holds the daemon-independent pieces: reading the current git
//! branch, reading a project's `DB_DATABASE`, and the parked-name scheme.

use std::path::Path;

/// The branch a project has checked out, from `.git/HEAD`.
///
/// `None` for a missing/unreadable HEAD *or* a detached head — branch-aware
/// databases follow named branches; a detached checkout keeps whatever data is
/// live rather than guessing.
pub fn git_branch(project: &Path) -> Option<String> {
    let head = std::fs::read_to_string(project.join(".git/HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?;
    (!branch.is_empty()).then(|| branch.to_string())
}

/// A project's database name from its `.env` (`DB_DATABASE=...`).
///
/// Minimal dotenv scan: first non-comment `DB_DATABASE` key wins; surrounding
/// single/double quotes are stripped. `None` when the file or key is absent.
pub fn env_database(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join(".env")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("DB_DATABASE") else { continue };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        let value = value.trim();
        let value = value
            .strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// The parked database name for a branch: `<base>@<branch>`, kept within
/// Postgres's 63-byte identifier limit.
///
/// When the full name doesn't fit it is truncated and suffixed with a short,
/// stable hash of the *full* name, so long branch names stay distinct and the
/// same branch always maps to the same database. The hash is fnv-1a rather
/// than `DefaultHasher` because the mapping must survive across builds.
pub fn branch_db_name(base: &str, branch: &str) -> String {
    const MAX: usize = 63;
    let full = format!("{base}@{branch}");
    if full.len() <= MAX {
        return full;
    }
    let hash = fnv1a(full.as_bytes());
    let tag = format!("-{hash:08x}");
    let keep = MAX - tag.len();
    // Truncate on a char boundary (branch names are usually ASCII, but don't
    // panic on the exception).
    let mut cut = keep;
    while !full.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{tag}", &full[..cut])
}

/// A branch's name back out of a parked database name, given the base.
pub fn branch_of_db(base: &str, db: &str) -> Option<String> {
    db.strip_prefix(&format!("{base}@")).map(|b| b.to_string())
}

/// Guard a name that will be interpolated into quoted Postgres identifiers and
/// string literals. Rejects both quote characters, backslash, and control
/// characters; everything else (`@`, `/`, `-`, `.`) is fine inside `"..."`.
pub fn safe_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.contains(['"', '\'', '\\'])
        && !name.chars().any(|c| c.is_control())
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dpl-branchdb-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_branch_from_head() {
        let dir = project();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/feat/new-schema\n").unwrap();
        assert_eq!(git_branch(&dir), Some("feat/new-schema".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detached_head_is_none() {
        let dir = project();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "8f7a2c91be1d4e2a\n").unwrap();
        assert_eq!(git_branch(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_db_database_from_env() {
        let dir = project();
        std::fs::write(
            dir.join(".env"),
            "# comment\nDB_CONNECTION=pgsql\nDB_DATABASE=\"my_app\"\nDB_DATABASE_IGNORED=x\n",
        )
        .unwrap();
        assert_eq!(env_database(&dir), Some("my_app".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_without_key_is_none() {
        let dir = project();
        std::fs::write(dir.join(".env"), "APP_NAME=x\n#DB_DATABASE=commented\n").unwrap();
        assert_eq!(env_database(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parked_names_fit_and_stay_distinct() {
        assert_eq!(branch_db_name("app", "main"), "app@main");
        assert_eq!(branch_db_name("app", "feat/x"), "app@feat/x");

        let long_a = branch_db_name("app", &"a".repeat(100));
        let long_b = branch_db_name("app", &format!("{}b", "a".repeat(99)));
        assert!(long_a.len() <= 63 && long_b.len() <= 63);
        assert_ne!(long_a, long_b, "truncated names must stay distinct");
        // Stable: same input, same name.
        assert_eq!(long_a, branch_db_name("app", &"a".repeat(100)));
    }

    #[test]
    fn round_trips_branch_out_of_db_name() {
        assert_eq!(branch_of_db("app", "app@feat/x"), Some("feat/x".into()));
        assert_eq!(branch_of_db("app", "other@main"), None);
        assert_eq!(branch_of_db("app", "app"), None);
    }

    #[test]
    fn identifier_guard() {
        assert!(safe_identifier("app@feat/x-2.0"));
        assert!(!safe_identifier("app\"; DROP DATABASE x; --"));
        assert!(!safe_identifier(""));
        assert!(!safe_identifier(&"x".repeat(64)));
    }
}
