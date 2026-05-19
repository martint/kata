use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use kata_core::Author;

use crate::auth::AuthMode;
use crate::error::AppError;
use crate::state::AppState;

/// Extracts the author identity from the request. The lookup depends
/// on the configured [`AuthMode`]:
///
/// - **`TrustClient`**: an `X-Review-Author` header wins; otherwise
///   the server's configured default is used. This is the historical
///   behaviour, suitable for localhost / single-user setups.
/// - **`TrustForwardedHeader`**: the configured trusted header
///   (default `X-Forwarded-Email`) is the only source. Missing
///   header is a 401 — the absence means the upstream proxy failed
///   to authenticate, not that we should impersonate the default.
pub struct ViewerAuthor(pub Author);

impl FromRequestParts<AppState> for ViewerAuthor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match state.auth.mode {
            AuthMode::TrustClient => {
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
            AuthMode::TrustForwardedHeader => {
                let header = &state.auth.trusted_header;
                let value = parts.headers.get(header.as_str()).ok_or_else(|| {
                    AppError::from(kata_service::ServiceError::Unauthorized(format!(
                        "missing {header} header (auth-mode=trust-forwarded-header)",
                    )))
                })?;
                let s = value.to_str().map_err(|_| {
                    AppError::from(kata_service::ServiceError::BadRequest(format!(
                        "{header} header is not valid utf-8",
                    )))
                })?;
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Err(AppError::from(kata_service::ServiceError::Unauthorized(
                        format!("{header} header is empty"),
                    )));
                }
                Ok(ViewerAuthor(Author::new(trimmed.to_owned())))
            }
        }
    }
}
