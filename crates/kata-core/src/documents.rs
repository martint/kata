//! Persisted documents: comments, responses, session manifest, review
//! manifest, repo manifest. Storage backends serialize these to whatever
//! medium they use.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{
    AnnotationId, Author, ChangeId, CommentId, CommitId, LineRange, RepoId, ResponseId, ReviewId,
    RevSet, SessionId, Side,
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
    /// A question for the author. Whether the answer satisfies the
    /// question is the author's call — responders should not auto-
    /// resolve.
    Question,
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

    /// True for "review-wide" comments — file/lines/side are all `None`
    /// and the comment is intentionally about the whole review rather
    /// than any specific commit in it. The UI renders these under the
    /// "All commits" row of the commits panel. `false` (the default)
    /// covers everything else, including commit-level comments
    /// (file/lines/side all `None`, but `review_wide = false`, meaning
    /// the comment is about the specific change at `anchor_change_id`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub review_wide: bool,

    pub flag: Flag,

    /// Markdown body. Storage backends pull this out of the TOML
    /// frontmatter at write time and append it after the closing fence.
    #[serde(default)]
    pub body: String,
}

/// An author-written annotation anchored to a region of code.
///
/// Annotations are the review *creator*'s way of giving reviewers extra
/// context — "this looks weird because legacy X", "the alternative
/// would be Y but it didn't work because Z" — without polluting the
/// review-comment thread. They look like comments at the anchor site
/// but they are not part of the review conversation:
///
/// * **Author-only**: only `manifest.created_by` can create, edit, or
///   delete annotations. Reviewers can read them but cannot reply.
/// * **One-way**: no threading, no responses, no resolution state.
/// * **No session**: published immediately on submit; no draft-batch
///   flow.
/// * **No flag**: severity makes no sense for context notes.
///
/// Anchor handling matches `Comment`: stored against the
/// `(anchor_change_id, anchor_commit_id, file, side, lines)` tuple
/// and re-projected onto the current patchset via `resolve_anchor`
/// so the annotation follows the code as it moves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub schema_version: u32,
    pub annotation_id: AnnotationId,
    pub review_id: ReviewId,
    pub author: Author,
    pub created_at: DateTime<Utc>,
    /// Last edit timestamp. Equals `created_at` for never-edited
    /// annotations; we don't carry per-edit history.
    pub updated_at: DateTime<Utc>,
    /// Patchset current when the annotation was first written. Used
    /// for scoping (annotations are visible in their own patchset and
    /// later ones) and for revival when the anchor moves.
    pub patchset: u32,
    pub anchor_change_id: ChangeId,
    pub anchor_commit_id: CommitId,

    /// Omitted for review-wide annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    /// Required when both `file` and `lines` are set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,

    /// Omitted for whole-file annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LineRange>,

    /// Markdown body.
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
    /// Opaque stable identifier — UUID v7 for new reviews, the
    /// bookmark slug for pre-numbering reviews carried over from
    /// older archives. Used internally and by storage; never shown to
    /// the user (the URL uses [`Self::number`], the UI shows
    /// [`Self::name`]). Comments and sessions still reference the
    /// review by this id, so it's also the join key for everything
    /// downstream.
    pub review_id: ReviewId,
    /// Per-repo monotonic counter assigned at create-review time.
    /// Drives the URL — `/r/<repo>/<number>` — and the breadcrumb
    /// display. Unique within a repo across active *and* archived
    /// reviews so that creating a new review on a branch that already
    /// has one (or several) just bumps the counter.
    #[serde(default)]
    pub number: u32,
    /// Human-readable label. Defaults to the bookmark name when the
    /// review is created; editable later (planned). Pure display —
    /// changing it never affects URLs or identity. Empty string for
    /// reviews migrated from before this field existed.
    #[serde(default)]
    pub name: String,
    pub revset: RevSet,
    pub created_at: DateTime<Utc>,
    pub created_by: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark: Option<String>,
    /// Author-written description of the change. Markdown. Only the
    /// `created_by` author may set or update it. Optional — older
    /// manifests on disk predate this field and deserialize with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub patchsets: Vec<Patchset>,
    pub current_patchset: u32,
    /// When set, the review is archived: the creator marked it as no
    /// longer warranting active attention. Archived reviews are hidden
    /// from the home screen by default and reject session / comment /
    /// response writes (only the creator can unarchive). Absent on
    /// active reviews; older manifests deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
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

/// Per-repo manifest at `$KATA_DATA/{repo-id}/repo.yaml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoManifest {
    pub schema_version: u32,
    pub repo_id: RepoId,
    /// The canonical filesystem path of `.jj/repo` that this id hashes from.
    /// Informational; the directory name is the source of truth.
    pub canonical_path: String,
}

