//! Persistence layer for the review tool.
//!
//! The [`Storage`] trait is the swap point — today there's only the
//! filesystem implementation that writes Markdown-with-YAML-frontmatter
//! under a user-configured root, but a database-backed impl can ship later
//! without touching consumers.

pub mod archive;
pub mod error;
pub mod frontmatter;
pub mod ids;
pub mod sqlite;
pub mod storage;

pub use error::{Error, Result};
pub use ids::{compute_repo_id, jj_repo_canonical_path};
pub use storage::{DraftsView, ReviewSummary, Storage};
