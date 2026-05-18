//! Spins up a throwaway jj repo in a tempdir, lets a test populate it, then
//! exercises the [`JjCli`] backend against it.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use kata_core::{ChangeId, CommitId, FileStatus, LineRange, RevSet};
use kata_jj::{AnchorResolution, FileCache, JjBackend, JjCli, build_diff, resolve_anchor};
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

    fn cli(&self) -> JjCli {
        JjCli::new(self.root.clone())
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
    let hunk = &hunks[0];

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
