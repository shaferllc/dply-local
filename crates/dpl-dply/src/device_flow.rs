//! OAuth-style device-authorization login against `/api/v1/auth/device/*`.
//! Port of dply-cli's `DeviceFlow.php`: [`start`](DeviceFlow::start) mints a
//! code pair, then the caller polls [`poll`](DeviceFlow::poll) until the user
//! approves in the browser. A 429 from the server's throttle is reported as
//! `Pending` so the caller just backs off and retries.

use serde::Deserialize;

use crate::error::{DplyApiError, Result};

/// Approval states the server can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Pending,
    Authorized,
    Denied,
    Expired,
}

impl DeviceStatus {
    fn parse(s: &str) -> Self {
        match s {
            "authorized" => DeviceStatus::Authorized,
            "denied" => DeviceStatus::Denied,
            "pending" => DeviceStatus::Pending,
            _ => DeviceStatus::Expired,
        }
    }
}

/// The code pair returned by `start`; the caller shows `user_code` +
/// `verification_uri_complete` to the user and polls with `device_code`.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    /// Minimum seconds between polls (server-requested, floored at 1).
    pub interval: u64,
}

/// One poll result.
#[derive(Debug, Clone)]
pub struct PollOutcome {
    pub status: DeviceStatus,
    /// Present only when `status == Authorized`.
    pub token: Option<String>,
}

#[derive(Deserialize)]
struct StartResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct PollResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

pub struct DeviceFlow {
    http: reqwest::blocking::Client,
}

impl DeviceFlow {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(crate::user_agent())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|source| DplyApiError::Transport {
                method: "BUILD".into(),
                url: "device-flow client".into(),
                source,
            })?;
        Ok(DeviceFlow { http })
    }

    fn url(host: &str, path: &str) -> String {
        format!("{}/{}", host.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    /// Begin the flow; returns the code pair to show the user.
    pub fn start(&self, host: &str) -> Result<DeviceCode> {
        let url = Self::url(host, "api/v1/auth/device/start");
        let resp = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .send()
            .map_err(|source| DplyApiError::Transport {
                method: "POST".into(),
                url: url.clone(),
                source,
            })?;

        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(DplyApiError::Api {
                status: status.as_u16(),
                method: "POST".into(),
                path: "/api/v1/auth/device/start".into(),
                message: format!("could not start device-flow login on {host}"),
                errors: Default::default(),
                raw: truncate(&body),
            });
        }

        let parsed: StartResponse =
            serde_json::from_str(&body).map_err(|source| DplyApiError::Decode {
                context: "device/start response".into(),
                source,
            })?;

        let require = |field: &str, v: Option<String>| -> Result<String> {
            match v {
                Some(s) if !s.is_empty() => Ok(s),
                _ => Err(DplyApiError::Api {
                    status: status.as_u16(),
                    method: "POST".into(),
                    path: "/api/v1/auth/device/start".into(),
                    message: format!("device-flow start response missing field: {field}"),
                    errors: Default::default(),
                    raw: truncate(&body),
                }),
            }
        };

        Ok(DeviceCode {
            device_code: require("device_code", parsed.device_code)?,
            user_code: require("user_code", parsed.user_code)?,
            verification_uri: require("verification_uri", parsed.verification_uri)?,
            verification_uri_complete: require(
                "verification_uri_complete",
                parsed.verification_uri_complete,
            )?,
            expires_in: parsed.expires_in.unwrap_or(600),
            interval: parsed.interval.unwrap_or(5).max(1),
        })
    }

    /// Poll once. A 429 is coerced to `Pending` so the caller's loop simply
    /// sleeps and tries again.
    pub fn poll(&self, host: &str, device_code: &str) -> Result<PollOutcome> {
        let url = Self::url(host, "api/v1/auth/device/poll");
        let resp = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .map_err(|source| DplyApiError::Transport {
                method: "POST".into(),
                url: url.clone(),
                source,
            })?;

        let status = resp.status();
        if status.as_u16() == 429 {
            return Ok(PollOutcome {
                status: DeviceStatus::Pending,
                token: None,
            });
        }
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(DplyApiError::Api {
                status: status.as_u16(),
                method: "POST".into(),
                path: "/api/v1/auth/device/poll".into(),
                message: "device-flow poll failed".into(),
                errors: Default::default(),
                raw: truncate(&body),
            });
        }

        let parsed: PollResponse =
            serde_json::from_str(&body).map_err(|source| DplyApiError::Decode {
                context: "device/poll response".into(),
                source,
            })?;
        let device_status = parsed
            .status
            .as_deref()
            .map(DeviceStatus::parse)
            .unwrap_or(DeviceStatus::Expired);
        let token = if device_status == DeviceStatus::Authorized {
            parsed.token.filter(|t| !t.is_empty())
        } else {
            None
        };
        Ok(PollOutcome {
            status: device_status,
            token,
        })
    }
}

fn truncate(s: &str) -> String {
    if s.len() > 500 {
        format!("{}…", &s[..500])
    } else {
        s.to_string()
    }
}
