//! Persisted documents: comments, responses, session manifest, review
//! manifest, repo manifest. Storage backends serialize these to whatever
//! medium they use.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{
    Author, ChangeId, CommentId, CommitId, LineRange, RepoId, ResponseId, ReviewId, RevSet,
    SessionId, Side,
};

pub const SCHEMA_VERSION: u32 = 1;

/// Severity / kind of a review comment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Flag {
    /// Reviewee must address this before the change is acceptable.
    MustDo,
    /// Optional improvement.
    Suggestion,
    /// Question, note, or anything else.
    Other,
}

/// Effect a response has on a comment's resolution state. `Unresolve` is
/// the universal reopen: it returns a comment from either `Resolved` or
/// `WontFix` back to open.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionAction {
    /// No state change — just discussion.
    Comment,
    Resolve,
    Unresolve,
    WontFix,
}

/// Lifecycle state of a review session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SessionStatus {
    Draft,
    Published,
    Discarded,
}

/// A single review comment. The Markdown body is held alongside the
/// frontmatter fields and is *not* part of the YAML serialization — storage
/// backends write it after the closing frontmatter fence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub schema_version: u32,
    pub comment_id: CommentId,
    pub session_id: SessionId,
    pub review_id: ReviewId,
    pub author: Author,
    pub created_at: DateTime<Utc>,
    /// Patchset that was current when the comment was written. Used to scope
    /// the comment to the right round when viewers browse history; comments
    /// are visible in their own patchset and all later ones.
    pub patchset: u32,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,

    /// Omitted for whole-review comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    /// Required when both `file` and `lines` are set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,

    /// Omitted for whole-file comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LineRange>,

    pub flag: Flag,

    /// Markdown body. Storage backends pull this out of the TOML
    /// frontmatter at write time and append it after the closing fence.
    #[serde(default)]
    pub body: String,
}

/// A response targeting an existing comment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub schema_version: u32,
    pub response_id: ResponseId,
    pub in_reply_to: CommentId,
    pub session_id: SessionId,
    pub author: Author,
    pub created_at: DateTime<Utc>,
    pub action: ResolutionAction,

    #[serde(default)]
    pub body: String,
}

/// Session manifest — one per draft-to-publish cycle per author per review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub review_id: ReviewId,
    pub author: Author,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
}

/// One round of review. Each refresh that observes a moved tip appends a new
/// patchset; comments anchor against the patchset that was current at write
/// time so older discussions stay readable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Patchset {
    pub n: u32,
    pub base_change: ChangeId,
    pub base_commit: CommitId,
    pub tip_change: ChangeId,
    pub tip_commit: CommitId,
    pub recorded_at: DateTime<Utc>,
    /// Patchset whose tip is an ancestor of this one's tip — i.e. the
    /// previous round if the bookmark fast-forwarded or amended. `None`
    /// when this is the first patchset or when the bookmark was moved to a
    /// disjoint branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_patchset: Option<u32>,
}

/// Per-review manifest. Holds the append-only patchset history; the current
/// patchset's `base_commit`/`tip_commit` is what the viewer renders by
/// default.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewManifest {
    pub schema_version: u32,
    pub review_id: ReviewId,
    pub revset: RevSet,
    pub created_at: DateTime<Utc>,
    pub created_by: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark: Option<String>,
    pub patchsets: Vec<Patchset>,
    pub current_patchset: u32,
}

impl ReviewManifest {
    /// The patchset numbered `n`, or `None` if no such patchset exists.
    pub fn patchset(&self, n: u32) -> Option<&Patchset> {
        self.patchsets.iter().find(|p| p.n == n)
    }

    /// The currently-active patchset.
    pub fn current(&self) -> &Patchset {
        self.patchset(self.current_patchset)
            .expect("current_patchset must refer to an existing patchset")
    }
}

/// Per-repo manifest at `$KATA_ROOT/{repo-id}/repo.yaml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoManifest {
    pub schema_version: u32,
    pub repo_id: RepoId,
    /// The canonical filesystem path of `.jj/repo` that this id hashes from.
    /// Informational; the directory name is the source of truth.
    pub canonical_path: String,
}

