//! HTTP handlers for GitHub PR import + auth-status. All GitHub
//! API I/O goes through `gh`; kata stores no GitHub credentials of
//! its own. See [`kata_service::github`] for the rationale.

use axum::Json;
use axum::extract::{Path, State};
use kata_core::{ReviewManifest, SessionId};
use kata_service::github::publish::{PublishCounts, PublishEvent};
use kata_service::github::{AuthStatus, GhCliClient, GithubClient as _, GithubError};
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::routes::author::ViewerAuthor;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    /// github.com PR URL pasted by the user. Tolerant of `/files`
    /// and `/commits/<sha>` suffixes — see
    /// [`kata_service::github::parse_pull_request_url`].
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct ImportResponse {
    /// Workspace slug — what URL paths use under `/r/<repo>/...`.
    pub repo_name: String,
    pub review: ReviewManifest,
}

/// `POST /api/github/import` — `{ "url": "https://github.com/..." }`.
/// Creates the kata review tied to the PR; phase 5 will additionally
/// pull existing PR discussion in as published kata comments.
pub async fn import(
    State(state): State<AppState>,
    ViewerAuthor(author): ViewerAuthor,
    Json(req): Json<ImportRequest>,
) -> AppResult<Json<ImportResponse>> {
    let imported = state.service.import_github_pr(&author, &req.url).await?;
    Ok(Json(ImportResponse {
        repo_name: imported.repo_name,
        review: imported.review,
    }))
}

#[derive(Debug, Serialize)]
pub struct GithubStatusResponse {
    /// True iff `gh` is installed *and* authenticated. The UI hides
    /// GitHub-related affordances entirely when this is false.
    pub connected: bool,
    /// Resolved github.com identity, when [`Self::connected`].
    pub github_login: Option<String>,
    /// Human-readable explanation when [`Self::connected`] is false
    /// — distinguishes "install gh" from "run `gh auth login`".
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PublishToGithubRequest {
    /// `COMMENT` (default), `APPROVE`, or `REQUEST_CHANGES`. Maps
    /// directly to GitHub's review `event` field.
    #[serde(default = "default_event")]
    pub event: PublishEvent,
    /// Optional review-level body (rendered above the inline
    /// comments on github.com). When omitted, the GH review has
    /// no body and only the inline + issue comments show up.
    #[serde(default)]
    pub body: Option<String>,
}

fn default_event() -> PublishEvent {
    PublishEvent::Comment
}

/// `POST /api/repos/{repo}/reviews/{n}/sessions/{session}/publish-github`
/// — publish the kata session as a GitHub PR review. Refuses
/// (BadRequest) when the kata review isn't bound to a GH PR.
pub async fn publish_to_github(
    State(state): State<AppState>,
    Path((repo_name, review_number, session_id)): Path<(String, u32, String)>,
    ViewerAuthor(author): ViewerAuthor,
    Json(req): Json<PublishToGithubRequest>,
) -> AppResult<Json<PublishCounts>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let review_id = state
        .service
        .resolve_review_number(&repo, review_number)
        .await?;
    let session = SessionId::new(session_id);
    let counts = state
        .service
        .publish_session_to_github(&repo, &review_id, &session, &author, req.event, req.body)
        .await?;
    Ok(Json(counts))
}

/// `GET /api/github/status` — does this kata host have a working
/// `gh` setup? Cheap (`gh api user` is a single REST call kata can
/// poll on the home screen). Replaces the per-user OAuth-state
/// endpoint we used to have.
pub async fn status(State(_state): State<AppState>) -> Json<GithubStatusResponse> {
    let client = GhCliClient::new();
    let resp = match client.auth_status().await {
        Ok(AuthStatus { login, .. }) => GithubStatusResponse {
            connected: true,
            github_login: Some(login),
            error: None,
        },
        Err(GithubError::NotInstalled) => GithubStatusResponse {
            connected: false,
            github_login: None,
            error: Some(
                "the `gh` CLI is not installed; install it from https://cli.github.com/".into(),
            ),
        },
        Err(GithubError::NotAuthenticated) => GithubStatusResponse {
            connected: false,
            github_login: None,
            error: Some("run `gh auth login` in a terminal".into()),
        },
        Err(e) => GithubStatusResponse {
            connected: false,
            github_login: None,
            // Best-effort surfacing; the user can always run
            // `gh auth status` themselves to dig deeper.
            error: Some(format!("gh: {e}")),
        },
    };
    Json(resp)
}
