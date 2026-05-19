use serde::{Deserialize, Serialize};

use crate::ids::{CommitId, LineRange};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    pub base: CommitId,
    pub tip: CommitId,
    pub files: Vec<FileChange>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed { old_path: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    #[serde(flatten)]
    pub status: FileStatus,
    /// `None` if either side is non-text (binary) or oversized — UI collapses.
    pub hunks: Option<Vec<Hunk>>,
    pub binary: bool,
    /// Added line count. Always populated so the file tree's +/- summary
    /// is accurate even before per-file hunks have been lazy-loaded.
    /// Zero for binary files (no line concept).
    #[serde(default)]
    pub added: u32,
    /// Removed line count. See [`Self::added`].
    #[serde(default)]
    pub removed: u32,
}

/// A region of a file diff. A regular hunk is a contiguous slice of
/// changed + surrounding context lines (the historical shape); a
/// conflict hunk wraps the structured conflict sides jj keeps on a
/// conflicted commit so the renderer can show each side stacked
/// instead of running the file through the regular diff machinery.
///
/// `kind` is the serde discriminator so the JSON wire format
/// distinguishes the two variants. Today the diff producer only ever
/// emits one variant per file — a file is either fully regular or
/// fully a conflict — but the type is per-hunk so a future iteration
/// can mix regular and conflict hunks in the same file without a
/// schema break.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Hunk {
    Regular(RegularHunk),
    Conflict(ConflictHunk),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegularHunk {
    /// `None` for pure-insertion hunks where no base lines are involved.
    pub base_range: Option<LineRange>,
    /// `None` for pure-deletion hunks where no tip lines are involved.
    pub tip_range: Option<LineRange>,
    pub lines: Vec<HunkLine>,
}

/// Structured conflict region. jj keeps conflicted commits as live
/// objects whose tree values carry every side of the merge instead
/// of being flattened to a single resolved blob; this is the shape
/// that information takes on its way to the renderer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictHunk {
    /// One entry per side of the merge. For the common 3-way case
    /// this is `[base, side_1, side_2]`; for N-way merges the list
    /// extends accordingly. Order is stable across calls but is not
    /// semantically significant beyond labelling.
    pub sides: Vec<ConflictSide>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictSide {
    /// Human-readable label for this side. Auto-derived from parent
    /// commit metadata where possible (`"from main"`, `"from feature"`,
    /// or the parent's one-line description); falls back to `"Base"`
    /// for the merge base term and `"Side N"` for unlabelled sides.
    pub label: String,
    /// Raw content lines on this side, in source order. Conflict
    /// rendering doesn't have a meaningful base/tip distinction —
    /// each side is its own self-contained version — so we drop the
    /// HunkLine `origin` / line-number plumbing and just keep the
    /// bytes.
    pub lines: Vec<String>,
}

/// Which side(s) a line exists on within a hunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineOrigin {
    /// Present on both sides (context).
    Context,
    /// Present only on the tip side (added).
    Added,
    /// Present only on the base side (removed).
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HunkLine {
    pub origin: LineOrigin,
    /// 1-based; `None` when the line doesn't exist on this side.
    pub base_line: Option<u32>,
    pub tip_line: Option<u32>,
    pub content: String,
}
