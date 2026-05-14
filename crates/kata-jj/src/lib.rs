//! jj integration for the review tool.
//!
//! Today this layer shells out to the `jj` binary. The [`JjBackend`] trait
//! exists so we can swap in a `jj-lib`-based implementation later without
//! touching consumers.

pub mod anchor;
pub mod backend;
pub mod cli;
pub mod diff;
pub mod error;

pub use anchor::{AnchorResolution, FileCache, resolve_anchor};
pub use backend::{Endpoint, JjBackend, ReviewRange};
pub use cli::JjCli;
pub use diff::build_diff;
pub use error::{Error, Result};
