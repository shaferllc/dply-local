//! Server-hosted sites (`/api/v1/sites/…`) — VM/managed-server deployments.

use serde_json::{json, Value};

use super::query;
use crate::client::unwrap_data;
use crate::{DplyClient, Result};

fn base(site: &str) -> String {
    format!("/api/v1/sites/{site}")
}

pub fn list(c: &DplyClient) -> Result<Value> {
    Ok(unwrap_data(c.get("/api/v1/sites", &[])?))
}

pub fn show(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&base(site), &[])?))
}

pub fn rename(c: &DplyClient, site: &str, name: &str) -> Result<Value> {
    Ok(unwrap_data(c.patch(&base(site), json!({ "name": name }))?))
}

pub fn deploy(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.post(&format!("{}/deploy", base(site)), json!({}))?))
}

pub fn deployments(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/deployments", base(site)), &[])?))
}

pub fn deployment(c: &DplyClient, site: &str, deployment: &str) -> Result<Value> {
    Ok(unwrap_data(
        c.get(&format!("{}/deployments/{deployment}", base(site)), &[])?,
    ))
}

pub fn commits(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/commits", base(site)), &[])?))
}

pub fn domains_list(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/domains", base(site)), &[])?))
}

pub fn domain_add(
    c: &DplyClient,
    site: &str,
    hostname: &str,
    is_primary: bool,
    www_redirect: bool,
) -> Result<Value> {
    c.post(
        &format!("{}/domains", base(site)),
        json!({ "hostname": hostname, "is_primary": is_primary, "www_redirect": www_redirect }),
    )
}

pub fn domain_remove(c: &DplyClient, site: &str, hostname: &str) -> Result<Value> {
    c.delete(&format!("{}/domains/{hostname}", base(site)), &[])
}

pub fn basic_auth_list(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/basic-auth", base(site)), &[])?))
}

pub fn basic_auth_add(
    c: &DplyClient,
    site: &str,
    username: &str,
    password: &str,
    path: &str,
) -> Result<Value> {
    c.post(
        &format!("{}/basic-auth", base(site)),
        json!({ "username": username, "password": password, "path": path }),
    )
}

pub fn basic_auth_remove(c: &DplyClient, site: &str, username: &str) -> Result<Value> {
    c.delete(&format!("{}/basic-auth/{username}", base(site)), &[])
}

pub fn databases(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/databases", base(site)), &[])?))
}

pub fn schedules(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/schedules", base(site)), &[])?))
}

pub fn ssl_status(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/ssl", base(site)), &[])?))
}

pub fn system_user(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/system-user", base(site)), &[])?))
}

pub fn uptime(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/uptime", base(site)), &[])?))
}

pub fn workers(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/workers", base(site)), &[])?))
}

pub fn errors(c: &DplyClient, site: &str, limit: &str) -> Result<Value> {
    let q = query(&[("limit", Some(limit.to_string()))]);
    Ok(unwrap_data(c.get(&format!("{}/errors", base(site)), &q)?))
}
