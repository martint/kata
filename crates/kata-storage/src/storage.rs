//! The [`Storage`] trait — the swap point between the filesystem-backed
//! implementation today and any future database-backed one.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kata_core::{
    Annotation, AnnotationId, ApiToken, ApiTokenId, Author, Comment, CommentId, OpId, RepoId,
    RepoManifest, Response, ResponseId, ReviewId, ReviewManifest, Session, SessionId,
};

/// One mapping row between a kata comment-or-response and the
/// GitHub object it was imported from. Persisted in
/// `github_comment_map`; consulted on refresh to dedup
/// already-imported items, and on publish (phase 6) to thread
/// replies back to the right upstream comment.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GithubCommentMapping {
    /// Stable GitHub GraphQL node id (`PRRC_kw...`). Primary dedup
    /// key — survives renames/edits and is what GraphQL mutations
    /// (resolve thread, reply to comment) take as input.
    pub github_node_id: String,
    /// REST API numeric id, when known. Convenient for REST-only
    /// endpoints in phase 6 (e.g. `POST .../pulls/N/comments`
    /// `in_reply_to` takes the REST id, not the node id).
    pub github_rest_id: Option<i64>,
    /// Discriminator: `"review_summary"`, `"line_comment"`,
    /// `"issue_comment"`, or `"thread_reply"`. Kept as a string so
    /// adding a new kind is additive.
    pub kind: String,
    pub review_id: ReviewId,
    pub pr_number: u32,
    /// Kata comment id, when this mapping points at one.
    pub kata_comment_id: Option<CommentId>,
    /// Kata response id, when this mapping points at one. Exactly
    /// one of [`Self::kata_comment_id`] / [`Self::kata_response_id`]
    /// is `Some` per row.
    pub kata_response_id: Option<ResponseId>,
    /// GitHub review-thread node id this row belongs to, when
    /// known. Threads anchor their resolution state at the thread
    /// level (not per-comment); phase 6 uses this to issue
    /// `resolveReviewThread` mutations.
    pub thread_node_id: Option<String>,
}

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

    /// Delete a review and everything that hangs off it: sessions,
    /// comments, responses, annotations, visit timestamps. No
    /// soft-delete — archive covers that case. Idempotent: deleting
    /// a review that doesn't exist is not an error.
    async fn delete_review(&self, repo: &RepoId, review: &ReviewId) -> Result<()>;

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

    // ---- API tokens -----------------------------------------------------

    /// Persist a freshly-minted token. The caller has already hashed
    /// the plaintext; only the hash + metadata ride into storage.
    async fn create_api_token(&self, token: &ApiToken) -> Result<()>;

    /// Look up a token by its `token_hash`. Returns `None` when no
    /// row matches — used as the "authenticate this bearer string"
    /// hot path, so it must be a single indexed lookup. The caller
    /// decides whether a revoked match counts as authenticated
    /// (currently: no).
    async fn lookup_api_token_by_hash(&self, hash: &str) -> Result<Option<ApiToken>>;

    /// All tokens in the installation, newest-first. `revoked` rows
    /// are included — `kata token list` distinguishes them in its
    /// output. Empty list when no tokens have ever been issued.
    async fn list_api_tokens(&self) -> Result<Vec<ApiToken>>;

    /// Soft-delete a token by id. Sets `revoked_at = now`; the row
    /// stays so `token_id` references in audit logs still resolve.
    /// Idempotent — revoking an already-revoked token is not an
    /// error (the timestamp is updated to the new revocation time).
    async fn revoke_api_token(&self, token_id: &ApiTokenId) -> Result<()>;

    /// Update `last_used_at` on a successful authentication. Fire-
    /// and-forget from the caller's perspective; a failure here
    /// must not reject the request that just authenticated.
    async fn touch_api_token(&self, token_id: &ApiTokenId) -> Result<()>;

    // ---- imported (non-draft) inserts ----------------------------------
    //
    // The "raw" methods below let kata land an externally-produced
    // session/comment/response directly into storage at its
    // archive-preserved content (no draft-phase, no
    // `created_at = now()` overwrite). Originally exposed for the
    // archive import path; phase-5 GitHub import reuses them for the
    // same reason — every imported item carries its github.com
    // `created_at` and an explicit author identity that we don't
    // want a draft-flow `now()` to clobber.

    async fn raw_insert_session(&self, repo: &RepoId, session: &Session) -> Result<()>;
    async fn raw_insert_comment(&self, repo: &RepoId, comment: &Comment) -> Result<()>;
    async fn raw_insert_response(&self, repo: &RepoId, response: &Response) -> Result<()>;

    /// Insert an imported comment **and** its
    /// `github_comment_map` row in the same SQL transaction.
    /// Crash-safe by construction — without the transaction, a
    /// failure between the two writes leaves the comment with no
    /// mapping row, which makes the next import re-insert it as a
    /// ghost duplicate (the dedup check goes through the mapping).
    async fn raw_insert_comment_with_mapping(
        &self,
        repo: &RepoId,
        comment: &Comment,
        mapping: &GithubCommentMapping,
    ) -> Result<()>;

    /// Counterpart of [`Self::raw_insert_comment_with_mapping`]
    /// for an imported response (thread reply). Same atomicity
    /// rationale.
    async fn raw_insert_response_with_mapping(
        &self,
        repo: &RepoId,
        response: &Response,
        mapping: &GithubCommentMapping,
    ) -> Result<()>;

    // ---- GitHub comment-id mapping -------------------------------------

    /// Persist a row in `github_comment_map`. Idempotent: a
    /// duplicate `(repo_id, github_node_id)` is treated as success
    /// (no-op) so refresh paths can re-run without conflict.
    async fn insert_github_comment_mapping(
        &self,
        repo: &RepoId,
        mapping: &GithubCommentMapping,
    ) -> Result<()>;

    /// True iff a row already exists for `(repo, github_node_id)`.
    /// Used during refresh to skip already-imported items.
    async fn is_github_comment_mapped(
        &self,
        repo: &RepoId,
        github_node_id: &str,
    ) -> Result<bool>;

    /// Find the GitHub mapping for a kata comment, if any. Drives
    /// publish (phase 6): when a kata response targets a comment
    /// that was imported from a GitHub thread, the reply has to
    /// be posted with `in_reply_to = <that github id>` so it
    /// threads correctly on github.com.
    async fn lookup_github_mapping_by_kata_comment(
        &self,
        repo: &RepoId,
        kata_comment_id: &CommentId,
    ) -> Result<Option<GithubCommentMapping>>;

    /// Counterpart of [`Self::lookup_github_mapping_by_kata_comment`]
    /// for responses (replies). Drives the publish-retry
    /// idempotency check — a reply whose mapping row already
    /// exists has already landed on github.com and must not be
    /// re-posted.
    ///
    /// A kata response may have up to two mapping rows: one for
    /// the reply text (`kind = "thread_reply" | "issue_comment"`)
    /// and, when the response's action is non-Comment, a separate
    /// row for the resolveReviewThread side-effect (`kind =
    /// "resolution"`). This method returns whichever row was
    /// inserted first — callers that need to distinguish the two
    /// should use [`Self::lookup_github_mapping_by_kata_response_kind`].
    async fn lookup_github_mapping_by_kata_response(
        &self,
        repo: &RepoId,
        kata_response_id: &ResponseId,
    ) -> Result<Option<GithubCommentMapping>>;

    /// Kind-scoped variant of
    /// [`Self::lookup_github_mapping_by_kata_response`]. Used by
    /// the publish loop to track the reply-post and the resolution
    /// side-effect independently — each has its own mapping row so
    /// a retry after one succeeded and the other failed can pick up
    /// where it left off (see the "resolve-with-text on retry"
    /// regression test in `crates/kata-service/tests/github_publish.rs`).
    async fn lookup_github_mapping_by_kata_response_kind(
        &self,
        repo: &RepoId,
        kata_response_id: &ResponseId,
        kind: &str,
    ) -> Result<Option<GithubCommentMapping>>;

    /// Single-comment fetch by id. Used by the publish path to
    /// quote a parent comment's body when a reply has to fall
    /// back to an issue comment (the parent isn't a thread that
    /// accepts `in_reply_to`, or the threaded post 422s). `None`
    /// when the comment doesn't exist.
    async fn get_comment_by_id(
        &self,
        repo: &RepoId,
        comment_id: &CommentId,
    ) -> Result<Option<Comment>>;

    /// Reverse lookup: from a GitHub GraphQL node id to the kata
    /// mapping (if any). Drives the refresh path's "attach a new
    /// reply to a previously-imported thread" branch — when a new
    /// reply lands on github.com, kata needs to find the kata
    /// comment id corresponding to the thread's anchor so the
    /// reply lands as a kata response under it.
    async fn lookup_github_mapping_by_node_id(
        &self,
        repo: &RepoId,
        github_node_id: &str,
    ) -> Result<Option<GithubCommentMapping>>;

    /// Mapping for the wrapping `pull_request_review` (or, in the
    /// deep-fallback path where both the bundle and the empty-
    /// comments shell 422, the issue comment its body landed as).
    /// Keyed by `(repo, review_id, pr_number, kind="review_body")`
    /// — there's no kata comment to thread it off of, so the
    /// "natural id" is the kata review itself. Drives publish
    /// idempotency: a retry whose body was already posted skips
    /// the body part of the wrapping review.
    async fn lookup_review_body_mapping(
        &self,
        repo: &RepoId,
        review_id: &ReviewId,
        pr_number: u32,
    ) -> Result<Option<GithubCommentMapping>>;
}
