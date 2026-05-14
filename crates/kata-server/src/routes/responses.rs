use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kata_core::{Response, ResponseId, SessionId};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::service::DraftResponseInput;
use crate::state::AppState;

pub async fn create_response(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, _review_number, session_id)): Path<(String, u32, SessionId)>,
    Json(input): Json<DraftResponseInput>,
) -> AppResult<(StatusCode, Json<Response>)> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let response = state
        .service
        .upsert_draft_response(&repo, &session_id, &author, None, input)
        .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn update_response(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, _review_number, session_id, response_id)): Path<(
        String,
        u32,
        SessionId,
        ResponseId,
    )>,
    Json(input): Json<DraftResponseInput>,
) -> AppResult<Json<Response>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let response = state
        .service
        .upsert_draft_response(&repo, &session_id, &author, Some(response_id), input)
        .await?;
    Ok(Json(response))
}

pub async fn delete_response(
    State(state): State<AppState>,
    Path((repo_name, review_number, session_id, response_id)): Path<(
        String,
        u32,
        SessionId,
        ResponseId,
    )>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    state
        .service
        .discard_draft_response(&repo, &review_id, &session_id, &response_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
