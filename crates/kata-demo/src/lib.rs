//! Build a self-contained kata demo: a tiny jj workspace with a few
//! curated commits and a SQLite database pre-seeded with reviews,
//! patchsets, draft + published comments, and annotations. The whole
//! thing is reproducible from scratch — no checked-in snapshots —
//! so the demo data evolves with the codebase rather than rotting
//! against schema changes.
//!
//! Reachable via the `kata demo` CLI subcommand, which calls
//! [`seed_demo`] into a tempdir (or a user-supplied `--data` dir)
//! and then starts the regular HTTP server pointed at it. The
//! frontend tour overlay (`?demo=1`) drives the user through the
//! seeded data step-by-step.
//!
//! NOTE: the seeder shells out to `jj` because constructing commits
//! via jj-lib's transaction API is several hundred lines of
//! ceremony for what's a one-shot setup. `jj` is therefore a
//! *demo-time* requirement, not a *runtime* one — `kata serve`
//! never spawns it.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;

use kata_core::{Author, RepoManifest, SCHEMA_VERSION};
use kata_service::{AnnotationInput, CreateReviewParams, DraftCommentInput, ReviewService};
use kata_storage::sqlite::SqliteStorage;
use kata_storage::{Storage, compute_repo_id, jj_repo_canonical_path};

/// Where the seeded artifacts ended up. The caller hands these
/// straight to the regular server-start path.
pub struct SeededDemo {
    /// Root of the jj working copy the demo data lives in.
    pub workspace_path: PathBuf,
    /// Repo slug the workspace was registered under (`demo`).
    pub repo_name: String,
    /// SQLite database path.
    pub db_path: PathBuf,
    /// Default identity for writes via the HTTP / MCP layer.
    pub author: Author,
}

/// Build the demo workspace + database under `root`. Idempotent for
/// a fresh `root`; errors out if the dir already contains a jj or
/// sqlite file (we don't want to clobber unrelated data).
pub async fn seed_demo(root: &Path) -> Result<SeededDemo, Error> {
    std::fs::create_dir_all(root).map_err(Error::io)?;
    let workspace_path = root.join("workspace");
    let db_path = root.join("kata.db");
    if workspace_path.exists() || db_path.exists() {
        return Err(Error::AlreadyExists);
    }
    std::fs::create_dir_all(&workspace_path).map_err(Error::io)?;

    let alice = Author::new("alice@example.com");
    let bob = Author::new("bob@example.com");

    seed_workspace(&workspace_path, &alice, &bob)?;

    // Now stand up storage + service exactly the way `kata serve`
    // does (see kata-server/src/main.rs) and call the same APIs
    // an HTTP / MCP client would. Anything that goes through the
    // service is guaranteed to look identical to a real-user
    // recording — no test-only shortcuts.
    let canonical = jj_repo_canonical_path(&workspace_path)
        .map_err(|e| Error::Setup(format!("canonical path: {e}")))?;
    let repo_id = compute_repo_id(&canonical);
    let storage = Arc::new(
        SqliteStorage::open(&db_path)
            .await
            .map_err(|e| Error::Setup(format!("open sqlite: {e}")))?,
    );
    storage
        .ensure_repo(&RepoManifest {
            schema_version: SCHEMA_VERSION,
            repo_id: repo_id.clone(),
            canonical_path: canonical.to_string_lossy().into_owned(),
        })
        .await
        .map_err(|e| Error::Setup(format!("ensure repo: {e}")))?;
    let jj = Arc::new(
        kata_jj::JjLib::new(workspace_path.clone())
            .map_err(|e| Error::Setup(format!("open JjLib: {e}")))?,
    );
    let mut builder = ReviewService::builder(storage.clone());
    builder
        .add_repo(
            "demo".into(),
            repo_id.clone(),
            canonical.to_string_lossy().into_owned(),
            jj,
        )
        .map_err(|e| Error::Setup(format!("register repo: {e}")))?;
    let service = builder.build();

    seed_reviews(&service, &repo_id, &workspace_path, &alice, &bob).await?;

    Ok(SeededDemo {
        workspace_path,
        repo_name: "demo".into(),
        db_path,
        author: alice,
    })
}

/// Build the jj workspace: a small TypeScript module evolving across
/// 3 commits on a `feature` bookmark. The story is intentionally
/// tiny — a refactor, a follow-up test commit, a bug fix — so every
/// step of the tour can point at recognisable code rather than
/// abstract diff hunks.
fn seed_workspace(
    workspace_path: &Path,
    alice: &Author,
    _bob: &Author,
) -> Result<(), Error> {
    jj_init(workspace_path)?;

    // -- Commit 1 (trunk): the file as it lives on `main` today.
    write_file(
        workspace_path,
        "greeter.ts",
        r#"// A friendly module that says hello.
export function greet(name: string): string {
  return "Hello, " + name + "!";
}
"#,
    )?;
    jj(workspace_path, &["describe", "-m", "Add greeter module"], alice)?;
    // Pin `main` on the trunk commit so the review's `trunk()..feature`
    // revset has a base to resolve against. Without `main` (or an
    // `origin/main`), the libjj `trunk()` alias falls through to
    // empty and review creation fails.
    jj(workspace_path, &["bookmark", "create", "main", "-r", "@"], alice)?;

    // -- Commit 2: split greet() into formatName + printGreeting.
    jj(workspace_path, &["new", "-m", "Split greeter into helpers"], alice)?;
    write_file(
        workspace_path,
        "greeter.ts",
        r#"// A friendly module that says hello.

/** Capitalise the first letter of a name, trimming whitespace. */
export function formatName(raw: string): string {
  const trimmed = raw.trim();
  return trimmed[0].toUpperCase() + trimmed.slice(1);
}

/** Format a greeting line for `name`. */
export function greet(name: string): string {
  return "Hello, " + formatName(name) + "!";
}
"#,
    )?;

    // -- Commit 3: add tests covering the new helpers.
    jj(workspace_path, &["new", "-m", "Add tests for greeter helpers"], alice)?;
    write_file(
        workspace_path,
        "greeter.test.ts",
        r#"import { greet, formatName } from "./greeter";
import { describe, expect, test } from "vitest";

describe("formatName", () => {
  test("capitalises the first letter", () => {
    expect(formatName("alice")).toBe("Alice");
  });
  test("trims leading whitespace", () => {
    expect(formatName("  bob")).toBe("Bob");
  });
});

describe("greet", () => {
  test("formats a greeting", () => {
    expect(greet("carol")).toBe("Hello, Carol!");
  });
});
"#,
    )?;

    // Pin the bookmark on the last commit so kata's review-create
    // path has something to anchor on.
    jj(workspace_path, &["bookmark", "create", "feature", "-r", "@"], alice)?;
    Ok(())
}

/// Create reviews + sessions + draft / published comments via the
/// real service APIs. Every entry here exercises a code path that
/// the tour will showcase, so adding a tour step that doesn't
/// have demo data is the signal to extend this function.
async fn seed_reviews(
    service: &ReviewService,
    repo_id: &kata_core::RepoId,
    workspace_path: &Path,
    alice: &Author,
    bob: &Author,
) -> Result<(), Error> {
    let manifest = service
        .create_review(
            repo_id,
            CreateReviewParams {
                name: "greeter".into(),
                revset: kata_core::RevSet::new("trunk()..feature"),
                bookmark: Some("feature".into()),
                created_by: alice.clone(),
                summary: Some(
                    "Refactor the greeter into composable helpers and \
                     cover them with tests. Open for review."
                        .into(),
                ),
            },
        )
        .await
        .map_err(|e| Error::Setup(format!("create_review: {e}")))?;
    let review_id = manifest.review_id.clone();

    // The PS1 endpoints are what we anchor demo comments against.
    let ps1 = manifest
        .patchsets
        .iter()
        .find(|p| p.n == manifest.current_patchset)
        .ok_or_else(|| Error::Setup("missing PS1".into()))?
        .clone();

    // Bob (reviewer) opens a session and leaves a couple of
    // comments. One is a must-do on a specific line in the test
    // file; the other is a question on the helper signature.
    let bob_session = service
        .start_session(repo_id, &review_id, bob)
        .await
        .map_err(|e| Error::Setup(format!("start_session bob: {e}")))?;

    service
        .upsert_draft_comment(
            repo_id,
            &review_id,
            &bob_session.session_id,
            bob,
            None,
            DraftCommentInput {
                anchor_change_id: ps1.tip_change.clone(),
                anchor_commit_id: ps1.tip_commit.clone(),
                file: Some("greeter.ts".into()),
                side: Some(kata_core::Side::Tip),
                lines: Some(kata_core::LineRange::new(5, 7)),
                columns: None,
                review_wide: false,
                flag: kata_core::Flag::Question,
                body: "Should `formatName` handle the empty-string case? \
                       `trimmed[0]` will throw on `\"\"`."
                    .into(),
            },
        )
        .await
        .map_err(|e| Error::Setup(format!("draft comment 1: {e}")))?;

    service
        .upsert_draft_comment(
            repo_id,
            &review_id,
            &bob_session.session_id,
            bob,
            None,
            DraftCommentInput {
                anchor_change_id: ps1.tip_change.clone(),
                anchor_commit_id: ps1.tip_commit.clone(),
                file: Some("greeter.test.ts".into()),
                side: Some(kata_core::Side::Tip),
                lines: Some(kata_core::LineRange::new(11, 13)),
                columns: None,
                review_wide: false,
                flag: kata_core::Flag::Suggestion,
                body: "Worth a test for the trim + capitalise interaction \
                       (`\"  carol\"` → `\"Carol\"`)."
                    .into(),
            },
        )
        .await
        .map_err(|e| Error::Setup(format!("draft comment 2: {e}")))?;

    service
        .publish_session(repo_id, &review_id, &bob_session.session_id)
        .await
        .map_err(|e| Error::Setup(format!("publish session: {e}")))?;

    // Alice (the author) drops a design note next to `formatName`
    // explaining why it lives as its own export. Annotations are
    // review-creator-only context — they show up alongside comments
    // but in the amber palette so a reader can tell at a glance
    // "this is from the author, not a reviewer".
    //
    // Anchored on the same line range as Bob's question so the
    // anchor's "group" has 2+ items — that's what triggers the
    // per-thread fold chevron the tour points at later.
    service
        .upsert_annotation(
            repo_id,
            &review_id,
            alice,
            None,
            AnnotationInput {
                anchor_change_id: ps1.tip_change.clone(),
                anchor_commit_id: ps1.tip_commit.clone(),
                file: Some("greeter.ts".into()),
                side: Some(kata_core::Side::Tip),
                lines: Some(kata_core::LineRange::new(5, 7)),
                body: "Note: kept `formatName` as its own export so we \
                       can reuse it for `Person.displayName` in a \
                       follow-up PR — that's why it isn't inlined into \
                       `greet`."
                    .into(),
            },
        )
        .await
        .map_err(|e| Error::Setup(format!("annotation: {e}")))?;

    // PS2: Alice addresses Bob's empty-string question with a guard
    // commit, then moves `feature` to the new tip. `refresh_review`
    // notices the bookmark moved and records the second patchset on
    // the same review — exactly the workflow a real contributor
    // follows after a review round.
    add_patchset(workspace_path, alice)?;
    service
        .refresh_review(repo_id, &review_id, alice, None)
        .await
        .map_err(|e| Error::Setup(format!("refresh_review (PS2): {e}")))?;

    Ok(())
}

/// Append a follow-up commit that guards `formatName` against the
/// empty-string case Bob flagged, then advances the `feature`
/// bookmark to it. Run from `seed_reviews` after PS1's comments are
/// published so the workspace mutation is visible in the demo as
/// "Alice answered the question in a new patchset".
fn add_patchset(workspace_path: &Path, alice: &Author) -> Result<(), Error> {
    jj(
        workspace_path,
        &["new", "-m", "Guard formatName against empty input"],
        alice,
    )?;
    write_file(
        workspace_path,
        "greeter.ts",
        r#"// A friendly module that says hello.

/** Capitalise the first letter of a name, trimming whitespace. */
export function formatName(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return "";
  return trimmed[0].toUpperCase() + trimmed.slice(1);
}

/** Format a greeting line for `name`. */
export function greet(name: string): string {
  return "Hello, " + formatName(name) + "!";
}
"#,
    )?;
    jj(workspace_path, &["bookmark", "set", "feature", "-r", "@"], alice)?;
    Ok(())
}

// ---- jj invocation helpers ----------------------------------------------

fn jj_init(workspace_path: &Path) -> Result<(), Error> {
    let status = StdCommand::new("jj")
        .args(["git", "init", "."])
        .current_dir(workspace_path)
        .status()
        .map_err(|e| Error::Jj(format!("jj git init: {e}")))?;
    if !status.success() {
        return Err(Error::Jj(format!(
            "jj git init exited with status {status}"
        )));
    }
    Ok(())
}

fn jj(workspace_path: &Path, args: &[&str], author: &Author) -> Result<(), Error> {
    let status = StdCommand::new("jj")
        .args(args)
        .current_dir(workspace_path)
        .env("JJ_USER", "Alice")
        .env("JJ_EMAIL", author.as_str())
        .status()
        .map_err(|e| Error::Jj(format!("jj {args:?}: {e}")))?;
    if !status.success() {
        return Err(Error::Jj(format!(
            "jj {args:?} exited with status {status}"
        )));
    }
    Ok(())
}

fn write_file(workspace_path: &Path, rel: &str, contents: &str) -> Result<(), Error> {
    let p = workspace_path.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(Error::io)?;
    }
    std::fs::write(&p, contents).map_err(Error::io)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("demo dir already populated")]
    AlreadyExists,
    #[error("io: {0}")]
    Io(std::io::Error),
    #[error("jj invocation failed: {0}")]
    Jj(String),
    #[error("demo setup: {0}")]
    Setup(String),
}

impl Error {
    fn io(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
