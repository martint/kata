//! Filesystem implementation of [`Storage`].
//!
//! Layout:
//!
//! ```text
//! $root/
//!   {repo-id}/
//!     repo.toml
//!     reviews/
//!       {review-id}/
//!         review.toml
//!         sessions/
//!           {author}/
//!             {session-id}/
//!               session.toml
//!               comments/{comment-id}.md
//!               responses/{response-id}.md
//! ```

use std::io;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use kata_core::{
    Author, Comment, CommentId, RepoId, RepoManifest, Response, ResponseId, ReviewId,
    ReviewManifest, SCHEMA_VERSION, Session, SessionId, SessionStatus,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Error, Result};
use crate::frontmatter;
use crate::ids::{
    ensure_author, ensure_comment_id, ensure_repo_id, ensure_response_id, ensure_review_id,
    ensure_session_id, new_session_id,
};
use crate::storage::{DraftsView, ReviewSummary, Storage};

pub struct FilesystemStorage {
    root: PathBuf,
}

impl FilesystemStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // ---- path layout ----------------------------------------------------

    fn repo_dir(&self, repo: &RepoId) -> PathBuf {
        self.root.join(repo.as_str())
    }
    fn repo_manifest_path(&self, repo: &RepoId) -> PathBuf {
        self.repo_dir(repo).join("repo.toml")
    }
    fn reviews_dir(&self, repo: &RepoId) -> PathBuf {
        self.repo_dir(repo).join("reviews")
    }
    fn review_dir(&self, repo: &RepoId, review: &ReviewId) -> PathBuf {
        self.reviews_dir(repo).join(review.as_str())
    }
    fn review_manifest_path(&self, repo: &RepoId, review: &ReviewId) -> PathBuf {
        self.review_dir(repo, review).join("review.toml")
    }
    fn sessions_dir(&self, repo: &RepoId, review: &ReviewId) -> PathBuf {
        self.review_dir(repo, review).join("sessions")
    }
    fn author_dir(&self, repo: &RepoId, review: &ReviewId, author: &Author) -> PathBuf {
        self.sessions_dir(repo, review).join(author.as_str())
    }
    fn session_dir(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        session: &SessionId,
    ) -> PathBuf {
        self.author_dir(repo, review, author).join(session.as_str())
    }
    fn session_manifest_path(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        session: &SessionId,
    ) -> PathBuf {
        self.session_dir(repo, review, author, session)
            .join("session.toml")
    }
    fn comments_dir(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        session: &SessionId,
    ) -> PathBuf {
        self.session_dir(repo, review, author, session).join("comments")
    }
    fn responses_dir(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        session: &SessionId,
    ) -> PathBuf {
        self.session_dir(repo, review, author, session).join("responses")
    }

    // ---- low-level I/O --------------------------------------------------

    async fn write_toml<T: Serialize>(&self, path: &Path, value: &T) -> Result<()> {
        let text = toml::to_string(value).map_err(|e| Error::Toml {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        atomic_write(path, text.as_bytes()).await
    }

    async fn read_toml<T: DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let bytes = tokio::fs::read(path).await.map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::Toml {
            path: path.to_path_buf(),
            message: format!("file is not utf-8: {e}"),
        })?;
        toml::from_str(text).map_err(|e| Error::Toml {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    async fn read_toml_optional<T: DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
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

    async fn write_markdown<T: Serialize>(
        &self,
        path: &Path,
        frontmatter: &T,
        body: &str,
    ) -> Result<()> {
        let encoded = frontmatter::encode(frontmatter, body)?;
        atomic_write(path, encoded.as_bytes()).await
    }

    // ---- session lookup -------------------------------------------------

    async fn locate_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<(Author, Session)> {
        let sessions_root = self.sessions_dir(repo, review);
        let mut authors = match tokio::fs::read_dir(&sessions_root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::NotFound {
                    what: format!("session {session}"),
                });
            }
            Err(source) => {
                return Err(Error::Io {
                    path: sessions_root,
                    source,
                });
            }
        };
        while let Some(entry) = authors.next_entry().await.map_err(|source| Error::Io {
            path: sessions_root.clone(),
            source,
        })? {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let author = Author::new(name);
            let manifest_path = self.session_manifest_path(repo, review, &author, session);
            if let Some(s) = self.read_toml_optional::<Session>(&manifest_path).await? {
                return Ok((author, s));
            }
        }
        Err(Error::NotFound {
            what: format!("session {session}"),
        })
    }

    async fn ensure_draft(&self, session: &Session) -> Result<()> {
        match session.status {
            SessionStatus::Draft => Ok(()),
            SessionStatus::Published => Err(Error::SessionState {
                session: session.session_id.to_string(),
                state: "published",
                expected: "draft",
            }),
            SessionStatus::Discarded => Err(Error::SessionState {
                session: session.session_id.to_string(),
                state: "discarded",
                expected: "draft",
            }),
        }
    }

    async fn find_open_draft(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<Option<Session>> {
        let dir = self.author_dir(repo, review, author);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })? {
            let path = entry.path().join("session.toml");
            let Some(session) = self.read_toml_optional::<Session>(&path).await? else {
                continue;
            };
            if matches!(session.status, SessionStatus::Draft) {
                return Ok(Some(session));
            }
        }
        Ok(None)
    }

    async fn list_session_dirs(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<(Author, SessionId)>> {
        let sessions_root = self.sessions_dir(repo, review);
        let mut out = Vec::new();
        let mut authors = match tokio::fs::read_dir(&sessions_root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(source) => {
                return Err(Error::Io {
                    path: sessions_root,
                    source,
                });
            }
        };
        while let Some(author_entry) = authors.next_entry().await.map_err(|source| Error::Io {
            path: sessions_root.clone(),
            source,
        })? {
            let Some(author_name) = author_entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let author = Author::new(author_name);
            let author_dir = author_entry.path();
            let mut sessions = tokio::fs::read_dir(&author_dir).await.map_err(|source| {
                Error::Io {
                    path: author_dir.clone(),
                    source,
                }
            })?;
            while let Some(s) = sessions.next_entry().await.map_err(|source| Error::Io {
                path: author_dir.clone(),
                source,
            })? {
                let Some(name) = s.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                out.push((author.clone(), SessionId::new(name)));
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl Storage for FilesystemStorage {
    async fn ensure_repo(&self, manifest: &RepoManifest) -> Result<()> {
        ensure_repo_id(&manifest.repo_id)?;
        let dir = self.repo_dir(&manifest.repo_id);
        create_dir_all(&dir).await?;
        let manifest_path = self.repo_manifest_path(&manifest.repo_id);
        if tokio::fs::try_exists(&manifest_path)
            .await
            .map_err(|source| Error::Io {
                path: manifest_path.clone(),
                source,
            })?
        {
            return Ok(());
        }
        self.write_toml(&manifest_path, manifest).await
    }

    async fn open_repo(&self, repo: &RepoId) -> Result<Option<RepoManifest>> {
        ensure_repo_id(repo)?;
        self.read_toml_optional(&self.repo_manifest_path(repo)).await
    }

    async fn list_reviews(&self, repo: &RepoId) -> Result<Vec<ReviewSummary>> {
        ensure_repo_id(repo)?;
        let dir = self.reviews_dir(repo);
        let mut out = Vec::new();
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })? {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let review_id = ReviewId::new(name);
            let manifest_path = self.review_manifest_path(repo, &review_id);
            let Some(manifest) = self.read_toml_optional::<ReviewManifest>(&manifest_path).await?
            else {
                continue;
            };
            let sessions = self.list_session_dirs(repo, &review_id).await?;
            let published = self.list_published_comments(repo, &review_id).await?;
            out.push(ReviewSummary {
                manifest,
                session_count: sessions.len(),
                published_comment_count: published.len(),
            });
        }
        Ok(out)
    }

    async fn open_review(&self, repo: &RepoId, review: &ReviewId) -> Result<ReviewManifest> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let path = self.review_manifest_path(repo, review);
        match self.read_toml_optional(&path).await? {
            Some(m) => Ok(m),
            None => Err(Error::NotFound {
                what: format!("review {review}"),
            }),
        }
    }

    async fn create_review(&self, repo: &RepoId, manifest: &ReviewManifest) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&manifest.review_id)?;
        let dir = self.review_dir(repo, &manifest.review_id);
        let path = self.review_manifest_path(repo, &manifest.review_id);
        if tokio::fs::try_exists(&path).await.map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })? {
            return Err(Error::ReviewExists {
                review: manifest.review_id.to_string(),
            });
        }
        create_dir_all(&dir).await?;
        self.write_toml(&path, manifest).await
    }

    async fn update_review(&self, repo: &RepoId, manifest: &ReviewManifest) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&manifest.review_id)?;
        let path = self.review_manifest_path(repo, &manifest.review_id);
        if !tokio::fs::try_exists(&path).await.map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })? {
            return Err(Error::NotFound {
                what: format!("review {}", manifest.review_id),
            });
        }
        self.write_toml(&path, manifest).await
    }

    async fn open_or_create_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<Session> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        ensure_author(author)?;
        if let Some(existing) = self.find_open_draft(repo, review, author).await? {
            return Ok(existing);
        }
        let session_id = new_session_id();
        let session = Session {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.clone(),
            review_id: review.clone(),
            author: author.clone(),
            status: SessionStatus::Draft,
            created_at: Utc::now(),
            published_at: None,
        };
        let dir = self.session_dir(repo, review, author, &session_id);
        create_dir_all(&dir).await?;
        create_dir_all(&self.comments_dir(repo, review, author, &session_id)).await?;
        create_dir_all(&self.responses_dir(repo, review, author, &session_id)).await?;
        let manifest_path = self.session_manifest_path(repo, review, author, &session_id);
        self.write_toml(&manifest_path, &session).await?;
        Ok(session)
    }

    async fn publish_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<()> {
        ensure_session_id(session)?;
        let (author, mut s) = self.locate_session(repo, review, session).await?;
        self.ensure_draft(&s).await?;
        s.status = SessionStatus::Published;
        s.published_at = Some(Utc::now());
        let path = self.session_manifest_path(repo, review, &author, session);
        self.write_toml(&path, &s).await
    }

    async fn discard_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<()> {
        ensure_session_id(session)?;
        let (author, mut s) = self.locate_session(repo, review, session).await?;
        self.ensure_draft(&s).await?;
        s.status = SessionStatus::Discarded;
        let path = self.session_manifest_path(repo, review, &author, session);
        self.write_toml(&path, &s).await
    }

    async fn upsert_draft_comment(&self, repo: &RepoId, comment: &Comment) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&comment.review_id)?;
        ensure_author(&comment.author)?;
        ensure_session_id(&comment.session_id)?;
        ensure_comment_id(&comment.comment_id)?;
        let session_path = self.session_manifest_path(
            repo,
            &comment.review_id,
            &comment.author,
            &comment.session_id,
        );
        let session: Session = self.read_toml(&session_path).await?;
        self.ensure_draft(&session).await?;
        let dir =
            self.comments_dir(repo, &comment.review_id, &comment.author, &comment.session_id);
        create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.md", comment.comment_id));
        self.write_markdown(&path, comment, &comment.body).await
    }

    async fn discard_draft_comment(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        comment: &CommentId,
    ) -> Result<()> {
        ensure_comment_id(comment)?;
        let (author, s) = self.locate_session(repo, review, session).await?;
        self.ensure_draft(&s).await?;
        let path = self
            .comments_dir(repo, review, &author, session)
            .join(format!("{comment}.md"));
        remove_if_exists(&path).await
    }

    async fn upsert_draft_response(&self, repo: &RepoId, response: &Response) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_session_id(&response.session_id)?;
        ensure_response_id(&response.response_id)?;
        ensure_author(&response.author)?;
        // Responses don't carry review_id directly; resolve via the session
        // manifest under the author's directory.
        let (_, session) = self
            .locate_session_by_author(repo, &response.author, &response.session_id)
            .await?;
        ensure_review_id(&session.review_id)?;
        self.ensure_draft(&session).await?;
        let dir = self.responses_dir(
            repo,
            &session.review_id,
            &response.author,
            &response.session_id,
        );
        create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.md", response.response_id));
        self.write_markdown(&path, response, &response.body).await
    }

    async fn discard_draft_response(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        response: &ResponseId,
    ) -> Result<()> {
        ensure_response_id(response)?;
        let (author, s) = self.locate_session(repo, review, session).await?;
        self.ensure_draft(&s).await?;
        let path = self
            .responses_dir(repo, review, &author, session)
            .join(format!("{response}.md"));
        remove_if_exists(&path).await
    }

    async fn list_published_comments(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Comment>> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let mut out = Vec::new();
        for (author, session_id) in self.list_session_dirs(repo, review).await? {
            let manifest_path = self.session_manifest_path(repo, review, &author, &session_id);
            let Some(session) = self.read_toml_optional::<Session>(&manifest_path).await? else {
                continue;
            };
            if !matches!(session.status, SessionStatus::Published) {
                continue;
            }
            let comments_dir = self.comments_dir(repo, review, &author, &session_id);
            read_markdown_dir(&comments_dir, &mut out, |c: Comment, body| Comment {
                body,
                ..c
            })
            .await?;
        }
        Ok(out)
    }

    async fn list_published_responses(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Response>> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let mut out = Vec::new();
        for (author, session_id) in self.list_session_dirs(repo, review).await? {
            let manifest_path = self.session_manifest_path(repo, review, &author, &session_id);
            let Some(session) = self.read_toml_optional::<Session>(&manifest_path).await? else {
                continue;
            };
            if !matches!(session.status, SessionStatus::Published) {
                continue;
            }
            let responses_dir = self.responses_dir(repo, review, &author, &session_id);
            read_markdown_dir(&responses_dir, &mut out, |r: Response, body| Response {
                body,
                ..r
            })
            .await?;
        }
        Ok(out)
    }

    async fn list_drafts_for(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<DraftsView> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        ensure_author(author)?;
        let Some(session) = self.find_open_draft(repo, review, author).await? else {
            return Ok(DraftsView::default());
        };
        let comments_dir = self.comments_dir(repo, review, author, &session.session_id);
        let responses_dir = self.responses_dir(repo, review, author, &session.session_id);
        let mut comments = Vec::new();
        read_markdown_dir(&comments_dir, &mut comments, |c: Comment, body| Comment {
            body,
            ..c
        })
        .await?;
        let mut responses = Vec::new();
        read_markdown_dir(&responses_dir, &mut responses, |r: Response, body| Response {
            body,
            ..r
        })
        .await?;
        Ok(DraftsView {
            session: Some(session),
            comments,
            responses,
        })
    }
}

impl FilesystemStorage {
    /// Variant of `locate_session` scoped to a single author. We scan that
    /// author's sessions across reviews because [`Response`] doesn't carry
    /// `review_id` directly.
    async fn locate_session_by_author(
        &self,
        repo: &RepoId,
        author: &Author,
        session: &SessionId,
    ) -> Result<(Author, Session)> {
        let reviews_root = self.reviews_dir(repo);
        let mut reviews = match tokio::fs::read_dir(&reviews_root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::NotFound {
                    what: format!("session {session}"),
                });
            }
            Err(source) => {
                return Err(Error::Io {
                    path: reviews_root,
                    source,
                });
            }
        };
        while let Some(entry) = reviews.next_entry().await.map_err(|source| Error::Io {
            path: reviews_root.clone(),
            source,
        })? {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let review_id = ReviewId::new(name);
            let manifest_path = self.session_manifest_path(repo, &review_id, author, session);
            if let Some(s) = self.read_toml_optional::<Session>(&manifest_path).await? {
                return Ok((author.clone(), s));
            }
        }
        Err(Error::NotFound {
            what: format!("session {session}"),
        })
    }
}

// ---- file helpers ------------------------------------------------------

async fn create_dir_all(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
}

async fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    create_dir_all(parent).await?;
    let temp = path.with_file_name(format!(
        "{}.tmp.{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("write"),
        crate::ids::uuid_v7().simple()
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

async fn remove_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

async fn read_markdown_dir<T, F>(dir: &Path, out: &mut Vec<T>, finalize: F) -> Result<()>
where
    T: DeserializeOwned,
    F: Fn(T, String) -> T,
{
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
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
    Ok(())
}
