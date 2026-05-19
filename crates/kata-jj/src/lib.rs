//! jj integration for the review tool.
//!
//! The [`JjBackend`] trait is the single I/O surface every other
//! kata crate goes through. The production implementation,
//! [`JjLib`], runs in-process against `jj-lib`; the `jj` CLI is no
//! longer a runtime requirement.

pub mod anchor;
pub mod backend;
pub mod diff;
pub mod error;
pub mod libjj;

pub use anchor::{AnchorResolution, FileCache, resolve_anchor};
pub use backend::{Endpoint, JjBackend, ReviewRange};
pub use diff::{build_diff, build_diff_metadata, compute_one_file_hunks};
pub use error::{Error, Result};
pub use libjj::JjLib;
