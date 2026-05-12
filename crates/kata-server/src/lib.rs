//! HTTP front-end for the review tool.
//!
//! Layering: [`ReviewService`] is the business-logic core sitting above
//! [`kata_storage::Storage`] and [`kata_jj::JjBackend`]. The
//! [`routes`] module adapts that surface to axum HTTP. A future MCP server
//! reuses the same [`ReviewService`] without going through HTTP.

pub mod config;
pub mod embedded;
pub mod error;
pub mod routes;
pub mod service;
pub mod state;

pub use config::ServerConfig;
pub use error::{AppError, AppResult};
pub use routes::{router, router_with_assets, router_with_embedded_assets};
pub use service::{
    CreateReviewParams, DraftCommentInput, DraftResponseInput, ReviewService, ReviewView,
};
pub use state::AppState;
