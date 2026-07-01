//! `Storage` implementation backed by SQLite.
//!
//! One file on disk per Kata workspace, WAL journal mode, one shared
//! [`rusqlite::Connection`] guarded by a `std::sync::Mutex`. Every trait
//! method shells the SQL out to a [`tokio::task::spawn_blocking`] worker so
//! the async runtime stays unblocked. We could pull in tokio-rusqlite for
//! a thinner wrapper, but it adds a dep doing the same dance underneath —
//! `spawn_blocking` is what tokio-rusqlite uses internally.
//!
//! Multi-step operations that need all-or-nothing semantics
//! (`open_or_create_session`, `update_review`, the comment/response
//! upserts that gate on session state) wrap themselves in
//! `BEGIN IMMEDIATE` transactions. WAL means lock contention happens only
//! between writers; readers proceed concurrently against the most recent
//! committed state.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use kata_core::{
    Annotation, AnnotationId, ApiToken, ApiTokenId, Author, ChangeId, ColumnRange, Comment,
    CommentId, CommitId, LineRange, OpId, RepoId, RepoManifest, Response, ResponseId, ReviewId,
    ReviewManifest, RevSet, SCHEMA_VERSION, Session, SessionId, SessionStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::error::{Error, Result};
use crate::ids::{
    ensure_annotation_id, ensure_author, ensure_comment_id, ensure_repo_id, ensure_response_id,
    ensure_review_id, ensure_session_id, new_session_id,
};
use crate::sqlite::migrate;
use crate::sqlite::serde_enums::{
    action_from_str, action_to_str, flag_from_str, flag_to_str, session_status_to_str,
    side_from_str, side_to_str,
};
use crate::storage::{DraftsView, ReviewSummary, ReviewVisit, Storage};

pub struct SqliteStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    /// Open the SQLite file at `path`, applying any pending migrations.
    /// The file is created if it doesn't exist; its parent directory must
    /// already exist (callers typically pass `<KATA_DATA>/kata.db`).
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            let mut conn = Connection::open(&path)?;
            // WAL: lets multiple readers proceed in parallel with one
            // writer. The 5s busy_timeout means a contended writer waits
            // up to that long for the lock rather than failing fast — at
            // our scale "contended" means two agents racing for the same
            // review's metadata, and 5s is generous.
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = NORMAL;",
            )?;
            migrate::run(&mut conn)?;
            Ok(conn)
        })
        .await
        .expect("blocking task panicked")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database. Each call creates a fresh DB; useful
    /// for tests.
    pub async fn open_in_memory() -> Result<Self> {
        let conn = tokio::task::spawn_blocking(|| -> Result<Connection> {
            let mut conn = Connection::open_in_memory()?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            migrate::run(&mut conn)?;
            Ok(conn)
        })
        .await
        .expect("blocking task panicked")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `f` on the shared connection inside a `spawn_blocking` worker.
    /// All trait methods bottom out here.
    async fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().expect("sqlite mutex poisoned");
            f(&mut conn)
        })
        .await
        .expect("blocking task panicked")
    }

    // ---- archive (export/import) helpers --------------------------------
    //
    // These read/write without the normal lifecycle gating that the
    // `Storage` trait imposes (`upsert_draft_comment` checks the session
    // is in draft state, `open_or_create_session` allocates a new id,
    // and so on). The export/import path needs to copy the store as-is:
    // a published comment imports back as published, a discarded
    // session preserves its id, etc.

    /// Every repo registered in the database, in insertion order.
    pub async fn list_all_repos(&self) -> Result<Vec<RepoManifest>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT repo_id, canonical_path, schema_version FROM repos ORDER BY created_at",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(RepoManifest {
                    repo_id: RepoId::new(row.get::<_, String>(0)?),
                    canonical_path: row.get(1)?,
                    schema_version: row.get(2)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    /// Every session for a review, regardless of `status`. The
    /// trait-level `list_drafts_for` filters to one author's drafts and
    /// `list_published_*` filter to published sessions — neither
    /// surfaces discarded sessions, which the archive needs to round-trip.
    pub async fn list_all_sessions(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Session>> {
        let repo_str = repo.as_str().to_owned();
        let review_clone = review.clone();
        let review_str = review.as_str().to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT session_id, schema_version, author, status, created_at, published_at
                 FROM sessions WHERE repo_id = ?1 AND review_id = ?2
                 ORDER BY created_at",
            )?;
            let rows = stmt.query_map(params![repo_str, review_str], |row| {
                let status_str: String = row.get(3)?;
                let status = crate::sqlite::serde_enums::session_status_to_str_inverse(&status_str)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                Ok(Session {
                    session_id: SessionId::new(row.get::<_, String>(0)?),
                    schema_version: row.get(1)?,
                    review_id: review_clone.clone(),
                    author: Author::new(row.get::<_, String>(2)?),
                    status,
                    created_at: row.get(4)?,
                    published_at: row.get(5)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    /// Every comment under one session — including comments under
    /// discarded sessions, which the trait-level read paths filter out.
    pub async fn list_all_comments_for_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<Comment>> {
        let session_str = session.as_str().to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT comment_id, session_id, review_id, schema_version, author,
                        created_at, patchset, anchor_change_id, anchor_commit_id,
                        file, side, line_start, line_end, col_start, col_end, review_wide,
                        flag, body, external_author
                 FROM comments WHERE session_id = ?1 ORDER BY created_at",
            )?;
            let rows = stmt.query_map(params![session_str], comment_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    /// Every response under one session. Counterpart to
    /// [`Self::list_all_comments_for_session`].
    pub async fn list_all_responses_for_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<Response>> {
        let session_str = session.as_str().to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT response_id, in_reply_to, session_id, schema_version, author,
                        created_at, action, body
                 FROM responses WHERE session_id = ?1 ORDER BY created_at",
            )?;
            let rows = stmt.query_map(params![session_str], response_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    /// Insert a session at its archive-preserved status (`published`,
    /// `discarded`, or `draft`), keeping the original id. Used by the
    /// import path; not on the `Storage` trait because the normal
    /// caller goes through `open_or_create_session` which always
    /// allocates a fresh draft.
    pub async fn raw_insert_session(
        &self,
        repo: &RepoId,
        session: &Session,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&session.review_id)?;
        ensure_session_id(&session.session_id)?;
        ensure_author(&session.author)?;
        let repo_str = repo.as_str().to_owned();
        let session = session.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO sessions (session_id, repo_id, review_id, schema_version, author,
                                       status, created_at, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session.session_id.as_str(),
                    repo_str,
                    session.review_id.as_str(),
                    session.schema_version,
                    session.author.as_str(),
                    session_status_to_str(session.status),
                    session.created_at,
                    session.published_at,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Insert a comment at its archive-preserved content. Bypasses the
    /// "session must be in draft" check that `upsert_draft_comment`
    /// uses — the archive can hold comments under any session status.
    pub async fn raw_insert_comment(
        &self,
        repo: &RepoId,
        comment: &Comment,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&comment.review_id)?;
        ensure_session_id(&comment.session_id)?;
        ensure_comment_id(&comment.comment_id)?;
        let repo_str = repo.as_str().to_owned();
        let comment = comment.clone();
        self.with_conn(move |conn| exec_insert_comment(conn, &repo_str, &comment))
            .await
    }

    /// Insert a response at its archive-preserved content. Counterpart
    /// to [`Self::raw_insert_comment`].
    pub async fn raw_insert_response(
        &self,
        repo: &RepoId,
        response: &Response,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_session_id(&response.session_id)?;
        ensure_response_id(&response.response_id)?;
        let repo_str = repo.as_str().to_owned();
        let response = response.clone();
        self.with_conn(move |conn| exec_insert_response(conn, &repo_str, &response))
            .await
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    // ---- repo manifest --------------------------------------------------

    async fn ensure_repo(&self, manifest: &RepoManifest) -> Result<()> {
        ensure_repo_id(&manifest.repo_id)?;
        let manifest = manifest.clone();
        let now = Utc::now();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO repos (repo_id, canonical_path, schema_version, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    manifest.repo_id.as_str(),
                    manifest.canonical_path,
                    manifest.schema_version,
                    now,
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn open_repo(&self, repo: &RepoId) -> Result<Option<RepoManifest>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT repo_id, canonical_path, schema_version
                 FROM repos WHERE repo_id = ?1",
                params![repo_str],
                |row| {
                    Ok(RepoManifest {
                        repo_id: RepoId::new(row.get::<_, String>(0)?),
                        canonical_path: row.get(1)?,
                        schema_version: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Error::from)
        })
        .await
    }

    // ---- reviews --------------------------------------------------------

    async fn list_reviews(&self, repo: &RepoId) -> Result<Vec<ReviewSummary>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        self.with_conn(move |conn| {
            // One query gets every review's manifest plus its session
            // count and published-comment count via correlated
            // subqueries. The FS impl had to walk session dirs and read
            // every session.toml to do this; here it's all indexed.
            let mut stmt = conn.prepare(
                "SELECT
                    r.review_id, r.number, r.name, r.schema_version, r.revset, r.bookmark,
                    r.summary, r.created_by, r.created_at, r.current_patchset, r.patchsets_json,
                    r.archived_at, r.github_pr,
                    (SELECT COUNT(*) FROM sessions s
                     WHERE s.repo_id = r.repo_id AND s.review_id = r.review_id) AS session_count,
                    (SELECT COUNT(*) FROM comments c
                     JOIN sessions s ON s.session_id = c.session_id
                     WHERE c.repo_id = r.repo_id AND c.review_id = r.review_id
                       AND s.status = 'published') AS published_comment_count
                 FROM reviews r
                 WHERE r.repo_id = ?1
                 ORDER BY r.number DESC",
            )?;
            let rows = stmt.query_map(params![repo_str], |row| {
                let session_count: i64 = row.get(13)?;
                let comment_count: i64 = row.get(14)?;
                let manifest = review_manifest_from_row(row)?;
                Ok(ReviewSummary {
                    manifest,
                    session_count: session_count as usize,
                    published_comment_count: comment_count as usize,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    async fn resolve_review_number(
        &self,
        repo: &RepoId,
        number: u32,
    ) -> Result<Option<ReviewId>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT review_id FROM reviews WHERE repo_id = ?1 AND number = ?2",
                params![repo_str, number],
                |row| row.get::<_, String>(0).map(ReviewId::new),
            )
            .optional()
            .map_err(Error::from)
        })
        .await
    }

    async fn open_review(&self, repo: &RepoId, review: &ReviewId) -> Result<ReviewManifest> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        let review_for_err = review.clone();
        self.with_conn(move |conn| {
            let opt = conn
                .query_row(
                    "SELECT review_id, number, name, schema_version, revset, bookmark, summary,
                            created_by, created_at, current_patchset, patchsets_json, archived_at,
                            github_pr
                     FROM reviews WHERE repo_id = ?1 AND review_id = ?2",
                    params![repo_str, review_str],
                    review_manifest_from_row,
                )
                .optional()?;
            match opt {
                Some(m) => Ok(m),
                None => Err(Error::NotFound {
                    what: format!("review {review_for_err}"),
                }),
            }
        })
        .await
    }

    async fn create_review(
        &self,
        repo: &RepoId,
        manifest: &ReviewManifest,
    ) -> Result<ReviewManifest> {
        ensure_repo_id(repo)?;
        ensure_review_id(&manifest.review_id)?;
        let repo_str = repo.as_str().to_owned();
        let mut manifest = manifest.clone();
        self.with_conn(move |conn| {
            let patchsets_json =
                serde_json::to_string(&manifest.patchsets).map_err(|source| Error::Json {
                    context: "patchsets".into(),
                    source,
                })?;
            // Assign a fresh per-repo number inside the same write
            // transaction as the INSERT, so two concurrent creates can't
            // pick the same number. `manifest.number > 0` means a caller
            // (the archive importer) supplied an explicit one; honour
            // it so round-tripping preserves URLs.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if manifest.number == 0 {
                let next: u32 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(number), 0) + 1 FROM reviews WHERE repo_id = ?1",
                        params![repo_str],
                        |row| row.get(0),
                    )?;
                manifest.number = next;
            }
            // Empty name → default to bookmark or review_id, in that
            // order. Older archives may not have carried a name at all.
            if manifest.name.is_empty() {
                manifest.name = manifest
                    .bookmark
                    .clone()
                    .unwrap_or_else(|| manifest.review_id.as_str().to_owned());
            }
            let github_pr_json = match &manifest.github_pr {
                Some(p) => Some(serde_json::to_string(p).map_err(|source| Error::Json {
                    context: "github_pr".into(),
                    source,
                })?),
                None => None,
            };
            let res = tx.execute(
                "INSERT INTO reviews
                    (repo_id, review_id, number, name, schema_version, revset, bookmark, summary,
                     created_by, created_at, current_patchset, patchsets_json, archived_at,
                     github_pr)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    repo_str,
                    manifest.review_id.as_str(),
                    manifest.number,
                    manifest.name,
                    manifest.schema_version,
                    manifest.revset.as_str(),
                    manifest.bookmark,
                    manifest.summary,
                    manifest.created_by.as_str(),
                    manifest.created_at,
                    manifest.current_patchset,
                    patchsets_json,
                    manifest.archived_at,
                    github_pr_json,
                ],
            );
            match res {
                Ok(_) => {
                    tx.commit()?;
                    Ok(manifest)
                }
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Err(Error::ReviewExists {
                        review: manifest.review_id.to_string(),
                    })
                }
                Err(e) => Err(Error::from(e)),
            }
        })
        .await
    }

    async fn update_review(&self, repo: &RepoId, manifest: &ReviewManifest) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&manifest.review_id)?;
        let repo_str = repo.as_str().to_owned();
        let manifest = manifest.clone();
        self.with_conn(move |conn| {
            let patchsets_json =
                serde_json::to_string(&manifest.patchsets).map_err(|source| Error::Json {
                    context: "patchsets".into(),
                    source,
                })?;
            let github_pr_json = match &manifest.github_pr {
                Some(p) => Some(serde_json::to_string(p).map_err(|source| Error::Json {
                    context: "github_pr".into(),
                    source,
                })?),
                None => None,
            };
            let affected = conn.execute(
                "UPDATE reviews
                    SET name = ?3,
                        schema_version = ?4,
                        revset = ?5,
                        bookmark = ?6,
                        summary = ?7,
                        created_by = ?8,
                        created_at = ?9,
                        current_patchset = ?10,
                        patchsets_json = ?11,
                        archived_at = ?12,
                        github_pr = ?13
                  WHERE repo_id = ?1 AND review_id = ?2",
                params![
                    repo_str,
                    manifest.review_id.as_str(),
                    manifest.name,
                    manifest.schema_version,
                    manifest.revset.as_str(),
                    manifest.bookmark,
                    manifest.summary,
                    manifest.created_by.as_str(),
                    manifest.created_at,
                    manifest.current_patchset,
                    patchsets_json,
                    manifest.archived_at,
                    github_pr_json,
                ],
            )?;
            if affected == 0 {
                return Err(Error::NotFound {
                    what: format!("review {}", manifest.review_id),
                });
            }
            Ok(())
        })
        .await
    }

    async fn delete_review(&self, repo: &RepoId, review: &ReviewId) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        self.with_conn(move |conn| {
            // annotations don't have a CASCADE FK back to reviews
            // (see V008__annotations.sql), so wipe them explicitly.
            // Everything else (sessions, comments, responses,
            // review_visits) is reached via the cascading FKs declared
            // in V001/V004.
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM annotations WHERE repo_id = ?1 AND review_id = ?2",
                params![repo_str, review_str],
            )?;
            tx.execute(
                "DELETE FROM reviews WHERE repo_id = ?1 AND review_id = ?2",
                params![repo_str, review_str],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    // ---- sessions -------------------------------------------------------

    async fn open_or_create_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<Session> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        ensure_author(author)?;
        let repo_str = repo.as_str().to_owned();
        let review_clone = review.clone();
        let review_str = review.as_str().to_owned();
        let author_clone = author.clone();
        let author_str = author.as_str().to_owned();
        self.with_conn(move |conn| {
            // BEGIN IMMEDIATE acquires a write lock up front, so the
            // SELECT and INSERT can't be raced by a concurrent agent
            // running the same operation against the same author/review.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing = tx
                .query_row(
                    "SELECT session_id, schema_version, created_at, published_at
                     FROM sessions
                     WHERE repo_id = ?1 AND review_id = ?2 AND author = ?3 AND status = 'draft'",
                    params![repo_str, review_str, author_str],
                    |row| {
                        Ok(Session {
                            session_id: SessionId::new(row.get::<_, String>(0)?),
                            schema_version: row.get(1)?,
                            review_id: review_clone.clone(),
                            author: author_clone.clone(),
                            status: SessionStatus::Draft,
                            created_at: row.get(2)?,
                            published_at: row.get(3)?,
                        })
                    },
                )
                .optional()?;
            if let Some(session) = existing {
                return Ok(session);
            }
            let session_id = new_session_id();
            let now = Utc::now();
            tx.execute(
                "INSERT INTO sessions (session_id, repo_id, review_id, schema_version, author,
                                       status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6)",
                params![
                    session_id.as_str(),
                    repo_str,
                    review_str,
                    SCHEMA_VERSION,
                    author_str,
                    now,
                ],
            )?;
            tx.commit()?;
            Ok(Session {
                schema_version: SCHEMA_VERSION,
                session_id,
                review_id: review_clone,
                author: author_clone,
                status: SessionStatus::Draft,
                created_at: now,
                published_at: None,
            })
        })
        .await
    }

    async fn publish_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<()> {
        flip_session_status(self, repo, review, session, SessionStatus::Published).await
    }

    async fn discard_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<()> {
        flip_session_status(self, repo, review, session, SessionStatus::Discarded).await
    }

    // ---- authoring ------------------------------------------------------

    async fn upsert_draft_comment(&self, repo: &RepoId, comment: &Comment) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&comment.review_id)?;
        ensure_session_id(&comment.session_id)?;
        ensure_comment_id(&comment.comment_id)?;
        let repo_str = repo.as_str().to_owned();
        let comment = comment.clone();
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            require_draft_session(&tx, &comment.session_id)?;
            let (line_start, line_end) = match &comment.lines {
                Some(LineRange { start, end }) => (Some(*start), Some(*end)),
                None => (None, None),
            };
            let (col_start, col_end) = match &comment.columns {
                Some(ColumnRange { start, end }) => (Some(*start), Some(*end)),
                None => (None, None),
            };
            let external_author_json = match &comment.external_author {
                Some(a) => Some(serde_json::to_string(a).map_err(|source| Error::Json {
                    context: "external_author".into(),
                    source,
                })?),
                None => None,
            };
            tx.execute(
                "INSERT INTO comments
                    (comment_id, repo_id, review_id, session_id, schema_version, author,
                     created_at, patchset, anchor_change_id, anchor_commit_id, file, side,
                     line_start, line_end, col_start, col_end, review_wide, flag, body,
                     external_author)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                 ON CONFLICT(comment_id) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    author = excluded.author,
                    created_at = excluded.created_at,
                    patchset = excluded.patchset,
                    anchor_change_id = excluded.anchor_change_id,
                    anchor_commit_id = excluded.anchor_commit_id,
                    file = excluded.file,
                    side = excluded.side,
                    line_start = excluded.line_start,
                    line_end = excluded.line_end,
                    col_start = excluded.col_start,
                    col_end = excluded.col_end,
                    review_wide = excluded.review_wide,
                    flag = excluded.flag,
                    body = excluded.body,
                    external_author = excluded.external_author",
                params![
                    comment.comment_id.as_str(),
                    repo_str,
                    comment.review_id.as_str(),
                    comment.session_id.as_str(),
                    comment.schema_version,
                    comment.author.as_str(),
                    comment.created_at,
                    comment.patchset,
                    comment.anchor_change_id.as_str(),
                    comment.anchor_commit_id.as_str(),
                    comment.file,
                    comment.side.map(side_to_str),
                    line_start,
                    line_end,
                    col_start,
                    col_end,
                    comment.review_wide as i64,
                    flag_to_str(comment.flag),
                    comment.body,
                    external_author_json,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn discard_draft_comment(
        &self,
        _repo: &RepoId,
        _review: &ReviewId,
        _session: &SessionId,
        comment: &CommentId,
    ) -> Result<()> {
        ensure_comment_id(comment)?;
        let comment_str = comment.as_str().to_owned();
        self.with_conn(move |conn| {
            // No `WHERE session_id = ?` — the FS impl just removes the
            // file unconditionally. Caller is responsible for only
            // calling this on comments belonging to a session they own.
            conn.execute(
                "DELETE FROM comments WHERE comment_id = ?1",
                params![comment_str],
            )?;
            Ok(())
        })
        .await
    }

    async fn upsert_draft_response(&self, repo: &RepoId, response: &Response) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_session_id(&response.session_id)?;
        ensure_response_id(&response.response_id)?;
        let repo_str = repo.as_str().to_owned();
        let response = response.clone();
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            require_draft_session(&tx, &response.session_id)?;
            // Look up the target comment's review_id; responses inherit
            // it. We could trust the caller, but having the FK and the
            // review_id stay in sync without the caller having to pass
            // it avoids a class of bug.
            let review_id: String = tx
                .query_row(
                    "SELECT review_id FROM comments WHERE comment_id = ?1",
                    params![response.in_reply_to.as_str()],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| Error::NotFound {
                    what: format!("comment {}", response.in_reply_to),
                })?;
            tx.execute(
                "INSERT INTO responses
                    (response_id, repo_id, review_id, session_id, in_reply_to, schema_version,
                     author, created_at, action, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(response_id) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    author = excluded.author,
                    created_at = excluded.created_at,
                    action = excluded.action,
                    body = excluded.body",
                params![
                    response.response_id.as_str(),
                    repo_str,
                    review_id,
                    response.session_id.as_str(),
                    response.in_reply_to.as_str(),
                    response.schema_version,
                    response.author.as_str(),
                    response.created_at,
                    action_to_str(response.action),
                    response.body,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn discard_draft_response(
        &self,
        _repo: &RepoId,
        _review: &ReviewId,
        _session: &SessionId,
        response: &ResponseId,
    ) -> Result<()> {
        ensure_response_id(response)?;
        let response_str = response.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM responses WHERE response_id = ?1",
                params![response_str],
            )?;
            Ok(())
        })
        .await
    }

    // ---- reading --------------------------------------------------------

    async fn list_published_comments(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Comment>> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT c.comment_id, c.session_id, c.review_id, c.schema_version, c.author,
                        c.created_at, c.patchset, c.anchor_change_id, c.anchor_commit_id,
                        c.file, c.side, c.line_start, c.line_end, c.col_start, c.col_end,
                        c.review_wide, c.flag, c.body, c.external_author
                 FROM comments c
                 JOIN sessions s ON s.session_id = c.session_id
                 WHERE c.repo_id = ?1 AND c.review_id = ?2 AND s.status = 'published'
                 ORDER BY c.created_at",
            )?;
            let rows = stmt.query_map(params![repo_str, review_str], comment_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    async fn get_comment_by_id(
        &self,
        repo: &RepoId,
        comment_id: &CommentId,
    ) -> Result<Option<Comment>> {
        ensure_repo_id(repo)?;
        ensure_comment_id(comment_id)?;
        let repo_str = repo.as_str().to_owned();
        let cid = comment_id.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT comment_id, session_id, review_id, schema_version, author,
                        created_at, patchset, anchor_change_id, anchor_commit_id,
                        file, side, line_start, line_end, col_start, col_end,
                        review_wide, flag, body, external_author
                   FROM comments
                  WHERE repo_id = ?1 AND comment_id = ?2",
                params![repo_str, cid],
                comment_from_row,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }

    async fn list_published_responses(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Response>> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT r.response_id, r.in_reply_to, r.session_id, r.schema_version, r.author,
                        r.created_at, r.action, r.body
                 FROM responses r
                 JOIN sessions s ON s.session_id = r.session_id
                 WHERE r.repo_id = ?1 AND r.review_id = ?2 AND s.status = 'published'
                 ORDER BY r.created_at",
            )?;
            let rows = stmt.query_map(params![repo_str, review_str], response_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    async fn list_annotations(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Annotation>> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT annotation_id, review_id, schema_version, author, created_at, updated_at,
                        patchset, anchor_change_id, anchor_commit_id, file, side, line_start,
                        line_end, body
                 FROM annotations
                 WHERE repo_id = ?1 AND review_id = ?2
                 ORDER BY created_at",
            )?;
            let rows = stmt.query_map(params![repo_str, review_str], annotation_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(Error::from)?);
            }
            Ok(out)
        })
        .await
    }

    async fn upsert_annotation(&self, repo: &RepoId, annotation: &Annotation) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&annotation.review_id)?;
        ensure_annotation_id(&annotation.annotation_id)?;
        let repo_str = repo.as_str().to_owned();
        let annotation = annotation.clone();
        self.with_conn(move |conn| {
            let (line_start, line_end) = match &annotation.lines {
                Some(LineRange { start, end }) => (Some(*start), Some(*end)),
                None => (None, None),
            };
            conn.execute(
                "INSERT INTO annotations
                    (annotation_id, repo_id, review_id, schema_version, author, created_at,
                     updated_at, patchset, anchor_change_id, anchor_commit_id, file, side,
                     line_start, line_end, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(annotation_id) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    updated_at = excluded.updated_at,
                    patchset = excluded.patchset,
                    anchor_change_id = excluded.anchor_change_id,
                    anchor_commit_id = excluded.anchor_commit_id,
                    file = excluded.file,
                    side = excluded.side,
                    line_start = excluded.line_start,
                    line_end = excluded.line_end,
                    body = excluded.body",
                params![
                    annotation.annotation_id.as_str(),
                    repo_str,
                    annotation.review_id.as_str(),
                    annotation.schema_version,
                    annotation.author.as_str(),
                    annotation.created_at,
                    annotation.updated_at,
                    annotation.patchset,
                    annotation.anchor_change_id.as_str(),
                    annotation.anchor_commit_id.as_str(),
                    annotation.file,
                    annotation.side.map(side_to_str),
                    line_start,
                    line_end,
                    annotation.body,
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn delete_annotation(
        &self,
        _repo: &RepoId,
        _review: &ReviewId,
        annotation: &AnnotationId,
    ) -> Result<()> {
        ensure_annotation_id(annotation)?;
        let id_str = annotation.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM annotations WHERE annotation_id = ?1",
                params![id_str],
            )?;
            Ok(())
        })
        .await
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
        let repo_str = repo.as_str().to_owned();
        let review_clone = review.clone();
        let review_str = review.as_str().to_owned();
        let author_clone = author.clone();
        let author_str = author.as_str().to_owned();
        self.with_conn(move |conn| {
            let session = conn
                .query_row(
                    "SELECT session_id, schema_version, created_at, published_at
                     FROM sessions
                     WHERE repo_id = ?1 AND review_id = ?2 AND author = ?3 AND status = 'draft'",
                    params![repo_str, review_str, author_str],
                    |row| {
                        Ok(Session {
                            session_id: SessionId::new(row.get::<_, String>(0)?),
                            schema_version: row.get(1)?,
                            review_id: review_clone.clone(),
                            author: author_clone.clone(),
                            status: SessionStatus::Draft,
                            created_at: row.get(2)?,
                            published_at: row.get(3)?,
                        })
                    },
                )
                .optional()?;
            let Some(session) = session else {
                return Ok(DraftsView::default());
            };

            let mut comment_stmt = conn.prepare(
                "SELECT comment_id, session_id, review_id, schema_version, author,
                        created_at, patchset, anchor_change_id, anchor_commit_id,
                        file, side, line_start, line_end, col_start, col_end, review_wide,
                        flag, body, external_author
                 FROM comments WHERE session_id = ?1 ORDER BY created_at",
            )?;
            let comments: Vec<Comment> = comment_stmt
                .query_map(params![session.session_id.as_str()], comment_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut response_stmt = conn.prepare(
                "SELECT response_id, in_reply_to, session_id, schema_version, author,
                        created_at, action, body
                 FROM responses WHERE session_id = ?1 ORDER BY created_at",
            )?;
            let responses: Vec<Response> = response_stmt
                .query_map(params![session.session_id.as_str()], response_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            Ok(DraftsView {
                session: Some(session),
                comments,
                responses,
            })
        })
        .await
    }

    async fn last_review_visit(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<Option<ReviewVisit>> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        ensure_author(author)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        let author_str = author.as_str().to_owned();
        self.with_conn(move |conn| {
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT op_id, visited_at FROM review_visits
                     WHERE repo_id = ?1 AND review_id = ?2 AND author = ?3",
                    params![repo_str, review_str, author_str],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((op_id, visited_at)) = row else {
                return Ok(None);
            };
            let visited_at = chrono::DateTime::parse_from_rfc3339(&visited_at)
                .map(|t| t.with_timezone(&chrono::Utc))
                .map_err(|_| Error::InvalidId {
                    label: "review_visits.visited_at".into(),
                    value: visited_at,
                    reason: "not a valid RFC 3339 timestamp",
                })?;
            Ok(Some(ReviewVisit {
                op_id: OpId::new(op_id),
                visited_at,
            }))
        })
        .await
    }

    async fn record_review_visit(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        op_id: &OpId,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(review)?;
        ensure_author(author)?;
        let repo_str = repo.as_str().to_owned();
        let review_str = review.as_str().to_owned();
        let author_str = author.as_str().to_owned();
        let op_id_str = op_id.as_str().to_owned();
        let visited_at = chrono::Utc::now().to_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO review_visits (repo_id, review_id, author, op_id, visited_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(repo_id, review_id, author) DO UPDATE SET
                   op_id = excluded.op_id,
                   visited_at = excluded.visited_at",
                params![repo_str, review_str, author_str, op_id_str, visited_at],
            )?;
            Ok(())
        })
        .await
    }

    async fn create_api_token(&self, token: &ApiToken) -> Result<()> {
        let token = token.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO api_tokens
                   (token_id, author, name, token_hash, prefix,
                    created_at, last_used_at, revoked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    token.token_id.as_str(),
                    token.author.as_str(),
                    token.name,
                    token.token_hash,
                    token.prefix,
                    token.created_at.to_rfc3339(),
                    token.last_used_at.map(|t| t.to_rfc3339()),
                    token.revoked_at.map(|t| t.to_rfc3339()),
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn lookup_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>> {
        let hash = hash.to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT token_id, author, name, token_hash, prefix,
                        created_at, last_used_at, revoked_at
                   FROM api_tokens
                  WHERE token_hash = ?1",
                params![hash],
                row_to_api_token,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }

    async fn list_api_tokens(&self) -> Result<Vec<ApiToken>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT token_id, author, name, token_hash, prefix,
                        created_at, last_used_at, revoked_at
                   FROM api_tokens
                  ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map([], row_to_api_token)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn revoke_api_token(&self, token_id: &ApiTokenId) -> Result<()> {
        let id = token_id.as_str().to_owned();
        let now = chrono::Utc::now().to_rfc3339();
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "UPDATE api_tokens SET revoked_at = ?2 WHERE token_id = ?1",
                params![id, now],
            )?;
            if affected == 0 {
                return Err(Error::NotFound {
                    what: format!("api token {id}"),
                });
            }
            Ok(())
        })
        .await
    }

    async fn touch_api_token(&self, token_id: &ApiTokenId) -> Result<()> {
        let id = token_id.as_str().to_owned();
        let now = chrono::Utc::now().to_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = ?2 WHERE token_id = ?1",
                params![id, now],
            )?;
            Ok(())
        })
        .await
    }

    async fn raw_insert_session(&self, repo: &RepoId, session: &Session) -> Result<()> {
        // Delegates to the inherent helper. Method-resolution prefers
        // the inherent definition, so this is a one-line shim.
        SqliteStorage::raw_insert_session(self, repo, session).await
    }

    async fn raw_insert_comment(&self, repo: &RepoId, comment: &Comment) -> Result<()> {
        SqliteStorage::raw_insert_comment(self, repo, comment).await
    }

    async fn raw_insert_response(&self, repo: &RepoId, response: &Response) -> Result<()> {
        SqliteStorage::raw_insert_response(self, repo, response).await
    }

    async fn insert_github_comment_mapping(
        &self,
        repo: &RepoId,
        mapping: &crate::storage::GithubCommentMapping,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let m = mapping.clone();
        self.with_conn(move |conn| exec_insert_github_mapping(conn, &repo_str, &m))
            .await
    }

    async fn raw_insert_comment_with_mapping(
        &self,
        repo: &RepoId,
        comment: &Comment,
        mapping: &crate::storage::GithubCommentMapping,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_review_id(&comment.review_id)?;
        ensure_session_id(&comment.session_id)?;
        ensure_comment_id(&comment.comment_id)?;
        let repo_str = repo.as_str().to_owned();
        let comment = comment.clone();
        let m = mapping.clone();
        self.with_conn(move |conn| {
            // BEGIN IMMEDIATE: take the write lock up-front so a
            // concurrent import racing on the same comment id can't
            // wedge half-applied state in.
            let tx = conn.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )?;
            exec_insert_comment(&tx, &repo_str, &comment)?;
            exec_insert_github_mapping(&tx, &repo_str, &m)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn raw_insert_response_with_mapping(
        &self,
        repo: &RepoId,
        response: &Response,
        mapping: &crate::storage::GithubCommentMapping,
    ) -> Result<()> {
        ensure_repo_id(repo)?;
        ensure_session_id(&response.session_id)?;
        ensure_response_id(&response.response_id)?;
        let repo_str = repo.as_str().to_owned();
        let response = response.clone();
        let m = mapping.clone();
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )?;
            exec_insert_response(&tx, &repo_str, &response)?;
            exec_insert_github_mapping(&tx, &repo_str, &m)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn is_github_comment_mapped(
        &self,
        repo: &RepoId,
        github_node_id: &str,
    ) -> Result<bool> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let node_id = github_node_id.to_owned();
        self.with_conn(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM github_comment_map
                  WHERE repo_id = ?1 AND github_node_id = ?2",
                params![repo_str, node_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
        .await
    }

    async fn lookup_github_mapping_by_kata_comment(
        &self,
        repo: &RepoId,
        kata_comment_id: &CommentId,
    ) -> Result<Option<crate::storage::GithubCommentMapping>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let cid = kata_comment_id.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT github_node_id, github_rest_id, kind, kata_comment_id,
                        kata_response_id, review_id, pr_number, thread_node_id
                   FROM github_comment_map
                  WHERE repo_id = ?1 AND kata_comment_id = ?2",
                params![repo_str, cid],
                row_to_github_mapping,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }

    async fn lookup_github_mapping_by_kata_response(
        &self,
        repo: &RepoId,
        kata_response_id: &ResponseId,
    ) -> Result<Option<crate::storage::GithubCommentMapping>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let rid = kata_response_id.as_str().to_owned();
        self.with_conn(move |conn| {
            // `LIMIT 1` because a response with a non-Comment action
            // can have two rows (reply mapping + resolution mapping);
            // callers using this generic lookup only care about
            // "was anything written for this response". `ORDER BY
            // kind` gives a deterministic pick without requiring a
            // real preference between the two.
            conn.query_row(
                "SELECT github_node_id, github_rest_id, kind, kata_comment_id,
                        kata_response_id, review_id, pr_number, thread_node_id
                   FROM github_comment_map
                  WHERE repo_id = ?1 AND kata_response_id = ?2
                  ORDER BY kind ASC
                  LIMIT 1",
                params![repo_str, rid],
                row_to_github_mapping,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }

    async fn lookup_github_mapping_by_kata_response_kind(
        &self,
        repo: &RepoId,
        kata_response_id: &ResponseId,
        kind: &str,
    ) -> Result<Option<crate::storage::GithubCommentMapping>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let rid = kata_response_id.as_str().to_owned();
        let kind_str = kind.to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT github_node_id, github_rest_id, kind, kata_comment_id,
                        kata_response_id, review_id, pr_number, thread_node_id
                   FROM github_comment_map
                  WHERE repo_id = ?1 AND kata_response_id = ?2 AND kind = ?3",
                params![repo_str, rid, kind_str],
                row_to_github_mapping,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }

    async fn lookup_review_body_mapping(
        &self,
        repo: &RepoId,
        review_id: &ReviewId,
        pr_number: u32,
    ) -> Result<Option<crate::storage::GithubCommentMapping>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let rid = review_id.as_str().to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT github_node_id, github_rest_id, kind, kata_comment_id,
                        kata_response_id, review_id, pr_number, thread_node_id
                   FROM github_comment_map
                  WHERE repo_id = ?1
                    AND review_id = ?2
                    AND pr_number = ?3
                    AND kind = 'review_body'
                  LIMIT 1",
                params![repo_str, rid, pr_number],
                row_to_github_mapping,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }

    async fn lookup_github_mapping_by_node_id(
        &self,
        repo: &RepoId,
        github_node_id: &str,
    ) -> Result<Option<crate::storage::GithubCommentMapping>> {
        ensure_repo_id(repo)?;
        let repo_str = repo.as_str().to_owned();
        let node_id = github_node_id.to_owned();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT github_node_id, github_rest_id, kind, kata_comment_id,
                        kata_response_id, review_id, pr_number, thread_node_id
                   FROM github_comment_map
                  WHERE repo_id = ?1 AND github_node_id = ?2",
                params![repo_str, node_id],
                row_to_github_mapping,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
    }
}

// ---- shared row extractors ---------------------------------------------

/// Build a [`GithubCommentMapping`] from a row whose columns are
/// `(github_node_id, github_rest_id, kind, kata_comment_id,
/// kata_response_id, review_id, pr_number, thread_node_id)` —
/// shared by both lookup paths.
fn row_to_github_mapping(
    row: &Row<'_>,
) -> rusqlite::Result<crate::storage::GithubCommentMapping> {
    let kata_cid: Option<String> = row.get(3)?;
    let kata_rid: Option<String> = row.get(4)?;
    Ok(crate::storage::GithubCommentMapping {
        github_node_id: row.get(0)?,
        github_rest_id: row.get(1)?,
        kind: row.get(2)?,
        kata_comment_id: kata_cid.map(CommentId::new),
        kata_response_id: kata_rid.map(ResponseId::new),
        review_id: ReviewId::new(row.get::<_, String>(5)?),
        pr_number: row.get(6)?,
        thread_node_id: row.get(7)?,
    })
}

/// SQL helper shared by `raw_insert_comment` and the transactional
/// `raw_insert_comment_with_mapping`. Takes a generic
/// connection-like handle so it works on both a top-level
/// `Connection` and an open `Transaction`.
fn exec_insert_comment(
    conn: &rusqlite::Connection,
    repo_str: &str,
    comment: &Comment,
) -> Result<()> {
    let (line_start, line_end) = match &comment.lines {
        Some(LineRange { start, end }) => (Some(*start), Some(*end)),
        None => (None, None),
    };
    let external_author_json = match &comment.external_author {
        Some(a) => Some(serde_json::to_string(a).map_err(|source| Error::Json {
            context: "external_author".into(),
            source,
        })?),
        None => None,
    };
    conn.execute(
        "INSERT INTO comments
            (comment_id, repo_id, review_id, session_id, schema_version, author,
             created_at, patchset, anchor_change_id, anchor_commit_id, file, side,
             line_start, line_end, review_wide, flag, body, external_author)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            comment.comment_id.as_str(),
            repo_str,
            comment.review_id.as_str(),
            comment.session_id.as_str(),
            comment.schema_version,
            comment.author.as_str(),
            comment.created_at,
            comment.patchset,
            comment.anchor_change_id.as_str(),
            comment.anchor_commit_id.as_str(),
            comment.file,
            comment.side.map(side_to_str),
            line_start,
            line_end,
            comment.review_wide as i64,
            flag_to_str(comment.flag),
            comment.body,
            external_author_json,
        ],
    )?;
    Ok(())
}

/// Counterpart of [`exec_insert_comment`] for a response.
fn exec_insert_response(
    conn: &rusqlite::Connection,
    repo_str: &str,
    response: &Response,
) -> Result<()> {
    let review_id: String = conn
        .query_row(
            "SELECT review_id FROM comments WHERE comment_id = ?1",
            params![response.in_reply_to.as_str()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound {
            what: format!("comment {}", response.in_reply_to),
        })?;
    conn.execute(
        "INSERT INTO responses
            (response_id, repo_id, review_id, session_id, in_reply_to, schema_version,
             author, created_at, action, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            response.response_id.as_str(),
            repo_str,
            review_id,
            response.session_id.as_str(),
            response.in_reply_to.as_str(),
            response.schema_version,
            response.author.as_str(),
            response.created_at,
            action_to_str(response.action),
            response.body,
        ],
    )?;
    Ok(())
}

/// SQL helper for `github_comment_map` inserts. Same `ON CONFLICT
/// DO NOTHING` shape as the trait method; idempotent on
/// `(repo_id, github_node_id)`.
fn exec_insert_github_mapping(
    conn: &rusqlite::Connection,
    repo_str: &str,
    m: &crate::storage::GithubCommentMapping,
) -> Result<()> {
    conn.execute(
        "INSERT INTO github_comment_map
            (repo_id, github_node_id, github_rest_id, kind,
             kata_comment_id, kata_response_id, review_id, pr_number,
             thread_node_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(repo_id, github_node_id) DO NOTHING",
        params![
            repo_str,
            m.github_node_id,
            m.github_rest_id,
            m.kind,
            m.kata_comment_id.as_ref().map(|c| c.as_str()),
            m.kata_response_id.as_ref().map(|r| r.as_str()),
            m.review_id.as_str(),
            m.pr_number,
            m.thread_node_id,
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn row_to_api_token(row: &Row<'_>) -> rusqlite::Result<ApiToken> {
    // Columns: token_id, author, name, token_hash, prefix,
    //          created_at, last_used_at, revoked_at.
    Ok(ApiToken {
        token_id: ApiTokenId::new(row.get::<_, String>(0)?),
        author: Author::new(row.get::<_, String>(1)?),
        name: row.get(2)?,
        token_hash: row.get(3)?,
        prefix: row.get(4)?,
        created_at: row.get(5)?,
        last_used_at: row.get(6)?,
        revoked_at: row.get(7)?,
    })
}

fn review_manifest_from_row(row: &Row<'_>) -> rusqlite::Result<ReviewManifest> {
    // Columns are: review_id, number, name, schema_version, revset,
    // bookmark, summary, created_by, created_at, current_patchset,
    // patchsets_json, archived_at, github_pr. The listing/opening
    // queries above project exactly that order; if you change one,
    // change the other.
    let patchsets_json: String = row.get(10)?;
    let patchsets = serde_json::from_str(&patchsets_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let github_pr_json: Option<String> = row.get(12)?;
    let github_pr = match github_pr_json {
        Some(s) => Some(serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?),
        None => None,
    };
    Ok(ReviewManifest {
        review_id: ReviewId::new(row.get::<_, String>(0)?),
        number: row.get(1)?,
        name: row.get(2)?,
        schema_version: row.get(3)?,
        revset: RevSet::new(row.get::<_, String>(4)?),
        bookmark: row.get(5)?,
        summary: row.get(6)?,
        created_by: Author::new(row.get::<_, String>(7)?),
        created_at: row.get(8)?,
        current_patchset: row.get(9)?,
        patchsets,
        archived_at: row.get(11)?,
        github_pr,
    })
}

fn comment_from_row(row: &Row<'_>) -> rusqlite::Result<Comment> {
    let side: Option<String> = row.get(10)?;
    let line_start: Option<u32> = row.get(11)?;
    let line_end: Option<u32> = row.get(12)?;
    let col_start: Option<u32> = row.get(13)?;
    let col_end: Option<u32> = row.get(14)?;
    let review_wide: i64 = row.get(15)?;
    let flag_str: String = row.get(16)?;
    let external_author_json: Option<String> = row.get(18)?;
    let external_author = match external_author_json {
        Some(s) => Some(serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                18,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?),
        None => None,
    };
    let side = match side {
        Some(s) => Some(side_from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?),
        None => None,
    };
    let lines = match (line_start, line_end) {
        (Some(s), Some(e)) => Some(LineRange::new(s, e)),
        _ => None,
    };
    // Either both column endpoints are stored or neither — partial
    // rows are treated as "no columns". `end > start` is NOT enforced
    // here: a multi-line comment's columns mean "start col on first
    // line, end col on last line", and the last line can perfectly
    // well end before the first line's start col.
    let columns = match (col_start, col_end) {
        (Some(s), Some(e)) => Some(ColumnRange { start: s, end: e }),
        _ => None,
    };
    let flag = flag_from_str(&flag_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(16, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Comment {
        comment_id: CommentId::new(row.get::<_, String>(0)?),
        session_id: SessionId::new(row.get::<_, String>(1)?),
        review_id: ReviewId::new(row.get::<_, String>(2)?),
        schema_version: row.get(3)?,
        author: Author::new(row.get::<_, String>(4)?),
        created_at: row.get(5)?,
        patchset: row.get(6)?,
        anchor_change_id: ChangeId::new(row.get::<_, String>(7)?),
        anchor_commit_id: CommitId::new(row.get::<_, String>(8)?),
        file: row.get(9)?,
        side,
        lines,
        columns,
        review_wide: review_wide != 0,
        flag,
        body: row.get(17)?,
        external_author,
    })
}

fn annotation_from_row(row: &Row<'_>) -> rusqlite::Result<Annotation> {
    let side: Option<String> = row.get(10)?;
    let line_start: Option<u32> = row.get(11)?;
    let line_end: Option<u32> = row.get(12)?;
    let side = match side {
        Some(s) => Some(side_from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?),
        None => None,
    };
    let lines = match (line_start, line_end) {
        (Some(s), Some(e)) => Some(LineRange::new(s, e)),
        _ => None,
    };
    Ok(Annotation {
        annotation_id: AnnotationId::new(row.get::<_, String>(0)?),
        review_id: ReviewId::new(row.get::<_, String>(1)?),
        schema_version: row.get(2)?,
        author: Author::new(row.get::<_, String>(3)?),
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        patchset: row.get(6)?,
        anchor_change_id: ChangeId::new(row.get::<_, String>(7)?),
        anchor_commit_id: CommitId::new(row.get::<_, String>(8)?),
        file: row.get(9)?,
        side,
        lines,
        body: row.get(13)?,
    })
}

fn response_from_row(row: &Row<'_>) -> rusqlite::Result<Response> {
    let action_str: String = row.get(6)?;
    let action = action_from_str(&action_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Response {
        response_id: ResponseId::new(row.get::<_, String>(0)?),
        in_reply_to: CommentId::new(row.get::<_, String>(1)?),
        session_id: SessionId::new(row.get::<_, String>(2)?),
        schema_version: row.get(3)?,
        author: Author::new(row.get::<_, String>(4)?),
        created_at: row.get(5)?,
        action,
        body: row.get(7)?,
    })
}

fn require_draft_session(tx: &Transaction<'_>, session: &SessionId) -> Result<()> {
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM sessions WHERE session_id = ?1",
            params![session.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    match status.as_deref() {
        Some("draft") => Ok(()),
        Some(other) => Err(Error::SessionState {
            session: session.to_string(),
            state: match other {
                "published" => "published",
                "discarded" => "discarded",
                _ => "unknown",
            },
            expected: "draft",
        }),
        None => Err(Error::NotFound {
            what: format!("session {session}"),
        }),
    }
}

async fn flip_session_status(
    storage: &SqliteStorage,
    repo: &RepoId,
    review: &ReviewId,
    session: &SessionId,
    target: SessionStatus,
) -> Result<()> {
    ensure_repo_id(repo)?;
    ensure_review_id(review)?;
    ensure_session_id(session)?;
    let session_clone = session.clone();
    let session_str = session.as_str().to_owned();
    let target_str = session_status_to_str(target);
    let needs_published_at = matches!(target, SessionStatus::Published);
    storage
        .with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            require_draft_session(&tx, &session_clone)?;
            let now = Utc::now();
            // `published_at` is set only on the publish path so the
            // discard column stays null for forensic clarity.
            if needs_published_at {
                tx.execute(
                    "UPDATE sessions SET status = ?2, published_at = ?3 WHERE session_id = ?1",
                    params![session_str, target_str, now],
                )?;
            } else {
                tx.execute(
                    "UPDATE sessions SET status = ?2 WHERE session_id = ?1",
                    params![session_str, target_str],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
}

#[cfg(test)]
mod round_trip_tests {
    //! Schema round-trip tests for fields added after V001 — annotation
    //! storage plus the `columns` add-on for comments. These guard the
    //! upsert / list / mapper paths against drift the way the existing
    //! migrate tests guard the schema itself.

    use super::*;
    use chrono::{NaiveDateTime, TimeZone};
    use kata_core::{ApiToken, ApiTokenId, Flag, Patchset, Side};
    use std::sync::atomic::{AtomicU64, Ordering};
    use uuid::{NoContext, Timestamp, Uuid};

    /// Bump a per-process counter so two `seed_review` calls in the
    /// same test process never collide on the per-repo `number`
    /// uniqueness constraint when they happen to hit the same repo.
    static SEED: AtomicU64 = AtomicU64::new(0);

    fn fresh_id(prefix: &str) -> String {
        let n = SEED.fetch_add(1, Ordering::Relaxed);
        let uuid = Uuid::new_v7(Timestamp::now(NoContext));
        format!("{prefix}-{n}-{uuid}")
    }

    fn ts(s: &str) -> chrono::DateTime<chrono::Utc> {
        let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").unwrap();
        chrono::Utc.from_utc_datetime(&naive)
    }

    async fn seed_review(
        store: &SqliteStorage,
    ) -> (RepoId, ReviewManifest, Author, SessionId) {
        let repo = RepoId::new(fresh_id("repo"));
        let review_id = ReviewId::new(fresh_id("rv"));
        let author = Author::new("alice@example.com");

        store
            .ensure_repo(&RepoManifest {
                schema_version: SCHEMA_VERSION,
                repo_id: repo.clone(),
                canonical_path: "/tmp/test".into(),
            })
            .await
            .expect("ensure_repo");

        let patchset = Patchset {
            n: 1,
            base_change: ChangeId::new("ch-base"),
            base_commit: CommitId::new("co-base"),
            tip_change: ChangeId::new("ch-tip"),
            tip_commit: CommitId::new("co-tip"),
            recorded_at: ts("2026-01-01T00:00:00Z"),
            parent_patchset: None,
        };
        let manifest = ReviewManifest {
            schema_version: SCHEMA_VERSION,
            review_id: review_id.clone(),
            number: 0,
            name: "test review".into(),
            revset: RevSet::new("trunk()..@"),
            created_at: ts("2026-01-01T00:00:00Z"),
            created_by: author.clone(),
            bookmark: None,
            summary: None,
            patchsets: vec![patchset],
            current_patchset: 1,
            archived_at: None,
            github_pr: None,
        };
        let manifest = store.create_review(&repo, &manifest).await.expect("create_review");

        let session = store
            .open_or_create_session(&repo, &review_id, &author)
            .await
            .expect("open_or_create_session");
        (repo, manifest, author, session.session_id)
    }

    #[tokio::test]
    async fn annotation_round_trips_all_fields() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, author, _sid) = seed_review(&store).await;

        let annotation = Annotation {
            schema_version: SCHEMA_VERSION,
            annotation_id: AnnotationId::new(fresh_id("an")),
            review_id: manifest.review_id.clone(),
            author: author.clone(),
            created_at: ts("2026-01-02T00:00:00Z"),
            updated_at: ts("2026-01-02T00:00:00Z"),
            patchset: 1,
            anchor_change_id: ChangeId::new("ch-tip"),
            anchor_commit_id: CommitId::new("co-tip"),
            file: Some("src/lib.rs".into()),
            side: Some(Side::Tip),
            lines: Some(LineRange::single(42)),
            body: "context for the reviewer".into(),
        };
        store.upsert_annotation(&repo, &annotation).await.unwrap();

        let list = store.list_annotations(&repo, &manifest.review_id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], annotation, "annotation must round-trip byte-for-byte");
    }

    #[tokio::test]
    async fn annotation_upsert_updates_in_place() {
        // Same id, different body + updated_at — the second write
        // should overwrite rather than insert a duplicate. Guards the
        // ON CONFLICT clause on annotation_id.
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, author, _sid) = seed_review(&store).await;
        let id = AnnotationId::new(fresh_id("an"));

        let mut annotation = Annotation {
            schema_version: SCHEMA_VERSION,
            annotation_id: id.clone(),
            review_id: manifest.review_id.clone(),
            author: author.clone(),
            created_at: ts("2026-01-02T00:00:00Z"),
            updated_at: ts("2026-01-02T00:00:00Z"),
            patchset: 1,
            anchor_change_id: ChangeId::new("ch-tip"),
            anchor_commit_id: CommitId::new("co-tip"),
            file: Some("src/lib.rs".into()),
            side: Some(Side::Tip),
            lines: Some(LineRange::single(7)),
            body: "first draft".into(),
        };
        store.upsert_annotation(&repo, &annotation).await.unwrap();

        annotation.body = "edited".into();
        annotation.updated_at = ts("2026-01-03T00:00:00Z");
        store.upsert_annotation(&repo, &annotation).await.unwrap();

        let list = store.list_annotations(&repo, &manifest.review_id).await.unwrap();
        assert_eq!(list.len(), 1, "second upsert must replace, not append");
        assert_eq!(list[0].body, "edited");
        assert_eq!(list[0].updated_at, ts("2026-01-03T00:00:00Z"));
    }

    #[tokio::test]
    async fn delete_review_removes_review_and_dependent_rows() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, author, sid) = seed_review(&store).await;

        // Seed every dependent shape we cascade through: an annotation
        // (no FK back to reviews, so the impl deletes it directly), a
        // draft comment (FK on session_id), and a review-visit row
        // (FK on review_id).
        let ann_id = AnnotationId::new(fresh_id("an"));
        store
            .upsert_annotation(
                &repo,
                &Annotation {
                    schema_version: SCHEMA_VERSION,
                    annotation_id: ann_id.clone(),
                    review_id: manifest.review_id.clone(),
                    author: author.clone(),
                    created_at: ts("2026-01-02T00:00:00Z"),
                    updated_at: ts("2026-01-02T00:00:00Z"),
                    patchset: 1,
                    anchor_change_id: ChangeId::new("ch-tip"),
                    anchor_commit_id: CommitId::new("co-tip"),
                    file: None,
                    side: None,
                    lines: None,
                    body: "context".into(),
                },
            )
            .await
            .unwrap();
        let comment = Comment {
            schema_version: SCHEMA_VERSION,
            comment_id: CommentId::new(fresh_id("cm")),
            session_id: sid.clone(),
            review_id: manifest.review_id.clone(),
            author: author.clone(),
            created_at: ts("2026-01-02T00:00:00Z"),
            patchset: 1,
            anchor_change_id: ChangeId::new("ch-tip"),
            anchor_commit_id: CommitId::new("co-tip"),
            file: None,
            side: None,
            lines: None,
            columns: None,
            review_wide: true,
            flag: Flag::Suggestion,
            body: "draft".into(),
            external_author: None,
        };
        store.upsert_draft_comment(&repo, &comment).await.unwrap();
        let visit_op = OpId::new("op-visit");
        store
            .record_review_visit(&repo, &manifest.review_id, &author, &visit_op)
            .await
            .unwrap();

        store
            .delete_review(&repo, &manifest.review_id)
            .await
            .unwrap();

        // Review row itself is gone.
        let err = store
            .open_review(&repo, &manifest.review_id)
            .await
            .expect_err("open_review on a deleted review must error");
        assert!(
            matches!(err, Error::NotFound { .. }),
            "expected NotFound, got {err:?}",
        );

        // Dependents are gone.
        let anns = store
            .list_annotations(&repo, &manifest.review_id)
            .await
            .unwrap();
        assert!(anns.is_empty(), "annotations must be cleared");
        let drafts = store
            .list_drafts_for(&repo, &manifest.review_id, &author)
            .await
            .unwrap();
        assert!(
            drafts.comments.is_empty(),
            "draft comments must be cleared via session cascade",
        );
        let visit = store
            .last_review_visit(&repo, &manifest.review_id, &author)
            .await
            .unwrap();
        assert!(visit.is_none(), "review visits must be cleared");
    }

    #[tokio::test]
    async fn delete_review_is_idempotent() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, _author, _sid) = seed_review(&store).await;

        store
            .delete_review(&repo, &manifest.review_id)
            .await
            .unwrap();
        // Second call against a now-missing row must succeed silently.
        store
            .delete_review(&repo, &manifest.review_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_annotation_removes_the_row() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, author, _sid) = seed_review(&store).await;

        let id = AnnotationId::new(fresh_id("an"));
        store
            .upsert_annotation(
                &repo,
                &Annotation {
                    schema_version: SCHEMA_VERSION,
                    annotation_id: id.clone(),
                    review_id: manifest.review_id.clone(),
                    author,
                    created_at: ts("2026-01-02T00:00:00Z"),
                    updated_at: ts("2026-01-02T00:00:00Z"),
                    patchset: 1,
                    anchor_change_id: ChangeId::new("ch-tip"),
                    anchor_commit_id: CommitId::new("co-tip"),
                    file: None,
                    side: None,
                    lines: None,
                    body: "review-wide".into(),
                },
            )
            .await
            .unwrap();
        store.delete_annotation(&repo, &manifest.review_id, &id).await.unwrap();
        let list = store.list_annotations(&repo, &manifest.review_id).await.unwrap();
        assert!(list.is_empty(), "annotation must be gone after delete");
    }

    #[tokio::test]
    async fn comment_with_columns_round_trips() {
        // Guards V009's col_start / col_end serde path. Without
        // these, an intra-line comment would silently degrade to
        // line-level on the next read.
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, author, sid) = seed_review(&store).await;

        let comment = Comment {
            schema_version: SCHEMA_VERSION,
            comment_id: CommentId::new(fresh_id("cm")),
            session_id: sid.clone(),
            review_id: manifest.review_id.clone(),
            author,
            created_at: ts("2026-01-02T00:00:00Z"),
            patchset: 1,
            anchor_change_id: ChangeId::new("ch-tip"),
            anchor_commit_id: CommitId::new("co-tip"),
            file: Some("src/lib.rs".into()),
            side: Some(Side::Tip),
            lines: Some(LineRange::single(42)),
            columns: Some(ColumnRange::new(4, 12)),
            review_wide: false,
            flag: Flag::Suggestion,
            body: "this slice".into(),
            external_author: None,
        };
        store.upsert_draft_comment(&repo, &comment).await.unwrap();

        let drafts = store
            .list_drafts_for(&repo, &manifest.review_id, &comment.author)
            .await
            .unwrap();
        assert_eq!(drafts.comments.len(), 1);
        assert_eq!(drafts.comments[0].columns, Some(ColumnRange::new(4, 12)));
        assert_eq!(drafts.comments[0], comment);
    }

    #[tokio::test]
    async fn comment_without_columns_persists_null() {
        // The other half: an old-style line-level comment must NOT
        // gain spurious columns from the round-trip.
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let (repo, manifest, author, sid) = seed_review(&store).await;

        let comment = Comment {
            schema_version: SCHEMA_VERSION,
            comment_id: CommentId::new(fresh_id("cm")),
            session_id: sid,
            review_id: manifest.review_id.clone(),
            author,
            created_at: ts("2026-01-02T00:00:00Z"),
            patchset: 1,
            anchor_change_id: ChangeId::new("ch-tip"),
            anchor_commit_id: CommitId::new("co-tip"),
            file: Some("src/lib.rs".into()),
            side: Some(Side::Tip),
            lines: Some(LineRange::new(40, 45)),
            columns: None,
            review_wide: false,
            flag: Flag::MustDo,
            body: "whole-range".into(),
            external_author: None,
        };
        store.upsert_draft_comment(&repo, &comment).await.unwrap();

        let drafts = store
            .list_drafts_for(&repo, &manifest.review_id, &comment.author)
            .await
            .unwrap();
        assert_eq!(drafts.comments.len(), 1);
        assert_eq!(drafts.comments[0].columns, None);
    }

    fn make_token(prefix: &str, author: &str, name: &str) -> ApiToken {
        ApiToken {
            token_id: ApiTokenId::new(fresh_id(prefix)),
            author: Author::new(author),
            name: name.to_owned(),
            token_hash: fresh_id("hash"),
            prefix: "kata_pat_abc".into(),
            created_at: ts("2026-01-01T00:00:00Z"),
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn create_lookup_round_trip_for_api_tokens() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let token = make_token("tok", "alice@example.com", "ci-agent");
        store.create_api_token(&token).await.unwrap();

        let found = store
            .lookup_api_token_by_hash(&token.token_hash)
            .await
            .unwrap()
            .expect("token must be findable by its hash");
        assert_eq!(found, token);

        let miss = store
            .lookup_api_token_by_hash("not-a-real-hash")
            .await
            .unwrap();
        assert!(miss.is_none());
    }

    #[tokio::test]
    async fn list_api_tokens_returns_newest_first() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let mut older = make_token("tok", "alice@example.com", "old");
        older.created_at = ts("2026-01-01T00:00:00Z");
        let mut newer = make_token("tok", "alice@example.com", "new");
        newer.created_at = ts("2026-02-01T00:00:00Z");
        store.create_api_token(&older).await.unwrap();
        store.create_api_token(&newer).await.unwrap();

        let list = store.list_api_tokens().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "new", "newest must be first");
        assert_eq!(list[1].name, "old");
    }

    #[tokio::test]
    async fn revoke_sets_timestamp_and_keeps_the_row() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let token = make_token("tok", "alice@example.com", "ci-agent");
        store.create_api_token(&token).await.unwrap();

        store.revoke_api_token(&token.token_id).await.unwrap();
        // Row stays; revoked_at populated. The auth path is the
        // place that interprets `revoked_at` as "reject" — storage
        // just records the timestamp.
        let found = store
            .lookup_api_token_by_hash(&token.token_hash)
            .await
            .unwrap()
            .expect("row must still exist after revoke");
        assert!(
            found.revoked_at.is_some(),
            "revoked_at must be populated after revoke",
        );
    }

    #[tokio::test]
    async fn revoking_unknown_token_errors() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let err = store
            .revoke_api_token(&ApiTokenId::new("not-a-real-id"))
            .await
            .expect_err("revoking a non-existent token must error");
        assert!(matches!(err, Error::NotFound { .. }));
    }

    #[tokio::test]
    async fn touch_updates_last_used_at() {
        let store = SqliteStorage::open_in_memory().await.unwrap();
        let token = make_token("tok", "alice@example.com", "ci-agent");
        store.create_api_token(&token).await.unwrap();
        assert!(token.last_used_at.is_none());

        store.touch_api_token(&token.token_id).await.unwrap();
        let found = store
            .lookup_api_token_by_hash(&token.token_hash)
            .await
            .unwrap()
            .unwrap();
        assert!(
            found.last_used_at.is_some(),
            "touch must populate last_used_at",
        );
    }
}

