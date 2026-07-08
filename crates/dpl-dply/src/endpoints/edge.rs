//! Edge sites (`/api/v1/edge/…`) — dply's serverless/CDN surface.

use serde_json::{json, Value};

use super::{body, query};
use crate::client::unwrap_data;
use crate::{DplyClient, Result};

fn base(site: &str) -> String {
    format!("/api/v1/edge/sites/{site}")
}

pub fn list(c: &DplyClient, status: Option<&str>) -> Result<Value> {
    let q = query(&[("status", status.map(str::to_string))]);
    Ok(unwrap_data(c.get("/api/v1/edge/sites", &q)?))
}

pub fn show(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&base(site), &[])?))
}

pub fn deploy(c: &DplyClient, site: &str, commit: Option<&str>, branch: Option<&str>) -> Result<Value> {
    let b = body(vec![
        ("git_commit", json!(commit)),
        ("git_branch", json!(branch)),
    ]);
    Ok(unwrap_data(c.post(&format!("{}/deployments", base(site)), b)?))
}

pub fn deployments(c: &DplyClient, site: &str, limit: u32) -> Result<Value> {
    let q = query(&[("limit", Some(limit.to_string()))]);
    Ok(unwrap_data(c.get(&format!("{}/deployments", base(site)), &q)?))
}

pub fn deployment(c: &DplyClient, site: &str, deployment: &str) -> Result<Value> {
    Ok(unwrap_data(
        c.get(&format!("{}/deployments/{deployment}", base(site)), &[])?,
    ))
}

pub fn rollback(c: &DplyClient, site: &str, deployment: &str) -> Result<Value> {
    c.post(
        &format!("{}/deployments/{deployment}/rollback", base(site)),
        json!({}),
    )
}

pub fn access_get(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/access", base(site)), &[])?))
}

pub fn access_set(
    c: &DplyClient,
    site: &str,
    mode: Option<&str>,
    password: Option<&str>,
    allowed_emails: &[String],
) -> Result<Value> {
    let mut pairs = vec![
        ("mode", json!(mode)),
        ("password", json!(password)),
    ];
    if !allowed_emails.is_empty() {
        pairs.push(("allowed_emails", json!(allowed_emails)));
    }
    c.patch(&format!("{}/access", base(site)), body(pairs))
}

pub fn env_list(c: &DplyClient, site: &str, scope: Option<&str>) -> Result<Value> {
    let q = query(&[("scope", scope.map(str::to_string))]);
    Ok(unwrap_data(c.get(&format!("{}/env", base(site)), &q)?))
}

pub fn env_set(c: &DplyClient, site: &str, key: &str, value: &str, scope: &str) -> Result<Value> {
    c.patch(
        &format!("{}/env/{key}", base(site)),
        json!({ "value": value, "scope": scope }),
    )
}

pub fn env_unset(c: &DplyClient, site: &str, key: &str, scope: &str) -> Result<Value> {
    c.delete(
        &format!("{}/env/{key}", base(site)),
        &[("scope", scope.to_string())],
    )
}

/// Bulk env replace from a file: `vars` is a list of `{key,value,scope}`.
pub fn env_put(c: &DplyClient, site: &str, vars: Vec<Value>, scope: &str) -> Result<Value> {
    c.put(
        &format!("{}/env", base(site)),
        json!({ "vars": vars, "scope": scope }),
    )
}

pub fn domains_list(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/domains", base(site)), &[])?))
}

pub fn domain_add(c: &DplyClient, site: &str, hostname: &str) -> Result<Value> {
    c.post(
        &format!("{}/domains", base(site)),
        json!({ "hostname": hostname }),
    )
}

pub fn domain_verify(c: &DplyClient, site: &str, hostname: &str) -> Result<Value> {
    c.post(
        &format!("{}/domains/{hostname}/verify", base(site)),
        json!({}),
    )
}

pub fn domain_remove(c: &DplyClient, site: &str, hostname: &str) -> Result<Value> {
    c.delete(&format!("{}/domains/{hostname}", base(site)), &[])
}

pub fn aliases(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/aliases", base(site)), &[])?))
}

pub fn previews_list(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/previews", base(site)), &[])?))
}

pub fn preview_create(c: &DplyClient, site: &str, branch: &str) -> Result<Value> {
    c.post(
        &format!("{}/previews", base(site)),
        json!({ "branch": branch }),
    )
}

pub fn preview_delete(c: &DplyClient, site: &str, id: &str) -> Result<Value> {
    c.delete(&format!("{}/previews/{id}", base(site)), &[])
}

pub fn preview_promote(c: &DplyClient, site: &str, id: &str) -> Result<Value> {
    c.post(&format!("{}/previews/{id}/promote", base(site)), json!({}))
}

pub fn usage(c: &DplyClient, site: &str, period: Option<&str>) -> Result<Value> {
    let q = query(&[("period", period.map(str::to_string))]);
    Ok(unwrap_data(c.get(&format!("{}/usage", base(site)), &q)?))
}

pub fn purge(c: &DplyClient, site: &str, paths: &[String]) -> Result<Value> {
    let b = if paths.is_empty() {
        json!({})
    } else {
        json!({ "paths": paths })
    };
    c.post(&format!("{}/cache/purge", base(site)), b)
}

/// One page of logs. `since` advances the window on each `--tail` iteration.
pub fn logs(c: &DplyClient, site: &str, limit: u32, since: Option<&str>) -> Result<Value> {
    let q = query(&[
        ("limit", Some(limit.to_string())),
        ("since", since.map(str::to_string)),
    ]);
    Ok(unwrap_data(c.get(&format!("{}/logs", base(site)), &q)?))
}

/// Lint a `dply.yaml`/`.yml`/`.json` config (site-independent endpoint).
pub fn lint(c: &DplyClient, filename: &str, content: &str) -> Result<Value> {
    c.post(
        "/api/v1/edge/lint",
        json!({ "path": filename, "content": content }),
    )
}
