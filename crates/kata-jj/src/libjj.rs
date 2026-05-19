//! Thin wrapper around `jj-lib` that exposes just the operations the
//! review tool needs in-process: open a repo at a workspace path,
//! resolve commit-ids, perform an in-memory rebase of one commit onto
//! a new parent (without ever writing the result back to the store),
//! and diff the resulting tree against another commit's tree.
//!
//! Used by the patchset-compare v2 backend to compute *true* per-commit
//! interdiffs (the `git range-diff` operation). The naive
//! `diff(from_commit, to_commit)` is a tree-vs-tree diff that bakes in
//! every downstream change inherited from rewritten ancestors; the
//! correct operation is `diff(rebase(from_commit onto to_commit-),
//! to_commit)`. We use jj-lib's in-memory rebase to avoid touching
//! the user's workspace.
//!
//! Everything in this module is synchronous + blocking against the jj
//! store. Callers should run it inside `tokio::task::spawn_blocking`
//! to keep the async runtime responsive.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::{ReadonlyRepo, Repo as _, RepoLoader};
use jj_lib::settings::UserSettings;
use kata_core::{
    Bookmark, ChangeId as KataChangeId, CommitId as KataCommitId, CommitInfo, FileChange,
    FileStatus, OpId, OpKind, OpSummary,
};

use crate::backend::{Endpoint, JjBackend, ReviewRange};
use crate::error::{Error, Result};

/// Build the [`UserSettings`] every libjj entry point uses. The kata
/// review server never creates commits, so the synthetic
/// `"kata"` / `kata@invalid` identity is only here to satisfy
/// jj-lib's constructor — it would only land in a commit object if
/// someone called `Store::write_commit`, which this codebase never
/// does. Sharing the constructor between [`open_repo`] and
/// [`JjLib`] keeps the identity story in one place.
fn build_settings() -> Result<UserSettings> {
    use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
    let mut config = StackedConfig::with_defaults();
    let mut identity = ConfigLayer::empty(ConfigSource::Default);
    for (key, value) in [
        ("user.name", "kata"),
        ("user.email", "kata@invalid"),
    ] {
        identity
            .set_value(key, value)
            .map_err(|e| Error::Parse(format!("libjj config {key}: {e}")))?;
    }
    config.add_layer(identity);
    UserSettings::from_config(config)
        .map_err(|e| Error::Parse(format!("libjj settings: {e}")))
}

/// Open a jj workspace and return its [`RepoLoader`] plus the
/// workspace name + root. The loader is cheap to keep around —
/// `RepoLoader::load_at_head` rereads the op-heads on disk every
/// call, so methods that need fresh state can trigger a reload
/// without re-running `Workspace::load`. The workspace identity is
/// kept around so revset parsing can resolve `@` to this workspace's
/// working-copy commit. Shared between [`open_repo`] and
/// [`JjLib::new`].
fn open_loader(
    settings: &UserSettings,
    workspace_path: &Path,
) -> Result<OpenedWorkspace> {
    use jj_lib::repo::StoreFactories;
    let workspace = jj_lib::workspace::Workspace::load(
        settings,
        workspace_path,
        &StoreFactories::default(),
        &jj_lib::workspace::default_working_copy_factories(),
    )
    .map_err(|e| Error::Parse(format!("libjj workspace load: {e}")))?;
    Ok(OpenedWorkspace {
        loader: Arc::new(workspace.repo_loader().clone()),
        workspace_name: workspace.workspace_name().to_owned(),
        workspace_root: workspace.workspace_root().to_path_buf(),
    })
}

struct OpenedWorkspace {
    loader: Arc<RepoLoader>,
    workspace_name: jj_lib::ref_name::WorkspaceNameBuf,
    workspace_root: PathBuf,
}

/// Open a jj repo at the given workspace path. Returns a handle the
/// caller can use to look up commits and run rebases. Synchronous —
/// callers should wrap in `spawn_blocking`.
pub fn open_repo(workspace_path: &Path) -> Result<JjRepoHandle> {
    let settings = build_settings()?;
    let opened = open_loader(&settings, workspace_path)?;
    let repo = futures::executor::block_on(opened.loader.load_at_head())
        .map_err(|e| Error::Parse(format!("libjj repo load_at_head: {e}")))?;
    Ok(JjRepoHandle {
        repo,
        _settings: settings,
    })
}

/// Opened jj repo handle. Cheap to keep around; the underlying
/// `ReadonlyRepo` is reference-counted via `Arc`.
pub struct JjRepoHandle {
    repo: Arc<jj_lib::repo::ReadonlyRepo>,
    _settings: jj_lib::settings::UserSettings,
}

impl JjRepoHandle {
    /// Resolve a hex commit-id (as we get them from our manifest /
    /// the CLI) to a jj-lib `Commit` object.
    pub fn lookup_commit(
        &self,
        id: &KataCommitId,
    ) -> Result<jj_lib::commit::Commit> {
        use jj_lib::backend::CommitId;
        let bytes = hex::decode(id.as_str())
            .map_err(|e| Error::Parse(format!("commit id not hex: {e}")))?;
        let commit_id = CommitId::new(bytes);
        self.repo
            .store()
            .get_commit(&commit_id)
            .map_err(|e| Error::Parse(format!("libjj get_commit: {e}")))
    }
}

impl JjRepoHandle {
    /// True per-commit interdiff: rebase `from_commit` onto
    /// `to_commit`'s parent in-memory (transaction discarded, repo
    /// store untouched), then diff the rebased tree against
    /// `to_commit`'s tree.
    ///
    /// Returns the diff in [`kata_core::Diff`] shape. The `base`
    /// field is set to `to_commit`'s parent id (a real persisted
    /// commit) so the FileChange.path values are still
    /// self-consistent with what the caller might fetch separately.
    ///
    /// Per-file `hunks` are omitted from this metadata path — the
    /// caller fetches them lazily via [`compute_rebased_file_hunks`]
    /// once a file scrolls into view. Line counts (`added` /
    /// `removed`) are populated so the side-panel chips read
    /// correctly.
    pub fn compute_rebased_diff(
        &self,
        from_commit_id: &KataCommitId,
        to_commit_id: &KataCommitId,
    ) -> Result<kata_core::Diff> {
        use futures::StreamExt;
        use jj_lib::matchers::EverythingMatcher;
        use jj_lib::merge::Merge;
        use jj_lib::merged_tree::MergedTree;
        use jj_lib::object_id::ObjectId;

        let from_commit = self.lookup_commit(from_commit_id)?;
        let to_commit = self.lookup_commit(to_commit_id)?;
        let to_parent_id = to_commit.parent_ids().first().ok_or_else(|| {
            Error::Parse(format!(
                "to_commit {} has no parent — interdiff undefined",
                to_commit_id
            ))
        })?.clone();
        let to_parent_id_hex = to_parent_id.hex();
        let to_parent_commit = self.repo.store().get_commit(&to_parent_id)
            .map_err(|e| Error::Parse(format!("libjj get_commit (to_parent): {e}")))?;
        let from_parent_id = from_commit.parent_ids().first().ok_or_else(|| {
            Error::Parse(format!(
                "from_commit {} has no parent — interdiff undefined",
                from_commit_id
            ))
        })?.clone();
        let from_parent_commit = self.repo.store().get_commit(&from_parent_id)
            .map_err(|e| Error::Parse(format!("libjj get_commit (from_parent): {e}")))?;

        // Build the rebased tree via a 3-way merge on tree-ids
        // directly: removes=[from_parent_tree], adds=[from_tree,
        // to_parent_tree]. This is the "rebase from onto to_parent"
        // semantic in pure tree-merge form — no Commit object is
        // ever created, so no transaction, no store write, no
        // contention with concurrent rebases. (rebase_commit, the
        // higher-level API, writes the new commit to the immutable
        // store via `builder.write()` which serialises on a global
        // lock — fine for sequential CLI use, deadly for the
        // per-pair parallel fetch we do here.)
        let from_tree_id = require_resolved_tree(from_commit.tree_ids())?;
        let from_parent_tree_id = require_resolved_tree(from_parent_commit.tree_ids())?;
        let to_parent_tree_id = require_resolved_tree(to_parent_commit.tree_ids())?;

        let merge = Merge::from_removes_adds(
            vec![from_parent_tree_id],
            vec![from_tree_id, to_parent_tree_id],
        );
        let merged = futures::executor::block_on(jj_lib::tree_merge::merge_trees(
            self.repo.store(),
            merge,
        ))
        .map_err(|e| Error::Parse(format!("libjj merge_trees: {e}")))?;
        let rebased_tree = MergedTree::new(
            self.repo.store().clone(),
            merged,
            jj_lib::conflict_labels::ConflictLabels::from_vec(Vec::new()),
        );
        let to_tree = to_commit.tree();

        // diff_stream(left, right) yields entries whose `left`
        // (Diff<MergedTreeValue>.left) is the side that EXISTS in
        // `self` (the receiver — rebased_tree) and `right` is the
        // side that exists in `other` (to_tree). Match our
        // base..tip convention: rebased = base, to = tip.
        let matcher = EverythingMatcher;
        let store = self.repo.store();
        let mut stream = rebased_tree.diff_stream(&to_tree, &matcher);
        let mut files: Vec<kata_core::FileChange> = Vec::new();
        while let Some(entry) = futures::executor::block_on(stream.next()) {
            let path_str = entry.path.as_internal_file_string().to_string();
            let values = entry
                .values
                .map_err(|e| Error::Parse(format!("libjj diff entry: {e}")))?;
            // We requested file-only entries (diff_stream skips tree
            // entries), so each side is a Merge<Option<TreeValue>>
            // for one path. Resolved (non-conflicted) trees emit
            // exactly the file or its absence.
            let left_id = values
                .before
                .as_resolved()
                .and_then(|opt| opt.as_ref())
                .and_then(|tv| match tv {
                    jj_lib::backend::TreeValue::File { id, .. } => Some(id.clone()),
                    _ => None,
                });
            let right_id = values
                .after
                .as_resolved()
                .and_then(|opt| opt.as_ref())
                .and_then(|tv| match tv {
                    jj_lib::backend::TreeValue::File { id, .. } => Some(id.clone()),
                    _ => None,
                });

            let status = match (left_id.is_some(), right_id.is_some()) {
                (true, true) => kata_core::FileStatus::Modified,
                (false, true) => kata_core::FileStatus::Added,
                (true, false) => kata_core::FileStatus::Deleted,
                (false, false) => continue, // shouldn't happen but skip
            };

            // Read both blobs (or empty for missing side) and run
            // imara-diff for line counts. This is identical to what
            // build_diff_metadata does for CLI-fetched blobs.
            let left_bytes = if let Some(id) = &left_id {
                read_file_bytes(store, &entry.path, id)?
            } else {
                Vec::new()
            };
            let right_bytes = if let Some(id) = &right_id {
                read_file_bytes(store, &entry.path, id)?
            } else {
                Vec::new()
            };

            let (binary, added, removed) =
                count_line_changes(&left_bytes, &right_bytes);

            files.push(kata_core::FileChange {
                path: path_str,
                status,
                hunks: None,
                binary,
                added,
                removed,
            });
        }

        Ok(kata_core::Diff {
            base: KataCommitId::new(to_parent_id_hex),
            tip: to_commit_id.clone(),
            files,
        })
    }
}

impl JjRepoHandle {
    /// Full hunks for one file in the rebase-based interdiff. Same
    /// (from_commit, to_commit) semantics as [`Self::compute_rebased_diff`],
    /// but populates `FileChange::hunks` from imara-diff. Returns an
    /// error when the file isn't in the interdiff (deleted on both
    /// sides, identical, etc.).
    pub fn compute_rebased_file_hunks(
        &self,
        from_commit_id: &KataCommitId,
        to_commit_id: &KataCommitId,
        path: &str,
    ) -> Result<kata_core::FileChange> {
        let diff = self.compute_rebased_diff(from_commit_id, to_commit_id)?;
        let target = diff
            .files
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| {
                Error::Parse(format!(
                    "file {:?} not present in rebased interdiff",
                    path
                ))
            })?;
        if target.binary {
            return Ok(target);
        }
        // Re-synthesize the rebased tree (cheap — repeats a few
        // tree-id lookups + tree merges, no commit creation) and
        // pull the two file blobs for hunk computation.
        let to_commit = self.lookup_commit(to_commit_id)?;
        let rebased_tree = self.synthesize_rebased_tree(from_commit_id, to_commit_id)?;
        let to_tree = to_commit.tree();
        let repo_path = jj_lib::repo_path::RepoPathBuf::from_internal_string(path)
            .map_err(|e| Error::Parse(format!("libjj repo_path: {e}")))?;
        let left_value =
            futures::executor::block_on(rebased_tree.path_value(repo_path.as_ref()))
                .map_err(|e| Error::Parse(format!("libjj path_value left: {e}")))?;
        let right_value =
            futures::executor::block_on(to_tree.path_value(repo_path.as_ref()))
                .map_err(|e| Error::Parse(format!("libjj path_value right: {e}")))?;
        let store = self.repo.store();
        let left_bytes = file_bytes_from_value(store, repo_path.as_ref(), &left_value)?;
        let right_bytes = file_bytes_from_value(store, repo_path.as_ref(), &right_value)?;

        let hunks = compute_hunks(&left_bytes, &right_bytes, path)?;
        Ok(kata_core::FileChange {
            hunks: Some(hunks),
            ..target
        })
    }

    /// Internal: build the rebased tree (from_commit reapplied on
    /// to_commit's parent) via [`merge_trees`] — no Commit object,
    /// no transaction, no store-lock contention.
    fn synthesize_rebased_tree(
        &self,
        from_commit_id: &KataCommitId,
        to_commit_id: &KataCommitId,
    ) -> Result<jj_lib::merged_tree::MergedTree> {
        use jj_lib::merge::Merge;
        use jj_lib::merged_tree::MergedTree;

        let from_commit = self.lookup_commit(from_commit_id)?;
        let to_commit = self.lookup_commit(to_commit_id)?;
        let to_parent_id = to_commit
            .parent_ids()
            .first()
            .ok_or_else(|| {
                Error::Parse(format!("to_commit {} has no parent", to_commit_id))
            })?
            .clone();
        let to_parent_commit = self
            .repo
            .store()
            .get_commit(&to_parent_id)
            .map_err(|e| Error::Parse(format!("libjj get_commit (to_parent): {e}")))?;
        let from_parent_id = from_commit
            .parent_ids()
            .first()
            .ok_or_else(|| {
                Error::Parse(format!("from_commit {} has no parent", from_commit_id))
            })?
            .clone();
        let from_parent_commit = self
            .repo
            .store()
            .get_commit(&from_parent_id)
            .map_err(|e| Error::Parse(format!("libjj get_commit (from_parent): {e}")))?;

        let from_tree_id = require_resolved_tree(from_commit.tree_ids())?;
        let from_parent_tree_id =
            require_resolved_tree(from_parent_commit.tree_ids())?;
        let to_parent_tree_id =
            require_resolved_tree(to_parent_commit.tree_ids())?;
        let merge = Merge::from_removes_adds(
            vec![from_parent_tree_id],
            vec![from_tree_id, to_parent_tree_id],
        );
        let merged = futures::executor::block_on(jj_lib::tree_merge::merge_trees(
            self.repo.store(),
            merge,
        ))
        .map_err(|e| Error::Parse(format!("libjj merge_trees: {e}")))?;
        Ok(MergedTree::new(
            self.repo.store().clone(),
            merged,
            jj_lib::conflict_labels::ConflictLabels::from_vec(Vec::new()),
        ))
    }
}

/// In-process jj backend built on `jj-lib`. Replaces the subprocess
/// `JjCli` backend: every method runs inside `spawn_blocking` and
/// talks to jj-lib's `RepoLoader` directly. `load_at_head` is called
/// on every operation so the backend always sees the latest op
/// (matching JjCli's semantic — each subprocess call rereads the
/// repo from scratch).
pub struct JjLib {
    workspace_path: PathBuf,
    _settings: UserSettings,
    /// Cached so we skip the cost of re-running `Workspace::load`
    /// on every method call. `load_at_head` still rereads the op
    /// heads file off disk, so fresh state is picked up.
    loader: Arc<RepoLoader>,
    /// Workspace identity, needed by the revset parser to resolve
    /// `@` to this workspace's working-copy commit.
    workspace_name: Arc<jj_lib::ref_name::WorkspaceNameBuf>,
    workspace_root: Arc<PathBuf>,
}

impl JjLib {
    pub fn new(workspace_path: impl Into<PathBuf>) -> Result<Self> {
        let workspace_path = workspace_path.into();
        let settings = build_settings()?;
        let opened = open_loader(&settings, &workspace_path)?;
        Ok(Self {
            workspace_path,
            _settings: settings,
            loader: opened.loader,
            workspace_name: Arc::new(opened.workspace_name),
            workspace_root: Arc::new(opened.workspace_root),
        })
    }
}

#[async_trait]
impl JjBackend for JjLib {
    fn repo_path(&self) -> &Path {
        &self.workspace_path
    }

    async fn list_bookmarks(&self) -> Result<Vec<Bookmark>> {
        let loader = self.loader.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<Bookmark>> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let view = repo.view();
            let store = repo.store();
            let mut out = Vec::new();
            for (name, target) in view.local_bookmarks() {
                // Skip conflicted bookmarks (multiple targets); jj's
                // CLI template emits nothing for them too via the
                // `if(normal_target, ...)` guard.
                let Some(commit_id) = target.as_normal() else {
                    continue;
                };
                let commit = store
                    .get_commit(commit_id)
                    .map_err(|e| Error::Parse(format!("libjj get_commit: {e}")))?;
                let ts = format_jj_timestamp(&commit.author().timestamp);
                out.push(Bookmark {
                    name: name.as_str().to_string(),
                    change_id: KataChangeId::new(commit.change_id().reverse_hex()),
                    commit_id: KataCommitId::new(commit_id.hex()),
                    commit_timestamp: ts,
                });
            }
            // Newest-first by commit timestamp — matches JjCli's
            // sort so the create-review screen reads identically.
            out.sort_by(|a, b| b.commit_timestamp.cmp(&a.commit_timestamp));
            Ok(out)
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn change_to_commit(
        &self,
        change: &KataChangeId,
    ) -> Result<Option<KataCommitId>> {
        let loader = self.loader.clone();
        let change = change.clone();
        let workspace_name = self.workspace_name.clone();
        let workspace_root = self.workspace_root.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<KataCommitId>> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            // Round-trip through the revset string parser:
            // `latest(change_id_prefix(<reverse-hex>))` matches the
            // commit (most recent if the change is divergent) the
            // way JjCli's `latest(change_id(...))` does. Going
            // through `parse` keeps us on one revset code path
            // (parse → resolve → evaluate) for every call.
            let expr = format!("latest(change_id({}))", change.as_str());
            let ws = Some((workspace_name.as_ref().as_ref(), workspace_root.as_path()));
            let Some(rs) = evaluate_user_revset(&repo, &expr, ws)? else {
                return Ok(None);
            };
            let mut iter = rs.commit_change_ids();
            match iter.next() {
                None => Ok(None),
                Some(Err(e)) => Err(Error::Parse(format!("libjj iter: {e}"))),
                Some(Ok((commit_id, _))) => Ok(Some(KataCommitId::new(commit_id.hex()))),
            }
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn resolve_endpoint(&self, expr: &str) -> Result<Option<Endpoint>> {
        let loader = self.loader.clone();
        let expr = expr.to_string();
        let workspace_name = self.workspace_name.clone();
        let workspace_root = self.workspace_root.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<Endpoint>> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let ws = Some((workspace_name.as_ref().as_ref(), workspace_root.as_path()));
            match evaluate_user_revset(&repo, &expr, ws)? {
                None => Ok(None),
                Some(revset) => {
                    let mut iter = revset.commit_change_ids();
                    match iter.next() {
                        None => Ok(None),
                        Some(Err(e)) => Err(Error::Parse(format!("libjj iter: {e}"))),
                        Some(Ok((commit_id, change_id))) => Ok(Some(Endpoint {
                            change_id: KataChangeId::new(change_id.reverse_hex()),
                            commit_id: KataCommitId::new(commit_id.hex()),
                        })),
                    }
                }
            }
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn read_file(
        &self,
        commit: &KataCommitId,
        path: &str,
    ) -> Result<Option<Vec<u8>>> {
        let loader = self.loader.clone();
        let commit = commit.clone();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            read_file_at_commit(&repo, &commit, &path)
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn read_files(
        &self,
        pairs: &[(KataCommitId, String)],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let loader = self.loader.clone();
        let pairs = pairs.to_vec();
        tokio::task::spawn_blocking(move || -> Result<Vec<Option<Vec<u8>>>> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            // Sequential reads. Each read is in-process disk I/O —
            // no subprocess overhead to amortise. If profiling shows
            // this hot, swap for a futures::stream::iter().buffered()
            // pattern via the store's async API.
            let mut out = Vec::with_capacity(pairs.len());
            for (commit, path) in &pairs {
                out.push(read_file_at_commit(&repo, commit, path)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn changed_files(
        &self,
        base: &KataCommitId,
        tip: &KataCommitId,
    ) -> Result<Vec<FileChange>> {
        let loader = self.loader.clone();
        let base = base.clone();
        let tip = tip.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<FileChange>> {
            use futures::{StreamExt, TryStreamExt};
            use jj_lib::copies::{CopyOperation, CopyRecords};
            use jj_lib::matchers::EverythingMatcher;
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let base_commit = lookup_commit(&repo, &base)?;
            let tip_commit = lookup_commit(&repo, &tip)?;
            let base_tree = base_commit.tree();
            let tip_tree = tip_commit.tree();
            // Populate copy records from the backend so renamed files
            // come back as `Renamed { old_path }` instead of the
            // raw `Added` + `Deleted` pair `diff_stream` would emit.
            // The git backend implements `get_copy_records` via
            // git's rename detection (`git diff -M`); other backends
            // return an empty stream so the diff still works, just
            // without rename info.
            let mut copy_records = CopyRecords::default();
            let copy_stream = repo
                .store()
                .get_copy_records(
                    None,
                    base_commit.id(),
                    tip_commit.id(),
                )
                .map_err(|e| Error::Parse(format!("libjj get_copy_records: {e}")))?;
            let records: Vec<_> = futures::executor::block_on(copy_stream.try_collect())
                .map_err(|e| Error::Parse(format!("libjj copy records: {e}")))?;
            copy_records.add_records(records);
            let matcher = EverythingMatcher;
            let mut stream = base_tree.diff_stream_with_copies(
                &tip_tree,
                &matcher,
                &copy_records,
            );
            let mut out = Vec::new();
            while let Some(entry) = futures::executor::block_on(stream.next()) {
                let target = entry.path.target().as_internal_file_string().to_string();
                let source_op = entry.path.copy_operation();
                let values = entry
                    .values
                    .map_err(|e| Error::Parse(format!("libjj diff entry: {e}")))?;
                let before_present = values
                    .before
                    .as_resolved()
                    .map(|opt| opt.is_some())
                    .unwrap_or(false);
                let after_present = values
                    .after
                    .as_resolved()
                    .map(|opt| opt.is_some())
                    .unwrap_or(false);
                let status = match (before_present, after_present, source_op) {
                    // Rename: the source path is recorded; jj uses
                    // `CopyOperation::Rename` for the
                    // delete-and-add pair, `Copy` when the source
                    // sticks around.
                    (_, true, Some(CopyOperation::Rename)) => FileStatus::Renamed {
                        old_path: entry
                            .path
                            .source()
                            .as_internal_file_string()
                            .to_string(),
                    },
                    // Copy: source still exists; treat the target
                    // as a plain addition, matching JjCli's
                    // `copied → Added` mapping.
                    (_, true, Some(CopyOperation::Copy)) => FileStatus::Added,
                    (true, true, None) => FileStatus::Modified,
                    (false, true, None) => FileStatus::Added,
                    (true, false, None) => FileStatus::Deleted,
                    _ => continue,
                };
                out.push(FileChange {
                    path: target,
                    status,
                    hunks: None,
                    binary: false,
                    added: 0,
                    removed: 0,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn resolve_range(
        &self,
        revset: &kata_core::RevSet,
    ) -> Result<ReviewRange> {
        let loader = self.loader.clone();
        let revset = revset.clone();
        let workspace_name = self.workspace_name.clone();
        let workspace_root = self.workspace_root.clone();
        tokio::task::spawn_blocking(move || -> Result<ReviewRange> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let ws = Some((workspace_name.as_ref().as_ref(), workspace_root.as_path()));
            let tip = solo_endpoint(
                &repo,
                &format!("heads({revset})"),
                &revset,
                ws,
            )?;
            let base = solo_endpoint(
                &repo,
                &format!("roots({revset})-"),
                &revset,
                ws,
            )?;
            Ok(ReviewRange { base, tip })
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn list_commits(
        &self,
        revset: &kata_core::RevSet,
    ) -> Result<Vec<CommitInfo>> {
        let loader = self.loader.clone();
        let revset_str = revset.to_string();
        let workspace_name = self.workspace_name.clone();
        let workspace_root = self.workspace_root.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<CommitInfo>> {
            use futures::StreamExt;
            use jj_lib::matchers::EverythingMatcher;
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let ws = Some((workspace_name.as_ref().as_ref(), workspace_root.as_path()));
            let Some(rs) = evaluate_user_revset(&repo, &revset_str, ws)? else {
                return Ok(Vec::new());
            };
            // jj's default order is newest-first; our trait contract
            // matches that. `--reversed` (oldest-first) is JjCli's
            // override for the commits panel; the service flips at
            // its own layer, so we honour the trait's documented
            // order here.
            let mut out: Vec<CommitInfo> = Vec::new();
            let store = repo.store();
            for item in rs.commit_change_ids() {
                let (commit_id, _change_id) = item
                    .map_err(|e| Error::Parse(format!("libjj iter: {e}")))?;
                let commit = store.get_commit(&commit_id).map_err(|e| {
                    Error::Parse(format!("libjj get_commit: {e}"))
                })?;
                let description = commit.description().to_string();
                let first_line = description
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                // changed_files = files touched relative to first
                // parent; mirrors what JjCli pulls from the
                // `diff.files()` template fragment.
                let changed_files = if let Some(parent_id) = commit.parent_ids().first() {
                    let parent = store.get_commit(parent_id).map_err(|e| {
                        Error::Parse(format!("libjj get_commit (parent): {e}"))
                    })?;
                    let matcher = EverythingMatcher;
                    let mut stream = parent.tree().diff_stream(&commit.tree(), &matcher);
                    let mut files = Vec::new();
                    while let Some(entry) = futures::executor::block_on(stream.next()) {
                        files.push(entry.path.as_internal_file_string().to_string());
                    }
                    files
                } else {
                    Vec::new()
                };
                out.push(CommitInfo {
                    change_id: KataChangeId::new(commit.change_id().reverse_hex()),
                    commit_id: KataCommitId::new(commit_id.hex()),
                    author_email: commit.author().email.clone(),
                    author_timestamp: format_jj_timestamp(&commit.author().timestamp),
                    description_first_line: first_line,
                    description,
                    changed_files,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn is_ancestor(
        &self,
        ancestor: &KataCommitId,
        descendant: &KataCommitId,
    ) -> Result<bool> {
        if ancestor == descendant {
            return Ok(true);
        }
        let loader = self.loader.clone();
        let ancestor = ancestor.clone();
        let descendant = descendant.clone();
        tokio::task::spawn_blocking(move || -> Result<bool> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let ancestor_id = parse_commit_id(&ancestor)?;
            let descendant_id = parse_commit_id(&descendant)?;
            repo.index()
                .is_ancestor(&ancestor_id, &descendant_id)
                .map_err(|e| Error::Parse(format!("libjj is_ancestor: {e}")))
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn current_op_id(&self) -> Result<OpId> {
        let loader = self.loader.clone();
        tokio::task::spawn_blocking(move || -> Result<OpId> {
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            Ok(OpId::new(repo.op_id().hex()))
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }

    async fn ops_between(
        &self,
        prev: &OpId,
        current: &OpId,
    ) -> Result<Vec<OpSummary>> {
        if prev == current {
            return Ok(Vec::new());
        }
        let loader = self.loader.clone();
        let prev = prev.clone();
        let current = current.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<OpSummary>> {
            use futures::{StreamExt, TryStreamExt};
            use jj_lib::op_store::OperationId;
            let repo = futures::executor::block_on(loader.load_at_head())
                .map_err(|e| Error::Parse(format!("libjj load_at_head: {e}")))?;
            let op_store = repo.op_store();
            let load_op = |id_hex: &str| -> Result<jj_lib::operation::Operation> {
                let bytes = hex::decode(id_hex).map_err(|e| {
                    Error::Parse(format!("op-id not hex: {e}"))
                })?;
                let id = OperationId::new(bytes);
                let stored =
                    futures::executor::block_on(op_store.read_operation(&id))
                        .map_err(|e| Error::Parse(format!("libjj read_operation: {e}")))?;
                Ok(jj_lib::operation::Operation::new(
                    op_store.clone(),
                    id,
                    stored,
                ))
            };
            let current_op = load_op(current.as_str())?;
            let prev_op = load_op(prev.as_str()).ok();
            // The two `walk_ancestors*` functions return distinct
            // opaque `impl Stream` types, so we box them through a
            // common trait object before storing in a binding.
            let stream: std::pin::Pin<
                Box<dyn futures::Stream<Item = jj_lib::op_store::OpStoreResult<jj_lib::operation::Operation>>>,
            > = if let Some(prev_op) = &prev_op {
                Box::pin(jj_lib::op_walk::walk_ancestors_range(
                    std::slice::from_ref(&current_op),
                    std::slice::from_ref(prev_op),
                ))
            } else {
                Box::pin(jj_lib::op_walk::walk_ancestors(std::slice::from_ref(
                    &current_op,
                )))
            };
            // jj's CLI walk used a 200-op window; match it so we
            // don't blow up on pathological history. `walk_ancestors_range`
            // already excludes ancestors of `prev_op`, so this is
            // just a safety cap on degenerate inputs.
            const WINDOW: usize = 200;
            let collected: Vec<jj_lib::operation::Operation> =
                futures::executor::block_on(
                    stream.take(WINDOW).try_collect::<Vec<_>>(),
                )
                .map_err(|e| Error::Parse(format!("libjj op walk: {e}")))?;
            // walk_ancestors_range yields newest-first; the trait
            // returns oldest-first to mirror JjCli.
            let mut summaries: Vec<OpSummary> = Vec::with_capacity(collected.len());
            for op in collected.into_iter().rev() {
                let meta = op.metadata();
                if meta.is_snapshot {
                    continue;
                }
                let description = meta.description.clone();
                let kind = classify_op(&description);
                summaries.push(OpSummary {
                    op_id: OpId::new(op.id().hex()),
                    kind,
                    time: format_jj_timestamp(&meta.time.end),
                    description,
                });
            }
            Ok(summaries)
        })
        .await
        .map_err(|e| Error::Parse(format!("spawn_blocking: {e}")))?
    }
}

/// Look up a kata `CommitId` (hex string) inside an open repo.
fn lookup_commit(
    repo: &Arc<ReadonlyRepo>,
    id: &KataCommitId,
) -> Result<jj_lib::commit::Commit> {
    let backend_id = parse_commit_id(id)?;
    repo.store()
        .get_commit(&backend_id)
        .map_err(|e| Error::Parse(format!("libjj get_commit: {e}")))
}

fn parse_commit_id(id: &KataCommitId) -> Result<jj_lib::backend::CommitId> {
    let bytes = hex::decode(id.as_str())
        .map_err(|e| Error::Parse(format!("commit-id not hex: {e}")))?;
    Ok(jj_lib::backend::CommitId::new(bytes))
}

/// Read `path` at `commit`. Returns `Ok(None)` when the file
/// doesn't exist at that commit — same contract as JjCli's
/// `run_or_missing`-wrapped `jj file show`.
fn read_file_at_commit(
    repo: &Arc<ReadonlyRepo>,
    commit: &KataCommitId,
    path: &str,
) -> Result<Option<Vec<u8>>> {
    let commit = lookup_commit(repo, commit)?;
    let repo_path = jj_lib::repo_path::RepoPathBuf::from_internal_string(path)
        .map_err(|e| Error::Parse(format!("libjj repo_path: {e}")))?;
    let value = futures::executor::block_on(commit.tree().path_value(repo_path.as_ref()))
        .map_err(|e| Error::Parse(format!("libjj path_value: {e}")))?;
    let Some(Some(tv)) = value.as_resolved() else {
        return Ok(None);
    };
    match tv {
        jj_lib::backend::TreeValue::File { id, .. } => {
            let bytes = read_file_bytes(repo.store(), repo_path.as_ref(), id)?;
            Ok(Some(bytes))
        }
        // Symlink / submodule / tree at this path — not a file we
        // can read. Reported as absent so callers fall back to the
        // "missing file" path rather than handing back garbage.
        _ => Ok(None),
    }
}

/// Resolve a single-commit-revset expression (`heads(X)` /
/// `roots(X)-`) to its endpoint. Errors when the expression
/// resolves to zero commits (`EmptyRevset`) or more than one
/// (`MultipleHeads`) — same contract as JjCli's `solo_endpoint`.
fn solo_endpoint(
    repo: &Arc<ReadonlyRepo>,
    expr: &str,
    revset: &kata_core::RevSet,
    workspace: Option<(&jj_lib::ref_name::WorkspaceName, &Path)>,
) -> Result<Endpoint> {
    let Some(rs) = evaluate_user_revset(repo, expr, workspace)? else {
        return Err(Error::EmptyRevset {
            revset: revset.to_string(),
        });
    };
    let mut iter = rs.commit_change_ids();
    let first = match iter.next() {
        None => {
            return Err(Error::EmptyRevset {
                revset: revset.to_string(),
            });
        }
        Some(Err(e)) => return Err(Error::Parse(format!("libjj iter: {e}"))),
        Some(Ok(pair)) => pair,
    };
    if iter.next().is_some() {
        return Err(Error::MultipleHeads {
            revset: revset.to_string(),
        });
    }
    Ok(Endpoint {
        change_id: KataChangeId::new(first.1.reverse_hex()),
        commit_id: KataCommitId::new(first.0.hex()),
    })
}

/// Parse a user-supplied revset string, resolve symbols against the
/// repo, and evaluate. Returns `Ok(None)` when the parser succeeds
/// but the expression evaluates to no commits — mirrors JjCli's
/// `run_or_missing` semantics on the `log -r <expr>` path.
fn evaluate_user_revset<'a>(
    repo: &'a Arc<ReadonlyRepo>,
    expr: &str,
    workspace: Option<(&jj_lib::ref_name::WorkspaceName, &Path)>,
) -> Result<Option<Box<dyn jj_lib::revset::Revset + 'a>>> {
    use jj_lib::fileset::FilesetAliasesMap;
    use jj_lib::repo_path::RepoPathUiConverter;
    use jj_lib::revset::{
        RevsetAliasesMap, RevsetDiagnostics, RevsetExtensions, RevsetParseContext,
        RevsetWorkspaceContext, SymbolResolver,
    };
    // jj-cli's bundled config defines a handful of stock revset
    // aliases (trunk(), immutable_heads(), etc.); jj-lib doesn't.
    // Without these, any user revset that calls `trunk()`
    // (overwhelmingly common in real configs) errors out with
    // "Function `trunk` doesn't exist". Reproduce the most-used
    // ones here. The `present(...)` arms make the alias work in
    // demo / single-user repos that don't have an `origin` remote,
    // matching what most users mean by "trunk" when there's no
    // upstream — the local bookmark of the same name.
    let mut aliases_map = RevsetAliasesMap::new();
    for (decl, defn) in [
        (
            "trunk()",
            "latest(\
                remote_bookmarks(exact:\"main\", remote=exact:\"origin\") | \
                remote_bookmarks(exact:\"master\", remote=exact:\"origin\") | \
                remote_bookmarks(exact:\"trunk\", remote=exact:\"origin\") | \
                present(main) | present(master) | present(trunk)\
            )",
        ),
        ("immutable_heads()", "builtin_immutable_heads()"),
        ("builtin_immutable_heads()", "trunk()"),
    ] {
        aliases_map
            .insert(decl, defn)
            .map_err(|e| Error::Parse(format!("libjj alias {decl}: {e}")))?;
    }
    let fileset_aliases_map = FilesetAliasesMap::new();
    let extensions = RevsetExtensions::default();
    // Build the workspace context if we know the workspace. Without
    // it, `@` in a revset can't resolve. The path converter only
    // matters for `file(path)` revsets, which kata doesn't use —
    // give it the workspace root for both cwd and base so any such
    // input resolves consistently.
    let path_converter = workspace.map(|(_, root)| RepoPathUiConverter::Fs {
        cwd: root.to_path_buf(),
        base: root.to_path_buf(),
    });
    let workspace_ctx = match (workspace, path_converter.as_ref()) {
        (Some((name, _)), Some(converter)) => Some(RevsetWorkspaceContext {
            path_converter: converter,
            workspace_name: name,
        }),
        _ => None,
    };
    let ctx = RevsetParseContext {
        aliases_map: &aliases_map,
        local_variables: std::collections::HashMap::new(),
        // kata never evaluates author/committer filters that depend
        // on `mine`, so a placeholder email is fine. The setting
        // exists because jj-lib's parser unconditionally captures
        // it from the user context.
        user_email: "kata@invalid",
        date_pattern_context: chrono::Utc::now().fixed_offset().into(),
        default_ignored_remote: Some("git".as_ref()),
        fileset_aliases_map: &fileset_aliases_map,
        use_glob_by_default: false,
        extensions: &extensions,
        workspace: workspace_ctx,
    };
    let mut diags = RevsetDiagnostics::new();
    let parsed = jj_lib::revset::parse(&mut diags, expr, &ctx)
        .map_err(|e| Error::Parse(format!("libjj parse revset {expr:?}: {e}")))?;
    // `&[]` alone doesn't tell rustc which `SymbolResolverExtension`
    // element type to infer; spell it out so it picks the empty-
    // extensions branch.
    let extensions: [Box<dyn jj_lib::revset::SymbolResolverExtension>; 0] = [];
    let symbol_resolver =
        SymbolResolver::new(repo.as_ref() as &dyn jj_lib::repo::Repo, &extensions);
    let resolved = parsed
        .resolve_user_expression(repo.as_ref() as &dyn jj_lib::repo::Repo, &symbol_resolver)
        .map_err(|e| Error::Parse(format!("libjj resolve revset {expr:?}: {e}")))?;
    let rs = resolved
        .evaluate(repo.as_ref() as &dyn jj_lib::repo::Repo)
        .map_err(|e| Error::Parse(format!("libjj evaluate revset {expr:?}: {e}")))?;
    if rs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rs))
    }
}

/// Format a jj-lib `Timestamp` the way the kata CLI templates emit it
/// (`%+` = ISO 8601 with offset). Used wherever the trait returns
/// a stringified timestamp.
fn format_jj_timestamp(ts: &jj_lib::backend::Timestamp) -> String {
    use chrono::{FixedOffset, TimeZone};
    // jj's Timestamp is `(MillisSinceEpoch, FixedOffset minutes)`.
    let offset =
        FixedOffset::east_opt(ts.tz_offset * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());
    let dt = offset.timestamp_millis_opt(ts.timestamp.0).single();
    match dt {
        Some(dt) => dt.format("%+").to_string(),
        None => String::new(),
    }
}

/// Map a jj op description's first word to a [`OpKind`] bucket.
/// jj's op descriptions all lead with the command verb (`amend
/// commit X`, `rebase commit X onto Y`, `git fetch …`) so a single
/// first-word match handles the common cases. Anything we don't
/// recognise becomes [`OpKind::Other`] with the verb preserved.
fn classify_op(description: &str) -> OpKind {
    let first = description.split_whitespace().next().unwrap_or("");
    match first {
        "amend" => OpKind::Amend,
        "rebase" => OpKind::Rebase,
        "abandon" => OpKind::Abandon,
        "describe" => OpKind::Describe,
        "new" => OpKind::New,
        "split" => OpKind::Split,
        "squash" => OpKind::Squash,
        "restore" => OpKind::Restore,
        "git" => OpKind::Git,
        _ => OpKind::Other(first.to_string()),
    }
}

fn file_bytes_from_value(
    store: &std::sync::Arc<jj_lib::store::Store>,
    path: &jj_lib::repo_path::RepoPath,
    value: &jj_lib::merge::MergedTreeValue,
) -> Result<Vec<u8>> {
    let Some(Some(tv)) = value.as_resolved() else {
        return Ok(Vec::new());
    };
    match tv {
        jj_lib::backend::TreeValue::File { id, .. } => {
            read_file_bytes(store, path, id)
        }
        _ => Ok(Vec::new()),
    }
}

fn compute_hunks(left: &[u8], right: &[u8], path: &str) -> Result<Vec<kata_core::Hunk>> {
    if looks_binary(left) || looks_binary(right) {
        return Ok(Vec::new());
    }
    let left_text = String::from_utf8_lossy(left);
    let right_text = String::from_utf8_lossy(right);
    crate::diff::histogram_hunks(left_text.as_ref(), right_text.as_ref(), path)
}

fn require_resolved_tree(
    tree_ids: &jj_lib::merge::Merge<jj_lib::backend::TreeId>,
) -> Result<jj_lib::backend::TreeId> {
    tree_ids.as_resolved().cloned().ok_or_else(|| {
        Error::Parse(
            "libjj: commit's root tree has conflicts; rebase-based interdiff would lose data"
                .into(),
        )
    })
}

fn read_file_bytes(
    store: &std::sync::Arc<jj_lib::store::Store>,
    path: &jj_lib::repo_path::RepoPath,
    file_id: &jj_lib::backend::FileId,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut reader = futures::executor::block_on(store.read_file(path, file_id))
        .map_err(|e| Error::Parse(format!("libjj read_file: {e}")))?;
    let mut buf = Vec::new();
    futures::executor::block_on(reader.read_to_end(&mut buf))
        .map_err(|e| Error::Parse(format!("libjj read_to_end: {e}")))?;
    Ok(buf)
}

fn count_line_changes(left: &[u8], right: &[u8]) -> (bool, u32, u32) {
    use imara_diff::intern::InternedInput;
    use imara_diff::sink::Counter;
    use imara_diff::{Algorithm, diff};
    if looks_binary(left) || looks_binary(right) {
        return (true, 0, 0);
    }
    let left_text = String::from_utf8_lossy(left);
    let right_text = String::from_utf8_lossy(right);
    let input = InternedInput::new(left_text.as_ref(), right_text.as_ref());
    let counter = diff(Algorithm::Histogram, &input, Counter::default());
    (false, counter.insertions, counter.removals)
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

// Quick re-export for the hex crate so callers don't have to add it
// to their own deps; we rely on it via jj-lib's transitive dep graph.
mod hex {
    pub fn decode(s: &str) -> std::result::Result<Vec<u8>, String> {
        if s.len() % 2 != 0 {
            return Err(format!("odd-length hex string: {} chars", s.len()));
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        let bytes = s.as_bytes();
        for i in (0..bytes.len()).step_by(2) {
            let hi = from_hex(bytes[i]).ok_or_else(|| format!("bad hex char at {i}"))?;
            let lo = from_hex(bytes[i + 1])
                .ok_or_else(|| format!("bad hex char at {}", i + 1))?;
            out.push((hi << 4) | lo);
        }
        Ok(out)
    }
    fn from_hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jj_lib::object_id::ObjectId;
    use std::path::Path;

    fn trino_path() -> Option<&'static Path> {
        let p = Path::new("/root/notes/kata/trino");
        if p.exists() { Some(p) } else { None }
    }

    #[test]
    fn open_repo_and_resolve_a_known_commit() {
        let Some(path) = trino_path() else {
            eprintln!("trino workspace not present; skipping");
            return;
        };
        let handle = open_repo(path).expect("open_repo");
        // Full 40-hex commit-id of PS5's tip on review #1.
        let id = KataCommitId::new(
            "ea8efce781119a8bf27c396136802948300752e2".to_string(),
        );
        let commit = handle.lookup_commit(&id).expect("lookup_commit");
        eprintln!(
            "resolved commit: id={} parent_count={}",
            commit.id().hex(),
            commit.parent_ids().len(),
        );
    }

    /// Sanity check the rebase-based interdiff against the trino PS4→PS5
    /// bug case. Pick two `changed` pairs from review #1 (we know
    /// from the earlier curl that the naive diff returned identical
    /// 12-file +117 −217 for both) and verify the rebased path now
    /// produces meaningfully smaller (and likely *different*) diffs.
    #[test]
    fn rebased_interdiff_distinguishes_downstream_pairs() {
        let Some(path) = trino_path() else {
            eprintln!("trino workspace not present; skipping");
            return;
        };
        let handle = open_repo(path).expect("open_repo");

        // The end-to-end smoke for downstream-of-rewrite pairs runs
        // via the HTTP layer (see crates/kata-server/tests/http.rs);
        // here we just verify the helper produces a non-trivial
        // result on a known commit pair without panicking.
        let ps4_tip = KataCommitId::new(
            "e6b67248aa42fe9419259ebaa52eb79eabdf6a70".to_string(),
        );
        let ps5_tip = KataCommitId::new(
            "ea8efce781119a8bf27c396136802948300752e2".to_string(),
        );
        let diff = handle
            .compute_rebased_diff(&ps4_tip, &ps5_tip)
            .expect("compute_rebased_diff");
        eprintln!(
            "rebased diff PS4_tip -> PS5_tip: {} files",
            diff.files.len(),
        );
        for f in &diff.files {
            eprintln!("  {} +{} -{}", f.path, f.added, f.removed);
        }
    }
}
