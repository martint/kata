//! Spins up a throwaway jj repo in a tempdir, lets a test populate it, then
//! exercises the [`JjLib`] backend against it. The fixture's setup still
//! shells out to `jj` (init / describe / new / bookmark) — building those
//! states through jj-lib's transaction API is more ceremony than it's
//! worth for tests. The backend-under-test is in-process libjj.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use kata_core::{ChangeId, CommitId, FileStatus, LineRange, ReviewId, RevSet};
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
async fn resolve_range_handles_branches_off_different_trunk_commits() {
    // Variation of the "aggregating branches" case where each side
    // branch is rooted at a *different* trunk commit (real PR stacks
    // often look like this — each PR branches off whatever main was
    // at the time). The revset's roots have several distinct parents
    // outside the set; resolve_range must pick the right one (the
    // shared ancestor) instead of failing as "multiple heads".
    let fx = Fixture::new();
    fx.write("seed.txt", "seed\n");
    fx.jj(&["describe", "-m", "trunk 1"]);
    // Branch a starts here.
    fx.jj(&["bookmark", "create", "branch-a", "-r", "@"]);
    // Trunk moves on.
    fx.jj(&["new", "-m", "trunk 2"]);
    // Branch b starts here.
    fx.jj(&["bookmark", "create", "branch-b", "-r", "@"]);
    // Trunk moves on again — this is main.
    fx.jj(&["new", "-m", "trunk 3"]);
    fx.jj(&["bookmark", "create", "main", "-r", "@"]);
    // Side commit on branch-a (off trunk 1).
    fx.jj(&["new", "branch-a", "-m", "a edit"]);
    fx.jj(&["bookmark", "set", "branch-a", "-r", "@"]);
    // Side commit on branch-b (off trunk 2).
    fx.jj(&["new", "branch-b", "-m", "b edit"]);
    fx.jj(&["bookmark", "set", "branch-b", "-r", "@"]);
    // Merge them on top of main.
    fx.jj(&["new", "main", "branch-a", "branch-b", "-m", "merge"]);
    fx.jj(&["bookmark", "create", "stack-tip", "-r", "@"]);

    let cli = fx.cli();
    let range = cli
        .resolve_range(&RevSet::new("trunk()..stack-tip"))
        .await
        .expect("resolve_range");
    let (_, tip_commit) = current_change_and_commit(&fx.root, "stack-tip");
    assert_eq!(range.tip.commit_id, tip_commit);
    let (_, trunk_commit) = current_change_and_commit(&fx.root, "main");
    assert_eq!(range.base.commit_id, trunk_commit);
}

#[tokio::test]
async fn resolve_range_handles_a_revset_aggregating_multiple_branches() {
    // User-reported bug: a revset like `trunk()..stack-tip` where
    // `stack-tip` is reachable via a merge commit that aggregates
    // several side branches. Each side branch carries its own
    // bookmark, so the revset spans several bookmark refs — but the
    // set's graph-theoretic `heads()` is still a single commit (the
    // merge / its descendant). `resolve_range` must not reject this
    // as "multiple heads".
    let fx = Fixture::new();
    fx.write("seed.txt", "seed\n");
    fx.jj(&["describe", "-m", "trunk commit"]);
    fx.jj(&["bookmark", "create", "main", "-r", "@"]);
    fx.jj(&["new", "main", "-m", "branch a"]);
    fx.jj(&["bookmark", "create", "branch-a", "-r", "@"]);
    fx.jj(&["new", "main", "-m", "branch b"]);
    fx.jj(&["bookmark", "create", "branch-b", "-r", "@"]);
    fx.jj(&["new", "main", "-m", "branch c"]);
    fx.jj(&["bookmark", "create", "branch-c", "-r", "@"]);
    fx.jj(&["new", "branch-a", "branch-b", "branch-c", "-m", "merge"]);
    fx.jj(&["bookmark", "create", "stack-tip", "-r", "@"]);

    let cli = fx.cli();
    let range = cli
        .resolve_range(&RevSet::new("trunk()..stack-tip"))
        .await
        .expect("resolve_range");
    let (tip_change, tip_commit) = current_change_and_commit(&fx.root, "stack-tip");
    assert_eq!(range.tip.commit_id, tip_commit);
    assert_eq!(range.tip.change_id, tip_change);
    let (base_change, base_commit) = current_change_and_commit(&fx.root, "main");
    assert_eq!(range.base.commit_id, base_commit);
    assert_eq!(range.base.change_id, base_change);
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
    //     Conflict hunk with one term per merge ancestor / side —
    //     not as a regular histogram diff that flattens the conflict.
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
        conflict.terms.len() >= 3,
        "expected at least 3 conflict terms (1 base + 2 parents), got {}",
        conflict.terms.len(),
    );
    // Labels: bases get "Base" (or "Base N"); adds get either parent
    // descriptions (when the merge structure matches the parent
    // count) or generic "Side N". Just check that labels are
    // non-empty and distinct enough for the renderer.
    for term in &conflict.terms {
        assert!(!term.label.is_empty(), "term label should not be empty");
    }
    // The first term should be a Base, the rest at least include
    // some Sides — verify the kind classification.
    let base_count = conflict
        .terms
        .iter()
        .filter(|t| t.kind == kata_core::ConflictTermKind::Base)
        .count();
    let side_count = conflict
        .terms
        .iter()
        .filter(|t| t.kind == kata_core::ConflictTermKind::Side)
        .count();
    assert!(base_count >= 1, "expected at least 1 Base term, got {base_count}");
    assert_eq!(
        side_count, 2,
        "expected exactly 2 Side terms for a 2-parent merge, got {side_count}",
    );
    // Sides carry per-line diffs against the first base — both sides
    // edited the same line `B`, so each Side term should contain
    // at least one Added line (their own version of the conflict
    // marker line) and at least one Removed line (the base's `B`).
    for term in &conflict.terms {
        if term.kind != kata_core::ConflictTermKind::Side {
            continue;
        }
        let added = term
            .lines
            .iter()
            .filter(|l| l.origin == kata_core::LineOrigin::Added)
            .count();
        let removed = term
            .lines
            .iter()
            .filter(|l| l.origin == kata_core::LineOrigin::Removed)
            .count();
        assert!(
            added >= 1,
            "side {:?} should have at least one Added line vs the base; got {} added, {} removed",
            term.label,
            added,
            removed,
        );
        assert!(
            removed >= 1,
            "side {:?} should have at least one Removed line vs the base; got {} added, {} removed",
            term.label,
            added,
            removed,
        );
    }
    // Base terms should be entirely Context (no Added / Removed) —
    // they have nothing to diff against themselves.
    let base = conflict
        .terms
        .iter()
        .find(|t| t.kind == kata_core::ConflictTermKind::Base)
        .expect("at least one Base term");
    assert!(
        base.lines.iter().all(|l| l.origin == kata_core::LineOrigin::Context),
        "Base term lines should all be Context, got {:?}",
        base.lines.iter().map(|l| l.origin).collect::<Vec<_>>(),
    );
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

/// `browse_log` produces a [`LogPage`] with one row per commit in
/// the revset, laid out into column-stem coordinates. We exercise
/// the simplest possible case — a linear chain — and assert the
/// graph row count + coordinate basics. The column-stem algorithm
/// itself has unit coverage in `log_graph::tests` against
/// synthetic input; this test is the integration probe that the
/// jj-lib stream feed wires into the algorithm correctly.
#[tokio::test]
async fn browse_log_lays_out_a_linear_chain() {
    let fx = Fixture::new();
    fx.write("a.txt", "trunk\n");
    fx.jj(&["describe", "-m", "trunk"]);
    fx.jj(&["new", "-m", "A"]);
    fx.jj(&["bookmark", "create", "trunk-mark", "-r", "@-"]);
    fx.write("a.txt", "trunk\na\n");
    fx.jj(&["new", "-m", "B"]);
    fx.write("a.txt", "trunk\na\nb\n");
    fx.jj(&["new", "-m", "C"]);
    fx.write("a.txt", "trunk\na\nb\nc\n");

    let cli = fx.cli();
    let revset = kata_core::RevSet::new("trunk-mark..@");
    let page = cli.browse_log(&revset, 16).await.unwrap();

    assert_eq!(page.rows.len(), 3, "expected three rows for A, B, C");
    assert!(!page.has_more);
    // All three rows sit in column 0 in a linear chain.
    for row in &page.rows {
        assert_eq!(row.location.col, 0, "linear chain stays in column 0");
    }
    // jj's iter_graph emits commits children-before-parents, so
    // the topo grouping yields C first, then B, then A.
    let descs: Vec<&str> = page
        .rows
        .iter()
        .map(|r| r.commit.description_first_line.as_str())
        .collect();
    assert_eq!(descs, ["C", "B", "A"], "topo order children-before-parents");
}

/// `working_copy_commit_id` returns the workspace's `@` commit.
#[tokio::test]
async fn working_copy_commit_id_returns_at() {
    let fx = Fixture::new();
    fx.write("a.txt", "hello\n");
    fx.jj(&["describe", "-m", "first"]);
    fx.jj(&["new", "-m", "second"]);

    let cli = fx.cli();
    let wc = cli.working_copy_commit_id().await.unwrap();
    assert!(wc.is_some(), "must report the working-copy commit");
    // The @ commit's description is "second" (the current change).
    let revset = kata_core::RevSet::new("@");
    let at_commits = cli.list_commits(&revset).await.unwrap();
    assert_eq!(at_commits.len(), 1);
    assert_eq!(wc.unwrap(), at_commits[0].commit_id);
}

/// A merge commit's per-commit diff shows only what the merge itself
/// introduces — not content it inherits from the branches it merges.
/// Regression: the scoped per-commit view diffed a merge against a
/// single (arbitrary) parent, so it surfaced the *other* branch's files
/// as if the merge authored them, and disagreed with the commits-panel
/// `changed_files` count.
#[tokio::test]
async fn commit_self_diff_excludes_content_inherited_from_merged_branches() {
    let fx = Fixture::new();
    fx.write("base.txt", "base\n");
    fx.jj(&["describe", "-m", "trunk"]);
    fx.jj(&["bookmark", "create", "main", "-r", "@"]);

    // Branch A adds a.txt.
    fx.jj(&["new", "main", "-m", "branch a"]);
    fx.write("a.txt", "aaa\n");
    fx.jj(&["bookmark", "create", "branch-a", "-r", "@"]);

    // Branch B adds b.txt.
    fx.jj(&["new", "main", "-m", "branch b"]);
    fx.write("b.txt", "bbb\n");
    fx.jj(&["bookmark", "create", "branch-b", "-r", "@"]);

    // Merge A and B, and add the merge's OWN file m.txt.
    fx.jj(&["new", "branch-a", "branch-b", "-m", "merge"]);
    fx.write("m.txt", "mmm\n");
    fx.jj(&["bookmark", "create", "merge", "-r", "@"]);

    let (_, merge_commit) = current_change_and_commit(&fx.root, "merge");

    // Per-commit scoped diff (commit_diff source): only m.txt — the
    // inherited a.txt / b.txt belong to the branches, not the merge.
    let handle = kata_jj::libjj::open_repo(&fx.root).expect("open_repo");
    let sd = handle
        .compute_commit_self_diff(&merge_commit)
        .expect("compute_commit_self_diff");
    let mut paths: Vec<&str> = sd.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec!["m.txt"],
        "merge self-diff must be only its own file, got {paths:?}"
    );
    assert_eq!(sd.tip.commit_id, merge_commit);
    // The lone file ships full hunks (scoped view renders them inline).
    assert!(sd.files[0].hunks.is_some());

    // The commits-panel `changed_files` must agree with the scoped diff.
    let cli = fx.cli();
    let commits = cli
        .list_commits(&RevSet::new("main..merge"))
        .await
        .expect("list_commits");
    let m = commits
        .iter()
        .find(|c| c.commit_id == merge_commit)
        .expect("merge commit in list");
    let mut cf: Vec<&str> = m.changed_files.iter().map(|f| f.path.as_str()).collect();
    cf.sort();
    assert_eq!(
        cf,
        vec!["m.txt"],
        "merge changed_files must match the scoped diff, got {cf:?}"
    );
}

/// A clean merge — one that only stitches its branches together with no
/// edits of its own — has an empty per-commit diff.
#[tokio::test]
async fn commit_self_diff_is_empty_for_a_clean_merge() {
    let fx = Fixture::new();
    fx.write("base.txt", "base\n");
    fx.jj(&["describe", "-m", "trunk"]);
    fx.jj(&["bookmark", "create", "main", "-r", "@"]);
    fx.jj(&["new", "main", "-m", "branch a"]);
    fx.write("a.txt", "aaa\n");
    fx.jj(&["bookmark", "create", "branch-a", "-r", "@"]);
    fx.jj(&["new", "main", "-m", "branch b"]);
    fx.write("b.txt", "bbb\n");
    fx.jj(&["bookmark", "create", "branch-b", "-r", "@"]);
    // Pure merge: no own content.
    fx.jj(&["new", "branch-a", "branch-b", "-m", "merge"]);
    fx.jj(&["bookmark", "create", "merge", "-r", "@"]);

    let (_, merge_commit) = current_change_and_commit(&fx.root, "merge");
    let handle = kata_jj::libjj::open_repo(&fx.root).expect("open_repo");
    let sd = handle
        .compute_commit_self_diff(&merge_commit)
        .expect("compute_commit_self_diff");
    assert!(
        sd.files.is_empty(),
        "a clean merge introduces nothing of its own, got {:?}",
        sd.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

/// Build two commits off `main`, then `jj abandon` both so they're
/// hidden from jj's view — no bookmark, not `@`, not a visible head.
/// Returns `(c_commit, d_commit)`. Abandoned commits are exactly what
/// `jj util gc --expire=now` collects (it prunes everything not kept by
/// a ref); a kata pin ref is then the only thing that can save one, so
/// this is the lever that distinguishes "pinned" from "collectable".
fn two_orphans(fx: &Fixture) -> (CommitId, CommitId) {
    fx.write("seed.txt", "seed\n");
    fx.jj(&["describe", "-m", "base"]);
    fx.jj(&["bookmark", "create", "main", "-r", "@"]);
    fx.jj(&["new", "main", "-m", "cfeat"]);
    fx.write("cfeat.txt", "C\n");
    let (_, c_commit) = current_change_and_commit(&fx.root, "@");
    fx.jj(&["new", "main", "-m", "dfeat"]);
    fx.write("dfeat.txt", "D\n");
    let (_, d_commit) = current_change_and_commit(&fx.root, "@");
    // Move the working copy off D, then hide both commits so neither is
    // a visible head jj's gc would keep on its own.
    fx.jj(&["new", "main", "-m", "scratch"]);
    fx.jj(&["abandon", c_commit.as_str()]);
    fx.jj(&["abandon", d_commit.as_str()]);
    (c_commit, d_commit)
}

/// Collect everything not kept by a ref, right now. jj keeps commits
/// referenced by any non-expired operation, so we first drop the
/// operation history (`jj op abandon ..@-`) — otherwise the orphans
/// stay reachable through old views — then run gc with an immediate
/// expiry. This is the documented "garbage-collect old operations and
/// their commits" recipe from `jj util gc --help`.
fn gc_now(fx: &Fixture) {
    fx.jj(&["op", "abandon", "..@-"]);
    fx.jj(&["util", "gc", "--expire=now"]);
}

#[tokio::test]
async fn pinned_commit_survives_gc() {
    // The core retention guarantee: a commit a review pins must survive
    // `jj util gc`, even when it's otherwise unreachable. The unpinned
    // sibling is the control — it confirms the gc actually collects
    // orphans, so the pinned one surviving is the pin's doing.
    let fx = Fixture::new();
    let (c_commit, d_commit) = two_orphans(&fx);

    let review = ReviewId::new("review-pin-test");
    fx.cli()
        .pin_commits(&review, std::slice::from_ref(&c_commit))
        .await
        .expect("pin_commits");

    gc_now(&fx);

    let cli = fx.cli();
    let kept = cli
        .read_file(&c_commit, "cfeat.txt")
        .await
        .expect("read pinned commit");
    assert_eq!(
        kept.as_deref(),
        Some(&b"C\n"[..]),
        "pinned commit's content should still be readable after gc",
    );
    let gone = cli.read_file(&d_commit, "dfeat.txt").await;
    assert!(
        gone.is_err(),
        "unpinned orphan should have been collected by gc, but read succeeded: {gone:?}",
    );
}

#[tokio::test]
async fn unpinned_review_commit_becomes_collectable() {
    // Deleting a review drops its pins; the commit it kept alive should
    // then be collectable again. Pin, unpin, gc — and the commit is
    // gone, proving `unpin_review` actually removed the ref.
    let fx = Fixture::new();
    let (c_commit, _d_commit) = two_orphans(&fx);

    let review = ReviewId::new("review-unpin-test");
    let cli = fx.cli();
    cli.pin_commits(&review, std::slice::from_ref(&c_commit))
        .await
        .expect("pin_commits");
    cli.unpin_review(&review).await.expect("unpin_review");

    gc_now(&fx);

    let gone = fx.cli().read_file(&c_commit, "cfeat.txt").await;
    assert!(
        gone.is_err(),
        "after unpin + gc the commit should be collected, but read succeeded: {gone:?}",
    );
}
