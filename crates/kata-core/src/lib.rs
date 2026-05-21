//! Domain types for the review tool. No I/O.

pub mod compare;
pub mod diff;
pub mod documents;
pub mod ids;

pub use compare::{
    ChangeStatus, PairDiffCounts, PatchsetCompareView, PatchsetEndpoints, PatchsetPair,
};
pub use diff::{
    ConflictHunk, ConflictSide, Diff, FileChange, FileStatus, Hunk, HunkLine, LineOrigin,
    RegularHunk,
};
pub use documents::{
    Annotation, ApiToken, Comment, Flag, Patchset, RepoManifest, ResolutionAction, Response,
    ReviewManifest, SCHEMA_VERSION, Session, SessionStatus,
};
pub use ids::{
    AnnotationId, ApiTokenId, Author, Bookmark, ChangeId, ColumnRange, CommentId, CommitId,
    CommitInfo, LineRange, LineRangeParseError, OpId, RepoId, RepoSummary, ResponseId, ReviewId,
    RevSet, SessionId, Side,
};
