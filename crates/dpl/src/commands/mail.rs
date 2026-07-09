//! `dpl mail` — browse, inspect and test the mail the daemon's SMTP sink caught.
//!
//! Captured messages are `.eml` files in `~/.dpl/mail`; the daemon writes them
//! and never delivers anything. Everything here reads that directory directly —
//! there is no daemon round-trip, so `dpl mail` works even when `dpld` is down.
//!
//! A message's *mailbox* is the SMTP username the sender authenticated with, and
//! is recorded as an `X-Dpl-Mailbox` header. See `dpld::mail` for why the
//! username is the only usable per-site identifier.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use mail_parser::{MessageParser, MimeHeaders, PartType};

/// The mailbox name for mail that arrived without an SMTP username, and the
/// value `--mailbox` takes to select it.
pub const UNATTRIBUTED: &str = "-";

const MAILBOX_HEADER: &str = "x-dpl-mailbox";

/// The port the daemon's sink listens on.
fn smtp_port() -> u16 {
    std::env::var("DPL_SMTP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(1025)
}

// ---------------------------------------------------------------- parsed model

/// One attachment or inline part.
#[derive(Debug, serde::Serialize)]
pub struct Attachment {
    /// Position in `attachments`, and the index `dpl mail save` takes.
    pub index: usize,
    pub name: String,
    pub mime: String,
    pub size: usize,
    /// Referenced from the HTML body by `cid:` rather than shown as a download.
    pub inline: bool,
    pub cid: Option<String>,
}

/// A fully parsed message.
#[derive(Debug, serde::Serialize)]
pub struct Message {
    pub id: String,
    pub mailbox: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    /// RFC3339, from the `Date:` header when present.
    pub date: Option<String>,
    /// Size of the `.eml` on disk, in bytes.
    pub size: u64,
    pub text: Option<String>,
    /// HTML body with `cid:` references already rewritten to `data:` URIs, so a
    /// viewer can show embedded images without reaching the network.
    pub html: Option<String>,
    /// Remote (`http`/`https`) resources the HTML would fetch if allowed. Drives
    /// the "N remote resources blocked" banner.
    pub remote_resources: usize,
    pub links: Vec<String>,
    pub attachments: Vec<Attachment>,
    pub headers: Vec<(String, String)>,
}

impl Message {
    fn preview(&self) -> String {
        let source = self.text.clone().or_else(|| self.html.as_deref().map(strip_tags));
        let flat = source.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
        truncate(&flat, 120)
    }

    /// Does this message match a free-text query? Searches the fields a person
    /// would actually remember: who, what, and the words in the body.
    fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        let hay: [&str; 5] = [
            &self.subject,
            &self.from,
            &self.to,
            &self.mailbox,
            self.text.as_deref().unwrap_or(""),
        ];
        if hay.iter().any(|f| f.to_lowercase().contains(&q)) {
            return true;
        }
        // Search the rendered HTML text, not its markup: a query for "reset"
        // shouldn't match a CSS class named `reset`.
        self.html.as_deref().map(strip_tags).is_some_and(|t| t.to_lowercase().contains(&q))
    }
}

/// Parse one `.eml` off disk.
pub fn parse(path: &Path) -> Result<Message> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let size = raw.len() as u64;
    let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();

    let parsed = MessageParser::default()
        .parse(&raw)
        .with_context(|| format!("{} is not a parsable message", path.display()))?;

    let headers: Vec<(String, String)> = parsed
        .headers_raw()
        .map(|(name, value)| (name.to_string(), value.trim().to_string()))
        .collect();

    let mailbox = headers
        .iter()
        .find(|(n, _)| n.to_lowercase() == MAILBOX_HEADER)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| UNATTRIBUTED.to_string());

    // Collect attachments first: the HTML body needs them to inline `cid:` refs.
    let attachments: Vec<(Attachment, Vec<u8>)> = parsed
        .attachments()
        .enumerate()
        .map(|(index, part)| {
            let mime = part
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{}", ct.ctype(), sub),
                    None => ct.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            let cid = part.content_id().map(|c| c.trim_matches(['<', '>']).to_string());
            let inline = cid.is_some()
                || part
                    .content_disposition()
                    .is_some_and(|d| d.ctype().eq_ignore_ascii_case("inline"));
            let bytes = part.contents().to_vec();
            let att = Attachment {
                index,
                name: part.attachment_name().unwrap_or("(unnamed)").to_string(),
                mime,
                size: bytes.len(),
                inline,
                cid,
            };
            (att, bytes)
        })
        .collect();

    // `body_text`/`body_html` silently convert one into the other when the
    // message has only one of them, so asking them directly would report every
    // message as having both. Inspect the part's real type instead: a plain-text
    // mail must not claim an HTML body, or the viewer offers an HTML tab showing
    // markup the sender never wrote.
    let text = parsed
        .text_part(0)
        .filter(|p| matches!(p.body, PartType::Text(_)))
        .and_then(|_| parsed.body_text(0))
        .map(|c| c.into_owned());
    let html = parsed
        .html_part(0)
        .filter(|p| matches!(p.body, PartType::Html(_)))
        .and_then(|_| parsed.body_html(0))
        .map(|c| c.into_owned())
        .map(|h| inline_cid_images(&h, &attachments));

    let remote_resources = html.as_deref().map(count_remote_resources).unwrap_or(0);
    let links = extract_links(html.as_deref(), text.as_deref());

    Ok(Message {
        id,
        mailbox,
        from: address_of(parsed.from()),
        to: address_of(parsed.to()),
        cc: address_of(parsed.cc()),
        subject: parsed.subject().unwrap_or_default().to_string(),
        date: parsed.date().map(|d| d.to_rfc3339()),
        size,
        text,
        html,
        remote_resources,
        links,
        attachments: attachments.into_iter().map(|(a, _)| a).collect(),
        headers,
    })
}

/// Read one attachment's bytes back out of a message.
fn attachment_bytes(path: &Path, index: usize) -> Result<(String, Vec<u8>)> {
    let raw = std::fs::read(path)?;
    let parsed = MessageParser::default().parse(&raw).context("unparsable message")?;
    let part = parsed
        .attachments()
        .nth(index)
        .with_context(|| format!("no attachment {index} in this message"))?;
    let name = part.attachment_name().unwrap_or("attachment").to_string();
    Ok((name, part.contents().to_vec()))
}

fn address_of(addr: Option<&mail_parser::Address<'_>>) -> String {
    let Some(addr) = addr else { return String::new() };
    addr.iter()
        .filter_map(|a| a.address.as_deref())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Replace `cid:` image references with `data:` URIs so embedded images render
/// with the network blocked. An unmatched `cid:` is left alone; it renders as a
/// broken image, which is the truth.
fn inline_cid_images(html: &str, attachments: &[(Attachment, Vec<u8>)]) -> String {
    let mut out = html.to_string();
    for (att, bytes) in attachments {
        let Some(cid) = &att.cid else { continue };
        let data_uri = format!("data:{};base64,{}", att.mime, B64.encode(bytes));
        for pattern in [format!("cid:{cid}"), format!("CID:{cid}")] {
            out = out.replace(&pattern, &data_uri);
        }
    }
    out
}

/// Count the `http(s)` resources the HTML would fetch — images, stylesheets,
/// scripts, CSS `url()`s. Anchors are excluded: a link is only followed if the
/// reader clicks it, so it isn't a tracking risk on render.
fn count_remote_resources(html: &str) -> usize {
    let lower = html.to_lowercase();
    let mut count = 0;
    for (i, _) in lower.match_indices("http") {
        // Walk back over the URL's opening quote to the attribute that owns it.
        let before = &lower[..i];
        let before = before.trim_end_matches(['"', '\'', '(', ' ', '=']);
        let is_resource = ["src", "background", "poster", "url", "srcset"]
            .iter()
            .any(|attr| before.ends_with(attr));
        // `url(` in a style attribute or <style> block.
        let in_css_url = before.ends_with("url");
        if is_resource || in_css_url {
            count += 1;
        }
    }
    count
}

/// Every URL a person might want to click — chiefly password-reset and
/// verification links. HTML anchors first (in document order), then any bare
/// URLs in the plain-text part that the HTML didn't already cover.
fn extract_links(html: Option<&str>, text: Option<&str>) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    let mut push = |url: String| {
        if !url.is_empty() && !links.contains(&url) {
            links.push(url);
        }
    };

    if let Some(html) = html {
        let lower = html.to_lowercase();
        for (i, _) in lower.match_indices("href=") {
            let rest = &html[i + 5..];
            let (quote, rest) = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => (q, &rest[1..]),
                _ => (' ', rest), // unquoted attribute
            };
            let end = rest.find(quote).unwrap_or_else(|| rest.find('>').unwrap_or(rest.len()));
            let url = rest[..end].trim();
            if url.starts_with("http://") || url.starts_with("https://") {
                push(decode_entities(url));
            }
        }
    }

    if let Some(text) = text {
        for word in text.split_whitespace() {
            if word.starts_with("http://") || word.starts_with("https://") {
                push(word.trim_end_matches(['.', ',', ')', '>', ';']).to_string());
            }
        }
    }
    links
}

/// The handful of entities that actually show up inside URLs.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#38;", "&").replace("&quot;", "\"")
}

/// Crude tag stripper, for previews and body search only — never for display.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut depth = 0usize;
    for c in html.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).replace("&nbsp;", " ")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

// ------------------------------------------------------------------- discovery

/// Every captured message, newest first, optionally filtered.
fn load_all(dir: &Path, mailbox: Option<&str>, search: Option<&str>) -> Result<Vec<Message>> {
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("eml") {
                files.push(p);
            }
        }
    }
    files.sort();
    files.reverse(); // newest first — ids lead with a millisecond timestamp

    Ok(files
        .iter()
        // A message that fails to parse is skipped rather than aborting the
        // whole listing; one malformed capture shouldn't hide the inbox.
        .filter_map(|p| parse(p).ok())
        .filter(|m| mailbox.is_none_or(|want| m.mailbox == want))
        .filter(|m| search.is_none_or(|q| m.matches(q)))
        .collect())
}

fn path_for(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.eml"))
}

// -------------------------------------------------------------------- commands

pub fn list(
    home: Option<&str>,
    mailbox: Option<String>,
    search: Option<String>,
    json: bool,
) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let messages = load_all(&dir, mailbox.as_deref(), search.as_deref())?;

    if json {
        let arr: Vec<_> = messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "from": m.from,
                    "to": m.to,
                    "subject": m.subject,
                    "mailbox": m.mailbox,
                    "date": m.date,
                    "size": m.size,
                    "preview": m.preview(),
                    "has_html": m.html.is_some(),
                    "attachments": m.attachments.iter().filter(|a| !a.inline).count(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    if messages.is_empty() {
        match (mailbox.as_deref(), search.as_deref()) {
            (_, Some(q)) => println!("No captured mail matching `{q}`."),
            (Some(m), _) => println!("No captured mail in mailbox `{m}`."),
            _ => println!(
                "No captured mail. Point your app's SMTP at 127.0.0.1:{} and set \
                 MAIL_USERNAME=<site> to file it under that site.\n\
                 Try `dpl mail send --html` to capture a sample message.",
                smtp_port()
            ),
        }
        return Ok(());
    }

    println!("{:<24}  {:<14}  {:<26}  SUBJECT", "ID", "MAILBOX", "TO");
    for m in &messages {
        let mut flags = String::new();
        if m.html.is_some() {
            flags.push_str(" [html]");
        }
        let n = m.attachments.iter().filter(|a| !a.inline).count();
        if n > 0 {
            flags.push_str(&format!(" [{n} attachment{}]", if n == 1 { "" } else { "s" }));
        }
        println!(
            "{:<24}  {:<14}  {:<26}  {}{}",
            m.id,
            truncate(&m.mailbox, 14),
            truncate(&m.to, 26),
            m.subject,
            flags
        );
    }
    Ok(())
}

pub fn mailboxes(home: Option<&str>, json: bool) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let messages = load_all(&dir, None, None)?;
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for m in &messages {
        *counts.entry(m.mailbox.clone()).or_default() += 1;
    }

    if json {
        let arr: Vec<_> = counts
            .iter()
            .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    if counts.is_empty() {
        println!("No captured mail yet.");
        return Ok(());
    }
    println!("{:<20}  MESSAGES", "MAILBOX");
    println!("{:<20}  {}", "(all)", messages.len());
    for (name, count) in counts {
        let label = if name == UNATTRIBUTED { "(no username)" } else { &name };
        println!("{label:<20}  {count}");
    }
    Ok(())
}

/// Which representation of a message `show` prints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Raw,
    Html,
    Text,
    Headers,
}

impl std::str::FromStr for Part {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "raw" => Ok(Part::Raw),
            "html" => Ok(Part::Html),
            "text" => Ok(Part::Text),
            "headers" => Ok(Part::Headers),
            other => anyhow::bail!("unknown part `{other}` (raw|html|text|headers)"),
        }
    }
}

pub fn show(home: Option<&str>, id: &str, part: Part, json: bool) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let path = path_for(&dir, id);

    // `--json` hands the GUI everything in one round-trip: bodies, headers,
    // links, attachment metadata and the remote-resource count.
    if json {
        let message = parse(&path).with_context(|| format!("no message {id}"))?;
        println!("{}", serde_json::to_string_pretty(&message)?);
        return Ok(());
    }

    match part {
        Part::Raw => {
            let body = std::fs::read_to_string(&path).with_context(|| format!("no message {id}"))?;
            println!("{body}");
        }
        Part::Html => {
            let m = parse(&path).with_context(|| format!("no message {id}"))?;
            match m.html {
                Some(h) => println!("{h}"),
                None => anyhow::bail!("message {id} has no HTML part"),
            }
        }
        Part::Text => {
            let m = parse(&path).with_context(|| format!("no message {id}"))?;
            match m.text.or_else(|| m.html.as_deref().map(strip_tags)) {
                Some(t) => println!("{t}"),
                None => anyhow::bail!("message {id} has no readable body"),
            }
        }
        Part::Headers => {
            let m = parse(&path).with_context(|| format!("no message {id}"))?;
            for (name, value) in m.headers {
                println!("{name}: {value}");
            }
        }
    }
    Ok(())
}

pub fn attachments(home: Option<&str>, id: &str, json: bool) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let m = parse(&path_for(&dir, id)).with_context(|| format!("no message {id}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&m.attachments)?);
        return Ok(());
    }
    if m.attachments.is_empty() {
        println!("No attachments.");
        return Ok(());
    }
    println!("{:<3}  {:<32}  {:<28}  {:>9}  KIND", "#", "NAME", "TYPE", "SIZE");
    for a in &m.attachments {
        println!(
            "{:<3}  {:<32}  {:<28}  {:>9}  {}",
            a.index,
            truncate(&a.name, 32),
            truncate(&a.mime, 28),
            human_size(a.size as u64),
            if a.inline { "inline" } else { "attachment" }
        );
    }
    println!("\nSave one with `dpl mail save {id} <#>`.");
    Ok(())
}

pub fn save(home: Option<&str>, id: &str, index: usize, out: Option<String>) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let (name, bytes) = attachment_bytes(&path_for(&dir, id), index)
        .with_context(|| format!("reading attachment {index} of {id}"))?;

    // `--out` may name a file or an existing directory to drop the file into.
    let target = match out {
        Some(p) => {
            let p = PathBuf::from(p);
            if p.is_dir() { p.join(&name) } else { p }
        }
        None => std::env::current_dir()?.join(&name),
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&target, &bytes).with_context(|| format!("writing {}", target.display()))?;
    println!("Saved {} ({}) → {}", name, human_size(bytes.len() as u64), target.display());
    Ok(())
}

pub fn links(home: Option<&str>, id: &str, json: bool) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let m = parse(&path_for(&dir, id)).with_context(|| format!("no message {id}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&m.links)?);
        return Ok(());
    }
    if m.links.is_empty() {
        println!("No links in this message.");
        return Ok(());
    }
    for link in &m.links {
        println!("{link}");
    }
    Ok(())
}

pub fn clear(home: Option<&str>, mailbox: Option<String>) -> Result<()> {
    let dir = dpl_core::paths::mail_dir(home)?;
    let messages = load_all(&dir, mailbox.as_deref(), None)?;
    let n = messages.len();
    for m in &messages {
        let _ = std::fs::remove_file(path_for(&dir, &m.id));
    }
    match mailbox.as_deref() {
        Some(m) => println!("Cleared {n} message(s) from mailbox `{m}`."),
        None => println!("Cleared {n} message(s)."),
    }
    Ok(())
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

// ----------------------------------------------------------------------- send

/// `dpl mail send` — deliver a message into the local sink.
///
/// This is a debugging aid: it proves the sink is reachable, exercises the
/// viewer with a realistic HTML mail, and lets you check a mailbox is wired up
/// without booting the app that would normally send.
pub fn send(
    to: String,
    from: String,
    subject: String,
    mailbox: Option<String>,
    html: bool,
    body: Option<String>,
) -> Result<()> {
    let port = smtp_port();
    let stream = TcpStream::connect(("127.0.0.1", port)).with_context(|| {
        format!("no SMTP sink on 127.0.0.1:{port} — is dpld running? (`dpl start`)")
    })?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    expect(&mut reader, "220")?;
    say(&mut writer, "EHLO dpl-cli")?;
    read_multiline(&mut reader, "250")?;

    // Authenticating names the mailbox; skipping it files the mail unattributed,
    // which is exactly what an app with no MAIL_USERNAME would do.
    if let Some(user) = &mailbox {
        say(&mut writer, "AUTH LOGIN")?;
        expect(&mut reader, "334")?;
        say(&mut writer, &B64.encode(user))?;
        expect(&mut reader, "334")?;
        say(&mut writer, &B64.encode("dpl"))?;
        expect(&mut reader, "235")?;
    }

    say(&mut writer, &format!("MAIL FROM:<{from}>"))?;
    expect(&mut reader, "250")?;
    say(&mut writer, &format!("RCPT TO:<{to}>"))?;
    expect(&mut reader, "250")?;
    say(&mut writer, "DATA")?;
    expect(&mut reader, "354")?;

    let data = compose(&to, &from, &subject, html, body.as_deref());
    // Dot-stuffing: a line that is just "." would end the DATA section early.
    for line in data.lines() {
        let line = if line.starts_with('.') { format!(".{line}") } else { line.to_string() };
        writeln!(writer, "{line}\r")?;
    }
    write!(writer, ".\r\n")?;
    writer.flush()?;
    expect(&mut reader, "250")?;

    say(&mut writer, "QUIT")?;

    println!(
        "Sent `{subject}` → {to}{}.\nRun `dpl mail list` to see it.",
        mailbox.map(|m| format!(" (mailbox `{m}`)")).unwrap_or_default()
    );
    Ok(())
}

fn say(w: &mut TcpStream, line: &str) -> Result<()> {
    write!(w, "{line}\r\n")?;
    w.flush()?;
    Ok(())
}

fn expect(reader: &mut BufReader<TcpStream>, code: &str) -> Result<String> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if !line.starts_with(code) {
        anyhow::bail!("SMTP sink replied `{}`, expected {code}", line.trim());
    }
    Ok(line)
}

/// Read an ESMTP multiline reply (`250-…` continuation lines, then `250 …`).
fn read_multiline(reader: &mut BufReader<TcpStream>, code: &str) -> Result<()> {
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if !line.starts_with(code) {
            anyhow::bail!("SMTP sink replied `{}`, expected {code}", line.trim());
        }
        // `250-` means more lines follow; `250 ` is the last.
        if line.as_bytes().get(3) != Some(&b'-') {
            return Ok(());
        }
    }
}

/// Build the RFC 5322 message. With `--html` we send `multipart/alternative`
/// carrying both a text and an HTML body, which is what a real app sends and
/// what the viewer's HTML/Text tabs need in order to have anything to show.
fn compose(to: &str, from: &str, subject: &str, html: bool, body: Option<&str>) -> String {
    let text = body.map(str::to_string).unwrap_or_else(|| {
        "This is a test message from `dpl mail send`.\n\n\
         Reset your password: https://example.test/reset?token=abc123\n"
            .to_string()
    });

    if !html {
        return format!(
            "From: {from}\nTo: {to}\nSubject: {subject}\n\
             Content-Type: text/plain; charset=utf-8\n\n{text}"
        );
    }

    // A remote image is included deliberately: it exercises the viewer's
    // remote-content blocking, which is the whole reason that banner exists.
    let html_body = format!(
        "<html><body style=\"font-family:-apple-system,sans-serif;padding:24px\">\
         <h1 style=\"color:#5b4ee5\">{subject}</h1>\
         <p>This is a test message from <code>dpl mail send</code>.</p>\
         <p><a href=\"https://example.test/reset?token=abc123\">Reset your password</a></p>\
         <img src=\"https://example.test/pixel.gif\" width=\"1\" height=\"1\" alt=\"\">\
         <p style=\"color:#888;font-size:12px\">Captured by dpl — never delivered.</p>\
         </body></html>"
    );

    let boundary = "dpl-boundary-8f2a";
    format!(
        "From: {from}\nTo: {to}\nSubject: {subject}\n\
         MIME-Version: 1.0\n\
         Content-Type: multipart/alternative; boundary=\"{boundary}\"\n\
         \n\
         --{boundary}\n\
         Content-Type: text/plain; charset=utf-8\n\n\
         {text}\n\
         --{boundary}\n\
         Content-Type: text/html; charset=utf-8\n\n\
         {html_body}\n\
         --{boundary}--\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_resources_not_anchors() {
        // An <a href> is only fetched if clicked, so it isn't a tracking risk.
        assert_eq!(count_remote_resources("<a href=\"https://x.test\">hi</a>"), 0);
        assert_eq!(count_remote_resources("<img src=\"https://x.test/p.gif\">"), 1);
        assert_eq!(count_remote_resources("<img src='http://x.test/p.gif'>"), 1);
        assert_eq!(
            count_remote_resources("<div style=\"background:url(https://x.test/b.png)\">"),
            1
        );
        // data: and cid: URIs are local and must not be counted.
        assert_eq!(count_remote_resources("<img src=\"data:image/gif;base64,AA\">"), 0);
        assert_eq!(count_remote_resources("<img src=\"cid:logo\">"), 0);
    }

    #[test]
    fn extracts_and_dedupes_links_in_document_order() {
        let html = "<a href=\"https://a.test/1\">a</a><a href='https://b.test/2'>b</a>\
                    <a href=\"https://a.test/1\">dup</a>";
        assert_eq!(extract_links(Some(html), None), vec!["https://a.test/1", "https://b.test/2"]);
    }

    #[test]
    fn decodes_entities_in_reset_urls() {
        // Laravel's HTML-escaped reset links are the main reason this exists.
        let html = "<a href=\"https://x.test/reset?token=1&amp;email=a%40b.test\">Reset</a>";
        assert_eq!(extract_links(Some(html), None), vec!["https://x.test/reset?token=1&email=a%40b.test"]);
    }

    #[test]
    fn finds_bare_urls_in_plain_text_and_strips_trailing_punctuation() {
        let text = "Go to https://x.test/verify?t=9, then log in.";
        assert_eq!(extract_links(None, Some(text)), vec!["https://x.test/verify?t=9"]);
    }

    #[test]
    fn html_links_win_and_text_adds_only_new_ones() {
        let html = "<a href=\"https://a.test\">a</a>";
        let text = "https://a.test and https://b.test";
        assert_eq!(extract_links(Some(html), Some(text)), vec!["https://a.test", "https://b.test"]);
    }

    #[test]
    fn cid_references_become_data_uris() {
        let att = Attachment {
            index: 0,
            name: "logo.gif".into(),
            mime: "image/gif".into(),
            size: 2,
            inline: true,
            cid: Some("logo123".into()),
        };
        let html = inline_cid_images("<img src=\"cid:logo123\">", &[(att, vec![0x47, 0x49])]);
        assert!(html.contains("data:image/gif;base64,R0k"));
        assert!(!html.contains("cid:"));
    }

    #[test]
    fn unmatched_cid_is_left_alone() {
        let html = inline_cid_images("<img src=\"cid:missing\">", &[]);
        assert_eq!(html, "<img src=\"cid:missing\">");
    }

    #[test]
    fn strip_tags_drops_markup_for_search() {
        assert_eq!(strip_tags("<p class=\"reset\">Hello <b>world</b></p>").trim(), "Hello world");
    }

    #[test]
    fn compose_html_is_multipart_with_both_bodies() {
        let msg = compose("a@b.test", "c@d.test", "Hi", true, None);
        assert!(msg.contains("Content-Type: multipart/alternative"));
        assert!(msg.contains("Content-Type: text/plain"));
        assert!(msg.contains("Content-Type: text/html"));
        assert!(msg.trim_end().ends_with("--dpl-boundary-8f2a--"));
    }

    #[test]
    fn compose_plain_has_no_multipart() {
        let msg = compose("a@b.test", "c@d.test", "Hi", false, Some("body"));
        assert!(!msg.contains("multipart"));
        assert!(msg.ends_with("body"));
    }

    /// Write an .eml to a temp path, parse it, clean up.
    fn parse_str(raw: &str) -> Message {
        let dir = std::env::temp_dir().join(format!("dpl-mail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}-0.eml", raw.len()));
        std::fs::write(&path, raw).unwrap();
        let m = parse(&path).unwrap();
        std::fs::remove_file(&path).ok();
        m
    }

    #[test]
    fn a_plain_text_mail_has_no_html_body() {
        // mail-parser's body_html() would happily synthesize HTML from the text
        // part. A plain message must not claim an HTML body.
        let m = parse_str("From: a@b.test\nSubject: Hi\nContent-Type: text/plain\n\nhello\n");
        assert_eq!(m.text.as_deref().map(str::trim), Some("hello"));
        assert!(m.html.is_none());
    }

    #[test]
    fn an_html_only_mail_has_no_text_body() {
        let m = parse_str(
            "From: a@b.test\nSubject: Hi\nContent-Type: text/html\n\n<p>hello</p>\n",
        );
        assert!(m.html.is_some());
        assert!(m.text.is_none());
    }

    #[test]
    fn multipart_alternative_yields_both_and_decodes_quoted_printable() {
        let raw = "From: a@b.test\nSubject: Hi\nMIME-Version: 1.0\n\
                   Content-Type: multipart/alternative; boundary=\"b\"\n\n\
                   --b\nContent-Type: text/plain; charset=utf-8\n\
                   Content-Transfer-Encoding: quoted-printable\n\nCaf=C3=A9 time\n\
                   --b\nContent-Type: text/html; charset=utf-8\n\n<p>Caf\u{e9}</p>\n--b--\n";
        let m = parse_str(raw);
        assert!(m.text.as_deref().unwrap().contains("Café"), "{:?}", m.text);
        assert!(m.html.as_deref().unwrap().contains("<p>"));
    }

    #[test]
    fn mailbox_falls_back_to_unattributed() {
        let m = parse_str("From: a@b.test\nSubject: x\n\nhi\n");
        assert_eq!(m.mailbox, UNATTRIBUTED);
        let m = parse_str("X-Dpl-Mailbox: blog\nFrom: a@b.test\nSubject: x\n\nhi\n");
        assert_eq!(m.mailbox, "blog");
    }

    #[test]
    fn human_size_reads_naturally() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
