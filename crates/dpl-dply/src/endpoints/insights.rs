//! Insights (`/api/v1/insights/…` and per-server findings).

use serde_json::Value;

use crate::client::unwrap_data;
use crate::{DplyClient, Result};

pub fn summary(c: &DplyClient) -> Result<Value> {
    Ok(unwrap_data(c.get("/api/v1/insights/summary", &[])?))
}

pub fn server(c: &DplyClient, server: &str) -> Result<Value> {
    Ok(unwrap_data(
        c.get(&format!("/api/v1/servers/{server}/insights"), &[])?,
    ))
}
