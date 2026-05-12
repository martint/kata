use axum::Json;
use axum::extract::{Path, Query, State};
use kata_core::{Author, Bookmark, ChangeId, CommitId, Diff, RepoSummary, ReviewId, ReviewManifest};
use kata_storage::ReviewSummary;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::service::{CreateReviewParams, ReviewView};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct WhoAmI {
    pub author: Author,
}

pub async fn whoami(ViewerAuthor(author): ViewerAuthor) -> Json<WhoAmI> {
    Json(WhoAmI { author })
}

pub async fn list_repos(State(state): State<AppState>) -> Json<Vec<RepoSummary>> {
    Json(state.service.list_repos())
}

pub async fn list_bookmarks(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
) -> AppResult<Json<Vec<Bookmark>>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.list_bookmarks(&repo).await?))
}

pub async fn list_reviews(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
) -> AppResult<Json<Vec<ReviewSummary>>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.list_reviews(&repo).await?))
}

pub async fn create_review(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
    Json(params): Json<CreateReviewParams>,
) -> AppResult<Json<ReviewManifest>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.create_review(&repo, params).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct OpenReviewQuery {
    #[serde(default)]
    pub patchset: Option<u32>,
}

pub async fn open_review(
    State(state): State<AppState>,
    ViewerAuthor(viewer): ViewerAuthor,
    Path((repo_name, review_id)): Path<(String, ReviewId)>,
    Query(q): Query<OpenReviewQuery>,
) -> AppResult<Json<ReviewView>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(
        state
            .service
            .open_review(&repo, &review_id, &viewer, q.patchset)
            .await?,
    ))
}

pub async fn refresh_review(
    State(state): State<AppState>,
    Path((repo_name, review_id)): Path<(String, ReviewId)>,
) -> AppResult<Json<ReviewManifest>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.refresh_review(&repo, &review_id).await?))
}

pub async fn commit_diff(
    State(state): State<AppState>,
    Path((repo_name, _review_id, change_id)): Path<(String, ReviewId, ChangeId)>,
) -> AppResult<Json<Diff>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.commit_diff(&repo, &change_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub commit: CommitId,
    pub path: String,
}

pub async fn read_file(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
    Query(q): Query<FileQuery>,
) -> AppResult<String> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(state.service.read_file_text(&repo, &q.commit, &q.path).await?)
}
