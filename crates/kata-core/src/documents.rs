//! Persisted documents: comments, responses, session manifest, review
//! manifest, repo manifest. Storage backends serialize these to whatever
//! medium they use.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{
    AnnotationId, ApiTokenId, Author, ChangeId, ColumnRange, CommentId, CommitId, LineRange,
    RepoId, ResponseId, ReviewId, RevSet, SessionId, Side,
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

    /// Optional column-range anchor, scoping the comment to a sub-
    /// region of the line range rather than the whole line(s). Two
    /// modes (see [`ColumnRange`] for full semantics):
    ///
    /// - Single-line (`lines.start == lines.end`): half-open
    ///   `[start, end)` within that line.
    /// - Multi-line (`lines.start < lines.end`): `start` is the
    ///   offset on the FIRST selected line, `end` is the offset on
    ///   the LAST one — typical for free-form text selections that
    ///   begin mid-line on one row and end mid-line on another.
    ///
    /// UTF-16 offsets. Inherits the line anchor's revival state —
    /// when the line is Drifted or Outdated the column highlight
    /// degrades to a plain line-level mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<ColumnRange>,

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

    /// Ghost-author identity when the comment was imported from an
    /// external source (e.g. a GitHub PR). `None` for native kata-
    /// authored comments. The UI renders this in place of the
    /// kata [`Author`] (avatar + `@login` link) so imported threads
    /// don't show up as written by a synthetic `gh:<login>` user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_author: Option<ExternalAuthor>,
}

/// Identity of a user from a non-kata source — currently only
/// github.com — preserved on imported comments + responses so the
/// UI can render the original author (avatar, `@login`, link back
/// to the source) rather than the synthetic ghost author kata
/// stores against [`Comment::author`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalAuthor {
    /// `"github"` today; carve out other sources as we add them.
    /// Kept as a free-form string rather than an enum so the
    /// archive format doesn't need a schema bump when we extend.
    pub source: String,
    pub login: String,
    /// Stable numeric identity from the source. Survives logname
    /// changes; used as the *real* dedup key during refresh.
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Profile URL on the source. UI uses this to make the `@login`
    /// chip a link back to the source's user page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
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
    /// GitHub PR this review is bound to, when it was created via the
    /// `/api/github/import` endpoint. `None` for native kata reviews
    /// and for reviews carried over from before this field existed.
    /// Reviews with `github_pr = Some(...)` swap the publish-session
    /// behaviour for the "Publish to GitHub" path; see phase 6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_pr: Option<GithubPr>,
}

/// Identity + provenance of a GitHub pull request a kata review is
/// bound to. Populated at import time and never edited afterwards
/// (a PR's `(owner, repo, number)` is immutable; the head SHA is
/// captured here purely as the import-time baseline for the head-
/// drift refusal in phase 6).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GithubPr {
    pub owner: String,
    pub repo: String,
    pub number: u32,
    /// The PR's `html_url` at import time — used by the UI to
    /// link back to github.com.
    pub html_url: String,
    /// PR head SHA at the moment kata imported the PR. Phase 6
    /// re-fetches and compares before publishing back so a force-
    /// pushed head doesn't get reviews anchored to lines that no
    /// longer exist.
    pub original_head_sha: String,
    /// PR base SHA at import time. Captured for the same reason —
    /// rebases that move the base are common.
    pub original_base_sha: String,
    /// Name of the git remote on the matched workspace that the
    /// PR was fetched through (typically `"origin"`). Recorded so
    /// review deletion can scope its branch cleanup to the right
    /// remote — without it, same-numbered PRs imported from
    /// different remotes would clobber each other's refs on
    /// delete. Defaults to empty for manifests written before this
    /// field existed; the delete path falls back to an all-remotes
    /// scan in that case.
    #[serde(default)]
    pub remote_name: String,
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

/// Server-side credential bound to an author identity. Long-lived
/// bearer tokens primarily for MCP agents and CI integrations that
/// can't authenticate interactively. Tokens carry no permissions
/// beyond their `author` claim — they substitute for the per-request
/// identity that `--auth-mode` would otherwise determine.
///
/// The plaintext token is shown to the user exactly once at creation
/// and is never stored — only the SHA-256 hash sits in the database.
/// `prefix` is the leading ~12 characters of the plaintext (`kata_pat_
/// AaBb`), enough for a human to recognise their tokens in
/// `kata token list` without enabling a guessing attack.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiToken {
    pub token_id: ApiTokenId,
    pub author: Author,
    /// Free-form label set at creation time, e.g. `"ci-agent"`. No
    /// uniqueness constraint — `kata token list` shows the full
    /// `(name, prefix, created_at)` tuple so collisions are
    /// distinguishable.
    pub name: String,
    /// SHA-256 of the plaintext token, hex-encoded. Lookups happen
    /// by hashing the presented Bearer / `?token=` value and matching
    /// against this column.
    pub token_hash: String,
    /// Human-friendly prefix of the plaintext (e.g. `kata_pat_AaBbCc`).
    /// Shown in listings so the operator can identify "which token
    /// they're holding" without exposing the full secret.
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    /// Updated lazily on every successful authentication. `None`
    /// means the token has never been used since creation (or since
    /// the last `kata token list` cycled, depending on read freshness).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    /// Set once revoked; the row is kept so audit lookups by
    /// `token_id` still resolve. Auth rejects any token whose
    /// `revoked_at` is non-null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
}

