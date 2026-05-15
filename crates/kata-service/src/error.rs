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

    #[error("internal error: {0}")]
    Internal(String),
}
