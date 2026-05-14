use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not resolve jj repo layout at {path}: {source}")]
    JjLayout {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid {label} {value:?}: {reason}")]
    InvalidId {
        label: String,
        value: String,
        reason: &'static str,
    },

    #[error("malformed markdown frontmatter at {path}: {detail}")]
    Frontmatter { path: PathBuf, detail: String },

    #[error("toml error at {path}: {message}")]
    Toml { path: PathBuf, message: String },

    #[error("session {session} is in state {state:?} but operation requires {expected:?}")]
    SessionState {
        session: String,
        state: &'static str,
        expected: &'static str,
    },

    #[error("not found: {what}")]
    NotFound { what: String },

    #[error("review {review} already exists")]
    ReviewExists { review: String },

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json serialization error at {context}: {source}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },
}
