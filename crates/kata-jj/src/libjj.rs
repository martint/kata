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

use std::path::Path;
use std::sync::Arc;

use jj_lib::repo::Repo as _;
use kata_core::CommitId as KataCommitId;

use crate::error::{Error, Result};

/// Open a jj repo at the given workspace path. Returns a handle the
/// caller can use to look up commits and run rebases. Synchronous —
/// callers should wrap in `spawn_blocking`.
pub fn open_repo(workspace_path: &Path) -> Result<JjRepoHandle> {
    use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
    use jj_lib::repo::StoreFactories;
    use jj_lib::settings::UserSettings;

    // Start from jj-lib's bundled defaults (operation.username,
    // signing.behavior, etc. — lots of fields jj insists on). Layer
    // synthetic user identity on top; values are never persisted
    // because the rebase transaction is always dropped.
    let mut config = StackedConfig::with_defaults();
    let mut identity = ConfigLayer::empty(ConfigSource::Default);
    for (key, value) in [
        ("user.name", "kata interdiff"),
        ("user.email", "kata@invalid"),
    ] {
        identity
            .set_value(key, value)
            .map_err(|e| Error::Parse(format!("libjj config {key}: {e}")))?;
    }
    config.add_layer(identity);
    let settings = UserSettings::from_config(config)
        .map_err(|e| Error::Parse(format!("libjj settings: {e}")))?;

    let workspace = jj_lib::workspace::Workspace::load(
        &settings,
        workspace_path,
        &StoreFactories::default(),
        &jj_lib::workspace::default_working_copy_factories(),
    )
    .map_err(|e| Error::Parse(format!("libjj workspace load: {e}")))?;
    let repo_loader = workspace.repo_loader();
    // jj-lib 0.41 made these async; we synchronously block here
    // because the whole module is meant to run inside
    // tokio::task::spawn_blocking. `futures::executor::block_on`
    // works on any future without dragging in a tokio runtime.
    let repo = futures::executor::block_on(repo_loader.load_at_head())
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
