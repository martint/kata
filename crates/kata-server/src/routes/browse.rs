//! HTTP surface for the repository-browser feature.
//!
//! Two endpoints back the log view: a paginated log lay-out
//! (`/log?revset=<expr>&max_rows=<n>`) and a single-commit detail
//! lookup (`/commits/{commit_id}`). The layout itself is done in
//! [`kata_jj::log_graph`] (Sapling-style column-stem); this module
//! is a thin adapter from query strings to service calls.

use axum::Json;
use axum::extract::{Path, Query, State};
use kata_core::{CommitId, LogPage, LogRow};
use serde::Deserialize;

use crate::error::AppResult;
use crate::state::AppState;

/// Default revset used when the caller doesn't specify one. Matches
/// the IDEAS.md recipe: named branches + the workspace's `@` +
/// a short window of recent neighbourhood (ancestors of `@` mixed
/// with its descendants).
const DEFAULT_REVSET: &str = "bookmarks() | @ | latest(@-.. | ..@, 50)";

/// Default page size. Generous because the SVG renderer
/// virtualises and a typical browse session wants enough rows to
/// scroll through without re-fetching every flick.
const DEFAULT_MAX_ROWS: usize = 200;

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// Free-form revset expression. Omit to use the default
    /// (`bookmarks() | @ | latest(@-.. | ..@, 50)`).
    #[serde(default)]
    pub revset: Option<String>,
    /// Cap on the number of rows returned. Defaults to
    /// [`DEFAULT_MAX_ROWS`]; callers can raise it but the server
    /// caps internally at a hard ceiling so a runaway request
    /// can't drag the whole repo into memory.
    #[serde(default)]
    pub max_rows: Option<usize>,
}

const MAX_ROWS_CEILING: usize = 2_000;

pub async fn log(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
    Query(q): Query<LogQuery>,
) -> AppResult<Json<LogPage>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let revset_str = q
        .revset
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_REVSET);
    let max_rows = q
        .max_rows
        .unwrap_or(DEFAULT_MAX_ROWS)
        .min(MAX_ROWS_CEILING);
    let revset = kata_core::RevSet::new(revset_str);
    let page = state.service.browse_log(&repo, &revset, max_rows).await?;
    Ok(Json(page))
}

pub async fn commit(
    State(state): State<AppState>,
    Path((repo_name, commit_id)): Path<(String, CommitId)>,
) -> AppResult<Json<Option<LogRow>>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.browse_commit(&repo, &commit_id).await?))
}
