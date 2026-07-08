//! A minimal FastCGI responder client — just enough to gateway one HTTP
//! request to a php-fpm pool and read the response back. We hand-roll the
//! protocol (rather than depend on an unstable crate) because the responder
//! flow is small and well-specified: BEGIN_REQUEST → PARAMS → STDIN → read
//! STDOUT until END_REQUEST.

use std::net::SocketAddr;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// Record types.
const FCGI_BEGIN_REQUEST: u8 = 1;
const FCGI_END_REQUEST: u8 = 3;
const FCGI_PARAMS: u8 = 4;
const FCGI_STDIN: u8 = 5;
const FCGI_STDOUT: u8 = 6;
const FCGI_STDERR: u8 = 7;
const FCGI_RESPONDER: u8 = 1;
const REQUEST_ID: u16 = 1;

/// The parsed CGI response from php-fpm.
pub struct FcgiResponse {
    /// `stdout` split into raw header block + body happens in the proxy; here
    /// we return the concatenated STDOUT stream and STDERR separately.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Execute one responder request against a php-fpm listening at `addr`.
/// `params` are the CGI/FastCGI name/value pairs; `body` is the request body.
pub async fn request(
    addr: SocketAddr,
    params: &[(String, String)],
    body: &[u8],
) -> Result<FcgiResponse> {
    let mut stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connecting to php-fpm at {addr}"))?;

    // BEGIN_REQUEST: role=RESPONDER, flags=0 (close after request).
    let begin = [0u8, FCGI_RESPONDER, 0, 0, 0, 0, 0, 0];
    write_record(&mut stream, FCGI_BEGIN_REQUEST, &begin).await?;

    // PARAMS (chunked if large), then an empty PARAMS to terminate.
    let mut param_bytes = Vec::new();
    for (k, v) in params {
        encode_pair(&mut param_bytes, k.as_bytes(), v.as_bytes());
    }
    write_stream(&mut stream, FCGI_PARAMS, &param_bytes).await?;
    write_record(&mut stream, FCGI_PARAMS, &[]).await?;

    // STDIN (the body), then an empty STDIN to terminate.
    write_stream(&mut stream, FCGI_STDIN, body).await?;
    write_record(&mut stream, FCGI_STDIN, &[]).await?;

    // Read records until END_REQUEST.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    loop {
        let mut header = [0u8; 8];
        stream
            .read_exact(&mut header)
            .await
            .context("reading FastCGI record header")?;
        let rtype = header[1];
        let content_len = u16::from_be_bytes([header[4], header[5]]) as usize;
        let padding = header[6] as usize;

        let mut content = vec![0u8; content_len];
        if content_len > 0 {
            stream.read_exact(&mut content).await.context("reading FastCGI content")?;
        }
        if padding > 0 {
            let mut pad = vec![0u8; padding];
            stream.read_exact(&mut pad).await.ok();
        }

        match rtype {
            FCGI_STDOUT => stdout.extend_from_slice(&content),
            FCGI_STDERR => stderr.extend_from_slice(&content),
            FCGI_END_REQUEST => break,
            _ => {}
        }
    }

    Ok(FcgiResponse { stdout, stderr })
}

/// Write one FastCGI record with the given type and content (<= 64KB).
async fn write_record(stream: &mut TcpStream, rtype: u8, content: &[u8]) -> Result<()> {
    if content.len() > u16::MAX as usize {
        bail!("FastCGI record too large");
    }
    let len = content.len() as u16;
    let header = [
        1, // version
        rtype,
        (REQUEST_ID >> 8) as u8,
        (REQUEST_ID & 0xff) as u8,
        (len >> 8) as u8,
        (len & 0xff) as u8,
        0, // padding length
        0, // reserved
    ];
    stream.write_all(&header).await?;
    stream.write_all(content).await?;
    Ok(())
}

/// Write a possibly-large payload as a sequence of records of <= 32KB each.
async fn write_stream(stream: &mut TcpStream, rtype: u8, data: &[u8]) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    for chunk in data.chunks(32 * 1024) {
        write_record(stream, rtype, chunk).await?;
    }
    Ok(())
}

/// Encode one FastCGI name/value pair (1- or 4-byte lengths).
fn encode_pair(out: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    encode_len(out, name.len());
    encode_len(out, value.len());
    out.extend_from_slice(name);
    out.extend_from_slice(value);
}

fn encode_len(out: &mut Vec<u8>, len: usize) {
    if len < 128 {
        out.push(len as u8);
    } else {
        let l = len as u32;
        out.push((l >> 24) as u8 | 0x80);
        out.push((l >> 16) as u8);
        out.push((l >> 8) as u8);
        out.push(l as u8);
    }
}
