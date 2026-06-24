pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Storage(#[from] kata_storage::Error),

    #[error(transparent)]
    Jj(#[from] kata_jj::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    BadRequest(String),

    /// The request didn't carry enough authentication to identify
    /// the actor. Surfaced from the auth-mode-aware author
    /// extractors in `kata-server` (and turned into a 401 by
    /// `AppError`) when the configured trusted header is missing
    /// or empty.
    #[error("{0}")]
    Unauthorized(String),

    #[error("internal error: {0}")]
    Internal(String),

    /// A typed 409-class conflict — the request can't be served at
    /// the current state, but the client has a specific recovery
    /// step it can take. `kind` is a stable machine-readable
    /// discriminator the SPA branches on (e.g. `"head_drift"`);
    /// `message` is the human-readable explanation rendered to
    /// the user. Added because head-drift refusal needs a robust
    /// signal the frontend can latch onto without substring-
    /// matching the prose.
    #[error("{message}")]
    Conflict { kind: String, message: String },
}
