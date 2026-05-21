//! Wire shape for the repository-browser log graph.
//!
//! The layout is computed *server-side* by [`kata_jj`]'s port of
//! Sapling-renderdag (a column-stem algorithm: walk topologically,
//! maintain a `Vec<Stem>` of in-flight column lines, emit edges
//! tagged by their visual shape). The client never walks the DAG;
//! it draws SVG paths between the pre-computed coordinates each
//! [`LogLine`] supplies.
//!
//! The mental model: imagine the textual `jj log --graph` output as
//! a 2-D grid. Each [`LogRow`] is one terminal row. `location` is
//! the `(column, row)` where the row's circle sits. `lines` are the
//! edge segments either originating from this row, terminating at
//! this row, or passing through it vertically. The renderer
//! deduplicates lines that span multiple rows so a single vertical
//! stem isn't drawn N times.
//!
//! Four edge shapes cover everything the algorithm emits:
//!
//! - [`LogLine::ToNode`]: a node's child↔parent edge where the
//!   parent's stem closed cleanly at the parent's row.
//! - [`LogLine::ToIntersection`]: a merge edge — the parent stem
//!   was already in progress in another column, so this child
//!   draws into that column at the boundary `(target.col, row+1)`.
//! - [`LogLine::FromNode`]: a *rescue* curve emitted after column
//!   compaction. The path leaves the source column, hops to `via`,
//!   runs straight down, then bends into the target column.
//! - [`LogLine::ToMissing`]: parent is outside the requested
//!   revset (jj tagged it `Missing`). Renderer draws the
//!   conventional `~` cap.

use serde::{Deserialize, Serialize};

use crate::ids::CommitInfo;

/// `(column, row)` position. Column 0 is leftmost; row indices are
/// monotonically increasing within a page. Carried as `(u32, u32)`
/// rather than a named struct so the serialized shape is the
/// compact two-tuple jjuicy's frontend already consumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogCoord {
    pub col: u32,
    pub row: u32,
}

impl LogCoord {
    pub fn new(col: u32, row: u32) -> Self {
        Self { col, row }
    }
}

/// An edge segment in the log graph. The variant tells the renderer
/// which path shape to use; `source` and `target` carry the
/// endpoints in row-coordinates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum LogLine {
    ToNode {
        source: LogCoord,
        target: LogCoord,
    },
    ToIntersection {
        source: LogCoord,
        target: LogCoord,
    },
    FromNode {
        source: LogCoord,
        target: LogCoord,
        /// Column the rescue curve runs straight down before bending
        /// into `target`. `None` means the renderer should curve
        /// directly source→target with no intermediate vertical.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<u32>,
    },
    ToMissing {
        source: LogCoord,
        target: LogCoord,
    },
}

/// One row of the log graph: the commit at this row plus the
/// edges incident to it. `bookmarks` and `is_working_copy` are
/// repo-level decoration that the backend's layout pass leaves
/// empty / false; [`kata_service`] populates them on the way out.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogRow {
    pub commit: CommitInfo,
    /// Position of the node's circle.
    pub location: LogCoord,
    /// Effective graph width at this row, in columns. The UI uses
    /// this to indent the text portion so neighbouring rows in a
    /// graph-connected run line up — even when this particular
    /// row only touches one column, a long stem passing through
    /// pulls its `padding` out to match.
    pub padding: u32,
    pub lines: Vec<LogLine>,
    /// Bookmarks pointing at `commit.commit_id`. Empty when no
    /// bookmark matches. Populated by the service layer; the
    /// layout algorithm itself doesn't see bookmarks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bookmarks: Vec<String>,
    /// True iff this row's `commit_id` matches the workspace's
    /// current `@`. The UI marks the working-copy node distinctly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_working_copy: bool,
}

/// One page of the log. `has_more` is true when the layout walk
/// hit its row cap before exhausting the revset — the caller can
/// raise `max_rows` to see further.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogPage {
    pub rows: Vec<LogRow>,
    pub has_more: bool,
}
