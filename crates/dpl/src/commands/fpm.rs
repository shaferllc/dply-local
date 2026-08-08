//! `dpl fpm` — the php-fpm pools behind every non-Octane site.
//!
//! php-fpm is the part of the stack dpl used to start and then never look at
//! again: masters were spawned, and nothing reported how loaded they were or
//! noticed when one died. The symptom of that gap is a slow site or a 502 with
//! no way to tell saturation from a crash, which is a diagnosis that used to
//! mean reading `ps` output by hand.
//!
//! A pool is keyed by (PHP version, Xdebug mode, profiler, preload) rather than
//! by site — `xdebug.mode` is read once when the process starts and cannot be
//! set per request — so many sites share one pool, and these commands act on
//! pools rather than on individual sites.

use anyhow::Result;
use dpl_core::ipc::{FpmPoolInfo, Request, Response};

use crate::daemon;

/// `dpl fpm` / `dpl fpm status` — one row per pool, with php-fpm's own counters.
pub fn status(home: Option<&str>, json: bool) -> Result<()> {
    let pools = list(home)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "pools": pools }))?);
        return Ok(());
    }

    if pools.is_empty() {
        println!("No php-fpm pools are running.");
        println!("A pool starts with the first request to a site that uses one.");
        return Ok(());
    }

    println!(
        "{:<8} {:<10} {:>6} {:>7} {:>6} {:>6} {:>5}  {}",
        "PHP", "XDEBUG", "SITES", "WORKERS", "BUSY", "QUEUE", "SLOW", "STATE"
    );
    for p in &pools {
        let (workers, busy, queue, slow) = match &p.stats {
            Some(s) => (
                s.total.to_string(),
                s.active.to_string(),
                s.listen_queue.to_string(),
                s.slow_requests.to_string(),
            ),
            None => ("-".into(), "-".into(), "-".into(), "-".into()),
        };
        // The detail is where a stopped or struggling pool explains itself; a
        // healthy one just says so rather than printing an empty column.
        let state = match (&p.detail, p.running) {
            (Some(d), _) => d.clone(),
            (None, true) => "ok".to_string(),
            (None, false) => "stopped".to_string(),
        };
        let mode = if p.profile { format!("{}+spx", p.mode) } else { p.mode.clone() };
        println!(
            "{:<8} {:<10} {:>6} {:>7} {:>6} {:>6} {:>5}  {}",
            p.php, mode, p.sites, workers, busy, queue, slow, state
        );
    }

    // Only worth saying when it has actually happened — otherwise it is noise
    // on every healthy listing.
    if pools.iter().any(|p| p.stats.as_ref().is_some_and(|s| s.max_children_reached > 0)) {
        println!();
        println!(
            "A pool has hit its worker ceiling: requests queued for want of a worker. \
             Raise `fpm_max_children` in ~/.dpl/config.toml, then `dpl fpm restart`."
        );
    }
    if pools.iter().any(|p| p.stats.as_ref().is_some_and(|s| s.slow_requests > 0)) {
        println!();
        println!("Slow requests were recorded — `dpl fpm slow` to see which endpoints.");
    }
    Ok(())
}

/// Every pool the daemon supervises.
pub fn list(home: Option<&str>) -> Result<Vec<FpmPoolInfo>> {
    match daemon::call(Request::FpmStatus, home)? {
        Response::FpmPools { pools } => Ok(pools),
        Response::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("unexpected response from the daemon"),
    }
}

/// `dpl fpm reload` — SIGUSR2 every pool, keeping the listening socket open.
pub fn reload(home: Option<&str>) -> Result<()> {
    say(daemon::call(Request::ReloadFpm, home)?)
}

/// `dpl fpm restart` — stop every pool and rebuild from config.
pub fn restart(home: Option<&str>) -> Result<()> {
    say(daemon::call(Request::RestartFpm, home)?)
}

/// `dpl fpm slow` — the requests php-fpm flagged as too slow.
///
/// Reads the pool's *error* log rather than its slowlog, because on macOS the
/// slowlog is always empty. php-fpm detects the slow request correctly and
/// records which script and URI it was, then tries to attach to the worker to
/// dump a PHP backtrace — and macOS refuses `task_for_pid()` to a master that
/// isn't root, so the trace is abandoned and the slowlog never written:
///
/// ```text
/// WARNING: child 50685, script '…/index.php' (request: "GET /admin")
///          executing too slow (5.31 sec), logging
/// ERROR:   task_for_pid() failed: … does not have enough privileges to trace
/// ```
///
/// The detection line survives that failure and carries what you actually need
/// — which endpoint, how long — so that is what this shows. Where tracing does
/// work (Linux, or a root master) the backtraces are appended below it.
///
/// Shows the busiest pool: concatenating every pool would interleave entries
/// from different PHP versions with nothing to tell them apart.
pub fn slow(home: Option<&str>, lines: usize, follow: bool) -> Result<()> {
    let pools = list(home)?;
    let Some(pool) = pools
        .iter()
        .max_by_key(|p| p.stats.as_ref().map(|s| s.slow_requests).unwrap_or(0))
    else {
        println!("No php-fpm pools are running, so nothing has been timed yet.");
        return Ok(());
    };

    let log = std::path::Path::new(&pool.log);
    if follow {
        // Filtering a followed stream would mean reimplementing the tail loop;
        // the pool log is low-volume, and the slow lines stand out in it.
        println!("Following the whole pool log — slow requests appear as \"executing too slow\".");
        return crate::commands::tail_log(log, lines, true, &format!("php {} pool log", pool.php));
    }

    let flagged: Vec<String> = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("executing too slow"))
        .rev()
        .take(lines)
        .map(str::to_string)
        .collect();

    if flagged.is_empty() {
        println!("No requests have exceeded the slow threshold yet.");
        println!("php-fpm flags one when it runs longer than the pool's slowlog timeout.");
        return Ok(());
    }
    for line in flagged.iter().rev() {
        println!("{line}");
    }

    // The backtraces, where the platform allows them to be collected at all.
    if let Some(sl) = pool.slowlog.as_deref() {
        let has_traces = std::fs::metadata(sl).map(|m| m.len() > 0).unwrap_or(false);
        if has_traces {
            println!();
            println!("— backtraces ({sl}) —");
            crate::commands::tail_log(std::path::Path::new(sl), lines, false, "")?;
        } else if cfg!(target_os = "macos") {
            println!();
            println!(
                "No PHP backtraces: macOS denies process tracing to a non-root php-fpm master, \
                 so the slowlog stays empty. The lines above still name the script and URI."
            );
        }
    }
    Ok(())
}

fn say(resp: Response) -> Result<()> {
    match resp {
        Response::Message { text } => {
            println!("{text}");
            Ok(())
        }
        Response::Error { message, .. } => anyhow::bail!(message),
        _ => anyhow::bail!("unexpected response from the daemon"),
    }
}
