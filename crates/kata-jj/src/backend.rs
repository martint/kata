use std::path::Path;

use async_trait::async_trait;
use kata_core::{
    Bookmark, ChangeId, CommitId, CommitInfo, ConflictTerm, FileChange, OpId, ReviewId, RevSet,
};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Endpoints of a review, resolved from a revset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewRange {
    pub base: Endpoint,
    pub tip: Endpoint,
}

/// One configured git remote on a workspace. Read-only — kata never
/// edits remotes; the operator owns `git remote add`. Used by the
/// GitHub PR resolver to find which workspace's underlying git
/// repo points at a given github.com `(owner, repo)`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitRemote {
    pub name: String,
    /// Fetch URL as configured in `.git/config`. May be any form
    /// git understands — `https://github.com/o/r.git`,
    /// `git@github.com:o/r.git`, `ssh://git@github.com/o/r.git`,
    /// or non-github URLs. The caller normalises before matching.
    pub fetch_url: String,
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

    /// Read the structured conflict sides of `path` at `commit`. Returns
    /// `Ok(None)` when the file is either resolved or absent — the caller
    /// then falls back to the regular [`Self::read_file`] path.
    /// Implementations that don't expose conflict structure (none today —
    /// the in-process libjj backend overrides this) keep the default
    /// `Ok(None)` so the rest of the diff pipeline degrades gracefully
    /// to "flatten the conflict to whatever `read_file` returned".
    async fn read_conflict_at(
        &self,
        _commit: &CommitId,
        _path: &str,
    ) -> Result<Option<Vec<ConflictTerm>>> {
        Ok(None)
    }

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
    /// `tip = heads(revset)` (the topological head of the set, errors
    /// when there's more than one), `base = heads(::tip & ~revset)`
    /// (the merge-base of the tip with whatever sits just outside
    /// the set — for a simple linear `A..B` this is just `A`).
    async fn resolve_range(&self, revset: &RevSet) -> Result<ReviewRange>;

    /// Metadata for every commit in `revset`, oldest first. The UI's
    /// commits panel reads top-to-bottom and the user expects
    /// chronological order; implementations whose underlying iterator
    /// is newest-first must reverse before returning. (The "patchset
    /// 1 → patchset N" thinking maps cleanly onto "first commit →
    /// last commit"; reversing here means no consumer has to remember
    /// to flip on their own.)
    async fn list_commits(&self, revset: &RevSet) -> Result<Vec<CommitInfo>>;

    /// Whether `ancestor` is reachable from `descendant` walking parent
    /// edges. True for `ancestor == descendant`. Used to detect whether a
    /// patchset fast-forwards from the previous one.
    async fn is_ancestor(
        &self,
        ancestor: &CommitId,
        descendant: &CommitId,
    ) -> Result<bool>;

    /// Current operation id, i.e. the head of `.jj/repo/op_heads`.
    /// Stored as the per-viewer visit baseline so the next open can
    /// compute "since you were here" as a delta from review-side
    /// timestamps. The op-id itself isn't surfaced to the UI.
    async fn current_op_id(&self) -> Result<OpId>;

    /// Walk `revset` in topological order and lay out a column-stem
    /// graph of the result. `max_rows` caps the page; the returned
    /// `has_more` is true iff the walk was cut short. Bookmarks and
    /// the working-copy marker on each row are NOT populated here —
    /// they're repo-level decoration the service layer adds on the
    /// way out.
    async fn browse_log(
        &self,
        revset: &RevSet,
        max_rows: usize,
    ) -> Result<kata_core::LogPage>;

    /// The workspace's current `@` commit id. Used by the service
    /// layer to flag the working-copy row in a [`browse_log`]
    /// result.
    async fn working_copy_commit_id(&self) -> Result<Option<CommitId>>;

    /// Keep `commits` reachable so neither `jj util gc` nor `git gc`
    /// collects them. A review pins its patchset endpoints (and, via
    /// reachability, every commit between a patchset's base and tip)
    /// this way, so a stale patchset's history survives the branch
    /// advancing. `review` namespaces the pins for later cleanup.
    ///
    /// Best-effort and idempotent: a commit that's already gone (or any
    /// other failure) is skipped/logged, never an error — pinning must
    /// not be able to break the write that triggered it. The default is
    /// a no-op so non-git backends (and the test stub) need do nothing.
    async fn pin_commits(&self, _review: &ReviewId, _commits: &[CommitId]) -> Result<()> {
        Ok(())
    }

    /// Drop every pin created for `review` (on review deletion). Default
    /// no-op; see [`Self::pin_commits`].
    async fn unpin_review(&self, _review: &ReviewId) -> Result<()> {
        Ok(())
    }

    /// Configured git remotes on this workspace's underlying git
    /// repo. Default is an empty list so non-git backends and the
    /// test stub don't have to invent shapes. Used by the GitHub PR
    /// resolver to match a PR's `(owner, repo)` to a workspace.
    async fn git_remotes(&self) -> Result<Vec<GitRemote>> {
        Ok(Vec::new())
    }

    /// Fetch `refs/pull/<n>/head` from `remote` into the local
    /// ref `refs/remotes/<remote>/kata-pr/<n>` so subsequent revset
    /// operations can see the PR head as a normal commit. The ref
    /// shape is a standard remote-tracking branch, which means
    /// jj's normal `import_refs` path picks it up — no special-
    /// case filter needed. Idempotent: re-running against an
    /// already-fetched PR fast-forwards or no-ops. The `+` force-
    /// update prefix is intentional — PRs get force-pushed, and
    /// kata wants the new head, not a non-fast-forward error.
    ///
    /// `base_sha` is fetched separately. The base is on whatever
    /// branch the PR targets; that branch may have advanced past
    /// the PR's base since the workspace last fetched it, in which
    /// case the base object isn't in the local store and the revset
    /// `<base>..<head>` fails to resolve. We fetch the base SHA
    /// directly (github.com allows fetch-by-SHA) and import it
    /// alongside the head's bookmark so both endpoints of the
    /// revset are visible.
    ///
    /// Default `unimplemented` so a misconfigured backend errors
    /// loudly rather than silently dropping the fetch.
    async fn git_fetch_pr_head(
        &self,
        _remote: &str,
        _pr_number: u32,
        _base_sha: &str,
    ) -> Result<()> {
        Err(crate::error::Error::Parse(
            "git_fetch_pr_head is not implemented for this backend".into(),
        ))
    }

    /// Drop the `refs/remotes/<remote>/kata-pr/<n>/{head,base}`
    /// refs that [`Self::git_fetch_pr_head`] created, and tell jj
    /// about the removal so the corresponding bookmarks drop out
    /// of the view too. Called when a kata review bound to a
    /// GitHub PR is deleted — otherwise the workspace accumulates
    /// orphan `kata-pr/*@<remote>` bookmarks. `remote` is taken
    /// from the manifest; pass `None` for legacy imports that
    /// don't have one recorded (the impl falls back to scanning
    /// every remote, which is incorrect across multi-remote
    /// workspaces but unblocks cleanup). Best-effort: missing
    /// refs are not an error.
    async fn git_delete_pr_head(
        &self,
        _remote: Option<&str>,
        _pr_number: u32,
    ) -> Result<()> {
        Ok(())
    }
}
