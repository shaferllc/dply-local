//! `dpld` — the per-user dpl daemon.
//!
//! Owns the control socket (lifecycle + site-management verbs) and the local
//! HTTP reverse proxy that serves `.test` sites from per-site `php -S`
//! backends. Later phases attach DNS, HTTPS, multi-PHP, and DB services to this
//! same process.

mod access;
mod appserver;
mod branchdb;
mod ca;
mod dns;
mod devserver;
mod dumps;
mod fastcgi;
mod fpm;
mod jetty;
mod launchd;
mod mail;
mod proxy;
mod registry;
mod server;
mod services;
mod tls;

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use tokio::sync::Mutex;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("DPLD_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    rt.block_on(async {
        // Before anything destructive: if another dpld owns the control socket,
        // this process must not run. `kill_orphans()` below reaps php-fpm masters
        // by command-line pattern and would tear down the *live* daemon's pools,
        // leaving every site 502 while launchd respawns us into the same collision.
        if server::instance_already_running().await {
            anyhow::bail!(
                "another dpld already owns the control socket — refusing to start \
                 (stop it first, e.g. `dpl stop`)"
            );
        }

        // Bind the proxy first so we know which port we actually got.
        let (listener, http_port) = proxy::bind_preferred().await?;
        tracing::info!(port = http_port, "proxy listening");
        if http_port != 80 {
            tracing::warn!(
                "serving on :{http_port}. Run `dpl setup` (sudo) once to route .test and hand \
                 :80/:443 to this daemon via launchd, then browse http://<name>.test with no port."
            );
        }

        // Start the access-log writer so the proxy can record traffic from the
        // first request.
        access::init();

        // Reap php-fpm masters leaked by a previous (SIGKILLed) daemon before
        // we spawn fresh ones, so they don't accumulate across restarts.
        fpm::FpmManager::kill_orphans();
        // Same for Octane servers: a SIGKILLed daemon never stopped them, and
        // Octane won't start a second server for a project that still has one.
        appserver::AppServers::kill_orphans();
        // Same for tunnel agents: a stray one still holds its reserved label,
        // so the new daemon's agent would be refused it.
        jetty::Tunnels::kill_orphans();

        // Build the site registry and start backends for the saved config.
        let mut registry = registry::Registry::load().context("loading registry")?;
        // Tunnels forward to the proxy, so they need the port it actually bound
        // — set before the first reconcile, which is what starts them.
        registry.set_http_port(http_port);
        let serving = registry.reconcile();
        tracing::info!(sites = serving, "initial reconcile complete");
        let registry = Arc::new(Mutex::new(registry));

        // HTTP server.
        let proxy_task = {
            let registry = registry.clone();
            tokio::spawn(async move {
                if let Err(e) = proxy::serve(listener, registry, http_port).await {
                    tracing::error!(error = %e, "http server stopped");
                }
            })
        };

        // HTTPS server (best-effort: needs the local CA; logs and continues if
        // it can't bind or build TLS).
        let tls_task = {
            let registry = registry.clone();
            tokio::spawn(async move {
                if let Err(e) = tls::serve(registry).await {
                    tracing::warn!(error = %e, "https server not started");
                }
            })
        };

        // DNS responder for *.test (used once the resolver is installed).
        let dns_task = tokio::spawn(async move {
            if let Err(e) = dns::serve().await {
                tracing::warn!(error = %e, "dns responder not started");
            }
        });

        // Mail sink (SMTP → ~/.dpl/mail).
        let mail_task = tokio::spawn(async move {
            if let Err(e) = mail::serve().await {
                tracing::warn!(error = %e, "mail sink not started");
            }
        });

        // Dumps receiver (LaraDumps-style debugger).
        let dumps_buf = dumps::Dumps::new();
        let dumps_task = {
            let dumps_buf = dumps_buf.clone();
            tokio::spawn(async move {
                if let Err(e) = dumps::serve(dumps_buf).await {
                    tracing::warn!(error = %e, "dumps receiver not started");
                }
            })
        };

        let mut svc = services::Services::new();
        let started = svc.reconcile();
        tracing::info!(instances = started, "service instances reconciled");
        let services = Arc::new(Mutex::new(svc));
        let state = server::DaemonState {
            started: Instant::now(),
            registry,
            services,
            http_port,
        };

        // Auto-switch branch databases when an attached site's git HEAD moves.
        let branchdb_task = {
            let state = state.clone();
            tokio::spawn(async move { branchdb::watch(state).await })
        };

        // Keep opted-in Node dev servers alive for the daemon's lifetime.
        let devserver_task = {
            let state = state.clone();
            tokio::spawn(async move { devserver::watch(state).await })
        };

        // Keep Octane servers alive, and reload their workers when the code
        // they're holding in memory changes on disk.
        let appserver_task = {
            let state = state.clone();
            tokio::spawn(async move { appserver::watch(state).await })
        };

        // Scrape php-fpm's own status page so a saturated pool can be named as
        // such, and respawn masters that die instead of waiting for a request
        // to notice.
        let fpm_task = {
            let state = state.clone();
            tokio::spawn(async move { fpm::watch(state).await })
        };

        // Keep shared sites' Jetty tunnels connected, so a public URL that is
        // meant to be permanent behaves like it.
        let jetty_task = {
            let state = state.clone();
            tokio::spawn(async move { jetty::watch(state).await })
        };

        let result = server::run(state).await;

        proxy_task.abort();
        tls_task.abort();
        dns_task.abort();
        mail_task.abort();
        dumps_task.abort();
        branchdb_task.abort();
        devserver_task.abort();
        appserver_task.abort();
        fpm_task.abort();
        jetty_task.abort();
        result
    })
}
