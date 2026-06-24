//! HTTP handlers for GitHub PR import + auth-status. All GitHub
//! API I/O goes through `gh`; kata stores no GitHub credentials of
//! its own. See [`kata_service::github`] for the rationale.

use axum::Json;
use axum::extract::State;
use kata_core::ReviewManifest;
use kata_service::github::{AuthStatus, GithubClient, GithubError};
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

/// `GET /api/github/status` — does this kata host have a working
/// `gh` setup? Cheap (`gh api user` is a single REST call kata can
/// poll on the home screen). Replaces the per-user OAuth-state
/// endpoint we used to have.
pub async fn status(State(_state): State<AppState>) -> Json<GithubStatusResponse> {
    let client = GithubClient::new();
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
