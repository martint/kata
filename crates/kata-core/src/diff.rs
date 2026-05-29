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
    /// All terms of the merge, in a stable order: bases first (the
    /// `removes()` of jj's `Merge`), then sides (the `adds()`). For
    /// the common 3-way case this is `[Base, Side, Side]`; criss-
    /// cross merges extend the base count, N-way merges extend the
    /// side count.
    pub terms: Vec<ConflictTerm>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictTerm {
    /// Human-readable label for this term. Auto-derived from parent
    /// commit metadata where possible (`"from main"`, `"from feature"`,
    /// or the parent's one-line description); falls back to `"Base"`
    /// for the merge base term and `"Side N"` for unlabelled sides.
    pub label: String,
    /// Whether this term is a `Base` (a merge ancestor, from
    /// `removes()`) or a `Side` (a conflicting version, from
    /// `adds()`). The renderer uses this to distinguish what to
    /// compare against what — a Base term renders as plain content,
    /// a Side term renders with `Added` / `Removed` origins computed
    /// vs. the first Base in the same `ConflictHunk`.
    pub kind: ConflictTermKind,
    /// Lines of this term, with origin tags relative to the first
    /// `Base` in the enclosing `ConflictHunk`. For a `Base` term all
    /// origins are `Context` (the base is its own reference); for a
    /// `Side` term they're a per-line diff against the base so the
    /// reader sees what *this* side added or removed. Line numbers
    /// follow the same `HunkLine` convention used by regular hunks.
    pub lines: Vec<HunkLine>,
}

/// Distinguishes the two kinds of merge terms exposed by jj. The
/// frontend reads this to label each block (`Base` vs `Side 1` /
/// `Side 2`) and to decide how to colour the diff origins.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictTermKind {
    /// A merge ancestor (negative term in jj's `Merge`). Most 3-way
    /// merges have exactly one of these; criss-cross merges can have
    /// more.
    Base,
    /// One of the conflicting versions (positive term — an `adds()`
    /// entry). Three-way merges have exactly two; N-way merges have
    /// N.
    Side,
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
