use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kata_core::{ReviewId, Session, SessionId};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::state::AppState;

pub async fn start_session(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_id)): Path<(String, ReviewId)>,
) -> AppResult<Json<Session>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.start_session(&repo, &review_id, &author).await?))
}

pub async fn publish_session(
    State(state): State<AppState>,
    Path((repo_name, review_id, session_id)): Path<(String, ReviewId, SessionId)>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    state.service.publish_session(&repo, &review_id, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn discard_session(
    State(state): State<AppState>,
    Path((repo_name, review_id, session_id)): Path<(String, ReviewId, SessionId)>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    state.service.discard_session(&repo, &review_id, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
