//! Spins up a throwaway jj repo in a tempdir, lets a test populate it, then
//! exercises the [`JjLib`] backend against it. The fixture's setup still
//! shells out to `jj` (init / describe / new / bookmark) — building those
//! states through jj-lib's transaction API is more ceremony than it's
//! worth for tests. The backend-under-test is in-process libjj.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use kata_core::{ChangeId, CommitId, FileStatus, LineRange, RevSet};
use kata_jj::{AnchorResolution, FileCache, JjBackend, JjLib, build_diff, resolve_anchor};
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        run_jj(&root, &["git", "init", "."]);
        Self { _dir: dir, root }
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, contents).expect("write");
    }

    fn remove(&self, rel: &str) {
        std::fs::remove_file(self.root.join(rel)).expect("remove");
    }

    fn rename(&self, from: &str, to: &str) {
        std::fs::rename(self.root.join(from), self.root.join(to)).expect("rename");
    }

    fn jj(&self, args: &[&str]) {
        run_jj(&self.root, args);
    }

    fn cli(&self) -> JjLib {
        JjLib::new(self.root.clone()).expect("open JjLib")
    }
}

fn run_jj(cwd: &Path, args: &[&str]) {
    let status = StdCommand::new("jj")
        .args(args)
        .current_dir(cwd)
        .env("JJ_USER", "Tester")
        .env("JJ_EMAIL", "test@example.com")
        .status()
        .unwrap_or_else(|e| panic!("running jj {:?}: {e}", args));
    assert!(status.success(), "jj {:?} failed", args);
}

fn current_change_and_commit(root: &Path, revset: &str) -> (ChangeId, CommitId) {
    let out = StdCommand::new("jj")
        .args(["--repository", root.to_str().unwrap(), "log", "--no-graph",
               "-r", revset, "-T", r#"change_id ++ " " ++ commit_id ++ "\n""#])
        .output()
        .expect("jj log");
    assert!(out.status.success(), "jj log failed: {}",
            String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text.lines().next().expect("non-empty log");
    let mut parts = line.splitn(2, ' ');
    let change = parts.next().unwrap().to_string();
    let commit = parts.next().unwrap().to_string();
    (ChangeId::new(change), CommitId::new(commit))
}

#[tokio::test]
async fn bookmarks_and_range_resolution() {
    let fx = Fixture::new();
    fx.write("a.txt", "hello\nworld\n");
    fx.jj(&["describe", "-m", "initial"]);
    fx.jj(&["new", "-m", "second"]);
    fx.write("a.txt", "hello\nworld\nagain\n");
    fx.jj(&["bookmark", "create", "feature", "-r", "@"]);

    let cli = fx.cli();
    let bookmarks = cli.list_bookmarks().await.unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].name, "feature");

    let range = cli.resolve_range(&RevSet::new("@-..@")).await.unwrap();
    let (tip_change, tip_commit) = current_change_and_commit(&fx.root, "@");
    let (base_change, base_commit) = current_change_and_commit(&fx.root, "@-");
    assert_eq!(range.tip.change_id, tip_change);
    assert_eq!(range.tip.commit_id, tip_commit);
    assert_eq!(range.base.change_id, base_change);
    assert_eq!(range.base.commit_id, base_commit);
}

#[tokio::test]
async fn changed_files_covers_add_modify_delete_rename() {
    let fx = Fixture::new();
    fx.write("keep.txt", "stable\n");
    fx.write("to_delete.txt", "bye\n");
    fx.write("to_modify.txt", "before\n");
    fx.write("to_rename.txt", "moved\n");
    fx.jj(&["describe", "-m", "initial"]);
    fx.jj(&["new", "-m", "edits"]);

    fx.write("added.txt", "fresh\n");
    fx.write("to_modify.txt", "after\n");
    fx.remove("to_delete.txt");
    fx.rename("to_rename.txt", "renamed.txt");

    let cli = fx.cli();
    let (_, base) = current_change_and_commit(&fx.root, "@-");
    let (_, tip) = current_change_and_commit(&fx.root, "@");

    let mut entries = cli.changed_files(&base, &tip).await.unwrap();
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let by_path = |p: &str| entries.iter().find(|e| e.path == p).cloned();

    assert!(matches!(by_path("added.txt").unwrap().status, FileStatus::Added));
    assert!(matches!(by_path("to_delete.txt").unwrap().status, FileStatus::Deleted));
    assert!(matches!(by_path("to_modify.txt").unwrap().status, FileStatus::Modified));
    let renamed = by_path("renamed.txt").unwrap();
    match renamed.status {
        FileStatus::Renamed { old_path } => assert_eq!(old_path, "to_rename.txt"),
        other => panic!("expected rename, got {:?}", other),
    }
}

#[tokio::test]
async fn diff_hunks_have_correct_line_numbers() {
    let fx = Fixture::new();
    fx.write("file.txt", "one\ntwo\nthree\nfour\nfive\n");
    fx.jj(&["describe", "-m", "initial"]);
    fx.jj(&["new", "-m", "edit middle"]);
    fx.write("file.txt", "one\ntwo\nTHREE\nfour\nfive\n");

    let cli = fx.cli();
    let (_, base) = current_change_and_commit(&fx.root, "@-");
    let (_, tip) = current_change_and_commit(&fx.root, "@");

    let diff = build_diff(&cli, &base, &tip).await.unwrap();
    let file = diff.files.iter().find(|f| f.path == "file.txt").unwrap();
    let hunks = file.hunks.as_ref().expect("text file should have hunks");
    assert!(!hunks.is_empty());
    let kata_core::Hunk::Regular(hunk) = &hunks[0] else {
        panic!("expected a regular hunk for a non-conflicted modify")
    };

    let removed: Vec<_> = hunk.lines.iter()
        .filter(|l| matches!(l.origin, kata_core::LineOrigin::Removed))
        .collect();
    let added: Vec<_> = hunk.lines.iter()
        .filter(|l| matches!(l.origin, kata_core::LineOrigin::Added))
        .collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(added.len(), 1);
    assert_eq!(removed[0].base_line, Some(3));
    assert_eq!(added[0].tip_line, Some(3));
    assert_eq!(removed[0].content.trim_end(), "three");
    assert_eq!(added[0].content.trim_end(), "THREE");
}

#[tokio::test]
async fn anchor_valid_when_commit_unchanged() {
    let fx = Fixture::new();
    fx.write("f.txt", "a\nb\nc\n");
    fx.jj(&["describe", "-m", "x"]);

    let cli = fx.cli();
    let (_, commit) = current_change_and_commit(&fx.root, "@");
    let cache = FileCache::default();
    let res = resolve_anchor(&cli, &cache, "f.txt", &commit, LineRange::new(1, 1), &commit).await.unwrap();
    assert_eq!(res, AnchorResolution::Valid);
}

#[tokio::test]
async fn anchor_moves_when_lines_shift() {
    let fx = Fixture::new();
    fx.write("f.txt", "alpha\nbeta\ngamma\n");
    fx.jj(&["describe", "-m", "initial"]);
    let (_, original) = current_change_and_commit(&fx.root, "@");

    // Same change_id, but rewrite the commit by inserting two lines above.
    fx.write("f.txt", "x\ny\nalpha\nbeta\ngamma\n");
    let (_, current) = current_change_and_commit(&fx.root, "@");
    assert_ne!(original, current, "commit id should change after amend");

    let cli = fx.cli();
    let cache = FileCache::default();
    let res = resolve_anchor(&cli, &cache, "f.txt", &original, LineRange::new(2, 2), &current)
        .await.unwrap();
    match res {
        AnchorResolution::Moved { new_range } => {
            assert_eq!(new_range, LineRange::new(4, 4));
        }
        other => panic!("expected Moved, got {other:?}"),
    }
}

#[tokio::test]
async fn anchor_valid_when_lines_unchanged_across_commits() {
    // Regression: a comment posted in a per-commit scoped view
    // anchors to the scoped commit, not the patchset tip. When the
    // backend builds the comment view it compares the anchor against
    // the patchset tip — a different commit id, so the fast path
    // doesn't fire. The original code then ran find_exact and, on
    // an unchanged file, returned Moved { new_range: original_range }
    // — a "moved to <same lines>" badge plastered on a comment that
    // hadn't moved at all. The same-range guard in resolve_anchor
    // turns that case back into Valid.
    let fx = Fixture::new();
    fx.write("f.txt", "alpha\nbeta\ngamma\n");
    fx.jj(&["describe", "-m", "initial"]);
    let (_, original) = current_change_and_commit(&fx.root, "@");

    // New commit, but the file (and the line of interest) is
    // identical — like a downstream commit in the same patchset
    // that doesn't touch this file.
    fx.jj(&["new", "-m", "downstream"]);
    fx.write("other.txt", "unrelated\n");
    let (_, current) = current_change_and_commit(&fx.root, "@");
    assert_ne!(original, current, "commit id should differ between revs");

    let cli = fx.cli();
    let cache = FileCache::default();
    let res = resolve_anchor(&cli, &cache, "f.txt", &original, LineRange::new(2, 2), &current)
        .await
        .unwrap();
    assert_eq!(res, AnchorResolution::Valid);
}

#[tokio::test]
async fn anchor_outdated_when_content_gone() {
    let fx = Fixture::new();
    fx.write("f.txt", "needle\n");
    fx.jj(&["describe", "-m", "initial"]);
    let (_, original) = current_change_and_commit(&fx.root, "@");

    fx.write("f.txt", "completely different content here\nand more lines\nstill unrelated\n");
    let (_, current) = current_change_and_commit(&fx.root, "@");

    let cli = fx.cli();
    let cache = FileCache::default();
    let res = resolve_anchor(&cli, &cache, "f.txt", &original, LineRange::new(1, 1), &current)
        .await.unwrap();
    match res {
        AnchorResolution::Outdated { original_content } => {
            assert!(original_content.contains("needle"));
        }
        AnchorResolution::Drifted { .. } => {} // acceptable if fuzzy threshold is lenient
        other => panic!("expected Outdated (or Drifted), got {other:?}"),
    }
}

/// Build a merge commit whose tree is conflicted, and verify that
/// (a) `list_commits` surfaces the conflicted path on the commit's
/// `conflict_paths`, and (b) `build_diff` emits a `Hunk::Conflict`
/// with one side per parent rather than running the file through
/// the regular histogram path.
#[tokio::test]
async fn merge_commit_with_conflict_emits_conflict_hunk_and_path() {
    let fx = Fixture::new();
    // Common ancestor.
    fx.write("conflicted.txt", "shared baseline\n");
    fx.jj(&["describe", "-m", "base"]);
    fx.jj(&["bookmark", "create", "base-mark", "-r", "@"]);

    // Side A: edits the same line one way.
    fx.jj(&["new", "-m", "side a"]);
    fx.write("conflicted.txt", "side A's version\n");
    fx.jj(&["bookmark", "create", "side-a", "-r", "@"]);
    let (_, a_commit) = current_change_and_commit(&fx.root, "@");

    // Side B: edits the same line another way, starting from the
    // shared base.
    fx.jj(&["new", "base-mark", "-m", "side b"]);
    fx.write("conflicted.txt", "side B's version\n");
    fx.jj(&["bookmark", "create", "side-b", "-r", "@"]);
    let (_, b_commit) = current_change_and_commit(&fx.root, "@");

    // Merge — `jj` will accept the merge but keep the file as a
    // conflicted tree value because the two sides edited the same
    // line in incompatible ways.
    fx.jj(&["new", &a_commit.to_string(), &b_commit.to_string(), "-m", "merge"]);
    let (_, merge_commit) = current_change_and_commit(&fx.root, "@");

    let cli = fx.cli();

    // (a) The merge commit's metadata should list `conflicted.txt`
    //     under `conflict_paths`. We resolve the merge via revset to
    //     match how the UI would.
    let revset = kata_core::RevSet::new(format!("{merge_commit}"));
    let commits = cli.list_commits(&revset).await.unwrap();
    let merge_meta = commits
        .iter()
        .find(|c| c.commit_id == merge_commit)
        .expect("merge commit should be in list_commits output");
    assert!(
        merge_meta.conflict_paths.iter().any(|p| p == "conflicted.txt"),
        "expected conflict_paths to include conflicted.txt, got {:?}",
        merge_meta.conflict_paths,
    );

    // (b) The diff against side A should render the conflict as a
    //     Conflict hunk with one side per parent — not as a regular
    //     histogram diff that flattens the conflict.
    let diff = build_diff(&cli, &a_commit, &merge_commit).await.unwrap();
    let file = diff
        .files
        .iter()
        .find(|f| f.path == "conflicted.txt")
        .expect("conflicted.txt should be in the merge diff");
    let hunks = file.hunks.as_ref().expect("conflict file should still ship hunks");
    let kata_core::Hunk::Conflict(conflict) = &hunks[0] else {
        panic!("expected a Conflict hunk, got {:?}", hunks[0])
    };
    // Removes (the merge bases) + adds (the parents) — at minimum
    // we expect 1 base + 2 sides, so 3 entries total.
    assert!(
        conflict.sides.len() >= 3,
        "expected at least 3 conflict sides (1 base + 2 parents), got {}",
        conflict.sides.len(),
    );
    // Side labels: removes get "Base"; adds get either parent
    // descriptions (when the merge structure matches the parent
    // count) or generic "Side N". Just check that labels are
    // non-empty and distinct enough for the renderer.
    for side in &conflict.sides {
        assert!(!side.label.is_empty(), "side label should not be empty");
    }
}

/// `list_commits` must return its result oldest-first. The trait
/// documents this and the UI's commits panel relies on it — a
/// regression to jj's native newest-first iteration order is what
/// the user surfaced as "the commits list is backwards now".
#[tokio::test]
async fn list_commits_returns_oldest_first() {
    let fx = Fixture::new();
    // Build a four-step chain: trunk → A → B → C (each edits the
    // same file so every commit has a distinct, recognisable
    // description). Use `jj describe` + `jj new` so the change-ids
    // are stable and we can match by description.
    // Each `jj new` opens a fresh change so its description sticks;
    // doing `describe` before `new` would rewrite the still-empty
    // working copy. Bookmark `trunk` AFTER the trunk commit is sealed
    // (i.e. a child change exists) so the bookmark pins it.
    fx.write("a.txt", "trunk\n");
    fx.jj(&["describe", "-m", "trunk"]);
    fx.jj(&["new", "-m", "A"]);
    fx.jj(&["bookmark", "create", "trunk-mark", "-r", "@-"]);
    fx.write("a.txt", "trunk\na\n");

    fx.jj(&["new", "-m", "B"]);
    fx.write("a.txt", "trunk\na\nb\n");

    fx.jj(&["new", "-m", "C"]);
    fx.write("a.txt", "trunk\na\nb\nc\n");

    // The revset covers everything reachable from C but not from
    // trunk-mark — A, B, C in chronological order.
    let cli = fx.cli();
    let revset = kata_core::RevSet::new("trunk-mark..@");
    let commits = cli.list_commits(&revset).await.unwrap();
    let descs: Vec<&str> = commits
        .iter()
        .map(|c| c.description_first_line.as_str())
        .collect();
    assert_eq!(
        descs,
        ["A", "B", "C"],
        "expected list_commits to return commits oldest-first; got {descs:?}",
    );
}
