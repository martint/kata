//! HTTP surface for the repository-browser feature.
//!
//! Two endpoints back the log view: a paginated log lay-out
//! (`/log?revset=<expr>&max_rows=<n>`) and a single-commit detail
//! lookup (`/commits/{commit_id}`). The layout itself is done in
//! [`kata_jj::log_graph`] (Sapling-style column-stem); this module
//! is a thin adapter from query strings to service calls.

use axum::Json;
use axum::extract::{Path, Query, State};
use kata_core::{ChangeId, CommitId, LogPage, LogRow};
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

pub async fn change(
    State(state): State<AppState>,
    Path((repo_name, change_id)): Path<(String, ChangeId)>,
) -> AppResult<Json<Option<LogRow>>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    Ok(Json(state.service.browse_change(&repo, &change_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub commit: CommitId,
    pub path: String,
}

#[derive(Debug, serde::Serialize)]
pub struct FileContent {
    /// True when the bytes don't decode as UTF-8 — the UI shows a
    /// placeholder instead of trying to render gibberish. Binary
    /// files come back with `content: ""` and `size` populated so
    /// the viewer can still tell the operator "1.2 MB, binary".
    pub binary: bool,
    pub content: String,
    /// Size in bytes regardless of binary-ness. Lets the UI label
    /// the file even when we don't render the content.
    pub size: usize,
}

#[derive(Debug, Deserialize)]
pub struct FileHistoryQuery {
    pub path: String,
    #[serde(default)]
    pub max_rows: Option<usize>,
}

/// `GET /api/repos/{repo}/browse/file-history?path=…` — commits
/// that touched `path`, newest-first via the underlying topo
/// order. Sharing the [`LogPage`] shape with the regular browse
/// log so the same renderer code paths work without per-endpoint
/// adapters. Server-side construction of `files("<path>")` keeps
/// revset-escaping out of the frontend.
pub async fn file_history(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
    Query(q): Query<FileHistoryQuery>,
) -> AppResult<Json<LogPage>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    // `files("path")` is the revset function jj-lib provides for
    // "commits that touched this path". Wrapping in `::@` would
    // restrict to the workspace's ancestry — but a file's
    // history may legitimately live on bookmarks the workspace
    // hasn't followed, so use the unbounded form and let the
    // row cap below trim.
    let escaped = escape_for_revset(&q.path);
    let revset_str = format!(r#"files("{escaped}")"#);
    let max_rows = q
        .max_rows
        .unwrap_or(DEFAULT_MAX_ROWS)
        .min(MAX_ROWS_CEILING);
    let revset = kata_core::RevSet::new(revset_str);
    let page = state.service.browse_log(&repo, &revset, max_rows).await?;
    Ok(Json(page))
}

/// Escape a path for embedding inside `files("…")` in a revset.
/// Backslashes and double-quotes are the two characters the
/// revset language reads specially inside double-quoted strings.
fn escape_for_revset(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

/// `GET /api/repos/{repo}/browse/file?commit=…&path=…` — return
/// the file's contents at a specific commit. The UI renders this
/// with the same Shiki-driven highlighting pipeline used by the
/// diff viewer. Returns 404 when the file doesn't exist at the
/// given (commit, path).
pub async fn file(
    State(state): State<AppState>,
    Path(repo_name): Path<String>,
    Query(q): Query<FileQuery>,
) -> AppResult<Json<FileContent>> {
    let repo = state.service.resolve_repo(&repo_name)?;
    let bytes = state
        .service
        .browse_file_bytes(&repo, &q.commit, &q.path)
        .await?
        .ok_or_else(|| {
            crate::error::AppError::from(kata_service::ServiceError::NotFound(format!(
                "file {} not found at commit {}",
                q.path, q.commit
            )))
        })?;
    let size = bytes.len();
    Ok(Json(match String::from_utf8(bytes) {
        Ok(content) => FileContent {
            binary: false,
            content,
            size,
        },
        Err(_) => FileContent {
            binary: true,
            content: String::new(),
            size,
        },
    }))
}
