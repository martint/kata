use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kata_core::{Comment, CommentId, SessionId};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::service::DraftCommentInput;
use crate::state::AppState;

pub async fn create_comment(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_number, session_id)): Path<(String, u32, SessionId)>,
    Json(input): Json<DraftCommentInput>,
) -> AppResult<(StatusCode, Json<Comment>)> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    let comment = state
        .service
        .upsert_draft_comment(&repo, &review_id, &session_id, &author, None, input)
        .await?;
    Ok((StatusCode::CREATED, Json(comment)))
}

pub async fn update_comment(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_number, session_id, comment_id)): Path<(
        String,
        u32,
        SessionId,
        CommentId,
    )>,
    Json(input): Json<DraftCommentInput>,
) -> AppResult<Json<Comment>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    let comment = state
        .service
        .upsert_draft_comment(&repo, &review_id, &session_id, &author, Some(comment_id), input)
        .await?;
    Ok(Json(comment))
}

pub async fn delete_comment(
    State(state): State<AppState>,
    Path((repo_name, review_number, session_id, comment_id)): Path<(
        String,
        u32,
        SessionId,
        CommentId,
    )>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    state
        .service
        .discard_draft_comment(&repo, &review_id, &session_id, &comment_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
