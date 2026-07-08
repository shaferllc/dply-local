//! Provisioned servers (`/api/v1/servers/…`): listing, remote commands,
//! firewall, and log shipping.

use serde_json::{json, Value};

use crate::client::unwrap_data;
use crate::{DplyClient, Result};

fn base(server: &str) -> String {
    format!("/api/v1/servers/{server}")
}

pub fn list(c: &DplyClient) -> Result<Value> {
    Ok(unwrap_data(c.get("/api/v1/servers", &[])?))
}

pub fn run(c: &DplyClient, server: &str, command: &str, user: &str) -> Result<Value> {
    Ok(unwrap_data(c.post(
        &format!("{}/run-command", base(server)),
        json!({ "command": command, "user": user }),
    )?))
}

pub fn firewall_show(c: &DplyClient, server: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/firewall", base(server)), &[])?))
}

pub fn firewall_apply(c: &DplyClient, server: &str) -> Result<Value> {
    c.post(&format!("{}/firewall/apply", base(server)), json!({}))
}

pub fn firewall_template(c: &DplyClient, server: &str, template: &str) -> Result<Value> {
    c.post(
        &format!("{}/firewall/templates/{template}", base(server)),
        json!({}),
    )
}

pub fn firewall_bundled(c: &DplyClient, server: &str, key: &str) -> Result<Value> {
    c.post(&format!("{}/firewall/bundled/{key}", base(server)), json!({}))
}

pub fn log_shipping_show(c: &DplyClient, server: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&format!("{}/log-shipping", base(server)), &[])?))
}

pub fn log_shipping_enable(c: &DplyClient, server: &str, sources: &[String]) -> Result<Value> {
    let mut b = serde_json::Map::new();
    if !sources.is_empty() {
        let map: serde_json::Map<String, Value> =
            sources.iter().map(|s| (s.clone(), json!(true))).collect();
        b.insert("sources".into(), Value::Object(map));
    }
    c.post(
        &format!("{}/log-shipping/enable", base(server)),
        Value::Object(b),
    )
}

pub fn log_shipping_resync(c: &DplyClient, server: &str) -> Result<Value> {
    c.post(&format!("{}/log-shipping/resync", base(server)), json!({}))
}

pub fn log_shipping_disable(c: &DplyClient, server: &str) -> Result<Value> {
    c.delete(&format!("{}/log-shipping", base(server)), &[])
}
