//! ConfigStore tests: host-resolution precedence, `$DPLY_TOKEN` override,
//! round-tripping the shared `~/.dply/config.json`, and default repointing on
//! logout. Uses an isolated temp HOME so the real config is never touched.
//!
//! These mutate process-wide env (`HOME`, `DPLY_HOST`, `DPLY_TOKEN`), so they
//! run in one test to avoid cross-test interference.

use dpl_dply::ConfigStore;
use serde_json::json;

#[test]
fn host_and_token_lifecycle() {
    let home = tempdir();
    let store = ConfigStore::new(Some(home.clone()));

    // Clean env for a deterministic run.
    std::env::remove_var("DPLY_HOST");
    std::env::remove_var("DPLY_TOKEN");

    // 1. No config yet → falls back to the built-in default host.
    assert_eq!(store.resolve_host(None).unwrap(), "https://dply.io");

    // 2. Store a token; it becomes the default host and is retrievable.
    store
        .set_host("https://dply.example/", "tok-1", json!({"operator": {"name": "Tom"}}), true)
        .unwrap();
    assert_eq!(store.resolve_host(None).unwrap(), "https://dply.example");
    assert_eq!(store.token_for("https://dply.example").unwrap().as_deref(), Some("tok-1"));

    // 3. Explicit --host flag beats the stored default.
    assert_eq!(
        store.resolve_host(Some("https://other.test")).unwrap(),
        "https://other.test"
    );

    // 4. $DPLY_HOST beats the stored default (but not an explicit flag).
    std::env::set_var("DPLY_HOST", "https://env-host.test");
    assert_eq!(store.resolve_host(None).unwrap(), "https://env-host.test");
    assert_eq!(store.resolve_host(Some("https://flag.test")).unwrap(), "https://flag.test");
    std::env::remove_var("DPLY_HOST");

    // 5. $DPLY_TOKEN overrides any stored token.
    std::env::set_var("DPLY_TOKEN", "env-token");
    assert_eq!(store.token_for("https://dply.example").unwrap().as_deref(), Some("env-token"));
    std::env::remove_var("DPLY_TOKEN");

    // 6. A second host coexists; forgetting the default repoints it.
    store.set_host("https://second.test", "tok-2", json!({}), true).unwrap();
    assert_eq!(store.default_host().unwrap().as_deref(), Some("https://second.test"));
    store.forget_host("https://second.test").unwrap();
    // default_host falls back to a remaining host, not None.
    assert_eq!(store.default_host().unwrap().as_deref(), Some("https://dply.example"));
    assert!(store.token_for("https://second.test").unwrap().is_none());

    // 7. The on-disk file is the shared ~/.dply/config.json path.
    let path = store.path().unwrap();
    assert!(path.ends_with(".dply/config.json"));
    assert!(path.exists());
}

/// Minimal temp-dir helper (avoids a dev-dep on `tempfile`).
fn tempdir() -> String {
    let base = std::env::temp_dir().join(format!("dpl-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&base);
    base.to_string_lossy().into_owned()
}
