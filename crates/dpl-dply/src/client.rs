//! Bearer-authed HTTP wrapper over the dply v1 API — port of dply-cli's
//! `DplyClient.php`. Scoped to one host + token; commands construct one via
//! [`DplyClient::for_active`] and call [`get`](DplyClient::get)/
//! [`post`](DplyClient::post)/etc. Auth, JSON, base URL, and error-envelope
//! decoding all live here so callers handle one error type.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::config_store::ConfigStore;
use crate::error::{DplyApiError, Result};

pub struct DplyClient {
    http: reqwest::blocking::Client,
    pub host: String,
    token: Option<String>,
}

impl DplyClient {
    /// Build a client for an explicit host/token pair.
    pub fn new(host: impl Into<String>, token: Option<String>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(crate::user_agent())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|source| DplyApiError::Transport {
                method: "BUILD".into(),
                url: "http client".into(),
                source,
            })?;
        Ok(DplyClient {
            http,
            host: host.into().trim_end_matches('/').to_string(),
            token,
        })
    }

    /// Resolve host + token from the shared `~/.dply/config.json`, honouring
    /// the `--host` flag / `$DPLY_HOST` / `default_host` precedence.
    pub fn for_active(host_flag: Option<&str>, home_override: Option<String>) -> Result<Self> {
        let store = ConfigStore::new(home_override);
        let host = store.resolve_host(host_flag)?;
        let token = store.token_for(&host)?;
        Self::new(host, token)
    }

    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// The token, or an auth error naming the host so the CLI can print a
    /// "run `dpl dply login`" hint.
    pub fn require_token(&self) -> Result<&str> {
        self.token
            .as_deref()
            .ok_or_else(|| DplyApiError::NotAuthenticated {
                host: self.host.clone(),
            })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.host, path.trim_start_matches('/'))
    }

    pub fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(reqwest::Method::GET, path, query, None)
    }

    pub fn delete(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(reqwest::Method::DELETE, path, query, None)
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request(reqwest::Method::POST, path, &[], Some(body))
    }

    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.request(reqwest::Method::PUT, path, &[], Some(body))
    }

    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.request(reqwest::Method::PATCH, path, &[], Some(body))
    }

    /// The one place requests are actually issued. Attaches auth, sends JSON,
    /// and turns a non-2xx into a decoded [`DplyApiError::Api`].
    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let url = self.url(path);
        let token = self.require_token()?;
        let mut req = self
            .http
            .request(method.clone(), &url)
            .header("Accept", "application/json")
            .bearer_auth(token);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(ref b) = body {
            req = req.json(b);
        }

        let resp = req.send().map_err(|source| DplyApiError::Transport {
            method: method.to_string(),
            url: url.clone(),
            source,
        })?;

        let status = resp.status();
        let text = resp.text().unwrap_or_default();

        if status.is_success() {
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text).map_err(|source| DplyApiError::Decode {
                context: format!("{method} {path}"),
                source,
            });
        }

        Err(decode_error_envelope(
            status.as_u16(),
            method.as_str(),
            path,
            &text,
        ))
    }

    /// Streaming GET for `edge:logs --tail`: returns the raw blocking
    /// response so the caller can read the body incrementally.
    pub fn stream(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<reqwest::blocking::Response> {
        let url = self.url(path);
        let token = self.require_token()?;
        self.http
            .get(&url)
            .header("Accept", "application/json")
            .bearer_auth(token)
            .query(query)
            .send()
            .map_err(|source| DplyApiError::Transport {
                method: "GET".into(),
                url,
                source,
            })
    }
}

/// dply list/detail endpoints return either a bare value or a `{"data": …}`
/// envelope. Mirror dply-cli: use `data` when it's present and an
/// array/object, else the value itself.
pub fn unwrap_data(value: Value) -> Value {
    if let Value::Object(ref map) = value {
        if let Some(inner) = map.get("data") {
            if inner.is_array() || inner.is_object() {
                return inner.clone();
            }
        }
    }
    value
}

/// Decode dply's `{ "message": "...", "errors": { field: [msgs] } }` error
/// body into a structured [`DplyApiError::Api`], falling back to the raw text
/// when the body isn't the expected shape.
fn decode_error_envelope(status: u16, method: &str, path: &str, body: &str) -> DplyApiError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default_status_message(status));

    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(errs) = parsed.as_ref().and_then(|v| v.get("errors")).and_then(Value::as_object) {
        for (field, msgs) in errs {
            let list = match msgs {
                Value::Array(a) => a
                    .iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect(),
                Value::String(s) => vec![s.clone()],
                other => vec![other.to_string()],
            };
            errors.insert(field.clone(), list);
        }
    }

    DplyApiError::Api {
        status,
        method: method.to_string(),
        path: path.to_string(),
        message,
        errors,
        raw: if body.len() > 1000 {
            format!("{}…", &body[..1000])
        } else {
            body.to_string()
        },
    }
}

fn default_status_message(status: u16) -> String {
    match status {
        401 => "unauthorized — token missing or expired".into(),
        403 => "forbidden".into(),
        404 => "not found".into(),
        422 => "validation failed".into(),
        429 => "rate limited".into(),
        500..=599 => "dply server error".into(),
        _ => format!("request failed ({status})"),
    }
}
