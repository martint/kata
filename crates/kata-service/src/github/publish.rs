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
    Author, Comment, RepoId, Response, ReviewId, ReviewManifest, SessionId, Side,
};
use kata_storage::{GithubCommentMapping, Storage};
use serde::Deserialize;

use super::client::{GithubClient, GithubError};
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
/// duplicate-body cases. Returns `None` when no match is found,
/// which causes the mapping to be skipped (with a warn log).
fn match_posted_comment<'a>(
    kata: &Comment,
    posted: &'a [PostedReviewComment],
) -> Option<&'a PostedReviewComment> {
    let want_path = kata.file.as_deref()?;
    let want_lines = kata.lines.as_ref()?;
    let want_side = match kata.side {
        Some(Side::Base) => "LEFT",
        _ => "RIGHT",
    };
    posted.iter().find(|p| {
        p.path == want_path
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
    client: &GithubClient,
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
    // Refuses if either endpoint moved since import. Head drift is
    // the dangerous case (inline comments would land against the
    // wrong lines); base drift is the symmetric "the target branch
    // moved" case captured for the same reason. Returned as a
    // typed Conflict so the SPA can offer one-click re-import
    // recovery without substring-matching the prose.
    let live_pr = client.fetch_pr(&pr_ref).await.map_err(github_to_service)?;
    let head_drifted = live_pr.head.sha != github_pr.original_head_sha;
    let base_drifted = live_pr.base.sha != github_pr.original_base_sha;
    if head_drifted || base_drifted {
        let endpoint = if head_drifted { "head" } else { "base" };
        let (old, new) = if head_drifted {
            (&github_pr.original_head_sha, &live_pr.head.sha)
        } else {
            (&github_pr.original_base_sha, &live_pr.base.sha)
        };
        return Err(ServiceError::Conflict {
            kind: "head_drift".into(),
            message: format!(
                "the PR {endpoint} moved since import (was {}, now {}). \
                 Re-import the PR to anchor inline comments against the new \
                 {endpoint}, then publish.",
                short(old),
                short(new),
            ),
        });
    }

    // ---- 2. Load this author's drafts in this session -----------
    let drafts = storage
        .list_drafts_for(repo, &review.review_id, author)
        .await?;
    if drafts.comments.is_empty() && drafts.responses.is_empty() && body.is_none() {
        return Err(ServiceError::BadRequest(
            "no draft comments, responses, or review body to publish".into(),
        ));
    }

    // ---- 3. Partition ------------------------------------------
    let mut new_inline: Vec<&Comment> = Vec::new();
    let mut issue_comments: Vec<&Comment> = Vec::new();
    for c in &drafts.comments {
        if c.review_wide || c.file.is_none() || c.lines.is_none() {
            issue_comments.push(c);
        } else {
            new_inline.push(c);
        }
    }

    let mut counts = PublishCounts {
        event: event.as_str().to_owned(),
        ..Default::default()
    };

    // ---- 4. Replies first (so they appear above the bundled review on GH) ----
    for r in &drafts.responses {
        let mapping = storage
            .lookup_github_mapping_by_kata_comment(repo, &r.in_reply_to)
            .await?;
        let Some(mapping) = mapping else {
            // Reply targets a native kata comment that was never on
            // github.com — we have nothing to thread it to. Post as
            // a fresh issue comment so the content isn't lost,
            // tagged so the reader knows where it came from.
            let posted: CreatedComment = client
                .post(
                    &format!(
                        "repos/{}/{}/issues/{}/comments",
                        pr_ref.owner, pr_ref.repo, pr_ref.number
                    ),
                    &serde_json::json!({
                        "body": format!("> (reply to local kata comment)\n\n{}", r.body),
                    }),
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
                None,
            )
            .await?;
            counts.issue_comments += 1;
            continue;
        };
        // Only review-thread comments accept `in_reply_to` on
        // `/pulls/N/comments`. Issue comments and review-summary
        // bodies are conversation-level on GitHub — no threaded
        // reply primitive — so a reply to one of those has to
        // land as a fresh issue comment, with a short quote for
        // context (mirrors the native-kata-comment fallback
        // above). Routing on `mapping.kind` rather than blindly
        // posting to `/pulls/N/comments` avoids a 422 from
        // github.com on `in_reply_to`-not-a-review-comment.
        let is_thread = matches!(
            mapping.kind.as_str(),
            "line_comment" | "thread_reply",
        );
        if !is_thread {
            let posted: CreatedComment = client
                .post(
                    &format!(
                        "repos/{}/{}/issues/{}/comments",
                        pr_ref.owner, pr_ref.repo, pr_ref.number
                    ),
                    &serde_json::json!({
                        "body": format!(
                            "> (reply to imported {} comment)\n\n{}",
                            mapping.kind, r.body,
                        ),
                    }),
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
                None,
            )
            .await?;
            counts.issue_comments += 1;
            continue;
        }
        let rest_id = mapping.github_rest_id.ok_or_else(|| {
            ServiceError::Internal(format!(
                "mapping for kata comment {} missing github_rest_id; \
                 cannot reply (only GraphQL node_id was stored)",
                r.in_reply_to,
            ))
        })?;
        let posted: CreatedComment = client
            .post(
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
            .map_err(github_to_service)?;
        record_mapping_for_response(
            storage,
            repo,
            &review.review_id,
            github_pr.number,
            r,
            posted.node_id,
            posted.id,
            "thread_reply",
            mapping.thread_node_id.clone(),
        )
        .await?;
        counts.replies += 1;
    }

    // ---- 5. Issue-comment-style kata comments ------------------
    for c in &issue_comments {
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

    // ---- 6. Bundled review with inline comments + body ---------
    // Skip the review submission when there's nothing to bundle and
    // no body — otherwise the user would get a stray empty review
    // on github.com.
    if !new_inline.is_empty() || body.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        let inline_payloads: Vec<serde_json::Value> = new_inline
            .iter()
            .map(|c| build_inline_payload(c))
            .collect();
        let mut payload = serde_json::json!({
            "commit_id": live_pr.head.sha,
            "event": event.as_str(),
            "comments": inline_payloads,
        });
        if let Some(b) = body.as_deref()
            && !b.trim().is_empty()
        {
            payload["body"] = serde_json::Value::String(b.to_owned());
        }
        let posted: CreatedReview = client
            .post(
                &format!(
                    "repos/{}/{}/pulls/{}/reviews",
                    pr_ref.owner, pr_ref.repo, pr_ref.number
                ),
                &payload,
            )
            .await
            .map_err(github_to_service)?;
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
        for kata_comment in &new_inline {
            let Some(remote) = match_posted_comment(kata_comment, &posted_comments) else {
                tracing::warn!(
                    kata_comment = %kata_comment.comment_id,
                    "no matching GitHub comment found in the published review; \
                     mapping skipped (this comment may re-import as a ghost on refresh)",
                );
                continue;
            };
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
        counts.new_inline_comments += new_inline.len();
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

fn short(sha: &str) -> &str {
    if sha.len() > 9 { &sha[..9] } else { sha }
}

fn github_to_service(err: GithubError) -> ServiceError {
    crate::github_error_to_service(err)
}

