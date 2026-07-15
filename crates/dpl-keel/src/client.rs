//! Bearer-authed wrapper over Keel Cloud's `/api/v1` — the keel twin of
//! `dpl_dply::DplyClient`. Errors arrive as `{"error": "…"}` envelopes; every
//! success is returned as raw JSON for the CLI/GUI to render.

use std::time::Duration;

use serde_json::Value;

use crate::config::KeelConfig;
use crate::error::{KeelApiError, Result};

pub struct KeelClient {
    http: reqwest::blocking::Client,
    pub url: String,
    token: Option<String>,
}

impl KeelClient {
    pub fn new(url: impl Into<String>, token: Option<String>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(crate::user_agent())
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|source| KeelApiError::Transport {
                method: "BUILD".into(),
                url: "http client".into(),
                source,
            })?;
        Ok(KeelClient { http, url: url.into().trim_end_matches('/').to_string(), token })
    }

    /// Resolve URL + token from env/`~/.keel/cloud.json`.
    pub fn for_active(home_override: Option<String>) -> Result<Self> {
        let config = KeelConfig::new(home_override)?;
        Self::new(config.url(), config.token())
    }

    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    fn require_token(&self) -> Result<&str> {
        self.token.as_deref().ok_or_else(|| KeelApiError::NotAuthenticated { url: self.url.clone() })
    }

    fn request(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = format!("{}/api/v1{path}", self.url);
        let token = self.require_token()?;
        let mut req = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(token)
            .header("Accept", "application/json");
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().map_err(|source| KeelApiError::Transport {
            method: method.to_string(),
            url: url.clone(),
            source,
        })?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(|source| KeelApiError::Transport {
            method: method.to_string(),
            url: url.clone(),
            source,
        })?;
        let value: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).map_err(|source| KeelApiError::Decode { url: url.clone(), source })?
        };
        if !(200..300).contains(&status) {
            let message = value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {status} from {url}"));
            return Err(KeelApiError::Api { status, message });
        }
        Ok(value)
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None)
    }
    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request(reqwest::Method::POST, path, Some(&body))
    }
    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.request(reqwest::Method::PUT, path, Some(&body))
    }
    pub fn delete(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::DELETE, path, None)
    }

    // ---- typed-ish convenience wrappers (Value out, like dpl-dply) ----

    pub fn me(&self) -> Result<Value> {
        self.get("/me")
    }
    pub fn sites(&self) -> Result<Value> {
        Ok(self.get("/sites")?.get("sites").cloned().unwrap_or(Value::Array(vec![])))
    }
    pub fn site(&self, id: &str) -> Result<Value> {
        Ok(self.get(&format!("/sites/{id}"))?.get("site").cloned().unwrap_or(Value::Null))
    }
    pub fn deploys(&self, id: &str) -> Result<Value> {
        Ok(self.get(&format!("/sites/{id}/deploys"))?.get("deploys").cloned().unwrap_or(Value::Array(vec![])))
    }
    pub fn publish(&self, id: &str) -> Result<Value> {
        self.post(&format!("/sites/{id}/publish"), Value::Object(Default::default()))
    }
    pub fn preview(&self, id: &str) -> Result<Value> {
        self.post(&format!("/sites/{id}/preview"), Value::Object(Default::default()))
    }
    pub fn secrets(&self, id: &str) -> Result<Value> {
        Ok(self.get(&format!("/sites/{id}/secrets"))?.get("keys").cloned().unwrap_or(Value::Array(vec![])))
    }
    pub fn set_secret(&self, id: &str, key: &str, value: &str) -> Result<Value> {
        self.put(&format!("/sites/{id}/secrets"), serde_json::json!({ "key": key, "value": value }))
    }
    pub fn delete_secret(&self, id: &str, key: &str) -> Result<Value> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/sites/{id}/secrets"),
            Some(&serde_json::json!({ "key": key })),
        )
    }
    pub fn set_domain(&self, id: &str, hostname: &str) -> Result<Value> {
        self.put(&format!("/sites/{id}/custom-domain"), serde_json::json!({ "hostname": hostname }))
    }
    pub fn clear_domain(&self, id: &str) -> Result<Value> {
        self.delete(&format!("/sites/{id}/custom-domain"))
    }
}
