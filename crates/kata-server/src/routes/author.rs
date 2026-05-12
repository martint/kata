use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use kata_core::Author;

use crate::error::AppError;
use crate::state::AppState;

/// Extracts the author identity from the request: an `X-Review-Author` header
/// overrides; otherwise the server-configured default is used.
pub struct ViewerAuthor(pub Author);

impl FromRequestParts<AppState> for ViewerAuthor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(value) = parts.headers.get("x-review-author") {
            let s = value.to_str().map_err(|_| {
                AppError::from(kata_service::ServiceError::BadRequest(
                    "x-review-author header is not valid utf-8".into(),
                ))
            })?;
            return Ok(ViewerAuthor(Author::new(s.to_owned())));
        }
        Ok(ViewerAuthor(state.default_author.clone()))
    }
}
