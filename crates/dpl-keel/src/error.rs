use thiserror::Error;

pub type Result<T> = std::result::Result<T, KeelApiError>;

#[derive(Debug, Error)]
pub enum KeelApiError {
    #[error("not logged in to Keel Cloud ({url}) — run `dpl keel login` (tokens: {url}/tokens)")]
    NotAuthenticated { url: String },

    /// The API answered with an error envelope (`{"error": "…"}`).
    #[error("{message}")]
    Api { status: u16, message: String },

    #[error("{method} {url} failed: {source}")]
    Transport {
        method: String,
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("could not parse the API response from {url}: {source}")]
    Decode {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("{0}")]
    Config(String),
}
