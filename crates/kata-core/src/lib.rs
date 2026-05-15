//! Domain types for the review tool. No I/O.

pub mod diff;
pub mod documents;
pub mod ids;

pub use diff::{Diff, FileChange, FileStatus, Hunk, HunkLine, LineOrigin};
pub use documents::{
    Comment, Flag, Patchset, RepoManifest, ResolutionAction, Response, ReviewManifest,
    SCHEMA_VERSION, Session, SessionStatus,
};
pub use ids::{
    Author, Bookmark, ChangeId, CommentId, CommitId, CommitInfo, LineRange,
    LineRangeParseError, OpId, OpKind, OpSummary, RepoId, RepoSummary, ResponseId, ReviewId,
    RevSet, SessionId, Side,
};
