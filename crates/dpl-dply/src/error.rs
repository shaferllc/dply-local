//! The single error type for every dply API interaction, mirroring
//! dply-cli's `DplyApiException`: a non-2xx response is decoded from the
//! server's JSON error envelope (`{"message": "...", "errors": {...}}`)
//! and surfaced with the HTTP status, the method/path for context, and the
//! raw body for debugging.

use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, DplyApiError>;

#[derive(Debug, thiserror::Error)]
pub enum DplyApiError {
    /// The caller tried an authenticated request with no stored token.
    #[error("not logged in to {host}. Run `dpl dply login --host={host}` first.")]
    NotAuthenticated { host: String },

    /// Transport-level failure (DNS, TLS, timeout, connection refused).
    #[error("request to {method} {url} failed: {source}")]
    Transport {
        method: String,
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// The server returned a non-2xx status. Fields mirror the dply error
    /// envelope so callers can present a consistent message.
    #[error("dply API {status} on {method} {path}: {message}")]
    Api {
        status: u16,
        method: String,
        path: String,
        /// Parsed `message` field, or a fallback derived from the status.
        message: String,
        /// Parsed per-field `errors` map when present (validation errors).
        errors: BTreeMap<String, Vec<String>>,
        /// Raw response body, truncated, for debugging.
        raw: String,
    },

    /// A 2xx response whose body didn't match the expected shape.
    #[error("could not decode dply response for {context}: {source}")]
    Decode {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    /// Local config (token store) problem.
    #[error(transparent)]
    Config(#[from] dpl_core::CoreError),
}

impl DplyApiError {
    /// The HTTP status code, if this error came from a server response.
    pub fn status(&self) -> Option<u16> {
        match self {
            DplyApiError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// True for auth failures (missing token or 401), which the CLI turns
    /// into a "run `dpl dply login`" hint.
    pub fn is_auth(&self) -> bool {
        matches!(self, DplyApiError::NotAuthenticated { .. })
            || self.status() == Some(401)
    }
}
