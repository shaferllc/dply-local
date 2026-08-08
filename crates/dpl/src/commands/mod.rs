//! Command handlers, split by family: `local` drives the daemon's `.test`
//! sites; `dply` is the platform subtree.

pub mod daemon;
pub mod dev;
pub mod doctor;
pub mod dply;
pub mod fpm;
pub mod keel;
pub mod local;
pub mod mail;
pub mod node;
pub mod octane;
pub mod parity;
pub mod preload;
pub mod profile;
pub mod share;
pub mod tags;
pub mod valet;
pub mod xdebug;

use std::path::Path;

use dpl_core::ipc::Response;

/// Print the last `lines` of a log file, then optionally follow it.
///
/// Shared by `dpl logs` and `dpl dev logs` so the two behave identically —
/// including the detail that matters: a file that *shrinks* has been rotated or
/// truncated by a restarting backend, so following resumes from the top rather
/// than waiting forever at an offset past the new end.
pub fn tail_log(path: &Path, lines: usize, follow: bool, label: &str) -> anyhow::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(lines);
    for line in &all[start..] {
        println!("{line}");
    }
    if !follow {
        if all.is_empty() {
            println!("(nothing logged yet — {})", path.display());
        }
        return Ok(());
    }

    let mut file = std::fs::File::open(path)?;
    let mut pos = file.seek(SeekFrom::End(0))?;
    println!("— following {label} (Ctrl-C to stop) —");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(pos);
        if len < pos {
            pos = 0;
        }
        if len > pos {
            file.seek(SeekFrom::Start(pos))?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            print!("{buf}");
            let _ = std::io::stdout().flush();
            pos = len;
        }
    }
}

/// Turn an unexpected daemon response into an error — shared by the local
/// command handlers and `main`.
pub fn unexpected(resp: Response) -> anyhow::Result<()> {
    match resp {
        Response::Error { message } => anyhow::bail!("daemon error: {message}"),
        other => anyhow::bail!("unexpected daemon response: {other:?}"),
    }
}
