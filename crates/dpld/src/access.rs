//! Request access logging — the "real traffic" the System panel tails.
//!
//! Every served request appends one line to `~/.dpl/logs/access.log` (all sites,
//! the traffic feed) and to `~/.dpl/logs/<site>.log` (that site alone, what
//! `dpl logs <site>` reads). Writing happens on a dedicated thread fed by a
//! channel, so the request path never blocks on disk — it just hands off a
//! formatted line and moves on.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

struct Entry {
    /// Line for the global access log (includes the host).
    global: String,
    /// Site name whose per-site log also gets a line, if the request matched one.
    site: Option<String>,
    /// Line for the per-site log (host omitted — it's implied by the file).
    per_site: String,
}

static TX: OnceLock<Sender<Entry>> = OnceLock::new();

/// Spawn the writer thread. Idempotent-ish: a second call is ignored because the
/// channel is already set.
pub fn init() {
    if TX.get().is_some() {
        return;
    }
    let Ok(dir) = dpl_core::paths::logs_dir(None) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let (tx, rx) = channel::<Entry>();

    std::thread::Builder::new()
        .name("dpl-access-log".into())
        .spawn(move || {
            // Open-append per line — on this thread, off the request path. A kept-
            // open handle would be faster but writes to a ghost inode if the log is
            // deleted or rotated; reopening each line survives that, and at dev
            // request rates the cost is nothing.
            let append = |path: std::path::PathBuf, line: &str| {
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, "{line}");
                }
            };
            for entry in rx {
                append(dir.join("access.log"), &entry.global);
                if let Some(name) = entry.site.as_deref().and_then(sanitize) {
                    append(dir.join(format!("{name}.log")), &entry.per_site);
                }
            }
        })
        .ok();

    let _ = TX.set(tx);
}

/// Record one served request. Cheap and non-blocking; drops the line if the
/// writer isn't up (before `init`, or if the channel is gone).
pub fn record(client: &str, method: &str, target: &str, host: &str, site: Option<&str>, status: u16, millis: u128) {
    let Some(tx) = TX.get() else { return };
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("[{ts}] {client} \"{method} {target}\" {status} {millis}ms");
    let _ = tx.send(Entry {
        global: format!("{line}  {host}"),
        site: site.map(str::to_string),
        per_site: line,
    });
}

/// A site name safe to use as a file stem (defends the per-site path against a
/// crafted Host header). `None` rejects anything with a path separator or dots.
fn sanitize(site: &str) -> Option<String> {
    let ok = !site.is_empty()
        && site.len() <= 128
        && site.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| site.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_path_tricks() {
        assert_eq!(sanitize("acme"), Some("acme".into()));
        assert_eq!(sanitize("blog-orbit"), Some("blog-orbit".into()));
        assert_eq!(sanitize("../etc/passwd"), None);
        assert_eq!(sanitize("a.b"), None);
        assert_eq!(sanitize(""), None);
    }
}
