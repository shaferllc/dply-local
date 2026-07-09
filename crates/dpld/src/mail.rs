//! A tiny SMTP sink for local development. Apps point their mailer at
//! `127.0.0.1:1025` (no TLS) and every message is captured to disk as a `.eml`
//! file under `~/.dpl/mail` instead of being delivered. `dpl mail`
//! lists/shows/clears them.
//!
//! We implement just enough of SMTP (RFC 5321) to accept mail from typical
//! frameworks: HELO/EHLO, AUTH, MAIL FROM, RCPT TO, DATA, RSET, NOOP, QUIT.
//!
//! ## Mailboxes
//!
//! Nothing on the wire says which *site* sent a message: the sender is a php-fpm
//! worker, and `MAIL FROM` is whatever the app configured (Laravel's default is
//! `hello@example.com` for every project). So dpl uses the SMTP **username** as
//! the mailbox name — set `MAIL_USERNAME=<site>` in a project's `.env` and its
//! mail is filed under `<site>`.
//!
//! Any password is accepted; this is a local sink, and authentication here is a
//! label, not a security boundary. Mail sent without AUTH is still captured, it
//! just lands unattributed. The mailbox is recorded as an `X-Dpl-Mailbox` header
//! rather than a subdirectory so message ids — and `dpl mail show <id>` — keep
//! working unchanged.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Header naming the mailbox a message was filed under.
pub const MAILBOX_HEADER: &str = "X-Dpl-Mailbox";

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
    // The authenticated username, which names the mailbox. Survives RSET: it
    // belongs to the connection, not the transaction.
    let mut mailbox: Option<String> = None;

    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break; // client hung up
        }
        let trimmed = line.trim_end();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("EHLO") {
            // Clients only try AUTH if we advertise it, and without AUTH we
            // never learn which site sent the message.
            write.write_all(b"250-dpl\r\n250 AUTH LOGIN PLAIN\r\n").await?;
        } else if upper.starts_with("HELO") {
            // Plain SMTP has no extensions to announce.
            write.write_all(b"250 dpl\r\n").await?;
        } else if upper.starts_with("AUTH") {
            match authenticate(trimmed, &mut reader, &mut write).await? {
                Some(user) => {
                    mailbox = sanitize_mailbox(&user);
                    write.write_all(b"235 2.7.0 Authentication successful\r\n").await?;
                }
                None => {
                    write.write_all(b"535 5.7.8 Authentication failed\r\n").await?;
                }
            }
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
            store(&dir, &from, &rcpts, mailbox.as_deref(), &body)?;
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
            // Be lenient with anything else.
            write.write_all(b"250 OK\r\n").await?;
        }
    }
    Ok(())
}

/// Run an AUTH exchange and return the username, or `None` if we couldn't read
/// one. Any password is accepted — see the module docs.
///
/// Handles the three shapes real clients send:
/// `AUTH PLAIN <base64>`, bare `AUTH PLAIN` then the payload on the next line,
/// and `AUTH LOGIN` (optionally carrying the username as an initial response).
async fn authenticate<R, W>(command: &str, reader: &mut R, write: &mut W) -> Result<Option<String>>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut parts = command.split_whitespace();
    let _auth = parts.next();
    let mech = parts.next().unwrap_or("").to_uppercase();
    let initial = parts.next();

    match mech.as_str() {
        "PLAIN" => {
            let payload = match initial {
                Some(p) => p.to_string(),
                None => {
                    write.write_all(b"334 \r\n").await?;
                    read_trimmed(reader).await?
                }
            };
            Ok(decode_plain(&payload))
        }
        "LOGIN" => {
            let user_b64 = match initial {
                Some(p) => p.to_string(),
                None => {
                    // base64("Username:")
                    write.write_all(b"334 VXNlcm5hbWU6\r\n").await?;
                    read_trimmed(reader).await?
                }
            };
            // base64("Password:") — we must consume it even though we ignore it.
            write.write_all(b"334 UGFzc3dvcmQ6\r\n").await?;
            let _password = read_trimmed(reader).await?;
            Ok(decode_b64_utf8(&user_b64).filter(|u| !u.is_empty()))
        }
        _ => Ok(None),
    }
}

async fn read_trimmed<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn decode_b64_utf8(s: &str) -> Option<String> {
    String::from_utf8(B64.decode(s.trim()).ok()?).ok()
}

/// SASL PLAIN is `authzid \0 authcid \0 passwd`; the username is `authcid`.
fn decode_plain(payload: &str) -> Option<String> {
    let decoded = String::from_utf8(B64.decode(payload.trim()).ok()?).ok()?;
    let mut fields = decoded.split('\0');
    let _authzid = fields.next()?;
    let authcid = fields.next()?;
    (!authcid.is_empty()).then(|| authcid.to_string())
}

/// Reduce a username to a safe mailbox name: lowercase, and only characters
/// that are harmless in a header value and a filter argument. `None` when
/// nothing usable survives, which files the message as unattributed.
fn sanitize_mailbox(user: &str) -> Option<String> {
    let name: String = user
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    // Laravel ships `MAIL_USERNAME=null` as a literal string in some setups.
    (!name.is_empty() && name != "null").then_some(name)
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
fn store(
    dir: &PathBuf,
    from: &str,
    rcpts: &[String],
    mailbox: Option<&str>,
    body: &str,
) -> Result<()> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{millis}-{seq}.eml"));

    std::fs::write(&path, envelope(from, rcpts, mailbox, body))
        .with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(file = %path.display(), mailbox = mailbox.unwrap_or("-"), "captured mail");
    Ok(())
}

/// Prepend dpl's headers to the raw DATA payload.
fn envelope(from: &str, rcpts: &[String], mailbox: Option<&str>, body: &str) -> String {
    let mut out = String::new();
    if let Some(m) = mailbox {
        out.push_str(&format!("{MAILBOX_HEADER}: {m}\n"));
    }
    // Only add envelope headers if the message doesn't already carry them.
    if !body.to_lowercase().contains("\nfrom:") && !body.to_lowercase().starts_with("from:") {
        out.push_str(&format!("X-Envelope-From: {from}\n"));
    }
    if !rcpts.is_empty() {
        out.push_str(&format!("X-Envelope-To: {}\n", rcpts.join(", ")));
    }
    out.push_str(body);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        B64.encode(s)
    }

    #[test]
    fn plain_takes_the_authcid_not_the_authzid() {
        // SASL PLAIN: authzid \0 authcid \0 passwd
        assert_eq!(decode_plain(&b64("\0blog\0secret")), Some("blog".into()));
        assert_eq!(decode_plain(&b64("admin\0blog\0secret")), Some("blog".into()));
    }

    #[test]
    fn plain_rejects_garbage_and_empty_users() {
        assert_eq!(decode_plain("not base64!!"), None);
        assert_eq!(decode_plain(&b64("\0\0secret")), None);
        assert_eq!(decode_plain(&b64("no-nulls")), None);
    }

    #[test]
    fn login_username_round_trips() {
        assert_eq!(decode_b64_utf8(&b64("shop")), Some("shop".into()));
        assert_eq!(decode_b64_utf8(" c2hvcA== "), Some("shop".into()));
        assert_eq!(decode_b64_utf8("!!!"), None);
    }

    #[test]
    fn mailbox_names_are_sanitized() {
        assert_eq!(sanitize_mailbox("Blog"), Some("blog".into()));
        assert_eq!(sanitize_mailbox("my-site_1.dev"), Some("my-site_1.dev".into()));
        // Path separators and spaces can't leak into a header or a filter arg.
        assert_eq!(sanitize_mailbox("../../etc/passwd"), Some("....etcpasswd".into()));
        assert_eq!(sanitize_mailbox("a b"), Some("ab".into()));
    }

    #[test]
    fn unusable_usernames_file_as_unattributed() {
        assert_eq!(sanitize_mailbox(""), None);
        assert_eq!(sanitize_mailbox("   "), None);
        assert_eq!(sanitize_mailbox("!!!"), None);
        // Laravel ships MAIL_USERNAME=null; the string is not a mailbox.
        assert_eq!(sanitize_mailbox("null"), None);
        assert_eq!(sanitize_mailbox("NULL"), None);
    }

    #[test]
    fn envelope_records_the_mailbox_and_keeps_existing_headers() {
        let body = "From: app@blog.test\nSubject: Hi\n\nbody\n";
        let out = envelope("app@blog.test", &["u@x.test".into()], Some("blog"), body);
        assert!(out.starts_with("X-Dpl-Mailbox: blog\n"));
        assert!(out.contains("X-Envelope-To: u@x.test\n"));
        // The message already had a From:, so we don't fabricate one.
        assert!(!out.contains("X-Envelope-From:"));
        assert!(out.ends_with(body));
    }

    #[test]
    fn envelope_omits_the_header_when_unauthenticated() {
        let out = envelope("a@b.test", &[], None, "Subject: x\n\nhi\n");
        assert!(!out.contains(MAILBOX_HEADER));
        assert!(out.contains("X-Envelope-From: a@b.test\n"));
    }
}
