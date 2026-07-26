//! `dpl.toml` — a project's committable environment spec.
//!
//! One file at the repo root captures everything dpl knows about a site that
//! isn't already a repo file (PHP pin, HTTPS, runtime, Xdebug, profiler,
//! preload, branch-aware database, required services, dev server), so a teammate
//! runs `dpl up` and gets an identical environment.
//!
//! The Node *version* is deliberately absent — that pin already lives in
//! `.nvmrc`/`.node-version`, which the repo carries. The dev *script* is here,
//! though, because no repo file says which of a project's scripts should be
//! supervised: `package.json` lists `dev`, `watch` and `build` without ranking
//! them, and picking one is a decision about the environment, not the code.
//!
//! Semantics are declarative: `dpl up` makes the site match the file exactly —
//! an absent key means "default/off", not "leave as is" — so re-running it is
//! idempotent and the file is the single source of truth.

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "dpl.toml";

/// Everything `dpl up` applies. Field names are the file's keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteSpec {
    /// Site name (`<name>.test`). Default: the folder name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// PHP version pin, e.g. `"8.3"`. Absent = the machine default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    /// Serve over HTTPS.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secure: bool,
    /// Application server: absent/`"fpm"` = php-fpm, else `"octane-swoole"`,
    /// `"octane-roadrunner"`, `"octane-frankenphp"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Xdebug mode for the site, e.g. `"debug"`. Absent = off/default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xdebug: Option<String>,
    /// SPX profiler on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub profile: bool,
    /// Opcache preload script, relative to the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload: Option<String>,
    /// Branch-aware database base name (see `dpl db attach`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Postgres port for `database`, when not 5432.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_port: Option<u16>,
    /// Engines this project needs running (checked, not auto-created):
    /// `["postgres", "redis", ...]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    /// package.json script the daemon supervises as this project's dev server
    /// (see `dpl dev`), e.g. `"dev"`. Absent = no dev server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
}

impl SiteSpec {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Serialize with a header comment, ready to commit.
    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).unwrap_or_default();
        format!(
            "# dpl.toml — this project's local environment, applied with `dpl up`.\n\
             # Commit it: a teammate with dpl gets the same site in one command.\n\
             {body}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let spec = SiteSpec {
            name: Some("myapp".into()),
            php: Some("8.3".into()),
            secure: true,
            runtime: Some("octane-swoole".into()),
            xdebug: Some("debug".into()),
            profile: false,
            preload: Some("preload.php".into()),
            database: Some("myapp".into()),
            db_port: None,
            services: vec!["postgres".into(), "redis".into()],
            dev: Some("dev".into()),
        };
        let text = spec.to_toml();
        assert_eq!(SiteSpec::from_toml(&text).unwrap(), spec);
    }

    #[test]
    fn empty_file_is_all_defaults() {
        let spec = SiteSpec::from_toml("").unwrap();
        assert_eq!(spec, SiteSpec::default());
    }

    #[test]
    fn defaults_are_omitted_from_output() {
        let text = SiteSpec { php: Some("8.4".into()), ..Default::default() }.to_toml();
        assert!(text.contains("php = \"8.4\""));
        for absent in ["secure", "profile", "runtime", "database", "services", "name", "dev"] {
            assert!(!text.contains(absent), "default field `{absent}` leaked into output:\n{text}");
        }
    }

    #[test]
    fn unknown_keys_are_rejected_loudly() {
        // A typo (`datbase = ...`) must fail, not silently apply nothing.
        assert!(SiteSpec::from_toml("datbase = \"oops\"").is_err());
    }
}
