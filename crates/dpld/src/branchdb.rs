//! Branch-aware databases: orchestration and the auto-switch watcher.
//!
//! The naming scheme and pure helpers live in [`dpl_core::branchdb`]; the
//! Postgres operations live in [`crate::services`]. This module ties them to
//! the registry: the `dpl db attach/detach/switch/branches/drop-branch`
//! request handler, plus a background task that watches every attached site's
//! `.git/HEAD` and switches automatically on checkout.
//!
//! Locking discipline: the registry lock is held only for config reads and
//! writes, never across database work — a first-visit template copy takes
//! seconds and every HTTP request routes through that lock. A module-wide op
//! mutex serializes the switches themselves, so the watcher and a manual
//! `dpl db switch` can never interleave their rename/copy sequences.

use anyhow::{bail, Context, Result};
use dpl_core::ipc::Response;

use crate::server::DaemonState;
use crate::services::{pg_branch_drop, pg_branch_list, pg_branch_switch, pg_db_size, pg_ensure_db, port_open};

/// Serializes every branch-db mutation (manual and automatic).
static OPS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How often the watcher polls attached sites' `.git/HEAD`. Each poll is one
/// tiny file read per attached site — cheap enough that event infrastructure
/// (which the daemon has none of) isn't worth its complexity here.
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Handle a `Request::BranchDb` — the single entry point for the CLI/GUI.
pub async fn dispatch(
    state: &DaemonState,
    action: &str,
    site: &str,
    branch: Option<String>,
    database: Option<String>,
    port: Option<u16>,
) -> Response {
    let result = match action {
        "attach" => attach(state, site, database, port).await,
        "detach" => detach(state, site).await,
        "switch" => switch(state, site, branch, port).await,
        "branches" => return branches(state, site, port).await,
        "drop-branch" => drop_branch(state, site, branch, port).await,
        other => Err(anyhow::anyhow!("unknown branch-db action: {other}")),
    };
    match result {
        Ok(text) => Response::Message { text },
        Err(e) => Response::Error { message: format!("{e:#}") },
    }
}

async fn attach(state: &DaemonState, site: &str, database: Option<String>, port: Option<u16>) -> Result<String> {
    let st = state.registry.lock().await.branch_db_state(site)?;
    let db = database
        .or_else(|| dpl_core::branchdb::env_database(&st.path))
        .with_context(|| format!("couldn't find DB_DATABASE in {}/.env — pass one with --database", st.path.display()))?;
    let on_branch = dpl_core::branchdb::git_branch(&st.path)
        .with_context(|| format!("{} isn't on a git branch (branch databases follow git)", st.path.display()))?;
    // Persist the port only when it isn't the default, so config stays quiet.
    let port = port.filter(|p| *p != 5432);
    let effective = port.unwrap_or(5432);
    if !port_open(effective) {
        bail!("nothing is listening on Postgres port {effective} — start it (or DBngin) first.");
    }
    let db2 = db.clone();
    let created = tokio::task::spawn_blocking(move || pg_ensure_db(effective, &db2)).await??;
    state.registry.lock().await.set_branch_db(site, Some(db.clone()), port, Some(on_branch.clone()))?;
    Ok(format!(
        "Branch databases on for {site}: `{db}`{} now tracks git branch `{on_branch}`. \
         Checkouts now switch it automatically.",
        if created { " (created)" } else { "" }
    ))
}

async fn detach(state: &DaemonState, site: &str) -> Result<String> {
    let st = state.registry.lock().await.branch_db_state(site)?;
    if st.database.is_none() {
        bail!("{site} has no branch database attached.");
    }
    state.registry.lock().await.set_branch_db(site, None, None, None)?;
    Ok(format!(
        "Branch databases off for {site}. Parked `<db>@<branch>` databases were kept — \
         drop them with `dpl db drop <name>` if you're done with them."
    ))
}

/// Switch `site`'s database to `to` (default: the checked-out branch). Shared
/// by the request handler and the watcher; serialized by the op mutex.
pub async fn switch(state: &DaemonState, site: &str, to: Option<String>, port_override: Option<u16>) -> Result<String> {
    let _ops = OPS.lock().await;
    let st = state.registry.lock().await.branch_db_state(site)?;
    let Some(db) = st.database else {
        bail!("{site} has no branch database attached — run `dpl db attach {site}` first.");
    };
    let to = to
        .or_else(|| dpl_core::branchdb::git_branch(&st.path))
        .with_context(|| format!("{} isn't on a git branch — pass one explicitly", st.path.display()))?;
    let Some(from) = st.db_branch else {
        bail!("no live branch recorded for {site} — re-run `dpl db attach {site}`.");
    };
    if from == to {
        return Ok(format!("`{db}` already tracks `{to}`."));
    }
    let port = port_override.or(st.port).unwrap_or(5432);
    if !port_open(port) {
        bail!("nothing is listening on Postgres port {port} — start it (or DBngin) first.");
    }
    let (db2, from2, to2) = (db.clone(), from.clone(), to.clone());
    let msg = tokio::task::spawn_blocking(move || pg_branch_switch(port, &db2, &from2, &to2)).await??;
    state
        .registry
        .lock()
        .await
        .set_branch_db(site, Some(db), st.port, Some(to))
        .context("switched, but couldn't record it")?;
    Ok(msg)
}

async fn branches(state: &DaemonState, site: &str, port_override: Option<u16>) -> Response {
    let err = |m: String| Response::Error { message: m };
    let st = match state.registry.lock().await.branch_db_state(site) {
        Ok(s) => s,
        Err(e) => return err(format!("{e:#}")),
    };
    let Some(db) = st.database else {
        return err(format!("{site} has no branch database attached."));
    };
    let live = st.db_branch.unwrap_or_else(|| "?".into());
    let port = port_override.or(st.port).unwrap_or(5432);
    let (db2, db3) = (db.clone(), db.clone());
    let live_size = tokio::task::spawn_blocking(move || pg_db_size(port, &db2))
        .await
        .map(|r| r.unwrap_or_else(|_| "?".into()))
        .unwrap_or_else(|_| "?".into());
    let parked = match tokio::task::spawn_blocking(move || pg_branch_list(port, &db3)).await {
        Ok(Ok(lines)) => lines,
        Ok(Err(e)) => return err(format!("{e:#}")),
        Err(e) => return err(format!("{e:#}")),
    };
    let mut lines = vec![format!("* {live}\t{live_size}\tlive in `{db}`")];
    lines.extend(parked.into_iter().map(|l| format!("  {l}\tparked")));
    Response::Lines { lines }
}

async fn drop_branch(state: &DaemonState, site: &str, branch: Option<String>, port_override: Option<u16>) -> Result<String> {
    let st = state.registry.lock().await.branch_db_state(site)?;
    let Some(db) = st.database else {
        bail!("{site} has no branch database attached.");
    };
    let Some(target) = branch else {
        bail!("usage: dpl db drop-branch <site> <branch>");
    };
    if st.db_branch.as_deref() == Some(target.as_str()) {
        bail!("`{target}` is the live branch in `{db}` — switch away before dropping it.");
    }
    let port = port_override.or(st.port).unwrap_or(5432);
    tokio::task::spawn_blocking(move || pg_branch_drop(port, &db, &target)).await?
}

/// Watch every attached site's `.git/HEAD` and switch the database when the
/// checked-out branch changes. Detached heads are left alone. A failing switch
/// (Postgres down, say) is logged once and not retried until HEAD changes
/// again, so the log doesn't fill with the same error every two seconds.
pub async fn watch(state: DaemonState) {
    let mut failed: std::collections::BTreeMap<String, String> = Default::default();
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        let attached = state.registry.lock().await.attached_branch_dbs();
        for (site, st) in attached {
            let Some(head) = dpl_core::branchdb::git_branch(&st.path) else { continue };
            if st.db_branch.as_ref() == Some(&head) {
                failed.remove(&site);
                continue;
            }
            if failed.get(&site) == Some(&head) {
                continue;
            }
            match switch(&state, &site, Some(head.clone()), None).await {
                Ok(msg) => {
                    failed.remove(&site);
                    tracing::info!(site = %site, "branch db: {msg}");
                }
                Err(e) => {
                    tracing::warn!(site = %site, error = format!("{e:#}"), "branch-db auto-switch failed");
                    failed.insert(site, head);
                }
            }
        }
    }
}
