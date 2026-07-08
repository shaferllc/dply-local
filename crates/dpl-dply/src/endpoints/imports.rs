//! Import/migration jobs (`/api/v1/imports/…`).

use serde_json::Value;

use crate::client::unwrap_data;
use crate::{DplyClient, Result};

pub fn migrations(c: &DplyClient) -> Result<Value> {
    Ok(unwrap_data(c.get("/api/v1/imports/migrations", &[])?))
}

pub fn migration(c: &DplyClient, id: &str) -> Result<Value> {
    Ok(unwrap_data(
        c.get(&format!("/api/v1/imports/migrations/{id}"), &[])?,
    ))
}
