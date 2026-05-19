//! Domain types for the review tool. No I/O.

pub mod compare;
pub mod diff;
pub mod documents;
pub mod ids;

pub use compare::{
    ChangeStatus, PairDiffCounts, PatchsetCompareView, PatchsetEndpoints, PatchsetPair,
};
pub use diff::{Diff, FileChange, FileStatus, Hunk, HunkLine, LineOrigin};
pub use documents::{
    Annotation, Comment, Flag, Patchset, RepoManifest, ResolutionAction, Response, ReviewManifest,
    SCHEMA_VERSION, Session, SessionStatus,
};
pub use ids::{
    AnnotationId, Author, Bookmark, ChangeId, ColumnRange, CommentId, CommitId, CommitInfo,
    LineRange, LineRangeParseError, OpId, OpKind, OpSummary, RepoId, RepoSummary, ResponseId,
    ReviewId, RevSet, SessionId, Side,
};
