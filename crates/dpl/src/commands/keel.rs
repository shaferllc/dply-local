//! `dpl keel <cmd>` — Keel Cloud, rendered exactly like the dply commands:
//! tables for lists, two-column detail for shows, raw JSON under `--json`.

use anyhow::{Context, Result};
use dpl_keel::{KeelClient, KeelConfig};

use crate::cli::KeelCommand;
use crate::output;

pub fn run(cmd: KeelCommand, home: Option<String>, json: bool) -> Result<()> {
    // Auth commands need no client.
    match &cmd {
        KeelCommand::Login { token, url } => return login(home, token.clone(), url.clone()),
        KeelCommand::Logout => {
            KeelConfig::new(home)?.logout()?;
            println!("Logged out of Keel Cloud.");
            return Ok(());
        }
        _ => {}
    }

    let client = KeelClient::for_active(home)?;
    match cmd {
        KeelCommand::Login { .. } | KeelCommand::Logout => unreachable!("handled above"),
        KeelCommand::Whoami => {
            let me = client.me()?;
            if json {
                output::json(&me);
            } else {
                println!("Keel Cloud · {}", client.url);
                output::detail(&me, &[
                    ("Name", &["name"]),
                    ("Email", &["email"]),
                    ("Team", &["team_id"]),
                    ("Plan", &["plan"]),
                    ("Site limit", &["site_limit"]),
                ]);
            }
        }
        KeelCommand::SitesList => {
            let sites = client.sites()?;
            if json {
                output::json(&sites);
            } else {
                output::table(&sites, &[
                    ("ID", &["id"]),
                    ("Name", &["name"]),
                    ("Status", &["status"]),
                    ("Production", &["custom_hostname", "prod_hostname"]),
                    ("Preview", &["preview_hostname"]),
                ]);
            }
        }
        KeelCommand::SitesShow { id } => {
            let site = client.site(&id)?;
            if json {
                output::json(&site);
            } else {
                output::detail(&site, &[
                    ("ID", &["id"]),
                    ("Name", &["name"]),
                    ("Slug", &["slug"]),
                    ("Preset", &["preset"]),
                    ("Status", &["status"]),
                    ("Keel", &["keel_version"]),
                    ("Production", &["prod_hostname"]),
                    ("Preview", &["preview_hostname"]),
                    ("Custom domain", &["custom_hostname"]),
                    ("Domain status", &["custom_hostname_status"]),
                    ("Git", &["git_html_url", "git_url"]),
                ]);
            }
        }
        KeelCommand::Deploys { id } => {
            let deploys = client.deploys(&id)?;
            if json {
                output::json(&deploys);
            } else {
                output::table(&deploys, &[
                    ("ID", &["id"]),
                    ("Target", &["target", "environment"]),
                    ("Status", &["status"]),
                    ("Created", &["created_at"]),
                ]);
            }
        }
        KeelCommand::Publish { id } => {
            let out = client.publish(&id)?;
            if json {
                output::json(&out);
            } else {
                let host = out.get("hostname").and_then(|v| v.as_str()).unwrap_or("-");
                println!("Publishing site {id} → https://{host}");
            }
        }
        KeelCommand::Preview { id } => {
            let out = client.preview(&id)?;
            if json {
                output::json(&out);
            } else {
                let host = out.get("hostname").and_then(|v| v.as_str()).unwrap_or("-");
                println!("Deploying preview for site {id} → https://{host}");
            }
        }
        KeelCommand::SecretsList { id } => {
            let keys = client.secrets(&id)?;
            if json {
                output::json(&keys);
            } else {
                match keys.as_array().filter(|a| !a.is_empty()) {
                    Some(list) => {
                        for k in list {
                            println!("{}", k.as_str().unwrap_or_default());
                        }
                    }
                    None => println!("(no secrets)"),
                }
            }
        }
        KeelCommand::SecretsSet { id, key, value } => {
            client.set_secret(&id, &key, &value)?;
            println!("Set `{key}` on site {id}.");
        }
        KeelCommand::SecretsUnset { id, key } => {
            client.delete_secret(&id, &key)?;
            println!("Removed `{key}` from site {id}.");
        }
        KeelCommand::DomainSet { id, hostname } => {
            let out = client.set_domain(&id, &hostname)?;
            if json {
                output::json(&out);
            } else {
                println!("Custom domain for site {id}: {hostname} ({})",
                    out.get("custom_hostname_status").and_then(|v| v.as_str()).unwrap_or("pending"));
            }
        }
        KeelCommand::DomainClear { id } => {
            client.clear_domain(&id)?;
            println!("Cleared the custom domain on site {id}.");
        }
    }
    Ok(())
}

/// Store the token — from `--token`, else prompted (paste from /tokens).
fn login(home: Option<String>, token: Option<String>, url: Option<String>) -> Result<()> {
    let config = KeelConfig::new(home.clone())?;
    let token = match token {
        Some(t) => t,
        None => {
            let tokens_url = url.as_deref().unwrap_or(dpl_keel::DEFAULT_URL).trim_end_matches('/').to_string();
            eprintln!("Mint a token at {tokens_url}/tokens, then paste it here.");
            eprint!("Token: ");
            use std::io::BufRead;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line).context("reading token")?;
            line.trim().to_string()
        }
    };
    if token.is_empty() {
        anyhow::bail!("no token given");
    }
    config.login(&token, url.as_deref())?;
    // Prove it works before claiming success.
    let client = KeelClient::for_active(home)?;
    match client.me() {
        Ok(me) => {
            let name = me.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let plan = me.get("plan").and_then(|v| v.as_str()).unwrap_or("free");
            println!("Logged in to Keel Cloud as {name} ({plan} plan).");
        }
        Err(e) => println!("Token stored, but verifying it failed: {e}"),
    }
    Ok(())
}
