//! A tiny DNS responder for the `.test` TLD.
//!
//! macOS's `/etc/resolver/test` (and the Linux equivalent) sends every `*.test`
//! lookup here; we answer every A query with `127.0.0.1` so any `<name>.test`
//! resolves to the local proxy — no `/etc/hosts` edits. AAAA and other types
//! get an empty NOERROR so resolvers fall back to the A record.
//!
//! We hand-roll the (very small) DNS wire format rather than pull in a full DNS
//! server crate.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use tokio::net::UdpSocket;

/// Default port; matches what the helper writes into `/etc/resolver/test`.
pub const DEFAULT_PORT: u16 = 5333;

pub fn port() -> u16 {
    std::env::var("DPL_DNS_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_PORT)
}

pub async fn serve() -> Result<()> {
    let port = port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let sock = UdpSocket::bind(addr).await.with_context(|| format!("binding DNS on {addr}"))?;
    tracing::info!(port, "dns responder listening (*.test → 127.0.0.1)");

    let mut buf = [0u8; 512];
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "dns recv failed");
                continue;
            }
        };
        if let Some(resp) = build_response(&buf[..n]) {
            let _ = sock.send_to(&resp, peer).await;
        }
    }
}

/// Build a response resolving any A query to 127.0.0.1; empty for other types.
fn build_response(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount < 1 {
        return None;
    }

    // Walk the QNAME labels to find the end of the question.
    let mut i = 12;
    while i < query.len() && query[i] != 0 {
        i += 1 + query[i] as usize;
    }
    if i >= query.len() {
        return None;
    }
    let qtype_pos = i + 1; // skip the zero label terminator
    if qtype_pos + 4 > query.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([query[qtype_pos], query[qtype_pos + 1]]);
    let question_end = qtype_pos + 4; // QTYPE(2) + QCLASS(2)

    let is_a = qtype == 1;
    let mut resp = Vec::with_capacity(question_end + 16);
    resp.extend_from_slice(&query[0..2]); // copy transaction ID
    let rd = query[2] & 0x01; // recursion-desired bit
    let flags: u16 = 0x8400 | ((rd as u16) << 8); // QR=1, AA=1, copy RD
    resp.extend_from_slice(&flags.to_be_bytes());
    resp.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    resp.extend_from_slice(&(is_a as u16).to_be_bytes()); // ANCOUNT
    resp.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    resp.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    resp.extend_from_slice(&query[12..question_end]); // echo the question

    if is_a {
        resp.extend_from_slice(&[0xC0, 0x0C]); // name pointer to the question
        resp.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
        resp.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
        resp.extend_from_slice(&60u32.to_be_bytes()); // TTL
        resp.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        resp.extend_from_slice(&[127, 0, 0, 1]); // 127.0.0.1
    }
    Some(resp)
}
