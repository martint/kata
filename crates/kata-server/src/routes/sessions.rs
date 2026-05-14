use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kata_core::{Session, SessionId};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::state::AppState;

pub async fn start_session(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
) -> AppResult<Json<Session>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    Ok(Json(state.service.start_session(&repo, &review_id, &author).await?))
}

pub async fn publish_session(
    State(state): State<AppState>,
    Path((repo_name, review_number, session_id)): Path<(String, u32, SessionId)>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    state.service.publish_session(&repo, &review_id, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn discard_session(
    State(state): State<AppState>,
    Path((repo_name, review_number, session_id)): Path<(String, u32, SessionId)>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    state.service.discard_session(&repo, &review_id, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
