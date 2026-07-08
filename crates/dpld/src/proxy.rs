//! The local HTTP server. Routes each request to a site by `Host` header, then
//! serves it like a real web server: static files straight from disk, and PHP
//! via FastCGI to the site's php-fpm pool with `try_files`-style front-
//! controller fallback to `index.php` (so Laravel/WordPress pretty URLs work).

use std::convert::Infallible;
use std::error::Error as StdError;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::registry::{Registry, SiteRoute};

type ProxyBody = BoxBody<Bytes, Box<dyn StdError + Send + Sync>>;

/// Bind the HTTP server, preferring port 80 and falling back to 8080 when a
/// privileged bind isn't allowed (rootless, pre-helper).
pub async fn bind_preferred() -> Result<(TcpListener, u16)> {
    for port in [preferred_port(), 8080] {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match TcpListener::bind(addr).await {
            Ok(l) => return Ok((l, port)),
            Err(e) => tracing::warn!(port, error = %e, "could not bind http port, trying next"),
        }
    }
    anyhow::bail!("could not bind the http server on port 80 or 8080")
}

fn preferred_port() -> u16 {
    std::env::var("DPL_HTTP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(80)
}

/// Serve HTTP forever. `server_port` is reported to PHP as `SERVER_PORT`.
pub async fn serve(listener: TcpListener, registry: Arc<Mutex<Registry>>, server_port: u16) -> Result<()> {
    accept_loop(listener, registry, server_port, false).await
}

/// Shared accept loop, used by both the HTTP and (wrapped) HTTPS servers.
pub async fn accept_loop(
    listener: TcpListener,
    registry: Arc<Mutex<Registry>>,
    server_port: u16,
    https: bool,
) -> Result<()> {
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "http accept failed");
                continue;
            }
        };
        let io = TokioIo::new(stream);
        let registry = registry.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| handle(req, registry.clone(), server_port, https));
            if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                tracing::debug!(error = %e, "http connection closed");
            }
        });
    }
}

pub(crate) async fn handle(
    req: Request<Incoming>,
    registry: Arc<Mutex<Registry>>,
    server_port: u16,
    https: bool,
) -> Result<Response<ProxyBody>, Infallible> {
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(strip_port)
        .unwrap_or_default();

    // Resolve the route while briefly holding the lock, then release it before
    // the (awaiting) FastCGI/file work. The registry knows the configured TLDs.
    let route = {
        let reg = registry.lock().await;
        reg.site_for_host(&host).and_then(|name| reg.resolve_request(&name))
    };

    match route {
        Some(route) => Ok(serve_site(req, route, &host, server_port, https).await),
        None => Ok(fallback_index(&registry, &host).await),
    }
}

/// Serve one request for a resolved site.
async fn serve_site(
    req: Request<Incoming>,
    route: SiteRoute,
    host: &str,
    server_port: u16,
    https: bool,
) -> Response<ProxyBody> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = percent_decode(uri.path());
    let query = uri.query().unwrap_or("").to_string();
    let headers = req.headers().clone();

    let target = resolve_target(&route.docroot, &path);

    match target {
        Target::Static(file) => serve_static(&file),
        Target::NotFound => error_page(StatusCode::NOT_FOUND, "Not found."),
        Target::Php { script, script_name, path_info } => {
            // Buffer the request body for FastCGI STDIN.
            let body = match req.into_body().collect().await {
                Ok(c) => c.to_bytes(),
                Err(_) => return error_page(StatusCode::BAD_REQUEST, "could not read request body"),
            };
            let params = build_params(
                &route, host, server_port, https, &method, &uri, &query, &script,
                &script_name, &path_info, &headers, body.len(),
            );
            match crate::fastcgi::request(route.fpm_addr, &params, &body).await {
                Ok(resp) => parse_cgi(&resp.stdout),
                Err(e) => error_page(
                    StatusCode::BAD_GATEWAY,
                    &format!("php-fpm did not respond ({e})"),
                ),
            }
        }
    }
}

/// What to do with a request path within a document root.
enum Target {
    Static(PathBuf),
    Php { script: PathBuf, script_name: String, path_info: String },
    NotFound,
}

/// `try_files`-style resolution: real file → serve/execute it; a directory →
/// its index; otherwise fall through to the front controller `index.php`.
fn resolve_target(docroot: &Path, url_path: &str) -> Target {
    let rel = sanitize(url_path);
    let candidate = docroot.join(&rel);

    if url_path.ends_with('/') || candidate.is_dir() {
        let dir = if candidate.is_dir() { candidate.clone() } else { docroot.to_path_buf() };
        let php = dir.join("index.php");
        if php.is_file() {
            return Target::Php {
                script: php,
                script_name: format!("{}index.php", ensure_trailing_slash(url_path)),
                path_info: String::new(),
            };
        }
        let html = dir.join("index.html");
        if html.is_file() {
            return Target::Static(html);
        }
    } else if candidate.is_file() {
        return if candidate.extension().and_then(|e| e.to_str()) == Some("php") {
            Target::Php { script: candidate, script_name: url_path.to_string(), path_info: String::new() }
        } else {
            Target::Static(candidate)
        };
    }

    // Front controller fallback.
    let front = docroot.join("index.php");
    if front.is_file() {
        Target::Php {
            script: front,
            script_name: "/index.php".to_string(),
            path_info: url_path.to_string(),
        }
    } else {
        Target::NotFound
    }
}

/// Assemble the FastCGI/CGI parameters for a PHP request.
#[allow(clippy::too_many_arguments)]
fn build_params(
    route: &SiteRoute,
    host: &str,
    server_port: u16,
    https: bool,
    method: &hyper::Method,
    uri: &hyper::Uri,
    query: &str,
    script: &Path,
    script_name: &str,
    path_info: &str,
    headers: &hyper::HeaderMap,
    content_length: usize,
) -> Vec<(String, String)> {
    let request_uri = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let mut p: Vec<(String, String)> = vec![
        ("GATEWAY_INTERFACE".into(), "CGI/1.1".into()),
        ("SERVER_SOFTWARE".into(), "dpl".into()),
        ("SERVER_PROTOCOL".into(), "HTTP/1.1".into()),
        ("REQUEST_METHOD".into(), method.to_string()),
        ("REQUEST_URI".into(), request_uri.to_string()),
        ("QUERY_STRING".into(), query.to_string()),
        ("DOCUMENT_ROOT".into(), route.docroot.to_string_lossy().into_owned()),
        ("SCRIPT_FILENAME".into(), script.to_string_lossy().into_owned()),
        ("SCRIPT_NAME".into(), script_name.to_string()),
        ("DOCUMENT_URI".into(), script_name.to_string()),
        ("REMOTE_ADDR".into(), "127.0.0.1".into()),
        ("REMOTE_PORT".into(), "0".into()),
        ("SERVER_ADDR".into(), "127.0.0.1".into()),
        ("SERVER_NAME".into(), host.to_string()),
        ("SERVER_PORT".into(), server_port.to_string()),
        ("REQUEST_SCHEME".into(), if https { "https".into() } else { "http".into() }),
    ];
    if https {
        p.push(("HTTPS".into(), "on".into()));
    }
    if !path_info.is_empty() {
        p.push(("PATH_INFO".into(), path_info.to_string()));
    }
    if content_length > 0 {
        p.push(("CONTENT_LENGTH".into(), content_length.to_string()));
    }
    if let Some(ct) = headers.get(hyper::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        p.push(("CONTENT_TYPE".into(), ct.to_string()));
    }
    // Forward request headers as HTTP_*.
    for (name, value) in headers {
        if let Ok(v) = value.to_str() {
            let key = format!("HTTP_{}", name.as_str().to_uppercase().replace('-', "_"));
            p.push((key, v.to_string()));
        }
    }
    p
}

/// Parse php-fpm's CGI output (headers + blank line + body) into a response.
fn parse_cgi(stdout: &[u8]) -> Response<ProxyBody> {
    let split = find_header_end(stdout);
    let (head, body) = match split {
        Some((end, body_start)) => (&stdout[..end], &stdout[body_start..]),
        None => return html(StatusCode::OK, stdout.to_vec()),
    };

    let head_str = String::from_utf8_lossy(head);
    let mut status = StatusCode::OK;
    let mut builder = Response::builder();

    for line in head_str.split("\r\n").flat_map(|l| l.split('\n')) {
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim();
        let value = value.trim();
        if key.eq_ignore_ascii_case("Status") {
            if let Some(code) = value.split_whitespace().next().and_then(|c| c.parse::<u16>().ok()) {
                status = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
            }
            continue;
        }
        if let (Ok(name), Ok(val)) = (HeaderName::from_bytes(key.as_bytes()), HeaderValue::from_str(value)) {
            builder = builder.header(name, val);
        }
    }

    builder
        .status(status)
        .body(Full::new(Bytes::copy_from_slice(body)).map_err(boxed).boxed())
        .unwrap_or_else(|_| error_page(StatusCode::INTERNAL_SERVER_ERROR, "bad CGI response"))
}

/// Serve a static file with a guessed content-type.
fn serve_static(file: &Path) -> Response<ProxyBody> {
    match std::fs::read(file) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", content_type(file))
            .header("content-length", bytes.len())
            .body(Full::new(Bytes::from(bytes)).map_err(boxed).boxed())
            .unwrap(),
        Err(_) => error_page(StatusCode::NOT_FOUND, "Not found."),
    }
}

async fn fallback_index(registry: &Arc<Mutex<Registry>>, host: &str) -> Response<ProxyBody> {
    let sites = registry.lock().await.site_infos();
    let mut list = String::new();
    for s in sites.iter().filter(|s| s.serving) {
        list.push_str(&format!(
            "<li><a href=\"{url}\">{host}</a> <small>({src})</small></li>",
            url = s.url, host = s.host, src = s.source
        ));
    }
    if list.is_empty() {
        list.push_str("<li><em>No sites are being served. Try <code>dpl link .</code> in a project.</em></li>");
    }
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>dpl</title>\
         <style>body{{font:15px -apple-system,system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem}}\
         h1{{font-size:1.2rem}} li{{margin:.3rem 0}} code{{background:#eee;padding:.1rem .3rem;border-radius:3px}}</style>\
         <h1>dpl — local sites</h1>\
         <p>No site is registered for <code>{host}</code>. Served sites:</p><ul>{list}</ul>"
    );
    html(StatusCode::NOT_FOUND, body.into_bytes())
}

// ---- helpers ----

fn strip_port(host: &str) -> String {
    host.split(':').next().unwrap_or(host).to_lowercase()
}

/// Percent-decode a URL path.
fn percent_decode(path: &str) -> String {
    percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned()
}

/// Turn a URL path into a safe relative path (drops `.`/`..`/empty segments).
fn sanitize(url_path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for seg in url_path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        out.push(seg);
    }
    out
}

fn ensure_trailing_slash(s: &str) -> String {
    if s.ends_with('/') { s.to_string() } else { format!("{s}/") }
}

/// Find the end of the CGI header block (`\r\n\r\n` or `\n\n`).
fn find_header_end(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, i + 4));
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some((i, i + 2));
        }
    }
    None
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

fn error_page(status: StatusCode, message: &str) -> Response<ProxyBody> {
    html(
        status,
        format!(
            "<!doctype html><meta charset=utf-8><title>dpl</title>\
             <body style=\"font:15px system-ui;max-width:40rem;margin:4rem auto\">\
             <h1>{}</h1><p>{message}</p>",
            status.as_u16()
        )
        .into_bytes(),
    )
}

fn html(status: StatusCode, body: Vec<u8>) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(body)).map_err(boxed).boxed())
        .unwrap()
}

fn boxed<E: Into<Box<dyn StdError + Send + Sync>>>(e: E) -> Box<dyn StdError + Send + Sync> {
    e.into()
}
