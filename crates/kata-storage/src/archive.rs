//! The file-based archive format that backs `kata export` and `kata import`.
//!
//! Same on-disk layout that the legacy filesystem store used (TOML
//! frontmatter + Markdown bodies, one file per entity), explicitly
//! preserved here as a *stable versioned format* the SQLite store can
//! round-trip into. Two reasons it stays human-shaped:
//!
//! 1. Each entity carries its own `schema_version` field, so an archive
//!    written by one version of Kata can be inspected (and, with a future
//!    importer, migrated) without depending on the runtime that produced
//!    it. The current SQLite schema is the *operational* shape — fast
//!    indexed queries, transactional consistency — but is not a migration
//!    target the way an archive is.
//! 2. The directory layout is debuggable: a reviewer can `grep` for a
//!    comment author, an analyst can pipe `.md` files through their tool
//!    of choice, and a backup pipeline is just `rsync`.
//!
//! Layout:
//!
//! ```text
//! <root>/
//!   <repo-id>/
//!     repo.toml
//!     reviews/
//!       <review-id>/
//!         review.toml
//!         sessions/
//!           <author>/
//!             <session-id>/
//!               session.toml
//!               comments/<comment-id>.md
//!               responses/<response-id>.md
//! ```

use std::io;
use std::path::{Path, PathBuf};

use kata_core::{
    Author, Comment, RepoId, RepoManifest, Response, ReviewId, ReviewManifest, Session, SessionId,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Error, Result};
use crate::frontmatter;
use crate::ids::{
    ensure_author, ensure_comment_id, ensure_repo_id, ensure_response_id, ensure_review_id,
    ensure_session_id, uuid_v7,
};
use crate::sqlite::SqliteStorage;
use crate::storage::Storage;

/// Copy the entire SQLite database into `root` as the on-disk archive
/// format. Idempotent at the file level — running it twice over the same
/// `root` overwrites earlier files atomically.
pub async fn export(storage: &SqliteStorage, root: &Path) -> Result<()> {
    create_dir_all(root).await?;
    for repo in storage.list_all_repos().await? {
        write_repo_manifest(root, &repo).await?;
        for summary in storage.list_reviews(&repo.repo_id).await? {
            let manifest = summary.manifest;
            write_review_manifest(root, &repo.repo_id, &manifest).await?;
            for session in storage
                .list_all_sessions(&repo.repo_id, &manifest.review_id)
                .await?
            {
                write_session_manifest(root, &repo.repo_id, &manifest.review_id, &session).await?;
                let comments = storage
                    .list_all_comments_for_session(&session.session_id)
                    .await?;
                for c in &comments {
                    write_comment(root, &repo.repo_id, &manifest.review_id, &session.author, c)
                        .await?;
                }
                let responses = storage
                    .list_all_responses_for_session(&session.session_id)
                    .await?;
                for r in &responses {
                    write_response(root, &repo.repo_id, &manifest.review_id, &session.author, r)
                        .await?;
                }
            }
        }
    }
    Ok(())
}

/// Read the archive at `root` and load it into `storage`. Any conflicts
/// with rows already in the database error out — import expects a clean
/// target. (Use a fresh `kata.db` when importing into an existing
/// install; the one-shot FS → SQLite migration is just an import into
/// an empty database.)
///
/// Errors if no valid `<repo-id>/repo.toml` is found at `root`, so a
/// typo in the source path doesn't silently no-op.
///
/// Responses are deferred to a second pass after every session and
/// comment has been written. A response's `in_reply_to` can target a
/// comment in a *different* session (reviewer replying to author's
/// earlier round), and the directory walker has no way to know whether
/// that target session will be visited before or after this one — so
/// inserting responses inline would either need a partial-FK schema or
/// a topological sort. Two passes is simpler.
pub async fn import(root: &Path, storage: &SqliteStorage) -> Result<()> {
    let mut pending_responses: Vec<(RepoId, Response)> = Vec::new();
    let mut repos_seen = 0usize;
    for repo_dir in list_subdirs(root).await? {
        let repo_id = RepoId::new(dir_name(&repo_dir)?);
        let Some(repo_manifest) = read_optional_toml::<RepoManifest>(&repo_manifest_path(
            root, &repo_id,
        ))
        .await?
        else {
            // Directory without a `repo.toml` is not a repo — skip
            // silently. Earlier exports always wrote one; this guard
            // exists for `.tmp.*` leftovers and the like.
            continue;
        };
        repos_seen += 1;
        storage.ensure_repo(&repo_manifest).await?;

        let reviews_root = reviews_dir(root, &repo_id);
        if !exists(&reviews_root).await? {
            continue;
        }
        for review_dir in list_subdirs(&reviews_root).await? {
            let review_id = ReviewId::new(dir_name(&review_dir)?);
            let Some(review_manifest) = read_optional_toml::<ReviewManifest>(
                &review_manifest_path(root, &repo_id, &review_id),
            )
            .await?
            else {
                continue;
            };
            storage.create_review(&repo_id, &review_manifest).await?;

            let sessions_root = sessions_dir(root, &repo_id, &review_id);
            if !exists(&sessions_root).await? {
                continue;
            }
            for author_dir in list_subdirs(&sessions_root).await? {
                let author = Author::new(dir_name(&author_dir)?);
                for session_dir in list_subdirs(&author_dir).await? {
                    let session_id = SessionId::new(dir_name(&session_dir)?);
                    let manifest_path = session_manifest_path(
                        root, &repo_id, &review_id, &author, &session_id,
                    );
                    let Some(session) = read_optional_toml::<Session>(&manifest_path).await?
                    else {
                        continue;
                    };
                    storage.raw_insert_session(&repo_id, &session).await?;

                    let comments_path =
                        comments_dir(root, &repo_id, &review_id, &author, &session_id);
                    for comment in read_markdown_dir::<Comment, _>(&comments_path, |c, body| {
                        Comment { body, ..c }
                    })
                    .await?
                    {
                        storage.raw_insert_comment(&repo_id, &comment).await?;
                    }

                    let responses_path =
                        responses_dir(root, &repo_id, &review_id, &author, &session_id);
                    for response in read_markdown_dir::<Response, _>(&responses_path, |r, body| {
                        Response { body, ..r }
                    })
                    .await?
                    {
                        pending_responses.push((repo_id.clone(), response));
                    }
                }
            }
        }
    }
    if repos_seen == 0 {
        return Err(Error::NotFound {
            what: format!(
                "no <repo-id>/repo.toml found under {} — not a kata archive",
                root.display()
            ),
        });
    }
    for (repo_id, response) in pending_responses {
        storage.raw_insert_response(&repo_id, &response).await?;
    }
    Ok(())
}

// ---- path layout (private) ---------------------------------------------

fn repo_dir(root: &Path, repo: &RepoId) -> PathBuf {
    root.join(repo.as_str())
}

fn repo_manifest_path(root: &Path, repo: &RepoId) -> PathBuf {
    repo_dir(root, repo).join("repo.toml")
}

fn reviews_dir(root: &Path, repo: &RepoId) -> PathBuf {
    repo_dir(root, repo).join("reviews")
}

fn review_dir(root: &Path, repo: &RepoId, review: &ReviewId) -> PathBuf {
    reviews_dir(root, repo).join(review.as_str())
}

fn review_manifest_path(root: &Path, repo: &RepoId, review: &ReviewId) -> PathBuf {
    review_dir(root, repo, review).join("review.toml")
}

fn sessions_dir(root: &Path, repo: &RepoId, review: &ReviewId) -> PathBuf {
    review_dir(root, repo, review).join("sessions")
}

fn author_dir(root: &Path, repo: &RepoId, review: &ReviewId, author: &Author) -> PathBuf {
    sessions_dir(root, repo, review).join(author.as_str())
}

fn session_dir(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    author: &Author,
    session: &SessionId,
) -> PathBuf {
    author_dir(root, repo, review, author).join(session.as_str())
}

fn session_manifest_path(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    author: &Author,
    session: &SessionId,
) -> PathBuf {
    session_dir(root, repo, review, author, session).join("session.toml")
}

fn comments_dir(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    author: &Author,
    session: &SessionId,
) -> PathBuf {
    session_dir(root, repo, review, author, session).join("comments")
}

fn responses_dir(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    author: &Author,
    session: &SessionId,
) -> PathBuf {
    session_dir(root, repo, review, author, session).join("responses")
}

// ---- writers ------------------------------------------------------------

async fn write_repo_manifest(root: &Path, manifest: &RepoManifest) -> Result<()> {
    ensure_repo_id(&manifest.repo_id)?;
    write_toml(&repo_manifest_path(root, &manifest.repo_id), manifest).await
}

async fn write_review_manifest(
    root: &Path,
    repo: &RepoId,
    manifest: &ReviewManifest,
) -> Result<()> {
    ensure_repo_id(repo)?;
    ensure_review_id(&manifest.review_id)?;
    write_toml(&review_manifest_path(root, repo, &manifest.review_id), manifest).await
}

async fn write_session_manifest(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    session: &Session,
) -> Result<()> {
    ensure_repo_id(repo)?;
    ensure_review_id(review)?;
    ensure_author(&session.author)?;
    ensure_session_id(&session.session_id)?;
    write_toml(
        &session_manifest_path(root, repo, review, &session.author, &session.session_id),
        session,
    )
    .await
}

async fn write_comment(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    author: &Author,
    comment: &Comment,
) -> Result<()> {
    ensure_comment_id(&comment.comment_id)?;
    let path = comments_dir(root, repo, review, author, &comment.session_id)
        .join(format!("{}.md", comment.comment_id));
    write_markdown(&path, comment, &comment.body).await
}

async fn write_response(
    root: &Path,
    repo: &RepoId,
    review: &ReviewId,
    author: &Author,
    response: &Response,
) -> Result<()> {
    ensure_response_id(&response.response_id)?;
    let path = responses_dir(root, repo, review, author, &response.session_id)
        .join(format!("{}.md", response.response_id));
    write_markdown(&path, response, &response.body).await
}

// ---- low-level I/O ------------------------------------------------------

async fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string(value).map_err(|e| Error::Toml {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    atomic_write(path, text.as_bytes()).await
}

async fn write_markdown<T: Serialize>(path: &Path, frontmatter: &T, body: &str) -> Result<()> {
    let encoded = frontmatter::encode(frontmatter, body)?;
    atomic_write(path, encoded.as_bytes()).await
}

async fn read_optional_toml<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let text = std::str::from_utf8(&bytes).map_err(|e| Error::Toml {
                path: path.to_path_buf(),
                message: format!("file is not utf-8: {e}"),
            })?;
            toml::from_str(text).map(Some).map_err(|e| Error::Toml {
                path: path.to_path_buf(),
                message: e.to_string(),
            })
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn read_markdown_dir<T, F>(dir: &Path, finalize: F) -> Result<Vec<T>>
where
    T: DeserializeOwned,
    F: Fn(T, String) -> T,
{
    let mut out = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(source) => {
            return Err(Error::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let bytes = tokio::fs::read(&path).await.map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let text = String::from_utf8(bytes).map_err(|e| Error::Frontmatter {
            path: path.clone(),
            detail: format!("file is not utf-8: {e}"),
        })?;
        let (value, body) = frontmatter::decode::<T>(&path, &text)?;
        out.push(finalize(value, body));
    }
    Ok(out)
}

async fn list_subdirs(parent: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut entries = match tokio::fs::read_dir(parent).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(source) => {
            return Err(Error::Io {
                path: parent.to_path_buf(),
                source,
            });
        }
    };
    while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })? {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .await
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?
            .is_dir();
        if is_dir {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn dir_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Io {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidData, "non-utf8 directory name"),
        })
}

async fn exists(path: &Path) -> Result<bool> {
    match tokio::fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn create_dir_all(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// Write `contents` to `path` via a sibling temp file + rename, so a
/// crashed or partial write never leaves an inconsistent file on disk.
async fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    create_dir_all(parent).await?;
    let temp = path.with_file_name(format!(
        "{}.tmp.{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("write"),
        uuid_v7().simple()
    ));
    tokio::fs::write(&temp, contents)
        .await
        .map_err(|source| Error::Io {
            path: temp.clone(),
            source,
        })?;
    if let Err(source) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(Error::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}
