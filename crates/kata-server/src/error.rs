use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kata_service::ServiceError;
use serde_json::json;

pub type AppResult<T> = std::result::Result<T, AppError>;

/// HTTP-layer wrapper around [`ServiceError`]. The single variant carries
/// the service error verbatim; [`IntoResponse`] maps it to a status code.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct AppError(#[from] pub ServiceError);

impl From<kata_storage::Error> for AppError {
    fn from(e: kata_storage::Error) -> Self {
        Self(ServiceError::Storage(e))
    }
}

impl From<kata_jj::Error> for AppError {
    fn from(e: kata_jj::Error) -> Self {
        Self(ServiceError::Jj(e))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            ServiceError::NotFound(_) => StatusCode::NOT_FOUND,
            ServiceError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ServiceError::Storage(kata_storage::Error::NotFound { .. }) => {
                StatusCode::NOT_FOUND
            }
            ServiceError::Storage(kata_storage::Error::ReviewExists { .. }) => {
                StatusCode::CONFLICT
            }
            ServiceError::Storage(kata_storage::Error::SessionState { .. }) => {
                StatusCode::CONFLICT
            }
            ServiceError::Storage(kata_storage::Error::InvalidId { .. }) => {
                StatusCode::BAD_REQUEST
            }
            ServiceError::Storage(kata_storage::Error::Frontmatter { .. }) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            ServiceError::Jj(kata_jj::Error::ChangeNotFound(_)) => StatusCode::NOT_FOUND,
            ServiceError::Jj(kata_jj::Error::EmptyRevset { .. }) => StatusCode::BAD_REQUEST,
            ServiceError::Jj(kata_jj::Error::MultipleHeads { .. }) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = self.0.to_string();
        if status.is_server_error() {
            tracing::error!(error = %message, "request failed");
        }
        (status, Json(json!({ "error": message }))).into_response()
    }
}
