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
}

/// A contiguous region of changed + surrounding context lines.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Hunk {
    /// `None` for pure-insertion hunks where no base lines are involved.
    pub base_range: Option<LineRange>,
    /// `None` for pure-deletion hunks where no tip lines are involved.
    pub tip_range: Option<LineRange>,
    pub lines: Vec<HunkLine>,
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
