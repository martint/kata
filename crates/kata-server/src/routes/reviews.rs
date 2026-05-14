use axum::Json;
use axum::extract::{Path, Query, State};
use kata_core::{Author, Bookmark, ChangeId, CommitId, RepoSummary, ReviewManifest};
use kata_storage::ReviewSummary;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::service::{CommitDiffView, CreateReviewParams, ReviewView};
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

#[derive(Debug, Deserialize)]
pub struct PreviewRevsetQuery {
    pub expr: String,
}

#[derive(Debug, serde::Serialize)]
pub struct RevsetPreview {
    pub count: usize,
}

/// Resolve `expr` against the repo's revset parser and report the
/// commit count. The new-review form calls this as the user types
/// (debounced) to warn before they submit an empty review.
pub async fn preview_revset(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
    Query(q): Query<PreviewRevsetQuery>,
) -> AppResult<Json<RevsetPreview>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let count = state.service.preview_revset(&repo, &q.expr).await?;
    Ok(Json(RevsetPreview { count }))
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
    /// When set, the response's `Diff` shows the patchset[compare]
    /// → patchset[patchset] delta instead of base..tip. Everything
    /// else (commits, comments, anchor resolution) keeps the normal
    /// patchset-scoped meaning.
    #[serde(default)]
    pub compare: Option<u32>,
}

pub async fn open_review(
    State(state): State<AppState>,
    ViewerAuthor(viewer): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
    Query(q): Query<OpenReviewQuery>,
) -> AppResult<Json<ReviewView>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    Ok(Json(
        state
            .service
            .open_review(&repo, &review_id, &viewer, q.patchset, q.compare)
            .await?,
    ))
}

#[derive(Debug, Default, Deserialize)]
pub struct RefreshReviewBody {
    /// Optional summary update. When present, the actor must be the
    /// review's creator. Omit (or send an empty object) to refresh
    /// without touching the summary.
    #[serde(default)]
    pub summary: Option<String>,
}

pub async fn refresh_review(
    State(state): State<AppState>,
    ViewerAuthor(actor): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
    body: Option<Json<RefreshReviewBody>>,
) -> AppResult<Json<ReviewManifest>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    let new_summary = body.and_then(|Json(b)| b.summary);
    Ok(Json(
        state
            .service
            .refresh_review(&repo, &review_id, &actor, new_summary)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSummaryBody {
    /// New summary. `null` or `""` clears it.
    pub summary: Option<String>,
}

pub async fn update_summary(
    State(state): State<AppState>,
    ViewerAuthor(actor): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
    Json(body): Json<UpdateSummaryBody>,
) -> AppResult<Json<ReviewManifest>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    Ok(Json(
        state
            .service
            .update_review_summary(&repo, &review_id, &actor, body.summary)
            .await?,
    ))
}

pub async fn archive_review(
    State(state): State<AppState>,
    ViewerAuthor(actor): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
) -> AppResult<Json<ReviewManifest>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    Ok(Json(
        state
            .service
            .set_review_archived(&repo, &review_id, &actor, true)
            .await?,
    ))
}

pub async fn unarchive_review(
    State(state): State<AppState>,
    ViewerAuthor(actor): ViewerAuthor,
    Path((repo_name, review_number)): Path<(String, u32)>,
) -> AppResult<Json<ReviewManifest>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    Ok(Json(
        state
            .service
            .set_review_archived(&repo, &review_id, &actor, false)
            .await?,
    ))
}

pub async fn commit_diff(
    State(state): State<AppState>,
    Path((repo_name, _review_number, change_id)): Path<(String, u32, ChangeId)>,
) -> AppResult<Json<CommitDiffView>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.commit_diff(&repo, &change_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct FileDiffQuery {
    pub path: String,
    #[serde(default)]
    pub ps: Option<u32>,
    /// Same semantics as [`OpenReviewQuery::compare`]. Threaded
    /// through every lazy file-diff fetch so the per-file hunks
    /// describe the same delta the metadata response did.
    #[serde(default)]
    pub compare: Option<u32>,
}

pub async fn file_diff(
    State(state): State<AppState>,
    Path((repo_name, review_number)): Path<(String, u32)>,
    Query(q): Query<FileDiffQuery>,
) -> AppResult<Json<kata_core::FileChange>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state.service.resolve_review_number(&repo, review_number).await?;
    Ok(Json(
        state
            .service
            .file_diff(&repo, &review_id, &q.path, q.ps, q.compare)
            .await?,
    ))
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
