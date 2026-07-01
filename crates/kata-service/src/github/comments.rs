//! Import the existing discussion of a GitHub PR into a freshly-
//! created kata review.
//!
//! Three GitHub-side sources are merged into kata's
//! comment/response model:
//!
//! 1. **Top-level (issue) comments on the PR conversation** —
//!    rendered as review-wide kata comments.
//! 2. **Review summaries** (a `Review` body with no inline comments,
//!    e.g. an "LGTM" or "Please address inline") — same: review-wide
//!    kata comments. The review state (`APPROVED` etc.) is
//!    prepended to the body so reviewers know what kata is showing
//!    them.
//! 3. **Review threads** — each thread becomes one kata comment
//!    (the anchor comment) plus N kata responses (the replies).
//!    Anchors are line-level when GitHub gives us a valid
//!    `(path, line, side, commit_id)`; degrade to review-wide if
//!    the SHA isn't resolvable in the local jj store.
//!
//! Idempotency: every imported item lands in `github_comment_map`
//! keyed by its GraphQL node id. A second import pass skips
//! anything already mapped, which is what makes phase 5.5 refresh
//! cheap and safe to re-run.
//!
//! Authorship: imported items keep their GitHub identity in
//! [`Comment::external_author`] (for UI rendering) and assign a
//! synthetic ghost [`Author`] of the form `gh:<login>` (for the
//! storage-layer structural identity that gates session ownership).
//! All imported sessions are inserted in the `published` state —
//! no draft-publish round-trip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kata_core::{
    Author, ChangeId, Comment, CommentId, CommitId, ExternalAuthor, Flag, LineRange,
    ResolutionAction, Response, ReviewId, ReviewManifest, SCHEMA_VERSION, Session,
    SessionId, SessionStatus, Side,
};
use kata_jj::JjBackend;
use kata_storage::{GithubCommentMapping, Storage};
use serde::Deserialize;

use super::client::{GithubClient, GithubClientExt, GithubError};
use super::url::PullRequestRef;
use crate::error::{ServiceError, ServiceResult};

/// GraphQL query the importer fires once per PR. Returns everything
/// kata needs in one round-trip: top-level conversation comments,
/// review summaries, and inline review threads (with `isResolved`
/// state and per-comment line anchors).
///
/// Pagination: each connection is fetched at `first: 100`. Real PRs
/// rarely exceed that on any of the three lists; if one does, the
/// importer logs and proceeds with the first 100 (phase 5.5 will
/// follow `pageInfo.endCursor`).
const PR_DISCUSSION_QUERY: &str = r#"
query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      comments(first: 100) {
        nodes {
          id databaseId body createdAt
          author { login ... on User { databaseId avatarUrl url } }
        }
        pageInfo { hasNextPage }
      }
      reviews(first: 100) {
        nodes {
          id databaseId body state submittedAt
          author { login ... on User { databaseId avatarUrl url } }
        }
        pageInfo { hasNextPage }
      }
      reviewThreads(first: 100) {
        nodes {
          id isResolved isOutdated path
          line startLine originalLine originalStartLine
          diffSide
          resolvedBy { login ... on User { databaseId avatarUrl url } }
          comments(first: 100) {
            nodes {
              id databaseId body createdAt
              originalCommit { oid }
              commit { oid }
              replyTo { id }
              author { login ... on User { databaseId avatarUrl url } }
            }
            pageInfo { hasNextPage }
          }
        }
        pageInfo { hasNextPage }
      }
    }
  }
}"#;

#[derive(Debug, Deserialize)]
struct GqlRoot {
    repository: Option<GqlRepository>,
}

#[derive(Debug, Deserialize)]
struct GqlRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<GqlPullRequest>,
}

#[derive(Debug, Deserialize)]
struct GqlPullRequest {
    comments: GqlConnection<GqlIssueComment>,
    reviews: GqlConnection<GqlReview>,
    #[serde(rename = "reviewThreads")]
    review_threads: GqlConnection<GqlReviewThread>,
}

#[derive(Debug, Deserialize)]
struct GqlConnection<T> {
    nodes: Vec<T>,
    #[serde(default, rename = "pageInfo")]
    page_info: Option<GqlPageInfo>,
}

#[derive(Debug, Deserialize)]
struct GqlPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Debug, Deserialize)]
struct GqlActor {
    login: String,
    // Present on `User`; absent for bots, deleted users, etc.
    #[serde(default, rename = "databaseId")]
    database_id: Option<i64>,
    #[serde(default, rename = "avatarUrl")]
    avatar_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GqlIssueComment {
    id: String,
    #[serde(rename = "databaseId")]
    database_id: Option<i64>,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    author: Option<GqlActor>,
}

#[derive(Debug, Deserialize)]
struct GqlReview {
    id: String,
    #[serde(rename = "databaseId")]
    database_id: Option<i64>,
    body: String,
    state: String,
    #[serde(rename = "submittedAt")]
    submitted_at: Option<DateTime<Utc>>,
    author: Option<GqlActor>,
}

#[derive(Debug, Deserialize)]
struct GqlReviewThread {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    #[serde(default, rename = "isOutdated")]
    is_outdated: bool,
    // GraphQL hoists the anchor onto the thread (one path/line per
    // thread; comments inside the thread all share it). REST does
    // the opposite — these fields live on each comment there.
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default, rename = "originalLine")]
    original_line: Option<u32>,
    #[serde(default, rename = "startLine")]
    start_line: Option<u32>,
    #[serde(default, rename = "originalStartLine")]
    original_start_line: Option<u32>,
    #[serde(default, rename = "diffSide")]
    diff_side: Option<String>,
    /// The user who flipped the thread to `isResolved`. `None` when
    /// the thread is open, or when GitHub can't surface the actor
    /// (very rare — typically a deleted account). We use this to
    /// attribute the synthetic kata `Resolve` response so the
    /// resolution shows up authored by the right person on import.
    #[serde(default, rename = "resolvedBy")]
    resolved_by: Option<GqlActor>,
    comments: GqlConnection<GqlThreadComment>,
}

#[derive(Debug, Deserialize)]
struct GqlThreadComment {
    id: String,
    #[serde(rename = "databaseId")]
    database_id: Option<i64>,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    #[serde(default, rename = "originalCommit")]
    original_commit: Option<GqlOid>,
    #[serde(default)]
    commit: Option<GqlOid>,
    #[serde(default, rename = "replyTo")]
    reply_to: Option<GqlNodeRef>,
    author: Option<GqlActor>,
}

#[derive(Debug, Deserialize)]
struct GqlOid {
    oid: String,
}

#[derive(Debug, Deserialize)]
struct GqlNodeRef {
    id: String,
}

/// What `import_pr_discussion` returns; mostly for logging /
/// future test assertions. The HTTP layer doesn't surface these
/// counts to the SPA — the user can see comments appear directly
/// in the imported review.
#[derive(Debug, Default, Clone)]
pub struct ImportCounts {
    pub issue_comments: usize,
    pub review_summaries: usize,
    pub threads: usize,
    pub thread_replies: usize,
    pub skipped_already_mapped: usize,
    /// Synthetic `Resolve` responses written when a GitHub thread
    /// is marked `isResolved`. These flip kata's resolution state on
    /// the anchor comment so the UI renders it as resolved (and
    /// collapsed by default) instead of just labelling it.
    pub thread_resolutions: usize,
}

/// Run the import. Caller guarantees the review exists and is
/// bound to `pr_ref` (i.e. created by phase 4's `import_github_pr`).
/// Best-effort per-item: a thread that fails to anchor doesn't
/// abort the whole import.
pub async fn import_pr_discussion(
    storage: &dyn Storage,
    jj: &dyn JjBackend,
    client: &dyn GithubClient,
    repo: &kata_core::RepoId,
    review: &ReviewManifest,
    pr_ref: &PullRequestRef,
) -> ServiceResult<ImportCounts> {
    let vars = serde_json::json!({
        "owner": pr_ref.owner,
        "repo": pr_ref.repo,
        "number": pr_ref.number,
    });
    let data: GqlRoot = client
        .graphql(PR_DISCUSSION_QUERY, vars)
        .await
        .map_err(github_to_service)?;
    let Some(pr) = data
        .repository
        .and_then(|r| r.pull_request)
    else {
        // PR went missing between create-review and import — rare
        // but not impossible (someone deleted the PR). Don't error
        // the whole flow; the review still exists in kata, just
        // without prior discussion.
        tracing::warn!(
            owner = %pr_ref.owner, repo = %pr_ref.repo, number = pr_ref.number,
            "PR not visible during comment import; skipping",
        );
        return Ok(ImportCounts::default());
    };

    let mut counts = ImportCounts::default();
    // One synthetic kata session per ghost author, reused for every
    // item that author contributed. The HashMap caches the id so
    // we don't INSERT-OR-IGNORE the same session row N times.
    let mut sessions: HashMap<String, SessionId> = HashMap::new();
    // Resolved git-SHA → (change_id, commit_id) so we don't call
    // `jj.resolve_endpoint` once per thread comment when most
    // comments share a SHA. `None` means "tried and failed" so we
    // can short-circuit to a review-wide degraded anchor.
    let mut sha_cache: HashMap<String, Option<(ChangeId, CommitId)>> = HashMap::new();
    // First-patchset endpoints, used as the anchor for review-wide
    // imports (issue comments + review summaries) and as the
    // fallback for inline threads whose anchor SHA isn't locally
    // resolvable. A review always has at least one patchset (kata
    // assigns it at create-review time), so the `None` arm here
    // genuinely shouldn't fire — surface it as an internal error
    // rather than handing back empty ids.
    let fallback_anchor = match review_first_patchset_anchor(review) {
        Some((c, k)) => (c.clone(), k.clone()),
        None => {
            return Err(ServiceError::Internal(
                "imported review has no patchset to anchor against".into(),
            ));
        }
    };

    // ---- 1. Top-level issue comments -----------------------------------
    if pr.comments.page_info.as_ref().map(|p| p.has_next_page).unwrap_or(false) {
        tracing::warn!(
            owner = %pr_ref.owner, repo = %pr_ref.repo, number = pr_ref.number,
            "PR has more than 100 issue comments; only the first page imported",
        );
    }
    for c in pr.comments.nodes {
        if storage.is_github_comment_mapped(repo, &c.id).await? {
            counts.skipped_already_mapped += 1;
            continue;
        }
        let Some(author_actor) = c.author.as_ref() else {
            // Deleted user / ghost — skip rather than invent an
            // identity. Their comment is gone from the UI too on
            // github.com.
            continue;
        };
        let session_id = ensure_ghost_session(
            storage,
            repo,
            &review.review_id,
            author_actor,
            &mut sessions,
        )
        .await?;
        let comment_id = kata_storage::ids::new_comment_id();
        let comment = build_review_wide_comment(
            comment_id.clone(),
            session_id,
            &review.review_id,
            review.current_patchset,
            ghost_author_for(author_actor),
            external_author_for(author_actor),
            c.created_at,
            c.body,
            fallback_anchor.0.clone(),
            fallback_anchor.1.clone(),
        );
        storage
            .raw_insert_comment_with_mapping(
                repo,
                &comment,
                &GithubCommentMapping {
                    github_node_id: c.id,
                    github_rest_id: c.database_id,
                    kind: "issue_comment".into(),
                    review_id: review.review_id.clone(),
                    pr_number: pr_ref.number,
                    kata_comment_id: Some(comment_id),
                    kata_response_id: None,
                    thread_node_id: None,
                },
            )
            .await?;
        counts.issue_comments += 1;
    }

    // ---- 2. Review summaries (the review.body, not threads) ------------
    if pr.reviews.page_info.as_ref().map(|p| p.has_next_page).unwrap_or(false) {
        tracing::warn!("PR has more than 100 reviews; only the first page imported");
    }
    for r in pr.reviews.nodes {
        if r.body.trim().is_empty() && r.state == "COMMENTED" {
            // Pure thread-only review — body is empty, the inline
            // comments are already accounted for under reviewThreads.
            continue;
        }
        // PENDING reviews are the viewer's own un-submitted draft
        // on github.com — not visible to anyone else and not yet
        // part of the PR's conversation. Skip them; otherwise
        // they'd import as a published kata comment timestamped
        // with `now()` (their `submitted_at` is null) for a
        // review that doesn't actually exist on the PR yet.
        if r.state == "PENDING" || r.submitted_at.is_none() {
            continue;
        }
        if storage.is_github_comment_mapped(repo, &r.id).await? {
            counts.skipped_already_mapped += 1;
            continue;
        }
        let Some(author_actor) = r.author.as_ref() else {
            continue;
        };
        let session_id = ensure_ghost_session(
            storage,
            repo,
            &review.review_id,
            author_actor,
            &mut sessions,
        )
        .await?;
        let body = if r.body.trim().is_empty() {
            format!("_Review submitted with no summary: **{}**_", r.state)
        } else {
            format!("**{}** — {}", r.state, r.body)
        };
        let created_at = r.submitted_at.unwrap_or_else(Utc::now);
        let comment_id = kata_storage::ids::new_comment_id();
        let comment = build_review_wide_comment(
            comment_id.clone(),
            session_id,
            &review.review_id,
            review.current_patchset,
            ghost_author_for(author_actor),
            external_author_for(author_actor),
            created_at,
            body,
            fallback_anchor.0.clone(),
            fallback_anchor.1.clone(),
        );
        storage
            .raw_insert_comment_with_mapping(
                repo,
                &comment,
                &GithubCommentMapping {
                    github_node_id: r.id,
                    github_rest_id: r.database_id,
                    kind: "review_summary".into(),
                    review_id: review.review_id.clone(),
                    pr_number: pr_ref.number,
                    kata_comment_id: Some(comment_id),
                    kata_response_id: None,
                    thread_node_id: None,
                },
            )
            .await?;
        counts.review_summaries += 1;
    }

    // ---- 3. Inline review threads --------------------------------------
    if pr.review_threads.page_info.as_ref().map(|p| p.has_next_page).unwrap_or(false) {
        tracing::warn!("PR has more than 100 review threads; only the first page imported");
    }
    for thread in pr.review_threads.nodes {
        // Capture everything we need from the thread struct up
        // front — once we move `thread.comments.nodes` below, the
        // partial-move rules forbid borrowing `&thread` again.
        let (file, lines, side) = anchor_geometry(&thread);
        let thread_id = thread.id;
        let thread_is_resolved = thread.is_resolved;
        let thread_is_outdated = thread.is_outdated;
        let thread_resolved_by = thread.resolved_by;
        // Inner pagination: a thread with >100 replies drops the
        // tail silently otherwise. Outer connections (issue
        // comments, reviews, reviewThreads) already warn on the
        // same signal; do the same here so truncation is at least
        // observable in logs.
        if thread
            .comments
            .page_info
            .as_ref()
            .map(|p| p.has_next_page)
            .unwrap_or(false)
        {
            tracing::warn!(
                thread = %thread_id,
                "review thread has more than 100 comments; only the first page imported",
            );
        }
        let mut thread_comments = thread.comments.nodes.into_iter();
        let Some(anchor) = thread_comments.next() else {
            continue;
        };
        if storage.is_github_comment_mapped(repo, &anchor.id).await? {
            counts.skipped_already_mapped += 1;
            // Still walk the replies — they may be new since last import.
            for reply in thread_comments {
                import_thread_reply(
                    storage,
                    repo,
                    &review.review_id,
                    pr_ref.number,
                    &thread_id,
                    &reply,
                    &mut sessions,
                    &mut counts,
                )
                .await?;
            }
            // If the thread flipped to resolved since the last
            // import — or was resolved at first import but we hadn't
            // shipped the synthetic-Resolve translation yet — write
            // the resolution now. Idempotent by the deterministic
            // mapping node id.
            if thread_is_resolved
                && let Some(parent_kata_id) =
                    lookup_kata_comment_id(storage, repo, &anchor.id).await?
            {
                maybe_insert_thread_resolution(
                    storage,
                    repo,
                    &review.review_id,
                    pr_ref.number,
                    &thread_id,
                    &parent_kata_id,
                    thread_resolved_by.as_ref(),
                    &mut sessions,
                    &mut counts,
                )
                .await?;
            }
            continue;
        }
        // Anchor resolution: prefer the comment's own commit_id
        // (where it was originally written), falling back to
        // original_commit, then the review fallback. The chosen SHA
        // is looked up in the local jj store to recover the
        // matching ChangeId.
        let sha = anchor
            .commit
            .as_ref()
            .map(|o| o.oid.clone())
            .or_else(|| anchor.original_commit.as_ref().map(|o| o.oid.clone()));
        // Try to resolve the thread's anchor commit to a local
        // change+commit pair. When that fails — the SHA is on a
        // branch the workspace hasn't fetched — degrade the
        // comment to review-wide rather than dropping it (the
        // text still has value even without a line anchor).
        let resolved = match &sha {
            Some(s) => resolve_sha_cached(jj, s, &mut sha_cache).await,
            None => None,
        };
        let (change_id, commit_id, anchored) = match resolved {
            Some(pair) => (pair.0, pair.1, true),
            None => (fallback_anchor.0.clone(), fallback_anchor.1.clone(), false),
        };
        let Some(author_actor) = anchor.author.as_ref() else {
            continue;
        };
        let session_id = ensure_ghost_session(
            storage,
            repo,
            &review.review_id,
            author_actor,
            &mut sessions,
        )
        .await?;
        let comment_id = kata_storage::ids::new_comment_id();
        // Geometry fields only set when the anchor resolved
        // locally. On fallback, the comment becomes review-wide so
        // kata's anchor resolver doesn't try (and fail) to place
        // it against a commit/file/line that don't line up.
        let (cmt_file, cmt_side, cmt_lines, review_wide) = if anchored {
            (file.clone(), side, lines, false)
        } else {
            (None, None, None, true)
        };
        let comment = Comment {
            schema_version: SCHEMA_VERSION,
            comment_id: comment_id.clone(),
            session_id,
            review_id: review.review_id.clone(),
            author: ghost_author_for(author_actor),
            created_at: anchor.created_at,
            patchset: review.current_patchset,
            anchor_change_id: change_id,
            anchor_commit_id: commit_id,
            file: cmt_file,
            side: cmt_side,
            lines: cmt_lines,
            columns: None,
            review_wide,
            // Flag is neutral on imported anchors — github.com has
            // no equivalent so we don't try to guess. The resolution
            // state translates to a synthetic `Resolve` response
            // below (when `thread_is_resolved`), which the UI's
            // resolution model picks up via `resolutionFor()`.
            flag: Flag::Question,
            // Only `_(outdated)_` survives as a body prefix —
            // kata has no first-class "outdated" state, so the
            // marker still earns its keep there. Resolution is
            // expressed via a synthetic Resolve response instead.
            body: if thread_is_outdated {
                format!("_(outdated)_\n\n{}", anchor.body)
            } else {
                anchor.body
            },
            external_author: external_author_for(author_actor),
        };
        storage
            .raw_insert_comment_with_mapping(
                repo,
                &comment,
                &GithubCommentMapping {
                    github_node_id: anchor.id,
                    github_rest_id: anchor.database_id,
                    kind: "line_comment".into(),
                    review_id: review.review_id.clone(),
                    pr_number: pr_ref.number,
                    kata_comment_id: Some(comment_id.clone()),
                    kata_response_id: None,
                    thread_node_id: Some(thread_id.clone()),
                },
            )
            .await?;
        counts.threads += 1;
        // Replies → kata responses.
        for reply in thread_comments {
            import_thread_reply_inner(
                storage,
                repo,
                &review.review_id,
                pr_ref.number,
                &thread_id,
                &comment_id,
                &reply,
                &mut sessions,
                &mut counts,
            )
            .await?;
        }
        // Resolved threads land a synthetic `Resolve` response so
        // kata's resolution model sees the thread as resolved
        // (which in turn makes the UI render it collapsed by
        // default). Attribution goes to the GitHub user who clicked
        // "Resolve conversation"; ghost author falls back to the
        // anchor's author when that's missing (very rare).
        if thread_is_resolved {
            maybe_insert_thread_resolution(
                storage,
                repo,
                &review.review_id,
                pr_ref.number,
                &thread_id,
                &comment_id,
                thread_resolved_by.as_ref(),
                &mut sessions,
                &mut counts,
            )
            .await?;
        }
    }

    Ok(counts)
}

/// Top-level wrapper used when the anchor was already mapped on a
/// prior import — we still want to capture any new replies.
#[allow(clippy::too_many_arguments)]
async fn import_thread_reply(
    storage: &dyn Storage,
    repo: &kata_core::RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    thread_node_id: &str,
    reply: &GqlThreadComment,
    sessions: &mut HashMap<String, SessionId>,
    counts: &mut ImportCounts,
) -> ServiceResult<()> {
    if storage.is_github_comment_mapped(repo, &reply.id).await? {
        counts.skipped_already_mapped += 1;
        return Ok(());
    }
    // We need the kata anchor comment's id to attach the response.
    // Look it up via the mapping: the parent thread's anchor must
    // already be mapped (we set it on a previous import pass).
    // For phase 5 simplicity, the reply's `reply_to` GraphQL field
    // points at the parent's node id; we look up our mapping for
    // that to find the kata comment id.
    let parent_node = reply
        .reply_to
        .as_ref()
        .map(|r| r.id.as_str())
        .unwrap_or(thread_node_id);
    let parent_comment = lookup_kata_comment_id(storage, repo, parent_node).await?;
    let Some(parent_comment_id) = parent_comment else {
        tracing::warn!(
            reply_node = %reply.id,
            parent_node = %parent_node,
            "thread reply with no mapped parent; skipping",
        );
        return Ok(());
    };
    import_thread_reply_inner(
        storage,
        repo,
        review_id,
        pr_number,
        thread_node_id,
        &parent_comment_id,
        reply,
        sessions,
        counts,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn import_thread_reply_inner(
    storage: &dyn Storage,
    repo: &kata_core::RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    thread_node_id: &str,
    parent_comment_id: &CommentId,
    reply: &GqlThreadComment,
    sessions: &mut HashMap<String, SessionId>,
    counts: &mut ImportCounts,
) -> ServiceResult<()> {
    if storage.is_github_comment_mapped(repo, &reply.id).await? {
        counts.skipped_already_mapped += 1;
        return Ok(());
    }
    let Some(author_actor) = reply.author.as_ref() else {
        return Ok(());
    };
    let session_id = ensure_ghost_session(
        storage,
        repo,
        review_id,
        author_actor,
        sessions,
    )
    .await?;
    let response_id = kata_storage::ids::new_response_id();
    let response = Response {
        schema_version: SCHEMA_VERSION,
        response_id: response_id.clone(),
        in_reply_to: parent_comment_id.clone(),
        session_id,
        author: ghost_author_for(author_actor),
        created_at: reply.created_at,
        // Replies on github.com carry no explicit resolution
        // semantic — they're just text. Use `Comment` (the kata
        // "no-action" action).
        action: ResolutionAction::Comment,
        body: reply.body.clone(),
    };
    storage
        .raw_insert_response_with_mapping(
            repo,
            &response,
            &GithubCommentMapping {
                github_node_id: reply.id.clone(),
                github_rest_id: reply.database_id,
                kind: "thread_reply".into(),
                review_id: review_id.clone(),
                pr_number,
                kata_comment_id: None,
                kata_response_id: Some(response_id),
                thread_node_id: Some(thread_node_id.to_owned()),
            },
        )
        .await?;
    counts.thread_replies += 1;
    Ok(())
}

/// Write a synthetic `Resolve` response on the anchor comment so
/// kata's resolution model treats the thread as resolved. Keyed by
/// a deterministic node id derived from the thread node id, which
/// makes refresh imports idempotent — a second pass over an
/// already-resolved thread is a no-op via `is_github_comment_mapped`.
///
/// Attribution: when GitHub gives us `resolvedBy`, the response is
/// authored by `gh:<login>` and carries the external author for UI
/// rendering. When it doesn't (rare — bot, deleted account, etc.),
/// we fall back to a generic `gh:github` ghost rather than skipping
/// the resolution, so the thread state still translates.
#[allow(clippy::too_many_arguments)]
async fn maybe_insert_thread_resolution(
    storage: &dyn Storage,
    repo: &kata_core::RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    thread_node_id: &str,
    parent_comment_id: &CommentId,
    resolved_by: Option<&GqlActor>,
    sessions: &mut HashMap<String, SessionId>,
    counts: &mut ImportCounts,
) -> ServiceResult<()> {
    let synth_node_id = thread_resolution_node_id(thread_node_id);
    if storage.is_github_comment_mapped(repo, &synth_node_id).await? {
        // Already imported on a prior pass.
        return Ok(());
    }
    let (author, external_author, session_id) = match resolved_by {
        Some(actor) => {
            let s = ensure_ghost_session(storage, repo, review_id, actor, sessions).await?;
            (ghost_author_for(actor), external_author_for(actor), s)
        }
        None => {
            // GitHub gave us a resolved thread but no `resolvedBy`
            // — typically a bot or deleted account. A
            // `gh:github` ghost session keeps the state translation
            // intact without inventing an identity.
            let placeholder = GqlActor {
                login: "github".into(),
                database_id: None,
                avatar_url: None,
                url: None,
            };
            let s = ensure_ghost_session(storage, repo, review_id, &placeholder, sessions).await?;
            (ghost_author_for(&placeholder), None, s)
        }
    };
    // Deterministic response id so the row itself is also idempotent
    // — `raw_insert_response_with_mapping` is a single-tx insert,
    // and a duplicate PK on the response would surface as an error.
    let response_id = kata_core::ResponseId::new(format!("gh-resolve-{thread_node_id}"));
    let response = Response {
        schema_version: SCHEMA_VERSION,
        response_id: response_id.clone(),
        in_reply_to: parent_comment_id.clone(),
        session_id,
        author,
        // No timestamp from GitHub on resolvedBy (the API doesn't
        // expose it on review threads). Use `now()` — the kata
        // resolution model only cares about ordering relative to
        // other responses on the same comment, and the synthetic
        // resolve is by construction the latest action.
        created_at: Utc::now(),
        action: ResolutionAction::Resolve,
        body: String::new(),
    };
    // External author for the UI ride along on the *Response*?
    // Today kata doesn't surface external_author on responses —
    // the synthetic resolve renders as the `gh:<login>` ghost's
    // structural identity. That's acceptable: the UI mainly
    // surfaces who resolved via the resolution state line in
    // CommentThread, which already special-cases ghost authors.
    let _ = external_author; // reserved for future response.external_author wiring
    storage
        .raw_insert_response_with_mapping(
            repo,
            &response,
            &GithubCommentMapping {
                github_node_id: synth_node_id,
                github_rest_id: None,
                kind: "thread_resolution".into(),
                review_id: review_id.clone(),
                pr_number,
                kata_comment_id: None,
                kata_response_id: Some(response_id),
                thread_node_id: Some(thread_node_id.to_owned()),
            },
        )
        .await?;
    counts.thread_resolutions += 1;
    Ok(())
}

fn thread_resolution_node_id(thread_node_id: &str) -> String {
    // Distinct namespace so a future GitHub change that surfaces a
    // real "resolution" node id can't collide with the synthetic.
    format!("kata-import-resolution:{thread_node_id}")
}

async fn lookup_kata_comment_id(
    storage: &dyn Storage,
    repo: &kata_core::RepoId,
    node_id: &str,
) -> ServiceResult<Option<CommentId>> {
    Ok(storage
        .lookup_github_mapping_by_node_id(repo, node_id)
        .await?
        .and_then(|m| m.kata_comment_id))
}

fn ensure_ghost_session_blocking(
    sessions: &mut HashMap<String, SessionId>,
    author_login: &str,
) -> Option<SessionId> {
    sessions.get(author_login).cloned()
}

async fn ensure_ghost_session(
    storage: &dyn Storage,
    repo: &kata_core::RepoId,
    review_id: &ReviewId,
    actor: &GqlActor,
    sessions: &mut HashMap<String, SessionId>,
) -> ServiceResult<SessionId> {
    if let Some(id) = ensure_ghost_session_blocking(sessions, &actor.login) {
        return Ok(id);
    }
    let ghost = ghost_author_for(actor);
    let session_id = kata_storage::ids::new_session_id();
    let session = Session {
        schema_version: SCHEMA_VERSION,
        session_id: session_id.clone(),
        review_id: review_id.clone(),
        author: ghost,
        // Imported items are immediately visible — no draft round-
        // trip. Reviewers see prior discussion the moment they open
        // the freshly-imported review.
        status: SessionStatus::Published,
        created_at: Utc::now(),
        published_at: Some(Utc::now()),
    };
    storage.raw_insert_session(repo, &session).await?;
    sessions.insert(actor.login.clone(), session_id.clone());
    Ok(session_id)
}

fn ghost_author_for(actor: &GqlActor) -> Author {
    // `gh:<login>` so the structural identity is unique-but-
    // grep-able. The user-visible identity is `external_author`.
    Author::new(format!("gh:{}", actor.login))
}

fn external_author_for(actor: &GqlActor) -> Option<ExternalAuthor> {
    Some(ExternalAuthor {
        source: "github".into(),
        login: actor.login.clone(),
        id: actor.database_id.unwrap_or(0),
        avatar_url: actor.avatar_url.clone(),
        html_url: actor.url.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn build_review_wide_comment(
    comment_id: CommentId,
    session_id: SessionId,
    review_id: &ReviewId,
    patchset: u32,
    author: Author,
    external_author: Option<ExternalAuthor>,
    created_at: DateTime<Utc>,
    body: String,
    change_id: ChangeId,
    commit_id: CommitId,
) -> Comment {
    Comment {
        schema_version: SCHEMA_VERSION,
        comment_id,
        session_id,
        review_id: review_id.clone(),
        author,
        created_at,
        patchset,
        anchor_change_id: change_id,
        anchor_commit_id: commit_id,
        file: None,
        side: None,
        lines: None,
        columns: None,
        review_wide: true,
        flag: Flag::Question,
        body,
        external_author,
    }
}

fn anchor_geometry(t: &GqlReviewThread) -> (Option<String>, Option<LineRange>, Option<Side>) {
    let file = t.path.clone();
    // Prefer the thread's *current* line (post-refresh) — falls
    // back to the original-at-write-time line, then to no anchor.
    // For multi-line ranges, GitHub gives us (startLine, line).
    let start = t.start_line.or(t.original_start_line);
    let end = t.line.or(t.original_line);
    let lines = match (start, end) {
        (Some(s), Some(e)) if s <= e => Some(LineRange::new(s, e)),
        (None, Some(e)) => Some(LineRange::new(e, e)),
        _ => None,
    };
    // GitHub's `diffSide` is `LEFT` or `RIGHT`. Map to kata's
    // `Side`. `null` (no side) happens for whole-file comments —
    // we can't represent that with our line-anchor type today, so
    // degrade to no side; the UI will treat it as right (new).
    let side = t.diff_side.as_deref().and_then(|s| match s {
        "LEFT" => Some(Side::Base),
        "RIGHT" => Some(Side::Tip),
        _ => None,
    });
    (file, lines, side)
}

async fn resolve_sha_cached(
    jj: &dyn JjBackend,
    sha: &str,
    cache: &mut HashMap<String, Option<(ChangeId, CommitId)>>,
) -> Option<(ChangeId, CommitId)> {
    if let Some(entry) = cache.get(sha) {
        return entry.clone();
    }
    let resolved = match jj.resolve_endpoint(sha).await {
        Ok(Some(ep)) => Some((ep.change_id, ep.commit_id)),
        _ => None,
    };
    cache.insert(sha.to_owned(), resolved.clone());
    resolved
}

fn review_first_patchset_anchor(review: &ReviewManifest) -> Option<(&ChangeId, &CommitId)> {
    // The PR-head endpoint of patchset 1 is the canonical "I have
    // no better anchor" target for review-wide imported comments.
    let ps = review.patchsets.first()?;
    Some((&ps.tip_change, &ps.tip_commit))
}

fn github_to_service(err: GithubError) -> ServiceError {
    crate::github_error_to_service(err)
}
