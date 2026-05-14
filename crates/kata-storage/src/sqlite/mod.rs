//! SQLite implementation of [`crate::Storage`].
//!
//! Single file on disk, WAL journal mode, one shared connection guarded by
//! a tokio Mutex. SQLite serializes writers internally; WAL keeps readers
//! lock-free against in-flight writes — which is the concurrency story
//! the filesystem store didn't have (multiple agents trampling each
//! other's TOML writes). Service-level multi-step operations that need
//! all-or-nothing semantics use [`Connection::transaction`] with
//! `BEGIN IMMEDIATE`.

mod migrate;
mod serde_enums;
mod storage;

pub use migrate::{MIGRATIONS, Migration, run as run_migrations};
pub use storage::SqliteStorage;
