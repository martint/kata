use kata_core::ChangeId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("jj exited with status {status}: {stderr}")]
    JjFailed { status: i32, stderr: String },

    #[error("io error invoking jj: {0}")]
    Io(#[from] std::io::Error),

    #[error("could not parse jj output: {0}")]
    Parse(String),

    #[error("change id not found: {0}")]
    ChangeNotFound(ChangeId),

    #[error("revset {revset:?} is empty")]
    EmptyRevset { revset: String },

    #[error("revset {revset:?} resolved to multiple heads; expected a single tip")]
    MultipleHeads { revset: String },
}
