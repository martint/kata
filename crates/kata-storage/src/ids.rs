//! Helpers for resolving and naming the storage-side ids.

use std::path::{Path, PathBuf};

use kata_core::{Author, CommentId, RepoId, ResponseId, ReviewId, SessionId};
use sha2::{Digest, Sha256};
use uuid::{NoContext, Timestamp, Uuid};

use crate::error::{Error, Result};

/// Resolve `workspace_root/.jj/repo` to the canonical path that all
/// workspaces of the same repo agree on.
///
/// jj stores `.jj/repo` either as a directory (the main workspace) or as a
/// small file pointing at the main workspace's repo directory (additional
/// workspaces). Both cases resolve to the same canonical directory.
pub fn jj_repo_canonical_path(workspace_root: &Path) -> Result<PathBuf> {
    let repo_marker = workspace_root.join(".jj").join("repo");
    let meta = std::fs::metadata(&repo_marker).map_err(|e| Error::JjLayout {
        path: repo_marker.clone(),
        source: e,
    })?;
    let target = if meta.is_dir() {
        repo_marker.clone()
    } else {
        let contents = std::fs::read_to_string(&repo_marker).map_err(|e| Error::JjLayout {
            path: repo_marker.clone(),
            source: e,
        })?;
        let trimmed = contents.trim();
        let candidate = PathBuf::from(trimmed);
        if candidate.is_absolute() {
            candidate
        } else {
            // Relative paths are resolved relative to the directory containing
            // the marker file (i.e. `.jj/`).
            repo_marker.parent().unwrap().join(candidate)
        }
    };
    target
        .canonicalize()
        .map_err(|e| Error::JjLayout { path: target, source: e })
}

/// SHA-256 of the canonical repo path, hex-encoded.
pub fn compute_repo_id(canonical_repo_path: &Path) -> RepoId {
    let mut hasher = Sha256::new();
    hasher.update(canonical_repo_path.as_os_str().as_encoded_bytes());
    RepoId::new(hex::encode(hasher.finalize()))
}

pub fn new_session_id() -> SessionId {
    SessionId::new(uuid_v7().to_string())
}

pub fn new_comment_id() -> CommentId {
    CommentId::new(uuid_v7().to_string())
}

pub fn new_response_id() -> ResponseId {
    ResponseId::new(uuid_v7().to_string())
}

pub(crate) fn uuid_v7() -> Uuid {
    Uuid::new_v7(Timestamp::now(NoContext))
}

/// Reject ids that would escape their directory or otherwise misbehave as
/// filesystem path components.
pub(crate) fn ensure_path_safe(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::InvalidId {
            label: label.to_owned(),
            value: value.to_owned(),
            reason: "empty",
        });
    }
    if value == "." || value == ".." {
        return Err(Error::InvalidId {
            label: label.to_owned(),
            value: value.to_owned(),
            reason: "reserved name",
        });
    }
    for c in value.chars() {
        let bad = matches!(c, '/' | '\\' | '\0') || c.is_control();
        if bad {
            return Err(Error::InvalidId {
                label: label.to_owned(),
                value: value.to_owned(),
                reason: "contains path-unsafe character",
            });
        }
    }
    Ok(())
}

pub(crate) fn ensure_review_id(id: &ReviewId) -> Result<()> {
    ensure_path_safe("review_id", id.as_str())
}

pub(crate) fn ensure_author(author: &Author) -> Result<()> {
    ensure_path_safe("author", author.as_str())
}

pub(crate) fn ensure_session_id(id: &SessionId) -> Result<()> {
    ensure_path_safe("session_id", id.as_str())
}

pub(crate) fn ensure_comment_id(id: &CommentId) -> Result<()> {
    ensure_path_safe("comment_id", id.as_str())
}

pub(crate) fn ensure_response_id(id: &ResponseId) -> Result<()> {
    ensure_path_safe("response_id", id.as_str())
}

pub(crate) fn ensure_repo_id(id: &RepoId) -> Result<()> {
    ensure_path_safe("repo_id", id.as_str())
}
