//! Wire types for the patchset-compare v2 view. Two patchsets of the
//! same review (PS_a → PS_b) are paired commit-by-commit using their
//! jj change-ids so the UI can attribute every diff to the commit that
//! produced it, rather than only showing the cumulative tree-vs-tree
//! delta.

use serde::{Deserialize, Serialize};

use crate::diff::Diff;
use crate::ids::{ChangeId, CommitId};

/// How a single `change_id` relates across the two patchsets being
/// compared. Four mutually exclusive states; the renderer picks an icon
/// + interaction per status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeStatus {
    /// Present in both, with identical commit-ids — nothing changed
    /// about this commit between the two patchsets. UI renders it
    /// inert.
    Same,
    /// Present in both, but the commit-ids differ — the author rewrote
    /// it. The interdiff between the two commit-ids tells the reviewer
    /// what changed.
    Changed,
    /// Present only in the *to* patchset. The reviewer wants to see the
    /// commit's own diff against its parent.
    AddedInTo,
    /// Present only in the *from* patchset. Same: render the commit's
    /// own diff against its parent so the reviewer can see what
    /// vanished.
    RemovedFromFrom,
}

/// One row in the per-change-id pair list. Either `from_commit` or
/// `to_commit` may be absent (depending on `status`), but the change_id
/// is always present — it's the row's identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchsetPair {
    pub change_id: ChangeId,
    pub status: ChangeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_commit: Option<CommitId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_commit: Option<CommitId>,
    /// First line of the commit message on the *from* side. `None` for
    /// `AddedInTo`. The UI shows it so reviewers can read the pair list
    /// without re-fetching commit metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_description: Option<String>,
    /// First line of the commit message on the *to* side. `None` for
    /// `RemovedFromFrom`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_description: Option<String>,
    /// For one-sided pairs (`AddedInTo` / `RemovedFromFrom`), the
    /// parent of the present-side commit. Lets the UI render the
    /// commit's own diff (`parent..commit`) when the user clicks the
    /// row — analogous to how `Changed` rows show the interdiff
    /// between two existing commits. `None` for `Same` / `Changed`
    /// (no parent diff is needed) and as a fallback when parent
    /// resolution failed (the row falls back to inert).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_commit: Option<CommitId>,
    /// Pre-computed diff counts for the row's effective endpoint
    /// pair (interdiff for `Changed`; `parent..commit` for
    /// `AddedInTo` / `RemovedFromFrom`). Lets the side panel show
    /// "3 files +7 −15" inline so reviewers can spot big rewrites
    /// at a glance without clicking every row. `None` for `Same`
    /// (no diff) and as a fallback when the count computation
    /// failed; the UI just omits the chip in that case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_counts: Option<PairDiffCounts>,
}

/// File / line counts for one pair-row's effective diff. Same
/// semantics as `FileChange.added` / `removed`, just summed across
/// the pair's file list.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PairDiffCounts {
    pub file_count: u32,
    pub added: u32,
    pub removed: u32,
}

/// Endpoint pair for a patchset, used to identify which side of a
/// compare each commit belongs to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchsetEndpoints {
    pub n: u32,
    pub base_commit: CommitId,
    pub tip_commit: CommitId,
}

/// Full response shape for `compare_patchsets`. Carries the cumulative
/// tree-vs-tree diff metadata (the current compare-mode view) plus the
/// per-change-id pair list that drives the per-commit view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchsetCompareView {
    pub from: PatchsetEndpoints,
    pub to: PatchsetEndpoints,
    /// Cumulative diff between `from.tip_commit` and `to.tip_commit` —
    /// file metadata only, no hunks (lazy-loaded via `/diff?path=`).
    pub cumulative: Diff,
    pub pairs: Vec<PatchsetPair>,
    /// True when `from.base_commit != to.base_commit`. The cumulative
    /// diff will then include upstream rebase noise — the UI surfaces
    /// a banner so the reader knows what they're looking at. We don't
    /// reproject automatically in v2; that lands when someone actually
    /// hits the case.
    pub compare_base_mismatch: bool,
}
