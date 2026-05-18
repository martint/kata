//! Re-exports of the application service for use by route handlers.
//! The implementation lives in [`kata_service`].

pub use kata_service::{
    AnchorView, AnnotationInput, AnnotationView, CommentView, CommitDiffView, CreateReviewParams,
    DiffCommitsResult, DraftCommentInput, DraftResponseInput, DraftsView, ResponseView,
    ReviewService, ReviewView,
};
