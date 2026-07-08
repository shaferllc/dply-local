//! The singular `site:env` surface. Shares the `/api/v1/sites/{site}/env`
//! path prefix with the plural group but — unlike `edge:env` — sends **no
//! `scope`** on writes. Kept separate to preserve that distinction.

use serde_json::{json, Value};

use crate::client::unwrap_data;
use crate::{DplyClient, Result};

fn env_base(site: &str) -> String {
    format!("/api/v1/sites/{site}/env")
}

pub fn env_list(c: &DplyClient, site: &str) -> Result<Value> {
    Ok(unwrap_data(c.get(&env_base(site), &[])?))
}

pub fn env_set(c: &DplyClient, site: &str, key: &str, value: &str) -> Result<Value> {
    c.patch(&format!("{}/{key}", env_base(site)), json!({ "value": value }))
}

pub fn env_unset(c: &DplyClient, site: &str, key: &str) -> Result<Value> {
    c.delete(&format!("{}/{key}", env_base(site)), &[])
}
