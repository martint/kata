//! HTTP routes for author-attached annotations.
//!
//! Unlike comments, these don't go through a session/draft cycle and
//! aren't gated by session membership — instead the service layer
//! enforces "only the review creator can write." The handler is
//! intentionally thin: validate the path, extract the actor, hand off
//! to the service.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kata_core::{Annotation, AnnotationId};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::service::AnnotationInput;
use crate::state::AppState;

pub async fn create_annotation(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
    Json(input): Json<AnnotationInput>,
) -> AppResult<(StatusCode, Json<Annotation>)> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state
        .service
        .resolve_review_number(&repo, review_number)
        .await?;
    let annotation = state
        .service
        .upsert_annotation(&repo, &review_id, &author, None, input)
        .await?;
    Ok((StatusCode::CREATED, Json(annotation)))
}

pub async fn update_annotation(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_number, annotation_id)): Path<(String, u32, AnnotationId)>,
    Json(input): Json<AnnotationInput>,
) -> AppResult<Json<Annotation>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state
        .service
        .resolve_review_number(&repo, review_number)
        .await?;
    let annotation = state
        .service
        .upsert_annotation(&repo, &review_id, &author, Some(annotation_id), input)
        .await?;
    Ok(Json(annotation))
}

pub async fn delete_annotation(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Path((repo_name, review_number, annotation_id)): Path<(String, u32, AnnotationId)>,
) -> AppResult<StatusCode> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state
        .service
        .resolve_review_number(&repo, review_number)
        .await?;
    state
        .service
        .delete_annotation(&repo, &review_id, &author, &annotation_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
