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
    Author, ChangeId, Comment, CommentId, CommitId, LineRange, RepoId, RepoManifest, Response,
    ResponseId, ReviewId, ReviewManifest, RevSet, SCHEMA_VERSION, Session, SessionId,
    SessionStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::error::{Error, Result};
use crate::ids::{
    ensure_author, ensure_comment_id, ensure_repo_id, ensure_response_id, ensure_review_id,
    ensure_session_id, new_session_id,
};
use crate::sqlite::migrate;
use crate::sqlite::serde_enums::{
    action_from_str, action_to_str, flag_from_str, flag_to_str, session_status_to_str,
    side_from_str, side_to_str,
};
use crate::storage::{DraftsView, ReviewSummary, Storage};

pub struct SqliteStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    /// Open the SQLite file at `path`, applying any pending migrations.
    /// The file is created if it doesn't exist; its parent directory must
    /// already exist (callers typically pass `<KATA_ROOT>/kata.db`).
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
                        file, side, line_start, line_end, flag, body
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
        self.with_conn(move |conn| {
            let (line_start, line_end) = match &comment.lines {
                Some(LineRange { start, end }) => (Some(*start), Some(*end)),
                None => (None, None),
            };
            conn.execute(
                "INSERT INTO comments
                    (comment_id, repo_id, review_id, session_id, schema_version, author,
                     created_at, patchset, anchor_change_id, anchor_commit_id, file, side,
                     line_start, line_end, flag, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                    flag_to_str(comment.flag),
                    comment.body,
                ],
            )?;
            Ok(())
        })
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
        self.with_conn(move |conn| {
            // Look up the comment's review_id so the FK and the column
            // stay consistent without the caller having to thread it.
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
        })
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
                    r.archived_at,
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
                let session_count: i64 = row.get(12)?;
                let comment_count: i64 = row.get(13)?;
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
                            created_by, created_at, current_patchset, patchsets_json, archived_at
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
            let res = tx.execute(
                "INSERT INTO reviews
                    (repo_id, review_id, number, name, schema_version, revset, bookmark, summary,
                     created_by, created_at, current_patchset, patchsets_json, archived_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                        archived_at = ?12
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
            tx.execute(
                "INSERT INTO comments
                    (comment_id, repo_id, review_id, session_id, schema_version, author,
                     created_at, patchset, anchor_change_id, anchor_commit_id, file, side,
                     line_start, line_end, flag, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                    flag = excluded.flag,
                    body = excluded.body",
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
                    flag_to_str(comment.flag),
                    comment.body,
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
                        c.file, c.side, c.line_start, c.line_end, c.flag, c.body
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
                        file, side, line_start, line_end, flag, body
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
}

// ---- shared row extractors ---------------------------------------------

fn review_manifest_from_row(row: &Row<'_>) -> rusqlite::Result<ReviewManifest> {
    // Columns are: review_id, number, name, schema_version, revset,
    // bookmark, summary, created_by, created_at, current_patchset,
    // patchsets_json, archived_at. The two listing/opening queries
    // above project exactly that order; if you change one, change the
    // other.
    let patchsets_json: String = row.get(10)?;
    let patchsets = serde_json::from_str(&patchsets_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
    })?;
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
    })
}

fn comment_from_row(row: &Row<'_>) -> rusqlite::Result<Comment> {
    let side: Option<String> = row.get(10)?;
    let line_start: Option<u32> = row.get(11)?;
    let line_end: Option<u32> = row.get(12)?;
    let flag_str: String = row.get(13)?;
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
    let flag = flag_from_str(&flag_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(13, rusqlite::types::Type::Text, Box::new(e))
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
        flag,
        body: row.get(14)?,
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

