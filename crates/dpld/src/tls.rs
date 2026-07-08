//! HTTPS server. Terminates TLS with per-`.test`-host leaf certificates minted
//! on demand by the [`LocalCa`](crate::ca::LocalCa), then hands the decrypted
//! stream to the same request handler the HTTP server uses (with
//! `HTTPS=on`). Prefers port 443, falling back to 8443 without the helper.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_rustls::TlsAcceptor;

use crate::ca::LocalCa;
use crate::registry::Registry;

/// Resolves (and caches) a leaf cert per SNI host, minting via the local CA.
struct CertResolver {
    ca: Arc<LocalCa>,
    cache: StdMutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl std::fmt::Debug for CertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CertResolver")
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name()?.to_string();
        if let Some(k) = self.cache.lock().unwrap().get(&name) {
            return Some(k.clone());
        }
        let (cert_der, key_der) = self.ca.issue(&name).ok()?;
        let signing = rustls::crypto::ring::sign::any_supported_type(&key_der).ok()?;
        let ck = Arc::new(CertifiedKey::new(vec![cert_der], signing));
        self.cache.lock().unwrap().insert(name, ck.clone());
        Some(ck)
    }
}

pub async fn serve(registry: Arc<Mutex<Registry>>) -> Result<()> {
    // Install a process-wide crypto provider (idempotent).
    let _ = rustls::crypto::ring::default_provider().install_default();

    let ca = Arc::new(LocalCa::load_or_create().context("initialising local CA")?);
    let resolver = Arc::new(CertResolver {
        ca,
        cache: StdMutex::new(HashMap::new()),
    });
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let (listener, port) = bind_preferred().await?;
    tracing::info!(port, "https server listening");

    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "https accept failed");
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let registry = registry.clone();
        tokio::spawn(async move {
            let tls = match acceptor.accept(stream).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::debug!(error = %e, "tls handshake failed");
                    return;
                }
            };
            let io = TokioIo::new(tls);
            let service =
                service_fn(move |req| crate::proxy::handle(req, registry.clone(), port, true));
            if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                tracing::debug!(error = %e, "https connection closed");
            }
        });
    }
}

/// Bind HTTPS, preferring 443 then 8443.
async fn bind_preferred() -> Result<(TcpListener, u16)> {
    let preferred = std::env::var("DPL_HTTPS_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(443);
    for port in [preferred, 8443] {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match TcpListener::bind(addr).await {
            Ok(l) => return Ok((l, port)),
            Err(e) => tracing::warn!(port, error = %e, "could not bind https port, trying next"),
        }
    }
    anyhow::bail!("could not bind the https server on port 443 or 8443")
}
