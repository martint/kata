//! Publish a kata session as a GitHub PR review.
//!
//! Used when the kata review is bound to a GitHub PR
//! ([`ReviewManifest::github_pr`] is `Some`). The flow:
//!
//! 1. Re-fetch the PR head SHA and refuse if it moved since import
//!    — line-anchored comments would land on the wrong content.
//! 2. Partition the session's drafts into:
//!    - **replies** to imported threads (kata response with
//!      `in_reply_to = <imported anchor>`) → posted individually
//!      via `POST /pulls/N/comments` with `in_reply_to`. Posted
//!      first so they appear in thread order on github.com.
//!    - **new inline comments** (kata Comment with file+lines+side,
//!      `review_wide = false`) → bundled into the GH review
//!      payload's `comments[]` array.
//!    - **review-wide kata comments** (no file/lines, OR
//!      `review_wide = true`) → posted as issue comments via
//!      `POST /issues/N/comments`. Pre-publish, they're separate
//!      kata items; on GH they have no review-bundling story.
//! 3. `POST /pulls/N/reviews` with `event = "COMMENT"` (MVP —
//!    APPROVE / REQUEST_CHANGES queued for phase 6.5).
//! 4. After all GH calls succeed, mark the kata session published
//!    locally so the drafts become visible to other viewers. The
//!    new GH ids land in `github_comment_map` so subsequent
//!    refreshes don't re-import them.
//!
//! Failure handling: we partial-state on the GH side (no atomic
//! transactions across HTTP calls). If anything errors mid-publish,
//! the kata session stays in draft and we log what made it through.
//! A retry will skip the items already mapped.

use kata_core::{
    Author, Comment, RepoId, ResolutionAction, Response, ReviewId, ReviewManifest, SessionId, Side,
};
use kata_storage::{GithubCommentMapping, Storage};
use serde::Deserialize;

use super::client::{GithubClient, GithubClientExt, GithubError};
use super::url::PullRequestRef;
use crate::error::{ServiceError, ServiceResult};

/// One-line outcome of a successful publish, surfaced to the
/// caller (and ultimately the SPA) so the user can see what kata
/// pushed without re-loading the review from scratch.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PublishCounts {
    pub new_inline_comments: usize,
    pub replies: usize,
    pub issue_comments: usize,
    /// Drafts skipped because a `github_comment_map` row already
    /// exists for them — i.e. an earlier publish attempt landed
    /// them on github.com but the surrounding session never moved
    /// to Published (mid-publish failure). The retry skips them
    /// instead of double-posting.
    #[serde(default)]
    pub skipped_already_published: usize,
    /// Number of `resolveReviewThread` / `unresolveReviewThread`
    /// GraphQL mutations that fired successfully. A non-Comment
    /// response on a thread-anchored parent triggers one; a
    /// resolve-only response (action != Comment AND empty body)
    /// produces one without also posting a comment.
    #[serde(default)]
    pub resolutions: usize,
    /// Resolution actions we couldn't translate to github.com because
    /// the parent comment has no thread anchor (issue comment or
    /// review summary parent). Recorded so the SPA can surface a
    /// "N resolutions dropped" hint instead of silently swallowing
    /// them.
    #[serde(default)]
    pub resolutions_dropped_no_thread: usize,
    /// The GH `event` we submitted the review with — echoed back
    /// so the SPA can confirm the choice without parsing the body.
    pub event: String,
}

/// The 3 review submission states GitHub accepts. Wire form
/// matches GitHub exactly (uppercase).
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PublishEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl PublishEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "COMMENT",
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreatedReview {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreatedComment {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    node_id: Option<String>,
}

/// One element of the array returned by
/// `GET /repos/{o}/{r}/pulls/{n}/reviews/{review_id}/comments`.
/// Used to recover the per-comment ids after the bundled-review
/// POST (which only returns the wrapping review's id).
#[derive(Debug, Deserialize)]
struct PostedReviewComment {
    id: i64,
    node_id: String,
    path: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    original_line: Option<u32>,
    #[serde(default)]
    original_start_line: Option<u32>,
    #[serde(default)]
    side: Option<String>,
    body: String,
}

/// Find the GitHub-returned comment that matches a kata-side
/// inline comment. We match on `(path, end-line, side, body)` —
/// the body is the strongest signal (it was the reviewer's
/// freshly-typed text) but path+line guard against unlikely
/// duplicate-body cases. `consumed` filters out posted comments
/// already matched earlier in the loop, so two kata comments with
/// identical `(path, line, side, body)` map to two distinct
/// posted comments instead of collapsing onto the first one.
/// Returns `None` when no match is found, which causes the
/// mapping to be skipped (with a warn log).
fn match_posted_comment<'a>(
    kata: &Comment,
    posted: &'a [PostedReviewComment],
    consumed: &std::collections::HashSet<&str>,
) -> Option<&'a PostedReviewComment> {
    let want_path = kata.file.as_deref()?;
    let want_lines = kata.lines.as_ref()?;
    let want_side = match kata.side {
        Some(Side::Base) => "LEFT",
        _ => "RIGHT",
    };
    posted.iter().find(|p| {
        !consumed.contains(p.node_id.as_str())
            && p.path == want_path
            && p.body == kata.body
            && p.side.as_deref().unwrap_or("RIGHT") == want_side
            && {
                let p_line = p.line.or(p.original_line);
                let p_start = p.start_line.or(p.original_start_line);
                p_line == Some(want_lines.end)
                    && (p_start.is_none() || p_start == Some(want_lines.start))
            }
    })
}

/// Run the publish.
#[allow(clippy::too_many_arguments)]
pub async fn publish_session_to_github(
    storage: &dyn Storage,
    client: &dyn GithubClient,
    repo: &RepoId,
    review: &ReviewManifest,
    session_id: &SessionId,
    author: &Author,
    event: PublishEvent,
    body: Option<String>,
) -> ServiceResult<PublishCounts> {
    let github_pr = review
        .github_pr
        .as_ref()
        .ok_or_else(|| ServiceError::BadRequest(
            "this review is not bound to a GitHub PR".into(),
        ))?
        .clone();
    let pr_ref = PullRequestRef {
        owner: github_pr.owner.clone(),
        repo: github_pr.repo.clone(),
        number: github_pr.number,
    };

    // ---- 1. Drift check -----------------------------------------
    // Only head drift is a hard refusal — the bundled review
    // anchors inline comments to `head.sha`, so a moved head would
    // mis-place them. Base drift is normal traffic on an active
    // target branch and doesn't affect what we post, so it gets a
    // warn log and the publish proceeds.
    let live_pr = client.fetch_pr(&pr_ref).await.map_err(github_to_service)?;
    if live_pr.head.sha != github_pr.original_head_sha {
        return Err(ServiceError::Conflict {
            kind: "head_drift".into(),
            message: format!(
                "the PR head moved since import (was {}, now {}). \
                 Re-import the PR to anchor inline comments against the new \
                 head, then publish.",
                short(&github_pr.original_head_sha),
                short(&live_pr.head.sha),
            ),
        });
    }
    if live_pr.base.sha != github_pr.original_base_sha {
        tracing::warn!(
            pr = github_pr.number,
            old_base = %short(&github_pr.original_base_sha),
            new_base = %short(&live_pr.base.sha),
            "PR base moved since import; publishing anyway \
             (inline comments anchor to head, not base)",
        );
    }

    // ---- 2. Load this author's drafts in this session -----------
    let drafts = storage
        .list_drafts_for(repo, &review.review_id, author)
        .await?;
    // Refuse an empty publish only when the event is the neutral
    // COMMENT — that variant carries no signal of its own, so with
    // no drafts and no body there's literally nothing to send and
    // the refusal is a hard input-validation error. APPROVE and
    // REQUEST_CHANGES *are* the signal: submitting them with no
    // drafts and no body is a valid verdict-only review (a plain
    // "LGTM" via the toolbar's Quick Approve button, for example),
    // so let those through.
    if matches!(event, PublishEvent::Comment)
        && drafts.comments.is_empty()
        && drafts.responses.is_empty()
        && body.is_none()
    {
        return Err(ServiceError::BadRequest(
            "no draft comments, responses, or review body to publish".into(),
        ));
    }

    // ---- 3. Partition ------------------------------------------
    // Three buckets:
    //  * `bundled_inline` — RIGHT-side inline comments, bundled
    //    into the wrapping review's `comments[]` (single POST).
    //  * `left_inline` — LEFT-side inline comments, posted
    //    individually against the base commit (the bundled-review
    //    path 422s when the LEFT line doesn't line up at the
    //    wrapping commit_id; individual posts let us pass the
    //    correct `commit_id` per comment, and fall back to an
    //    issue comment if even the individual post 422s).
    //  * `issue_comments` — review-wide kata comments (no file/
    //    lines, or `review_wide`) → issue comments on the PR.
    let mut bundled_inline: Vec<&Comment> = Vec::new();
    let mut left_inline: Vec<&Comment> = Vec::new();
    let mut issue_comments: Vec<&Comment> = Vec::new();
    for c in &drafts.comments {
        if c.review_wide || c.file.is_none() || c.lines.is_none() {
            issue_comments.push(c);
        } else if matches!(c.side, Some(Side::Base)) {
            left_inline.push(c);
        } else {
            bundled_inline.push(c);
        }
    }

    let mut counts = PublishCounts {
        event: event.as_str().to_owned(),
        ..Default::default()
    };

    // ---- 4. Replies first (so they appear above the bundled review on GH) ----
    //
    // Each response carries up to two pieces of GH-side work: a
    // *reply post* (when the body is non-empty) and a *resolution
    // side-effect* (when the action isn't `Comment`). We track them
    // as separate mapping rows so a retry after one succeeded and
    // the other failed can pick up the pending piece without redoing
    // the completed one — critical for the resolve-with-text case
    // (reply lands, then the mutation errors: without a separate
    // gate the retry would silently skip the mutation forever).
    for r in &drafts.responses {
        let parent_mapping = storage
            .lookup_github_mapping_by_kata_comment(repo, &r.in_reply_to)
            .await?;
        let needs_reply_post = !r.body.trim().is_empty();
        let needs_resolution = !matches!(r.action, ResolutionAction::Comment);
        // Kind-scoped lookups so the two pieces of work track their
        // own idempotency independently. The reply post lands under
        // "thread_reply" or "issue_comment" (whichever endpoint it
        // used); the resolution side-effect lands under "resolution".
        let reply_thread_mapping = storage
            .lookup_github_mapping_by_kata_response_kind(repo, &r.response_id, "thread_reply")
            .await?;
        let reply_issue_mapping = storage
            .lookup_github_mapping_by_kata_response_kind(repo, &r.response_id, "issue_comment")
            .await?;
        let reply_done = reply_thread_mapping.is_some() || reply_issue_mapping.is_some();
        let resolution_done = storage
            .lookup_github_mapping_by_kata_response_kind(repo, &r.response_id, "resolution")
            .await?
            .is_some();
        let reply_pending = needs_reply_post && !reply_done;
        let resolution_pending = needs_resolution && !resolution_done;
        if !reply_pending && !resolution_pending {
            counts.skipped_already_published += 1;
            continue;
        }
        // 4a. Reply post (skipped when the response is resolve-only
        //     or the reply mapping already exists from a prior pass).
        if reply_pending {
            // Only review-thread comments accept `in_reply_to` on
            // `/pulls/N/comments`. Anything else (no parent mapping,
            // or parent mapped to an issue comment / review summary)
            // → post as an issue comment quoting the parent for
            // context, the same way GitHub's own "Quote reply" UI
            // does. Threaded posts that 422 fall through to the
            // same quoted-issue-comment path so the reply isn't
            // silently lost.
            let is_thread = parent_mapping
                .as_ref()
                .map(|m| matches!(m.kind.as_str(), "line_comment" | "thread_reply"))
                .unwrap_or(false);
            let mut quoted_fallback_reason: Option<&'static str> = (!is_thread).then(|| {
                if parent_mapping.is_some() { "non_thread_parent" } else { "no_mapping" }
            });
            let mut posted_via_thread: Option<CreatedComment> = None;
            if is_thread {
                if let Some(rest_id) = parent_mapping.as_ref().and_then(|m| m.github_rest_id) {
                    match client
                        .post::<CreatedComment>(
                            &format!(
                                "repos/{}/{}/pulls/{}/comments",
                                pr_ref.owner, pr_ref.repo, pr_ref.number
                            ),
                            &serde_json::json!({
                                "body": r.body,
                                "in_reply_to": rest_id,
                            }),
                        )
                        .await
                    {
                        Ok(p) => posted_via_thread = Some(p),
                        Err(GithubError::Validation { stderr }) => {
                            tracing::warn!(
                                kata_response = %r.response_id,
                                error = %stderr,
                                "threaded reply rejected by github (422); \
                                 falling back to a quoted issue comment",
                            );
                            quoted_fallback_reason = Some("threaded_422");
                        }
                        Err(e) => return Err(github_to_service(e)),
                    }
                } else {
                    // Parent mapping recorded only the GraphQL node
                    // id, not the REST id needed for `in_reply_to`.
                    // Shouldn't happen for thread-anchor mappings
                    // written by the import path, but handle
                    // gracefully.
                    quoted_fallback_reason = Some("missing_rest_id");
                }
            }
            if let Some(posted) = posted_via_thread {
                record_mapping_for_response(
                    storage,
                    repo,
                    &review.review_id,
                    github_pr.number,
                    r,
                    posted.node_id,
                    posted.id,
                    "thread_reply",
                    parent_mapping.as_ref().and_then(|m| m.thread_node_id.clone()),
                )
                .await?;
            } else {
                // Quoted-issue-comment path. Look up the parent
                // comment for a real `> @author wrote:` quote so the
                // reader sees what we're responding to even without
                // thread context on github. The parent-mapping (when
                // present) names the kata comment that mirrors the
                // imported one; otherwise the reply targets a native
                // kata comment and we look it up directly.
                let parent_id = parent_mapping
                    .as_ref()
                    .and_then(|m| m.kata_comment_id.clone())
                    .unwrap_or_else(|| r.in_reply_to.clone());
                let parent = storage.get_comment_by_id(repo, &parent_id).await.ok().flatten();
                let body = build_quoted_reply_body(
                    parent.as_ref(),
                    &r.body,
                    quoted_fallback_reason,
                );
                let posted: CreatedComment = client
                    .post(
                        &format!(
                            "repos/{}/{}/issues/{}/comments",
                            pr_ref.owner, pr_ref.repo, pr_ref.number
                        ),
                        &serde_json::json!({ "body": body }),
                    )
                    .await
                    .map_err(github_to_service)?;
                record_mapping_for_response(
                    storage,
                    repo,
                    &review.review_id,
                    github_pr.number,
                    r,
                    posted.node_id,
                    posted.id,
                    "issue_comment",
                    parent_mapping.as_ref().and_then(|m| m.thread_node_id.clone()),
                )
                .await?;
            }
            counts.replies += 1;
        }
        // 4b. Resolution side-effect (gated independently of the
        //     reply post so a retry after 4a succeeded but 4b
        //     errored actually re-fires the mutation). Order in the
        //     fresh case: reply first so the explanation shows above
        //     the resolved event in the github.com timeline.
        if resolution_pending {
            apply_resolution_side_effect(
                client,
                storage,
                repo,
                &review.review_id,
                github_pr.number,
                parent_mapping.as_ref(),
                r,
                &mut counts,
            )
            .await?;
        }
    }

    // ---- 5. Issue-comment-style kata comments ------------------
    for c in &issue_comments {
        if storage
            .lookup_github_mapping_by_kata_comment(repo, &c.comment_id)
            .await?
            .is_some()
        {
            counts.skipped_already_published += 1;
            continue;
        }
        let posted: CreatedComment = client
            .post(
                &format!(
                    "repos/{}/{}/issues/{}/comments",
                    pr_ref.owner, pr_ref.repo, pr_ref.number
                ),
                &serde_json::json!({ "body": c.body }),
            )
            .await
            .map_err(github_to_service)?;
        record_mapping_for_comment(
            storage,
            repo,
            &review.review_id,
            github_pr.number,
            c,
            posted.node_id,
            posted.id,
            "issue_comment",
            None,
        )
        .await?;
        counts.issue_comments += 1;
    }

    // ---- 6. LEFT-side inline comments ----------------------------
    // Posted individually with `commit_id = live_pr.head.sha` and
    // `side = "LEFT"` — GitHub's documented pattern for anchoring a
    // comment to the original/left side of the diff at the PR's
    // head. (Using the PR's base SHA as commit_id 422s every time
    // because the base commit isn't part of the PR's own commit
    // list.) A 422 here still falls back to a quoted issue comment
    // whose footer link points at the base SHA so the reader can
    // navigate to the pre-PR version of the file.
    for c in &left_inline {
        if storage
            .lookup_github_mapping_by_kata_comment(repo, &c.comment_id)
            .await?
            .is_some()
        {
            counts.skipped_already_published += 1;
            continue;
        }
        post_inline_with_issue_fallback(
            client,
            storage,
            repo,
            &review.review_id,
            &pr_ref,
            c,
            &live_pr.head.sha,
            &github_pr.original_base_sha,
            "LEFT",
        )
        .await?;
        counts.new_inline_comments += 1;
    }

    // ---- 7. Bundled review (RIGHT-side inline) + review body ----
    // Pre-filter the bundle for idempotency: drop any RIGHT-side
    // inlines that already have a `github_comment_map` row from a
    // prior attempt. The wrapping review POST itself is not
    // idempotent on github.com (a retry creates a duplicate
    // `pull_request_review` even if every inline inside is the
    // same), but skipping previously-mapped inlines means the
    // wrapping review only gets re-created when there's actually
    // new work for it to carry.
    let mut bundled_to_post: Vec<&Comment> = Vec::with_capacity(bundled_inline.len());
    for c in &bundled_inline {
        if storage
            .lookup_github_mapping_by_kata_comment(repo, &c.comment_id)
            .await?
            .is_some()
        {
            counts.skipped_already_published += 1;
        } else {
            bundled_to_post.push(c);
        }
    }

    // Review-body idempotency: if a prior attempt already landed the
    // wrapping review's body (either via /reviews or, in the deep
    // double-422 path, as an issue comment), strip the body from
    // this attempt so we don't double-post it. The body is per-
    // publish-call and there's no kata item to dedup it against, so
    // we record it under a synthetic `kind=review_body` mapping
    // keyed off the kata review id.
    let body_already_posted = storage
        .lookup_review_body_mapping(repo, &review.review_id, github_pr.number)
        .await?
        .is_some();
    let effective_body: Option<&str> = body
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .filter(|_| !body_already_posted);
    if body_already_posted && body.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        counts.skipped_already_published += 1;
    }

    // Fire the wrapping review when there's inline work to bundle,
    // when the user supplied a body to attach, OR when the event is
    // non-neutral (APPROVE / REQUEST_CHANGES) and we haven't already
    // submitted the wrapping review on a prior attempt. The event
    // check is what carries the user's approval choice — without it,
    // an APPROVE with no fresh inlines and no body would silently
    // drop on the floor because the POST never fires. Idempotency
    // for the event-only path rides on the same `review_body`
    // mapping row the body path uses (the mapping is now written
    // on every successful wrapping POST, not only body-carrying ones).
    let event_pending = !matches!(event, PublishEvent::Comment) && !body_already_posted;
    if !bundled_to_post.is_empty() || effective_body.is_some() || event_pending {
        let inline_payloads: Vec<serde_json::Value> = bundled_to_post
            .iter()
            .map(|c| build_inline_payload(c))
            .collect();
        let mut payload = serde_json::json!({
            "commit_id": live_pr.head.sha,
            "event": event.as_str(),
            "comments": inline_payloads,
        });
        if let Some(b) = effective_body {
            payload["body"] = serde_json::Value::String(b.to_owned());
        }
        let bundled_endpoint = format!(
            "repos/{}/{}/pulls/{}/reviews",
            pr_ref.owner, pr_ref.repo, pr_ref.number,
        );
        match client.post::<CreatedReview>(&bundled_endpoint, &payload).await {
            Ok(posted) => {
                // Record the review-body mapping on every successful
                // wrapping POST — not just those that carried a body.
                // The mapping is the durable "we already submitted the
                // wrapping review for this session" marker, which the
                // event-pending path (APPROVE / REQUEST_CHANGES with
                // no body) leans on for retry idempotency. Without
                // this, a retry after mid-publish failure would fire
                // a second APPROVE review on github.com.
                if let Some(node_id) = posted.node_id.clone() {
                    record_review_body_mapping(
                        storage,
                        repo,
                        &review.review_id,
                        github_pr.number,
                        node_id,
                        posted.id,
                    )
                    .await?;
                }
                // The /reviews POST response carries only the wrapping
                // review's id — not the per-comment ids. Follow up with a
                // GET to fetch the per-comment ids and record one mapping
                // row per kata comment, keyed on its individual github
                // node id. Without this, the next import would re-pull
                // our own comments as ghosts of ourselves.
                let review_id_for_lookup = posted.id.ok_or_else(|| {
                    ServiceError::Internal(
                        "GitHub returned no review id for the bundled review; \
                         inline comment mappings cannot be recorded".into(),
                    )
                })?;
                let posted_comments: Vec<PostedReviewComment> = client
                    .get(&format!(
                        "repos/{}/{}/pulls/{}/reviews/{}/comments",
                        pr_ref.owner, pr_ref.repo, pr_ref.number, review_id_for_lookup,
                    ))
                    .await
                    .map_err(github_to_service)?;
                let mut consumed: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for kata_comment in &bundled_to_post {
                    let Some(remote) =
                        match_posted_comment(kata_comment, &posted_comments, &consumed)
                    else {
                        tracing::warn!(
                            kata_comment = %kata_comment.comment_id,
                            "no matching GitHub comment found in the published review; \
                             mapping skipped (this comment may re-import as a ghost on refresh)",
                        );
                        continue;
                    };
                    consumed.insert(remote.node_id.as_str());
                    record_mapping_for_comment(
                        storage,
                        repo,
                        &review.review_id,
                        github_pr.number,
                        kata_comment,
                        Some(remote.node_id.clone()),
                        Some(remote.id),
                        "line_comment",
                        None,
                    )
                    .await?;
                }
                counts.new_inline_comments += bundled_to_post.len();
            }
            Err(GithubError::Validation { stderr }) => {
                // The bundled review POST is atomic: one bad inline
                // anchor (a line outside the diff hunks, a multi-line
                // range that doesn't align, etc.) 422s the whole batch.
                // Recover by:
                //  1. If the review carries a body or a non-Comment
                //     event, re-submit the wrapping review with an
                //     empty `comments[]` so the body / approval state
                //     still lands.
                //  2. Post each inline individually with the same
                //     422→issue-comment fallback as the LEFT-side path.
                //     Any individual comment that still 422s falls
                //     back to an issue comment carrying file:line
                //     context — its body never silently drops.
                tracing::warn!(
                    error = %stderr,
                    "bundled review POST rejected by github (422); \
                     falling back to per-comment individual posts \
                     (one or more inline anchors are outside the diff)",
                );
                let needs_shell = effective_body.is_some()
                    || !matches!(event, PublishEvent::Comment);
                if needs_shell {
                    let mut shell = payload.clone();
                    shell["comments"] = serde_json::json!([]);
                    match client
                        .post::<CreatedReview>(&bundled_endpoint, &shell)
                        .await
                    {
                        Ok(shell_posted) => {
                            // Record the wrapping-review mapping on
                            // every successful shell POST, regardless
                            // of whether it carried a body — the row
                            // is what keeps a retry from re-submitting
                            // an APPROVE / REQUEST_CHANGES wrapping.
                            if let Some(node_id) = shell_posted.node_id {
                                record_review_body_mapping(
                                    storage,
                                    repo,
                                    &review.review_id,
                                    github_pr.number,
                                    node_id,
                                    shell_posted.id,
                                )
                                .await?;
                            }
                        }
                        Err(GithubError::Validation { stderr }) => {
                            // Even the empty-comments shell 422'd.
                            // If there's a body, surface it as an
                            // issue comment so it isn't lost — and
                            // record a review-body mapping under
                            // that issue comment's node id so a
                            // future import doesn't re-import the
                            // body as a ghost of ourselves and a
                            // retry doesn't double-post.
                            if let Some(b) = effective_body {
                                tracing::warn!(
                                    error = %stderr,
                                    "empty-comments review shell rejected too; \
                                     posting review body as an issue comment",
                                );
                                let posted: CreatedComment = client
                                    .post(
                                        &format!(
                                            "repos/{}/{}/issues/{}/comments",
                                            pr_ref.owner,
                                            pr_ref.repo,
                                            pr_ref.number,
                                        ),
                                        &serde_json::json!({ "body": b }),
                                    )
                                    .await
                                    .map_err(github_to_service)?;
                                if let Some(node_id) = posted.node_id {
                                    record_review_body_mapping(
                                        storage,
                                        repo,
                                        &review.review_id,
                                        github_pr.number,
                                        node_id,
                                        posted.id,
                                    )
                                    .await?;
                                }
                            }
                        }
                        Err(other) => return Err(github_to_service(other)),
                    }
                }
                for c in &bundled_to_post {
                    post_inline_with_issue_fallback(
                        client,
                        storage,
                        repo,
                        &review.review_id,
                        &pr_ref,
                        c,
                        &live_pr.head.sha,
                        &live_pr.head.sha,
                        "RIGHT",
                    )
                    .await?;
                    counts.new_inline_comments += 1;
                }
            }
            Err(e) => return Err(github_to_service(e)),
        }
    }

    // ---- 7. Dual-write: mark local session published -----------
    storage
        .publish_session(repo, &review.review_id, session_id)
        .await?;

    Ok(counts)
}

/// Compose the GH /reviews comments[] entry for one kata inline
/// comment. Maps file/line/side + body. Single-line comments use
/// just `line`; multi-line use `start_line` + `line`.
fn build_inline_payload(c: &Comment) -> serde_json::Value {
    // Both fields are pre-validated by the partition above:
    // file.is_some() && lines.is_some() && !review_wide.
    let path = c.file.as_deref().unwrap_or("");
    let lines = c.lines.as_ref().expect("partition ensures Some");
    let side_str = match c.side {
        Some(Side::Base) => "LEFT",
        // Default to RIGHT for unset / Tip — matches what new
        // inline comments default to on github.com.
        _ => "RIGHT",
    };
    if lines.start == lines.end {
        serde_json::json!({
            "path": path,
            "line": lines.end,
            "side": side_str,
            "body": c.body,
        })
    } else {
        serde_json::json!({
            "path": path,
            "start_line": lines.start,
            "line": lines.end,
            "start_side": side_str,
            "side": side_str,
            "body": c.body,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_mapping_for_comment(
    storage: &dyn Storage,
    repo: &RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    c: &Comment,
    node_id: Option<String>,
    rest_id: Option<i64>,
    kind: &str,
    thread_node_id: Option<String>,
) -> ServiceResult<()> {
    let Some(node_id) = node_id else {
        // GH returned no node id — log and skip the mapping. The
        // comment is on github.com regardless; the only downside
        // is a future refresh might re-import it as a "new" item.
        tracing::warn!(
            kata_comment = %c.comment_id,
            kind = %kind,
            "no node_id returned from github.com; mapping skipped",
        );
        return Ok(());
    };
    storage
        .insert_github_comment_mapping(
            repo,
            &GithubCommentMapping {
                github_node_id: node_id,
                github_rest_id: rest_id,
                kind: kind.to_owned(),
                review_id: review_id.clone(),
                pr_number,
                kata_comment_id: Some(c.comment_id.clone()),
                kata_response_id: None,
                thread_node_id,
            },
        )
        .await?;
    Ok(())
}

/// Record a `kind=review_body` mapping for the wrapping
/// `pull_request_review` (or, in the deep-fallback path, the
/// issue comment its body landed as). Both `kata_comment_id` and
/// `kata_response_id` are `None`: the body is per-publish-call
/// and has no kata item to thread off of. Dedup is by
/// (repo, review_id, pr_number, kind="review_body").
async fn record_review_body_mapping(
    storage: &dyn Storage,
    repo: &RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    github_node_id: String,
    github_rest_id: Option<i64>,
) -> ServiceResult<()> {
    storage
        .insert_github_comment_mapping(
            repo,
            &GithubCommentMapping {
                github_node_id,
                github_rest_id,
                kind: "review_body".into(),
                review_id: review_id.clone(),
                pr_number,
                kata_comment_id: None,
                kata_response_id: None,
                thread_node_id: None,
            },
        )
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn record_mapping_for_response(
    storage: &dyn Storage,
    repo: &RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    r: &Response,
    node_id: Option<String>,
    rest_id: Option<i64>,
    kind: &str,
    thread_node_id: Option<String>,
) -> ServiceResult<()> {
    let Some(node_id) = node_id else {
        tracing::warn!(
            kata_response = %r.response_id,
            kind = %kind,
            "no node_id returned from github.com; mapping skipped",
        );
        return Ok(());
    };
    storage
        .insert_github_comment_mapping(
            repo,
            &GithubCommentMapping {
                github_node_id: node_id,
                github_rest_id: rest_id,
                kind: kind.to_owned(),
                review_id: review_id.clone(),
                pr_number,
                kata_comment_id: None,
                kata_response_id: Some(r.response_id.clone()),
                thread_node_id,
            },
        )
        .await?;
    Ok(())
}

/// Post one inline comment individually against `commit_sha` with
/// the same 422 → quoted-issue-comment fallback the LEFT-side path
/// relies on. Shared by:
///  - section 6 (LEFT-side inline, with `commit_sha =
///    original_base_sha` / `side = "LEFT"`),
///  - the section-7 bundled-review 422 fallback (RIGHT-side inline,
///    with `commit_sha = live head` / `side = "RIGHT"`).
///
/// The body footer carries both a markdown link and plaintext
/// file:line so the reader can navigate even when the inline
/// anchor is missing.
#[allow(clippy::too_many_arguments)]
async fn post_inline_with_issue_fallback(
    client: &dyn GithubClient,
    storage: &dyn Storage,
    repo: &RepoId,
    review_id: &ReviewId,
    pr_ref: &PullRequestRef,
    c: &Comment,
    inline_commit_sha: &str,
    footer_link_sha: &str,
    side: &'static str,
) -> ServiceResult<()> {
    // POST commit_id is always a commit in the PR's own commit list
    // (the live head, regardless of side) — the PR's base SHA is
    // not part of that list and 422s. `side` alone tells github
    // which side of the diff to anchor on. The success body is
    // just `c.body`; the file:line footer earns its keep only on
    // the issue-comment fallback where the inline anchor is lost,
    // and its link points at `footer_link_sha` — the base SHA for
    // LEFT-side comments (so the reader can see the file as it
    // was before the PR), the head SHA for RIGHT-side.
    let inline_payload = serde_json::json!({
        "path": c.file.as_deref().unwrap_or(""),
        "line": c.lines.as_ref().map(|r| r.end).unwrap_or(0),
        "side": side,
        "commit_id": inline_commit_sha,
        "body": c.body,
    });
    let line_endpoint = format!(
        "repos/{}/{}/pulls/{}/comments",
        pr_ref.owner, pr_ref.repo, pr_ref.number,
    );
    match client
        .post::<CreatedComment>(&line_endpoint, &inline_payload)
        .await
    {
        Ok(posted) => {
            record_mapping_for_comment(
                storage,
                repo,
                review_id,
                pr_ref.number,
                c,
                posted.node_id,
                posted.id,
                "line_comment",
                None,
            )
            .await
        }
        Err(GithubError::Validation { stderr }) => {
            tracing::warn!(
                kata_comment = %c.comment_id,
                side = %side,
                error = %stderr,
                "individual inline comment rejected by github (422); \
                 falling back to an issue comment with file:line context",
            );
            let body = inline_fallback_body(
                &pr_ref.owner, &pr_ref.repo, footer_link_sha, side, c,
            );
            let posted: CreatedComment = client
                .post(
                    &format!(
                        "repos/{}/{}/issues/{}/comments",
                        pr_ref.owner, pr_ref.repo, pr_ref.number
                    ),
                    &serde_json::json!({ "body": body }),
                )
                .await
                .map_err(github_to_service)?;
            record_mapping_for_comment(
                storage,
                repo,
                review_id,
                pr_ref.number,
                c,
                posted.node_id,
                posted.id,
                "issue_comment",
                None,
            )
            .await
        }
        Err(e) => Err(github_to_service(e)),
    }
}

fn short(sha: &str) -> &str {
    if sha.len() > 9 { &sha[..9] } else { sha }
}

/// Footer appended to the fallback issue-comment shape when an
/// individual inline POST 422s. Carries both a markdown link
/// (rendered on github.com when the surrounding chrome supports
/// it) and a plaintext `path:line` repetition so the pointer
/// survives notification emails / mobile views / any other
/// context where markdown might not render. Renders after the
/// original comment body so the comment reads naturally first
/// and the context is a footnote. `side` distinguishes base vs.
/// head in the human-facing tag.
fn inline_fallback_body(
    owner: &str,
    repo: &str,
    commit_sha: &str,
    side: &str,
    c: &Comment,
) -> String {
    let path = c.file.as_deref().unwrap_or("");
    let line = c.lines.as_ref().map(|r| r.end).unwrap_or(0);
    let side_label = if side == "LEFT" { "base-side" } else { "head-side" };
    format!(
        "{body}\n\n— _re: [`{path}:{line}`](https://github.com/{owner}/{repo}/blob/{commit_sha}/{path}#L{line}) ({side_label}, `{path}:{line}`)_",
        body = c.body,
    )
}

/// Quoted-reply body in the shape GitHub's own "Quote reply"
/// UI produces: an `@author wrote:` attribution line, the parent
/// body with `> ` line-prefixes, a blank, then the reply text.
/// When the parent isn't available (lookup failed, native kata
/// comment removed, etc.), falls back to a short tag noting why.
/// `reason` is a machine-readable hint at why the threaded path
/// wasn't taken — surfaced as a small inline note so the reader
/// knows this reply landed as a standalone comment by necessity.
fn build_quoted_reply_body(
    parent: Option<&Comment>,
    reply_body: &str,
    reason: Option<&str>,
) -> String {
    // Only render a real `@login` when the parent has an
    // `external_author` — those are verified GitHub identities and
    // pinging them is intended. For ghost authors (`gh:<login>`)
    // the structural author IS a GitHub handle, so `@login` is
    // still safe. For native kata authors (e.g. `alice`), backtick
    // the name instead — posting `@alice` to github.com would
    // ping whoever owns that handle, a stranger to this review.
    let attribution = parent
        .map(|p| {
            if let Some(ea) = p.external_author.as_ref() {
                format!("@{}", ea.login)
            } else {
                let a = p.author.as_str();
                if let Some(login) = a.strip_prefix("gh:") {
                    format!("@{login}")
                } else {
                    format!("`{a}`")
                }
            }
        })
        .unwrap_or_else(|| "kata".to_owned());
    let mut quoted = String::new();
    quoted.push_str(&format!("> {attribution} wrote:\n"));
    let parent_text = parent.map(|p| p.body.as_str()).unwrap_or("(original comment unavailable)");
    for line in parent_text.lines() {
        quoted.push_str("> ");
        quoted.push_str(line);
        quoted.push('\n');
    }
    let reason_note = reason
        .map(|r| format!(" _(posted as issue comment: {r})_\n"))
        .unwrap_or_default();
    format!("{quoted}{reason_note}\n{reply_body}")
}

fn github_to_service(err: GithubError) -> ServiceError {
    crate::github_error_to_service(err)
}

/// GraphQL mutation that flips a review thread's resolved/open
/// state on github.com. `resolved = true` calls
/// `resolveReviewThread`; `false` calls `unresolveReviewThread`.
/// Both mutations accept the same `input: { threadId }` shape and
/// are idempotent on the server — calling resolve on an
/// already-resolved thread is a no-op (returns the same thread).
/// Translate a kata `Response.action` into a GitHub thread mutation
/// and record a `kind=resolution` mapping row so the mutation is
/// idempotent across retries. Three outcomes:
///
/// - `action == Comment`: no-op (nothing to translate). Caller
///   shouldn't reach this branch — the loop already gates on
///   `!matches!(action, Comment)` before calling — but we're
///   defensive here so future call sites don't have to duplicate
///   the check.
/// - `action != Comment && parent has thread_node_id`: fire the
///   `resolveReviewThread` / `unresolveReviewThread` mutation, then
///   write the mapping row. Order matters: if the mutation errors,
///   no mapping row lands and a retry re-fires (the mutation is
///   idempotent server-side, so re-firing is safe). `resolutions`
///   is bumped on success.
/// - `action != Comment && parent has no thread`: log a warn (GH
///   has no thread to resolve on issue-comment / review-summary
///   parents), write a mapping row so retries don't repeat the
///   warn, and bump `resolutions_dropped_no_thread`.
///
/// Shared by the resolve-only branch and the resolve-with-text
/// tail — every kata resolution flows through here so the mapping-
/// row write is in one place.
#[allow(clippy::too_many_arguments)]
async fn apply_resolution_side_effect(
    client: &dyn GithubClient,
    storage: &dyn Storage,
    repo: &RepoId,
    review_id: &ReviewId,
    pr_number: u32,
    parent_mapping: Option<&GithubCommentMapping>,
    response: &Response,
    counts: &mut PublishCounts,
) -> ServiceResult<()> {
    if matches!(response.action, ResolutionAction::Comment) {
        return Ok(());
    }
    let thread_node_id = parent_mapping.and_then(|m| m.thread_node_id.clone());
    if let Some(thread_id) = thread_node_id.as_deref() {
        let resolved = !matches!(response.action, ResolutionAction::Unresolve);
        resolve_github_thread(client, thread_id, resolved).await?;
        counts.resolutions += 1;
    } else {
        tracing::warn!(
            kata_response = %response.response_id,
            action = ?response.action,
            "resolution action on a non-thread parent; \
             GitHub has no thread to resolve — dropping",
        );
        counts.resolutions_dropped_no_thread += 1;
    }
    // Persist the mapping row *after* the mutation (or after the
    // drop-with-warn decision) so a mid-publish crash before the row
    // lands leaves the retry free to re-attempt. The mutation is
    // idempotent server-side, so re-firing on retry is a no-op.
    record_mapping_for_response(
        storage,
        repo,
        review_id,
        pr_number,
        response,
        Some(synth_resolution_node_id(&response.response_id)),
        None,
        "resolution",
        thread_node_id,
    )
    .await?;
    Ok(())
}

async fn resolve_github_thread(
    client: &dyn GithubClient,
    thread_node_id: &str,
    resolved: bool,
) -> ServiceResult<()> {
    let mutation = if resolved {
        r#"mutation($threadId: ID!) {
          resolveReviewThread(input: { threadId: $threadId }) {
            thread { id isResolved }
          }
        }"#
    } else {
        r#"mutation($threadId: ID!) {
          unresolveReviewThread(input: { threadId: $threadId }) {
            thread { id isResolved }
          }
        }"#
    };
    client
        .graphql_raw(mutation, serde_json::json!({ "threadId": thread_node_id }))
        .await
        .map_err(github_to_service)?;
    Ok(())
}

/// Deterministic node-id for the mapping row we write when a
/// resolve-only response lands as a GitHub thread mutation rather
/// than a comment. There's no GitHub comment to point at, but the
/// mapping row is what keeps a retry from re-calling the mutation
/// — so we synthesise one keyed on the kata response id. Prefix
/// keeps it grep-able and obviously not a real GH node id.
fn synth_resolution_node_id(response_id: &kata_core::ResponseId) -> String {
    format!("kata-resolution:{}", response_id.as_str())
}

