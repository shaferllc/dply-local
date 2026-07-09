//! One error type for the shared layer. Crates on top add their own
//! (e.g. `dpl-dply` has `DplyApiError`) and convert into `anyhow` at the
//! binary edge.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("could not resolve $HOME — set HOME or pass --config-dir")]
    NoHome,

    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config at {path} is not valid TOML: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error("ipc protocol error: {0}")]
    Ipc(String),

    /// A validation failure with a message already written for the user.
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CoreError {
    /// Attach a path to a raw [`std::io::Error`] — the common case when
    /// reading/writing config or socket files.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io {
            path: path.into(),
            source,
        }
    }
}
