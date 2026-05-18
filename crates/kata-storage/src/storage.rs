//! The [`Storage`] trait — the swap point between the filesystem-backed
//! implementation today and any future database-backed one.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kata_core::{
    Annotation, AnnotationId, Author, Comment, CommentId, OpId, RepoId, RepoManifest, Response,
    ResponseId, ReviewId, ReviewManifest, Session, SessionId,
};

use crate::error::Result;

/// What the storage layer remembers from a reviewer's previous open of
/// a review. The op-id is the jj-side baseline for "what operations
/// have happened in the repo since"; the timestamp is the wall-clock
/// baseline for "what comments / responses have landed since".
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewVisit {
    pub op_id: OpId,
    pub visited_at: DateTime<Utc>,
}

/// Lightweight summary returned by listing.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewSummary {
    pub manifest: ReviewManifest,
    pub session_count: usize,
    pub published_comment_count: usize,
}

/// Everything an author can currently see of their own work-in-progress in
/// a given review.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DraftsView {
    pub session: Option<Session>,
    pub comments: Vec<Comment>,
    pub responses: Vec<Response>,
}

#[async_trait]
pub trait Storage: Send + Sync {
    // ---- repo manifest --------------------------------------------------

    /// Idempotent — creates the per-repo subdirectory and manifest if it
    /// does not yet exist, no-op otherwise.
    async fn ensure_repo(&self, manifest: &RepoManifest) -> Result<()>;

    async fn open_repo(&self, repo: &RepoId) -> Result<Option<RepoManifest>>;

    // ---- reviews --------------------------------------------------------

    async fn list_reviews(&self, repo: &RepoId) -> Result<Vec<ReviewSummary>>;

    async fn open_review(&self, repo: &RepoId, review: &ReviewId) -> Result<ReviewManifest>;

    /// Look up the internal `review_id` from the per-repo `number`
    /// that the URL carries. Returns `None` when no review with that
    /// number exists.
    async fn resolve_review_number(
        &self,
        repo: &RepoId,
        number: u32,
    ) -> Result<Option<ReviewId>>;

    /// Persist `manifest`. Returns the manifest as actually stored —
    /// the storage layer may fill in fields the caller left to be
    /// assigned (per-repo `number`, default `name`), so the caller
    /// should treat the returned value as authoritative.
    async fn create_review(
        &self,
        repo: &RepoId,
        manifest: &ReviewManifest,
    ) -> Result<ReviewManifest>;

    /// Replace an existing review manifest in place. Used to record an
    /// updated `last_seen_*` after the bookmark moves.
    async fn update_review(&self, repo: &RepoId, manifest: &ReviewManifest) -> Result<()>;

    // ---- sessions -------------------------------------------------------

    /// Return the author's open draft session for `review`, creating one if
    /// none is open. An author has at most one open draft session per
    /// review at a time.
    async fn open_or_create_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<Session>;

    /// Flip the session from `Draft` to `Published`. Errors if the session
    /// is already finalised.
    async fn publish_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<()>;

    /// Flip the session from `Draft` to `Discarded`. Drafts inside become
    /// invisible to readers but the files stay on disk for forensics.
    async fn discard_session(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
    ) -> Result<()>;

    // ---- authoring ------------------------------------------------------

    /// Write or replace a draft comment. The session must be in `Draft`.
    async fn upsert_draft_comment(&self, repo: &RepoId, comment: &Comment) -> Result<()>;

    async fn discard_draft_comment(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        comment: &CommentId,
    ) -> Result<()>;

    /// Write or replace a draft response. The session must be in `Draft`.
    async fn upsert_draft_response(&self, repo: &RepoId, response: &Response) -> Result<()>;

    async fn discard_draft_response(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        session: &SessionId,
        response: &ResponseId,
    ) -> Result<()>;

    // ---- reading --------------------------------------------------------

    async fn list_published_comments(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Comment>>;

    async fn list_published_responses(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Response>>;

    // ---- annotations ----------------------------------------------------

    /// All annotations attached to `review`. Annotations skip the
    /// session/draft flow entirely (the creator authors them
    /// individually and they go live on submit), so there's no
    /// "draft annotations" counterpart — every annotation visible to
    /// the storage layer is visible to readers.
    async fn list_annotations(
        &self,
        repo: &RepoId,
        review: &ReviewId,
    ) -> Result<Vec<Annotation>>;

    /// Insert or replace the annotation by id. Caller is responsible
    /// for the creator-only permission check before invoking.
    async fn upsert_annotation(&self, repo: &RepoId, annotation: &Annotation) -> Result<()>;

    /// Delete the annotation. Caller is responsible for the
    /// creator-only permission check before invoking.
    async fn delete_annotation(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        annotation: &AnnotationId,
    ) -> Result<()>;

    /// Everything `author` can still edit in `review`: their open session
    /// (if any) plus its draft comments and responses.
    async fn list_drafts_for(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<DraftsView>;

    // ---- per-reviewer visit log ----------------------------------------

    /// What this `author` saw the last time they opened `review`: the
    /// jj op-id at that point and the wall-clock timestamp. `None` when
    /// the reviewer has never opened this review before — the service
    /// treats that as "no since-you-last-looked baseline yet."
    async fn last_review_visit(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
    ) -> Result<Option<ReviewVisit>>;

    /// Upsert `author`'s last-seen op-id for `review`. Idempotent — runs
    /// on every open_review and just overwrites the previous high-water
    /// mark.
    async fn record_review_visit(
        &self,
        repo: &RepoId,
        review: &ReviewId,
        author: &Author,
        op_id: &OpId,
    ) -> Result<()>;
}
