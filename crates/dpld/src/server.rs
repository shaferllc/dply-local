//! Control socket: accept connections on the per-user unix socket, decode a
//! length-prefixed [`Envelope<Request>`], dispatch, and reply with an
//! [`Envelope<Response>`]. One request/response per connection keeps the CLI
//! side trivial. Site-management verbs mutate the shared [`Registry`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context};
use dpl_core::ipc::{DaemonStatus, Envelope, Request, Response, MAX_FRAME};
use dpl_core::PROTOCOL_VERSION;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};

use crate::registry::Registry;
use crate::services::Services;

#[derive(Clone)]
pub struct DaemonState {
    pub started: Instant,
    pub registry: Arc<Mutex<Registry>>,
    pub services: Arc<Mutex<Services>>,
    pub http_port: u16,
}

/// Whether a live dpld already owns the control socket.
///
/// Startup does destructive things — `kill_orphans()` reaps php-fpm masters by
/// pattern, which cannot tell *our* orphans from another daemon's live pools.
/// A second instance must therefore find this out and bail *before* it touches
/// anything, not when it finally gets around to binding the socket.
pub async fn instance_already_running() -> bool {
    match dpl_core::paths::daemon_socket(None) {
        Ok(path) => path.exists() && probe_alive(&path).await,
        Err(_) => false,
    }
}

/// Bind the socket and serve until a `Shutdown` request (or SIGINT/SIGTERM)
/// arrives. Cleans up a stale socket file left by a previous crash.
pub async fn run(state: DaemonState) -> anyhow::Result<()> {
    let socket_path = dpl_core::paths::daemon_socket(None).context("resolving socket path")?;
    prepare_socket_dir(&socket_path)?;

    // A leftover socket file from an unclean shutdown would make bind() fail
    // with EADDRINUSE even though nobody is listening — remove it first.
    if socket_path.exists() {
        if probe_alive(&socket_path).await {
            bail!(
                "another dpld is already listening on {}",
                socket_path.display()
            );
        }
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding {}", socket_path.display()))?;
    restrict_socket(&socket_path)?;
    tracing::info!(socket = %socket_path.display(), "dpld control socket listening");

    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let state = state.clone();
                        let shutdown_tx = shutdown_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle(stream, state, shutdown_tx).await {
                                tracing::warn!(error = %e, "connection error");
                            }
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "accept failed"),
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("shutting down");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received ctrl-c, shutting down");
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn handle(
    mut stream: UnixStream,
    state: DaemonState,
    shutdown_tx: mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let env: Envelope<Request> = read_frame(&mut stream).await?;

    if env.protocol != PROTOCOL_VERSION {
        let resp = Envelope::new(Response::Error {
            message: format!(
                "protocol mismatch: daemon v{PROTOCOL_VERSION}, client v{}",
                env.protocol
            ),
        });
        write_frame(&mut stream, &resp).await?;
        return Ok(());
    }

    let response = dispatch(env.payload, &state, &shutdown_tx).await;
    write_frame(&mut stream, &Envelope::new(response)).await
}

/// Turn a request into a response, mutating the registry for site verbs.
async fn dispatch(
    request: Request,
    state: &DaemonState,
    shutdown_tx: &mpsc::Sender<()>,
) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Version => Response::Version {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        Request::Status => {
            let reg = state.registry.lock().await;
            Response::Status {
                status: DaemonStatus {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    uptime_secs: state.started.elapsed().as_secs(),
                    proxy_running: true,
                    site_count: reg.serving_count(),
                },
            }
        }
        Request::Shutdown => {
            let _ = shutdown_tx.send(()).await;
            Response::Ok
        }
        Request::ListSites => {
            let reg = state.registry.lock().await;
            Response::Sites {
                sites: reg.site_infos(),
                http_port: state.http_port,
                tld: reg.primary_tld(),
            }
        }
        Request::Park { path } => mutate(state, |r| r.park(&path)).await,
        Request::Unpark { path } => mutate(state, |r| r.unpark(&path)).await,
        Request::ImportSites { parked, links } => {
            mutate(state, |r| r.import_sites(&parked, &links)).await
        }
        Request::RemoveSites { parked, links } => {
            mutate(state, |r| r.remove_sites(&parked, &links)).await
        }
        Request::SetResolution { mode } => {
            mutate(state, |r| r.set_resolution(&mode)).await
        }
        Request::SetRuntime { site, runtime } => {
            mutate(state, |r| r.set_runtime(&site, &runtime)).await
        }
        Request::SetTags { site, tags } => {
            mutate(state, |r| r.set_tags(&site, &tags)).await
        }
        Request::SetDev { site, script } => {
            mutate(state, |r| r.set_dev(&site, script.as_deref())).await
        }
        Request::RestartDev { site } => {
            mutate(state, |r| r.restart_dev(&site)).await
        }
        Request::DevStatus => {
            let servers = state
                .registry
                .lock()
                .await
                .dev_statuses()
                .into_iter()
                .map(|d| dpl_core::ipc::DevServerInfo {
                    site: d.site,
                    script: d.script,
                    agent: d.agent,
                    running: d.running,
                    pgid: d.pgid,
                    port: d.port,
                    detail: d.detail,
                    log: d.log,
                })
                .collect();
            Response::DevServers { servers }
        }
        Request::SetXdebug { mode, site, port, ide_key } => {
            mutate(state, |r| {
                r.set_xdebug(mode.as_deref(), site.as_deref(), port, ide_key.as_deref())
            })
            .await
        }
        Request::Link { name, path } => {
            mutate(state, |r| r.link(name.as_deref(), &path)).await
        }
        Request::Unlink { name } => mutate(state, |r| r.unlink(&name)).await,
        Request::Relink { name, path } => mutate(state, |r| r.relink(&name, &path)).await,
        Request::Secure { name, secure } => {
            mutate(state, |r| r.set_secure(&name, secure)).await
        }
        Request::SetProfile { site, on } => {
            mutate(state, |r| r.set_profile(&site, on)).await
        }
        Request::SetPreload { site, script } => {
            mutate(state, |r| r.set_preload(&site, script)).await
        }
        Request::Proxy { action, name, target } => match action.as_str() {
            "set" => match target {
                Some(t) => mutate(state, |r| r.proxy_set(&name, &t)).await,
                None => Response::Error { message: "usage: dpl proxy <host> <target>".into() },
            },
            "remove" => mutate(state, |r| r.proxy_remove(&name)).await,
            other => Response::Error { message: format!("unknown proxy action: {other}") },
        },
        Request::UsePhp { version, site } => {
            mutate(state, |r| r.use_php(&version, site.as_deref())).await
        }
        Request::Reload => mutate(state, |r| r.reload()).await,
        Request::RepairBackends => mutate(state, |r| r.repair_backends()).await,
        Request::Tld { action, name } => {
            let mut reg = state.registry.lock().await;
            match action.as_str() {
                "list" => Response::Lines { lines: reg.tld_list() },
                "add" => match name {
                    Some(t) => svc_result(Some(reg.tld_add(&t))),
                    None => Response::Error { message: "usage: dpl tld add <name>".into() },
                },
                "remove" => match name {
                    Some(t) => svc_result(Some(reg.tld_remove(&t))),
                    None => Response::Error { message: "usage: dpl tld remove <name>".into() },
                },
                "primary" => match name {
                    Some(t) => svc_result(Some(reg.tld_primary(&t))),
                    None => Response::Error { message: "usage: dpl tld primary <name>".into() },
                },
                other => Response::Error { message: format!("unknown tld action: {other}") },
            }
        }
        Request::Service { action, name, engine, version, port } => {
            let mut svc = state.services.lock().await;
            match action.as_str() {
                "list" => Response::ServiceList { services: svc.list() },
                "versions" => Response::Versions { versions: svc.available_versions(engine.as_deref()) },
                "create" => match (name.as_deref(), engine.as_deref()) {
                    (Some(n), Some(e)) => svc_result(Some(svc.create(n, e, version.as_deref(), port))),
                    _ => Response::Error { message: "usage: create <name> --engine <e> [--version v] [--port p]".into() },
                },
                "start" => svc_result(name.as_deref().map(|n| svc.start(n))),
                "stop" => svc_result(name.as_deref().map(|n| svc.stop(n))),
                "restart" => svc_result(name.as_deref().map(|n| svc.restart(n))),
                "delete" => svc_result(name.as_deref().map(|n| svc.delete(n))),
                other => Response::Error { message: format!("unknown service action: {other}") },
            }
        }
        Request::Db { action, engine, name, port, file } => {
            let svc = state.services.lock().await;
            match svc.db(&action, &engine, name.as_deref(), port, file.as_deref()) {
                Ok(text) => {
                    if action == "list" {
                        Response::Lines { lines: text.lines().map(str::to_string).collect() }
                    } else {
                        Response::Message { text }
                    }
                }
                Err(e) => Response::Error { message: format!("{e:#}") },
            }
        }
        Request::BranchDb { action, site, branch, database, port } => {
            crate::branchdb::dispatch(state, &action, &site, branch, database, port).await
        }
        Request::ApplySpec { path, spec } => apply_spec(state, &path, spec).await,
        Request::ExportSpec { path } => {
            let reg = state.registry.lock().await;
            match reg.export_spec(&path) {
                Ok(spec) => Response::Message { text: spec.to_toml() },
                Err(e) => Response::Error { message: format!("{e:#}") },
            }
        }
        Request::Unknown => Response::Error {
            message: "unknown operation (client is newer than daemon?)".into(),
        },
    }
}

/// `dpl up`: apply a project's `dpl.toml` in one registry pass, then do the
/// environment-side checks — ensure the branch database exists and report any
/// required services that aren't listening. Settings warnings degrade, they
/// don't fail; the site should come up as far as this machine allows.
async fn apply_spec(state: &DaemonState, path: &str, spec: dpl_core::spec::SiteSpec) -> Response {
    let (name, mut warnings) = {
        let mut reg = state.registry.lock().await;
        match reg.apply_spec(path, &spec) {
            Ok(r) => r,
            Err(e) => return Response::Error { message: format!("{e:#}") },
        }
    };

    // Branch database: make sure the base exists (like `dpl db attach`).
    if let Some(db) = spec.database.clone() {
        let port = spec.db_port.unwrap_or(5432);
        if crate::services::port_open(port) {
            match tokio::task::spawn_blocking(move || crate::services::pg_ensure_db(port, &db)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warnings.push(format!("branch database: {e:#}")),
                Err(e) => warnings.push(format!("branch database: {e}")),
            }
        } else {
            warnings.push(format!("branch database: nothing listening on Postgres port {port}"));
        }
    }

    // Required services: check, don't create — versions and data are choices.
    for engine in &spec.services {
        match crate::services::default_port_of(engine) {
            Some(port) if crate::services::port_open(port) => {}
            Some(port) => warnings.push(format!(
                "service `{engine}` isn't listening on :{port} — start it, or `dpl service create <name> --engine {engine}`"
            )),
            None => warnings.push(format!("unknown service `{engine}` in dpl.toml")),
        }
    }

    let mut text = format!("Up: {name}.test matches dpl.toml.");
    for w in &warnings {
        text.push_str(&format!("\n  ⚠ {w}"));
    }
    Response::Message { text }
}

/// Map a service action's `Option<Result<String>>` to a response (None means
/// the action needed a name that wasn't given).
fn svc_result(r: Option<anyhow::Result<String>>) -> Response {
    match r {
        Some(Ok(text)) => Response::Message { text },
        Some(Err(e)) => Response::Error { message: format!("{e:#}") },
        None => Response::Error { message: "this action requires a service name".into() },
    }
}

/// Run a registry mutation and map its `Result<String>` to a response.
async fn mutate(
    state: &DaemonState,
    f: impl FnOnce(&mut Registry) -> anyhow::Result<String>,
) -> Response {
    let mut reg = state.registry.lock().await;
    match f(&mut reg) {
        Ok(text) => Response::Message { text },
        Err(e) => Response::Error {
            message: format!("{e:#}"),
        },
    }
}

async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> anyhow::Result<T> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .context("reading frame length")?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        bail!("frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len as usize];
    stream
        .read_exact(&mut buf)
        .await
        .context("reading frame body")?;
    serde_json::from_slice(&buf).context("decoding frame")
}

async fn write_frame<T: serde::Serialize>(stream: &mut UnixStream, value: &T) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value).context("encoding frame")?;
    let len = u32::try_from(bytes.len()).context("frame too large to encode")?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn probe_alive(path: &PathBuf) -> bool {
    match UnixStream::connect(path).await {
        Ok(mut stream) => {
            write_frame(&mut stream, &Envelope::new(Request::Ping))
                .await
                .is_ok()
                && read_frame::<Envelope<Response>>(&mut stream).await.is_ok()
        }
        Err(_) => false,
    }
}

fn prepare_socket_dir(socket_path: &PathBuf) -> anyhow::Result<()> {
    if let Some(dir) = socket_path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(())
}

fn restrict_socket(socket_path: &PathBuf) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
            .context("restricting socket permissions")?;
    }
    Ok(())
}
