//! Operator/account info (`/api/v1/operator/…`). Also used post-login to
//! sniff the profile for `whoami`.

use serde_json::Value;

use crate::client::unwrap_data;
use crate::{DplyClient, Result};

pub fn summary(c: &DplyClient) -> Result<Value> {
    Ok(unwrap_data(c.get("/api/v1/operator/summary", &[])?))
}

pub fn readme(c: &DplyClient) -> Result<Value> {
    Ok(unwrap_data(c.get("/api/v1/operator/readme", &[])?))
}
