use std::path::Path;

use async_trait::async_trait;
use kata_core::{Bookmark, ChangeId, CommitId, CommitInfo, FileChange, RevSet};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Endpoints of a review, resolved from a revset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewRange {
    pub base: Endpoint,
    pub tip: Endpoint,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub change_id: ChangeId,
    pub commit_id: CommitId,
}

/// Operations the review tool needs from jj. Pure I/O surface — all
/// derived/structural work happens in modules above this one.
#[async_trait]
pub trait JjBackend: Send + Sync {
    /// Canonical path to the repo's `.jj/repo` directory. Used to derive a
    /// stable repo-id that's shared across workspaces of the same repo.
    fn repo_path(&self) -> &Path;

    async fn list_bookmarks(&self) -> Result<Vec<Bookmark>>;

    /// Current commit id for a change id, or `None` if the change has been
    /// abandoned and has no commit anywhere.
    async fn change_to_commit(&self, change: &ChangeId) -> Result<Option<CommitId>>;

    /// Resolve an arbitrary revset expression to a single endpoint
    /// (`change_id` + `commit_id`). `None` when the revset is empty.
    /// Returns the first match if the revset has multiple heads — the
    /// caller is responsible for picking a single-rev expression.
    async fn resolve_endpoint(&self, expr: &str) -> Result<Option<Endpoint>>;

    /// Read a file's contents at a specific commit. `Ok(None)` if the file
    /// does not exist at that commit.
    async fn read_file(&self, commit: &CommitId, path: &str) -> Result<Option<Vec<u8>>>;

    /// Read many `(commit, path)` blobs in one call. Implementations
    /// can amortise process startup across the batch — the [`JjCli`]
    /// override drives `git cat-file --batch` so 252 reads cost one
    /// fork+exec, not 252. The default falls back to a sequential
    /// loop of [`Self::read_file`] for backends that don't have a
    /// faster path. Order of `pairs` is preserved in the returned
    /// `Vec`; each slot is `None` exactly when the file doesn't
    /// exist at that `(commit, path)`.
    ///
    /// [`JjCli`]: crate::cli::JjCli
    async fn read_files(
        &self,
        pairs: &[(CommitId, String)],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        let mut out = Vec::with_capacity(pairs.len());
        for (commit, path) in pairs {
            out.push(self.read_file(commit, path).await?);
        }
        Ok(out)
    }

    /// Metadata for every file that differs between `base` and `tip`. The
    /// returned [`FileChange`]s have `hunks: None` — the diff module fills
    /// them in by reading both sides.
    async fn changed_files(
        &self,
        base: &CommitId,
        tip: &CommitId,
    ) -> Result<Vec<FileChange>>;

    /// Resolve `revset` to its base and tip endpoints. Convention:
    /// `tip = heads(revset)`, `base = roots(revset)-` (the parent of the
    /// earliest commit in the set).
    async fn resolve_range(&self, revset: &RevSet) -> Result<ReviewRange>;

    /// Metadata for every commit in `revset`, in jj's default log order
    /// (newest first).
    async fn list_commits(&self, revset: &RevSet) -> Result<Vec<CommitInfo>>;

    /// Whether `ancestor` is reachable from `descendant` walking parent
    /// edges. True for `ancestor == descendant`. Used to detect whether a
    /// patchset fast-forwards from the previous one.
    async fn is_ancestor(
        &self,
        ancestor: &CommitId,
        descendant: &CommitId,
    ) -> Result<bool>;

}
