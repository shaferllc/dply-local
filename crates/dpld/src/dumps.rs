//! The `dumps` receiver — the app-side of a LaraDumps-style debugger.
//!
//! A dedicated HTTP listener (default :9912) that the `shaferllc/dumps` PHP
//! package POSTs JSON payloads to when a site calls `dumps($x)`. Payloads land
//! in a bounded ring buffer and are broadcast to any connected viewer.
//!
//! Endpoints:
//! - `POST /`             ingest one dump payload (JSON)
//! - `GET  /dumps/stream` backfill the ring, then live-tail as NDJSON
//! - `POST /dumps/clear`  empty the ring
//!
//! Because `dpld` launches the php-fpm pools, it injects `DUMPS_HOST/PORT/
//! ENABLED` into their environment (see [`crate::fpm`]) so `dumps()` works with
//! zero config in any served `.test` site — and is a silent no-op everywhere
//! else (production, CI).

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

type DumpsBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

const RING_CAP: usize = 1000;
pub const DEFAULT_PORT: u16 = 9912;

/// Receiver port; php-fpm is told the same via env injection.
pub fn port() -> u16 {
    std::env::var("DPL_DUMPS_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_PORT)
}

/// Shared buffer + broadcast for received dumps.
pub struct Dumps {
    ring: Mutex<VecDeque<Value>>,
    tx: broadcast::Sender<Value>,
    next_id: AtomicU64,
    /// Paused breakpoints awaiting Continue/Stop, keyed by token.
    pending: Mutex<HashMap<String, oneshot::Sender<String>>>,
}

impl Dumps {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(512);
        Arc::new(Dumps {
            ring: Mutex::new(VecDeque::with_capacity(RING_CAP)),
            tx,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Register a paused breakpoint; the returned receiver resolves when the
    /// app resumes it (or the caller times out).
    fn register_pause(&self, token: String) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(token, tx);
        rx
    }

    /// Resume a paused breakpoint with an action ("continue"/"stop").
    fn resume(&self, token: &str, action: String) {
        if let Some(tx) = self.pending.lock().unwrap().remove(token) {
            let _ = tx.send(action);
        }
    }

    fn drop_pause(&self, token: &str) {
        self.pending.lock().unwrap().remove(token);
    }

    /// Stamp a payload with a monotonic id + receive time, buffer it (evicting
    /// the oldest past the cap), and broadcast it to live viewers.
    fn push(&self, mut payload: Value) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        if let Value::Object(map) = &mut payload {
            map.insert("id".into(), Value::from(id));
            map.insert("received_at".into(), Value::from(now));
        }
        {
            let mut ring = self.ring.lock().unwrap();
            if ring.len() >= RING_CAP {
                ring.pop_front();
            }
            ring.push_back(payload.clone());
        }
        let _ = self.tx.send(payload); // ignore "no receivers"
    }

    fn snapshot(&self) -> Vec<Value> {
        self.ring.lock().unwrap().iter().cloned().collect()
    }

    fn clear(&self) {
        self.ring.lock().unwrap().clear();
    }
}

/// Serve the receiver forever.
pub async fn serve(dumps: Arc<Dumps>) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    let listener = TcpListener::bind(addr).await.with_context(|| format!("binding dumps on {addr}"))?;
    tracing::info!(port = port(), "dumps receiver listening");

    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => { tracing::warn!(error = %e, "dumps accept failed"); continue; }
        };
        let io = TokioIo::new(stream);
        let dumps = dumps.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| handle(req, dumps.clone()));
            // Keep-alive so a viewer's long stream connection stays open.
            let _ = http1::Builder::new().serve_connection(io, service).await;
        });
    }
}

async fn handle(req: Request<Incoming>, dumps: Arc<Dumps>) -> Result<Response<DumpsBody>, std::convert::Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    // Pause/resume carry a token in the path.
    if method == Method::GET {
        if let Some(token) = path.strip_prefix("/dumps/pause/") {
            return Ok(pause(token.to_string(), dumps).await);
        }
    }
    if method == Method::POST {
        if let Some(token) = path.strip_prefix("/dumps/resume/") {
            let action = if query.contains("action=stop") { "stop" } else { "continue" };
            dumps.resume(token, action.to_string());
            return Ok(text(StatusCode::OK, "ok"));
        }
    }

    let resp = match (&method, path.as_str()) {
        (&Method::GET, "/dumps/stream") => stream_response(dumps),
        (&Method::POST, "/dumps/clear") => { dumps.clear(); text(StatusCode::OK, "cleared") }
        (&Method::POST, "/") | (&Method::POST, "/dumps") => ingest(req, dumps).await,
        (&Method::GET, "/") | (&Method::GET, "/health") => text(StatusCode::OK, "dumps ok"),
        _ => text(StatusCode::NOT_FOUND, "not found"),
    };
    Ok(resp)
}

/// Hold a paused breakpoint open until the app resumes it or we time out
/// (fail-open to "continue" so a closed app never hangs the site).
async fn pause(token: String, dumps: Arc<Dumps>) -> Response<DumpsBody> {
    let rx = dumps.register_pause(token.clone());
    let action = match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(action)) => action,
        _ => {
            dumps.drop_pause(&token);
            "continue".to_string()
        }
    };
    text(StatusCode::OK, &action)
}

async fn ingest(req: Request<Incoming>, dumps: Arc<Dumps>) -> Response<DumpsBody> {
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return text(StatusCode::BAD_REQUEST, "bad body"),
    };
    match serde_json::from_slice::<Value>(&body) {
        Ok(payload) => { dumps.push(payload); text(StatusCode::OK, "ok") }
        Err(e) => text(StatusCode::BAD_REQUEST, &format!("invalid json: {e}")),
    }
}

/// Backfill the ring, then live-tail — one NDJSON line per dump.
fn stream_response(dumps: Arc<Dumps>) -> Response<DumpsBody> {
    let rx = dumps.tx.subscribe();
    let backfill: Vec<Result<Frame<Bytes>, Box<dyn std::error::Error + Send + Sync>>> =
        dumps.snapshot().into_iter().map(|v| Ok(Frame::data(ndjson(&v)))).collect();

    let live = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(v) => Some(Ok(Frame::data(ndjson(&v)))),
        Err(_) => None, // lagged — the ring already holds recent ones
    });

    let stream = tokio_stream::iter(backfill).chain(live);
    let body = StreamBody::new(stream).boxed();

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/x-ndjson")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

fn ndjson(v: &Value) -> Bytes {
    let mut s = serde_json::to_vec(v).unwrap_or_default();
    s.push(b'\n');
    Bytes::from(s)
}

fn text(status: StatusCode, msg: &str) -> Response<DumpsBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from(msg.to_string())).map_err(|e: std::convert::Infallible| match e {}).boxed())
        .unwrap()
}
