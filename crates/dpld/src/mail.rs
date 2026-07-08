//! A tiny SMTP sink for local development. Apps point their mailer at
//! `127.0.0.1:1025` (no auth, no TLS) and every message is captured to disk as
//! a `.eml` file under `~/.dpl/mail` instead of being delivered. `dpl mail`
//! lists/shows/clears them.
//!
//! We implement just enough of SMTP (RFC 5321) to accept mail from typical
//! frameworks: HELO/EHLO, MAIL FROM, RCPT TO, DATA, RSET, NOOP, QUIT.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn port() -> u16 {
    std::env::var("DPL_SMTP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(1025)
}

pub async fn serve() -> Result<()> {
    let port = port();
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding SMTP on 127.0.0.1:{port}"))?;
    let dir = dpl_core::paths::mail_dir(None)?;
    std::fs::create_dir_all(&dir).ok();
    tracing::info!(port, "mail sink listening (SMTP → ~/.dpl/mail)");

    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "smtp accept failed");
                continue;
            }
        };
        let dir = dir.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, dir).await {
                tracing::debug!(error = %e, "smtp connection error");
            }
        });
    }
}

async fn handle(stream: TcpStream, dir: PathBuf) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    write.write_all(b"220 dpl mail sink\r\n").await?;

    let mut line = String::new();
    let mut from = String::new();
    let mut rcpts: Vec<String> = Vec::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break; // client hung up
        }
        let trimmed = line.trim_end();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            write.write_all(b"250 dpl\r\n").await?;
        } else if upper.starts_with("MAIL FROM") {
            from = extract_addr(trimmed);
            rcpts.clear();
            write.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("RCPT TO") {
            rcpts.push(extract_addr(trimmed));
            write.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("DATA") {
            write.write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n").await?;
            let body = read_data(&mut reader).await?;
            store(&dir, &from, &rcpts, &body)?;
            write.write_all(b"250 OK: queued\r\n").await?;
        } else if upper.starts_with("RSET") {
            from.clear();
            rcpts.clear();
            write.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("NOOP") {
            write.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("QUIT") {
            write.write_all(b"221 Bye\r\n").await?;
            break;
        } else {
            // Be lenient with anything else (AUTH, etc.).
            write.write_all(b"250 OK\r\n").await?;
        }
    }
    Ok(())
}

/// Read the DATA payload until a lone `.` line; undo dot-stuffing.
async fn read_data<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Result<String> {
    let mut body = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let content = line.trim_end_matches(['\r', '\n']);
        if content == "." {
            break;
        }
        // Dot-stuffing: a leading ".." represents a literal ".".
        let content = content.strip_prefix('.').unwrap_or(content);
        body.push_str(content);
        body.push('\n');
    }
    Ok(body)
}

/// Persist a captured message as `<millis>-<seq>.eml`, prefixing envelope info
/// as headers so `dpl mail list` can show sender/recipient even if the DATA had
/// none.
fn store(dir: &PathBuf, from: &str, rcpts: &[String], body: &str) -> Result<()> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{millis}-{seq}.eml"));

    // Only add envelope headers if the message doesn't already carry them.
    let mut out = String::new();
    if !body.to_lowercase().contains("\nfrom:") && !body.to_lowercase().starts_with("from:") {
        out.push_str(&format!("X-Envelope-From: {from}\n"));
    }
    if !rcpts.is_empty() {
        out.push_str(&format!("X-Envelope-To: {}\n", rcpts.join(", ")));
    }
    out.push_str(body);
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(file = %path.display(), "captured mail");
    Ok(())
}

/// Pull the address out of `MAIL FROM:<a@b>` / `RCPT TO:<a@b>`.
fn extract_addr(line: &str) -> String {
    if let (Some(start), Some(end)) = (line.find('<'), line.find('>')) {
        if start < end {
            return line[start + 1..end].to_string();
        }
    }
    line.split(':').nth(1).map(|s| s.trim().to_string()).unwrap_or_default()
}
