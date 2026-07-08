//! Dispatch for the `dpl dply …` subtree. Each arm calls a
//! [`dpl_dply::endpoints`] function and renders via [`crate::output`], so the
//! wire logic stays in the client crate and the view logic stays here.

pub mod auth;

use anyhow::Context as _;
use dpl_dply::{endpoints as ep, DplyClient};
use serde_json::{json, Value};

use crate::cli::DplyCommand;
use crate::output;

/// Shared per-invocation context: the resolved dply client plus the global
/// `--json` flag.
struct Ctx {
    client: DplyClient,
    json: bool,
}

impl Ctx {
    /// Render a list payload as a table, or raw JSON under `--json`.
    fn rows(&self, value: &Value, columns: &[output::Column]) {
        if self.json {
            output::json(value);
        } else {
            output::table(value, columns);
        }
    }

    /// Render a single object as a detail block, or raw JSON.
    fn detail(&self, value: &Value, fields: &[output::Column]) {
        if self.json {
            output::json(value);
        } else {
            output::detail(value, fields);
        }
    }

    /// Dump all scalar keys, or raw JSON.
    fn dump(&self, value: &Value) {
        if self.json {
            output::json(value);
        } else {
            output::dump(value);
        }
    }
}

pub fn run(
    cmd: DplyCommand,
    host_flag: Option<&str>,
    home: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    // Auth commands manage their own store and don't need an authed client.
    match &cmd {
        DplyCommand::Login(args) => return auth::login(host_flag, home, args.no_browser),
        DplyCommand::Logout => return auth::logout(host_flag, home),
        DplyCommand::Whoami => return auth::whoami(host_flag, home, json),
        _ => {}
    }

    let client = DplyClient::for_active(host_flag, home).context("resolving dply client")?;
    let cx = Ctx { client, json };
    let c = &cx.client;

    match cmd {
        // handled above
        DplyCommand::Login(_) | DplyCommand::Logout | DplyCommand::Whoami => unreachable!(),

        // ---------- edge ----------
        DplyCommand::EdgeSites { status } => cx.rows(
            &ep::edge::list(c, status.as_deref())?,
            &[
                ("ID", &["id"]),
                ("NAME", &["name"]),
                ("STATUS", &["status"]),
                ("FRAMEWORK", &["build.framework", "framework"]),
                ("URL", &["live_url", "hostname"]),
                ("UPDATED", &["updated_at"]),
            ],
        ),
        DplyCommand::EdgeShow { site } => cx.detail(
            &ep::edge::show(c, &site)?,
            &[
                ("ID", &["id"]),
                ("Name", &["name"]),
                ("Status", &["status"]),
                ("Backend", &["edge_backend", "backend"]),
                ("Runtime", &["runtime_mode"]),
                ("Framework", &["build.framework"]),
                ("Repo", &["source.repo"]),
                ("Branch", &["source.branch"]),
                ("Build cmd", &["build.command"]),
                ("Output", &["build.output"]),
                ("Live URL", &["live_url"]),
                ("Active deploy", &["active_deployment_id"]),
                ("Created", &["created_at"]),
                ("Updated", &["updated_at"]),
            ],
        ),
        DplyCommand::EdgeDeploy { site, commit, branch } => {
            let r = ep::edge::deploy(c, &site, commit.as_deref(), branch.as_deref())?;
            action_result(&cx, &r, "Deployment queued.");
        }
        DplyCommand::EdgeDeployments { site, limit } => cx.rows(
            &ep::edge::deployments(c, &site, limit)?,
            &[
                ("ID", &["id"]),
                ("STATUS", &["status"]),
                ("COMMIT", &["git_commit"]),
                ("BRANCH", &["git_branch"]),
                ("PUBLISHED", &["published_at"]),
                ("CREATED", &["created_at"]),
            ],
        ),
        DplyCommand::EdgeDeployment { site, deployment } => cx.detail(
            &ep::edge::deployment(c, &site, &deployment)?,
            &[
                ("ID", &["id"]),
                ("Status", &["status"]),
                ("Commit", &["git_commit"]),
                ("Branch", &["git_branch"]),
                ("Subject", &["meta.commit.subject"]),
                ("Author", &["meta.commit.author"]),
                ("Storage", &["storage_prefix"]),
                ("KV version", &["cf_kv_version"]),
                ("Build log", &["build_log_path"]),
                ("Published", &["published_at"]),
                ("Failed", &["failed_at"]),
                ("Failure", &["failure_reason"]),
                ("Created", &["created_at"]),
            ],
        ),
        DplyCommand::EdgeRollback { site, deployment, yes } => {
            if !yes && !confirm(&format!("Roll back {site} to deployment {deployment}?"))? {
                println!("Aborted.");
                return Ok(());
            }
            let r = ep::edge::rollback(c, &site, &deployment)?;
            action_result(&cx, &r, "Rollback queued.");
        }
        DplyCommand::EdgeAccess { site, mode, password, allowed_email } => {
            if mode.is_some() || password.is_some() || !allowed_email.is_empty() {
                ep::edge::access_set(c, &site, mode.as_deref(), password.as_deref(), &allowed_email)?;
            }
            cx.detail(
                &ep::edge::access_get(c, &site)?,
                &[
                    ("Mode", &["mode"]),
                    ("Password set", &["password_set", "has_password"]),
                    ("Allowed emails", &["allowed_emails"]),
                    ("Updated", &["updated_at"]),
                ],
            );
        }
        DplyCommand::EdgeEnv { site, set, unset, from_file, scope } => {
            edge_env(&cx, &site, &set, &unset, from_file.as_deref(), &scope)?;
        }
        DplyCommand::EdgeDomains { site, add, verify, remove } => {
            if let Some(h) = add.as_deref() {
                ep::edge::domain_add(c, &site, h)?;
            }
            if let Some(h) = verify.as_deref() {
                ep::edge::domain_verify(c, &site, h)?;
            }
            if let Some(h) = remove.as_deref() {
                ep::edge::domain_remove(c, &site, h)?;
            }
            cx.rows(
                &ep::edge::domains_list(c, &site)?,
                &[
                    ("HOSTNAME", &["hostname"]),
                    ("STATUS", &["status"]),
                    ("VERIFIED", &["verified_at"]),
                    ("CREATED", &["created_at"]),
                ],
            );
        }
        DplyCommand::EdgeAliases { site } => cx.rows(
            &ep::edge::aliases(c, &site)?,
            &[
                ("HOSTNAME", &["hostname"]),
                ("DEPLOYMENT", &["deployment_id", "deployment.id"]),
                ("CREATED", &["created_at"]),
            ],
        ),
        DplyCommand::EdgePreviews { site, create, delete, promote } => {
            if let Some(b) = create.as_deref() {
                ep::edge::preview_create(c, &site, b)?;
            }
            if let Some(id) = delete.as_deref() {
                ep::edge::preview_delete(c, &site, id)?;
            }
            if let Some(id) = promote.as_deref() {
                ep::edge::preview_promote(c, &site, id)?;
            }
            cx.rows(
                &ep::edge::previews_list(c, &site)?,
                &[
                    ("ID", &["id"]),
                    ("BRANCH", &["preview_branch", "branch"]),
                    ("PR", &["preview_pr_number"]),
                    ("STATUS", &["status"]),
                    ("URL", &["live_url"]),
                    ("UPDATED", &["updated_at"]),
                ],
            );
        }
        DplyCommand::EdgeUsage { site, period } => {
            cx.dump(&ep::edge::usage(c, &site, period.as_deref())?)
        }
        DplyCommand::EdgePurge { site, paths } => {
            ep::edge::purge(c, &site, &paths)?;
            if paths.is_empty() {
                println!("✓ Purged all cached paths for {site}.");
            } else {
                println!("✓ Purged {} path(s) for {site}.", paths.len());
            }
        }
        DplyCommand::EdgeLogs { site, limit, since, tail, interval } => {
            edge_logs(c, &site, limit, since, tail, interval)?;
        }
        DplyCommand::EdgeLint { path } => edge_lint(&cx, path.as_deref())?,

        // ---------- servers ----------
        DplyCommand::ServersList => cx.rows(
            &ep::servers::list(c)?,
            &[
                ("ID", &["id"]),
                ("NAME", &["name"]),
                ("PROVIDER", &["provider"]),
                ("REGION", &["region"]),
                ("STATUS", &["status"]),
                ("IP", &["ip_address", "ip"]),
                ("UPDATED", &["updated_at"]),
            ],
        ),
        DplyCommand::ServersRun { server, user, cmd } => {
            if cmd.is_empty() {
                anyhow::bail!("no command given. Usage: dpl dply servers:run <server> -- <cmd>…");
            }
            let joined = cmd.join(" ");
            let r = ep::servers::run(c, &server, &joined, &user)?;
            if cx.json {
                output::json(&r);
            } else {
                let out = dpl_dply::models::cell_of(&r, &["stdout", "output"]);
                let err = dpl_dply::models::cell_of(&r, &["stderr"]);
                let code = dpl_dply::models::cell_of(&r, &["exit_code"]);
                if !out.is_empty() {
                    println!("{out}");
                }
                if !err.is_empty() {
                    eprintln!("{err}");
                }
                println!("[exit {code}]");
            }
        }
        DplyCommand::ServersFirewall { server, apply, template, bundled } => {
            if let Some(t) = template.as_deref() {
                ep::servers::firewall_template(c, &server, t)?;
                println!("✓ Applied firewall template `{t}`.");
            } else if let Some(k) = bundled.as_deref() {
                ep::servers::firewall_bundled(c, &server, k)?;
                println!("✓ Applied bundled firewall `{k}`.");
            } else if apply {
                ep::servers::firewall_apply(c, &server)?;
                println!("✓ Firewall apply queued.");
            } else {
                let r = ep::servers::firewall_show(c, &server)?;
                let rules = r.get("rules").cloned().unwrap_or(r);
                cx.rows(
                    &rules,
                    &[
                        ("ACTION", &["action"]),
                        ("PROTO", &["protocol"]),
                        ("PORT", &["port", "port_range"]),
                        ("SOURCE", &["source"]),
                        ("COMMENT", &["comment"]),
                    ],
                );
            }
        }
        DplyCommand::ServersLogShipping { server, enable, resync, disable, source } => {
            if enable {
                ep::servers::log_shipping_enable(c, &server, &source)?;
                println!("✓ Log shipping enabled.");
            } else if resync {
                ep::servers::log_shipping_resync(c, &server)?;
                println!("✓ Log shipping resync queued.");
            } else if disable {
                ep::servers::log_shipping_disable(c, &server)?;
                println!("✓ Log shipping disabled.");
            } else {
                cx.detail(
                    &ep::servers::log_shipping_show(c, &server)?,
                    &[
                        ("Addon enabled", &["addon_enabled"]),
                        ("Installed", &["installed"]),
                        ("Status", &["status"]),
                        ("Version", &["version"]),
                        ("Last seen", &["last_seen_at"]),
                        ("Destination", &["destination"]),
                        ("Shipping", &["shipping"]),
                        ("Error", &["error_message"]),
                    ],
                );
            }
        }

        // ---------- sites ----------
        DplyCommand::SitesList => cx.rows(
            &ep::sites::list(c)?,
            &[
                ("ID", &["id"]),
                ("NAME", &["name"]),
                ("SERVER", &["server.name", "server_name"]),
                ("RUNTIME", &["runtime", "runtime_profile"]),
                ("STATUS", &["status"]),
                ("UPDATED", &["updated_at"]),
            ],
        ),
        DplyCommand::SitesShow { site } => cx.detail(
            &ep::sites::show(c, &site)?,
            &[
                ("Name", &["name"]),
                ("Slug", &["slug"]),
                ("Server", &["server_name"]),
                ("Runtime", &["runtime"]),
                ("Runtime ver", &["runtime_version"]),
                ("Status", &["status"]),
                ("SSL", &["ssl_status"]),
                ("Repo", &["git_repository_url"]),
                ("Branch", &["git_branch"]),
                ("Last deploy", &["last_deploy_at"]),
            ],
        ),
        DplyCommand::SitesRename { site, name } => {
            let r = ep::sites::rename(c, &site, &name)?;
            if cx.json {
                output::json(&r);
            } else {
                println!("✓ Renamed to {}", dpl_dply::models::cell_of(&r, &["name"]));
            }
        }
        DplyCommand::SitesDeploy { site } => {
            let r = ep::sites::deploy(c, &site)?;
            action_result(&cx, &r, "Deployment queued.");
        }
        DplyCommand::SitesDeployments { site } => cx.rows(
            &ep::sites::deployments(c, &site)?,
            &[
                ("ID", &["id"]),
                ("STATUS", &["status"]),
                ("COMMIT", &["commit", "git_commit"]),
                ("STARTED", &["started_at"]),
                ("FINISHED", &["finished_at"]),
            ],
        ),
        DplyCommand::SitesDeployment { site, deployment } => cx.detail(
            &ep::sites::deployment(c, &site, &deployment)?,
            &[
                ("ID", &["id"]),
                ("Status", &["status"]),
                ("Commit", &["commit", "git_commit"]),
                ("Branch", &["branch", "git_branch"]),
                ("Author", &["commit_author"]),
                ("Subject", &["commit_subject"]),
                ("Started", &["started_at"]),
                ("Finished", &["finished_at"]),
                ("Duration", &["duration"]),
            ],
        ),
        DplyCommand::SitesCommits { site } => cx.rows(
            &ep::sites::commits(c, &site)?,
            &[
                ("SHA", &["short_sha", "sha"]),
                ("MESSAGE", &["message"]),
                ("AUTHOR", &["author_name"]),
                ("WHEN", &["committed_at"]),
            ],
        ),
        DplyCommand::SitesDomainsAdd { site, hostname, primary, www_redirect } => {
            ep::sites::domain_add(c, &site, &hostname, primary, www_redirect)?;
            println!("✓ Added domain {hostname}.");
        }
        DplyCommand::SitesDomainsList { site } => cx.rows(
            &ep::sites::domains_list(c, &site)?,
            &[
                ("HOSTNAME", &["hostname"]),
                ("PRIMARY", &["is_primary"]),
                ("WWW REDIRECT", &["www_redirect"]),
            ],
        ),
        DplyCommand::SitesDomainsRemove { site, hostname } => {
            ep::sites::domain_remove(c, &site, &hostname)?;
            println!("✓ Removed domain {hostname}.");
        }
        DplyCommand::SitesBasicAuthAdd { site, username, password, path } => {
            ep::sites::basic_auth_add(c, &site, &username, &password, &path)?;
            println!("✓ Added basic-auth user {username}.");
        }
        DplyCommand::SitesBasicAuthList { site } => cx.rows(
            &ep::sites::basic_auth_list(c, &site)?,
            &[("USERNAME", &["username"]), ("PATH", &["path"])],
        ),
        DplyCommand::SitesBasicAuthRemove { site, username } => {
            ep::sites::basic_auth_remove(c, &site, &username)?;
            println!("✓ Removed basic-auth user {username}.");
        }
        DplyCommand::SitesDbList { site } => cx.rows(
            &ep::sites::databases(c, &site)?,
            &[
                ("NAME", &["name"]),
                ("ENGINE", &["engine"]),
                ("USER", &["username"]),
                ("HOST", &["host"]),
                ("SITE OWNED", &["site_owned"]),
            ],
        ),
        DplyCommand::SitesSchedules { site } => {
            let r = ep::sites::schedules(c, &site)?;
            if cx.json {
                output::json(&r);
            } else {
                println!("Deploy schedules:");
                output::table(
                    r.get("deploy_schedules").unwrap_or(&Value::Null),
                    &[
                        ("CRON", &["cron_expression"]),
                        ("BRANCH", &["git_branch"]),
                        ("TZ", &["timezone"]),
                        ("ACTIVE", &["is_active"]),
                        ("LAST RUN", &["last_run_at"]),
                    ],
                );
                println!("\nCron jobs:");
                output::table(
                    r.get("cron_jobs").unwrap_or(&Value::Null),
                    &[
                        ("CRON", &["cron_expression"]),
                        ("COMMAND", &["command"]),
                        ("USER", &["user"]),
                        ("ENABLED", &["enabled"]),
                        ("LAST RUN", &["last_run_at"]),
                    ],
                );
            }
        }
        DplyCommand::SitesSslStatus { site } => {
            let r = ep::sites::ssl_status(c, &site)?;
            if cx.json {
                output::json(&r);
            } else {
                println!("SSL status: {}", dpl_dply::models::cell_of(&r, &["ssl_status"]));
                output::table(
                    r.get("data").unwrap_or(&Value::Null),
                    &[
                        ("PROVIDER", &["provider_type"]),
                        ("CHALLENGE", &["challenge_type"]),
                        ("STATUS", &["status"]),
                        ("EXPIRES", &["expires_at"]),
                        ("INSTALLED", &["last_installed_at"]),
                    ],
                );
            }
        }
        DplyCommand::SitesSystemUser { site } => cx.detail(
            &ep::sites::system_user(c, &site)?,
            &[("Username", &["username"]), ("Server", &["server_name"])],
        ),
        DplyCommand::SitesUptime { site } => cx.rows(
            &ep::sites::uptime(c, &site)?,
            &[
                ("LABEL", &["label"]),
                ("PATH", &["path"]),
                ("STATUS", &["status"]),
                ("HTTP", &["http_status"]),
                ("LATENCY", &["latency_ms"]),
                ("CHECKED", &["last_checked_at"]),
            ],
        ),
        DplyCommand::SitesWorkers { site } => cx.rows(
            &ep::sites::workers(c, &site)?,
            &[
                ("TYPE", &["type"]),
                ("NAME", &["name"]),
                ("COMMAND", &["command"]),
                ("SCALE", &["scale"]),
                ("ACTIVE", &["is_active"]),
            ],
        ),
        DplyCommand::SitesErrors { site, limit } => cx.rows(
            &ep::sites::errors(c, &site, &limit)?,
            &[
                ("CATEGORY", &["category"]),
                ("TITLE", &["title"]),
                ("WHEN", &["occurred_at"]),
            ],
        ),

        // ---------- site (singular VM env) ----------
        DplyCommand::SiteEnv { site, set, unset, from_file } => {
            site_env(&cx, &site, &set, &unset, from_file.as_deref())?;
        }

        // ---------- insights / imports / operator ----------
        DplyCommand::InsightsSummary => cx.dump(&ep::insights::summary(c)?),
        DplyCommand::InsightsServer { server } => {
            let r = ep::insights::server(c, &server)?;
            let findings = r
                .get("findings")
                .or_else(|| r.get("data"))
                .cloned()
                .unwrap_or(r);
            cx.rows(
                &findings,
                &[
                    ("SEVERITY", &["severity"]),
                    ("CATEGORY", &["category"]),
                    ("TITLE", &["title", "message"]),
                    ("DETECTED", &["detected_at", "created_at"]),
                    ("ACK", &["acknowledged_at"]),
                ],
            );
        }
        DplyCommand::ImportsMigrations => cx.rows(
            &ep::imports::migrations(c)?,
            &[
                ("ID", &["id"]),
                ("SOURCE", &["source"]),
                ("STATUS", &["status"]),
                ("ITEMS", &["item_count", "items"]),
                ("UPDATED", &["updated_at"]),
            ],
        ),
        DplyCommand::ImportsMigration { migration } => {
            cx.dump(&ep::imports::migration(c, &migration)?)
        }
        DplyCommand::OperatorSummary => cx.detail(
            &ep::operator::summary(c)?,
            &[
                ("Operator", &["operator.name"]),
                ("Email", &["operator.email"]),
                ("Role", &["operator.role"]),
                ("Organization", &["organization.name"]),
                ("Org ID", &["organization.id"]),
                ("Plan", &["organization.plan"]),
            ],
        ),
        DplyCommand::OperatorReadme { raw } => {
            let r = ep::operator::readme(c)?;
            if raw || cx.json {
                output::json(&r);
            } else {
                let text = dpl_dply::models::cell_of(&r, &["markdown", "body", "content"]);
                if text.is_empty() {
                    output::json(&r);
                } else {
                    println!("{text}");
                }
            }
        }
    }
    Ok(())
}

/// Print an `id`/`status` acknowledgement for a queued mutation.
fn action_result(cx: &Ctx, r: &Value, verb: &str) {
    if cx.json {
        output::json(r);
        return;
    }
    let id = dpl_dply::models::cell_of(r, &["id"]);
    let status = dpl_dply::models::cell_of(r, &["status"]);
    print!("✓ {verb}");
    if !id.is_empty() {
        print!(" (id {id}");
        if !status.is_empty() {
            print!(", {status}");
        }
        print!(")");
    }
    println!();
}

/// edge:env — apply --from-file (bulk PUT) or --set/--unset (per-key), then
/// list. Values are never returned by the API, so we show key/scope/updated.
fn edge_env(
    cx: &Ctx,
    site: &str,
    set: &[String],
    unset: &[String],
    from_file: Option<&str>,
    scope: &str,
) -> anyhow::Result<()> {
    let c = &cx.client;
    if let Some(path) = from_file {
        let vars = parse_env_file(path)?
            .into_iter()
            .map(|(k, v)| json!({ "key": k, "value": v, "scope": scope }))
            .collect::<Vec<_>>();
        ep::edge::env_put(c, site, vars, scope)?;
    } else {
        for pair in set {
            let (k, v) = split_kv(pair)?;
            ep::edge::env_set(c, site, k, v, scope)?;
        }
        for k in unset {
            ep::edge::env_unset(c, site, k, scope)?;
        }
    }
    cx.rows(
        &ep::edge::env_list(c, site, Some(scope))?,
        &[("KEY", &["key"]), ("SCOPE", &["scope"]), ("UPDATED", &["updated_at"])],
    );
    Ok(())
}

/// site:env — the singular VM env surface; no `scope`.
fn site_env(
    cx: &Ctx,
    site: &str,
    set: &[String],
    unset: &[String],
    from_file: Option<&str>,
) -> anyhow::Result<()> {
    let c = &cx.client;
    if let Some(path) = from_file {
        for (k, v) in parse_env_file(path)? {
            ep::site::env_set(c, site, &k, &v)?;
        }
    } else {
        for pair in set {
            let (k, v) = split_kv(pair)?;
            ep::site::env_set(c, site, k, v)?;
        }
        for k in unset {
            ep::site::env_unset(c, site, k)?;
        }
    }
    cx.rows(&ep::site::env_list(c, site)?, &[("KEY", &["key"])]);
    Ok(())
}

/// edge:logs — one page, or a poll loop under `--tail` advancing `since`.
fn edge_logs(
    c: &DplyClient,
    site: &str,
    limit: u32,
    mut since: Option<String>,
    tail: bool,
    interval: u64,
) -> anyhow::Result<()> {
    loop {
        let page = ep::edge::logs(c, site, limit, since.as_deref())?;
        let rows = dpl_dply::models::rows(&page);
        for row in &rows {
            let ts = dpl_dply::models::cell_of(row, &["timestamp", "logged_at"]);
            let status = dpl_dply::models::cell_of(row, &["status"]);
            let method = dpl_dply::models::cell_of(row, &["method"]);
            let path = dpl_dply::models::cell_of(row, &["path", "url"]);
            let ms = dpl_dply::models::cell_of(row, &["ms"]);
            let msg = dpl_dply::models::cell_of(row, &["message"]);
            println!(
                "{ts}  {status:>3} {method:<6} {path} {ms}ms {msg}",
            );
            // Advance the tail window to the newest timestamp we've seen.
            if !ts.is_empty() {
                since = Some(ts);
            }
        }
        if !tail {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(interval.max(1)));
    }
    Ok(())
}

/// edge:lint — POST a local `dply.{yaml,yml,json}` (or an explicit path).
fn edge_lint(cx: &Ctx, path: Option<&str>) -> anyhow::Result<()> {
    let resolved = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => ["dply.yaml", "dply.yml", "dply.json"]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.exists())
            .context("no dply.yaml/dply.yml/dply.json found in the current directory")?,
    };
    let content = std::fs::read_to_string(&resolved)
        .with_context(|| format!("reading {}", resolved.display()))?;
    let filename = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("dply.yaml");

    let r = ep::edge::lint(&cx.client, filename, &content)?;
    if cx.json {
        output::json(&r);
        return Ok(());
    }
    let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
    print_list(&r, "errors", "✗");
    print_list(&r, "warnings", "⚠");
    if ok {
        println!("✓ {} is valid.", resolved.display());
    } else {
        anyhow::bail!("{} has lint errors.", resolved.display());
    }
    Ok(())
}

fn print_list(r: &Value, key: &str, marker: &str) {
    if let Some(items) = r.get(key).and_then(Value::as_array) {
        for item in items {
            println!("{marker} {}", dpl_dply::models::cell(Some(item)));
        }
    }
}

/// Split `KEY=value` (value may contain `=`).
fn split_kv(pair: &str) -> anyhow::Result<(&str, &str)> {
    pair.split_once('=')
        .filter(|(k, _)| !k.is_empty())
        .context(format!("expected KEY=value, got `{pair}`"))
}

/// Parse a dotenv-style file into ordered key/value pairs. Skips blanks and
/// `#` comments; strips surrounding quotes.
fn parse_env_file(path: &str) -> anyhow::Result<Vec<(String, String)>> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            let v = v.trim().trim_matches('"').trim_matches('\'');
            out.push((k.to_string(), v.to_string()));
        }
    }
    Ok(out)
}

/// Simple y/N confirmation on stdin.
fn confirm(prompt: &str) -> anyhow::Result<bool> {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
