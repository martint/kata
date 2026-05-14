//! Round-trip the file-based archive format against `SqliteStorage`.
//!
//! Exports a fully-populated SQLite database (repos, reviews, sessions
//! in every status, comments with every shape, responses with every
//! action) into a temp directory, then imports it back into a fresh
//! database and asserts the two databases agree on every entity.
//! Anything that diverges between the export and import paths shows up
//! as an asymmetric round-trip.

use std::path::Path;

use chrono::Utc;
use kata_core::{
    Author, ChangeId, Comment, CommitId, Flag, LineRange, Patchset, RepoId, RepoManifest,
    ResolutionAction, Response, ReviewId, ReviewManifest, RevSet, SCHEMA_VERSION, Side,
};
use kata_storage::sqlite::SqliteStorage;
use kata_storage::{Storage, archive};
use tempfile::TempDir;

fn fake_repo_id() -> RepoId {
    RepoId::new("a".repeat(64))
}

fn manifest_for(repo: &RepoId) -> RepoManifest {
    RepoManifest {
        schema_version: SCHEMA_VERSION,
        repo_id: repo.clone(),
        canonical_path: "/tmp/example/.jj/repo".into(),
    }
}

fn review_manifest(review: &ReviewId, author: &Author) -> ReviewManifest {
    let now = Utc::now();
    ReviewManifest {
        schema_version: SCHEMA_VERSION,
        review_id: review.clone(),
        // Hard-code a number so the round-trip test can compare
        // before / after dictionaries exactly. (When the storage
        // layer auto-assigns, the imported copy may pick a different
        // number, which is fine in practice but trips the strict
        // equality assertion in this suite.)
        number: 1,
        name: review.as_str().to_owned(),
        revset: RevSet::trunk_to(review.as_str()),
        created_at: now,
        created_by: author.clone(),
        bookmark: Some(review.as_str().to_owned()),
        patchsets: vec![Patchset {
            n: 1,
            base_change: ChangeId::new("basechange"),
            base_commit: CommitId::new("basecommit"),
            tip_change: ChangeId::new("tipchange"),
            tip_commit: CommitId::new("tipcommit"),
            recorded_at: now,
            parent_patchset: None,
        }],
        current_patchset: 1,
        summary: Some("Author-written summary.".into()),
    }
}

/// Build a database holding every entity shape the archive format needs
/// to round-trip: published + discarded + draft sessions, line-level +
/// file-level + review-wide comments, every `Flag` value, every
/// `ResolutionAction`, optional fields on both populated and empty.
async fn seed_database(storage: &SqliteStorage) {
    let repo = fake_repo_id();
    let review = ReviewId::new("feature-xyz");
    let alice = Author::new("alice@example.com");
    let bob = Author::new("bob@example.com");

    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &alice))
        .await
        .unwrap();

    // Published session with two comments and a resolve.
    let pub_session = storage
        .open_or_create_session(&repo, &review, &alice)
        .await
        .unwrap();
    let line_comment = Comment {
        schema_version: SCHEMA_VERSION,
        comment_id: kata_storage::ids::new_comment_id(),
        session_id: pub_session.session_id.clone(),
        review_id: review.clone(),
        author: alice.clone(),
        created_at: Utc::now(),
        patchset: 1,
        anchor_change_id: ChangeId::new("tipchange"),
        anchor_commit_id: CommitId::new("tipcommit"),
        file: Some("src/foo.rs".into()),
        side: Some(Side::Tip),
        lines: Some(LineRange::new(10, 15)),
        flag: Flag::MustDo,
        body: "this needs a doc comment\n".into(),
    };
    let file_level = Comment {
        comment_id: kata_storage::ids::new_comment_id(),
        file: Some("README.md".into()),
        side: None,
        lines: None,
        flag: Flag::Suggestion,
        ..line_comment.clone()
    };
    storage.upsert_draft_comment(&repo, &line_comment).await.unwrap();
    storage.upsert_draft_comment(&repo, &file_level).await.unwrap();
    let resp = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_storage::ids::new_response_id(),
        in_reply_to: line_comment.comment_id.clone(),
        session_id: pub_session.session_id.clone(),
        author: alice.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Resolve,
        // Trailing newline matches the archive's frontmatter encoding —
        // every Markdown body ends with \n on disk, so seeding without
        // one would round-trip asymmetrically and the comparison below
        // would flag it.
        body: "fixed\n".into(),
    };
    storage.upsert_draft_response(&repo, &resp).await.unwrap();
    storage
        .publish_session(&repo, &review, &pub_session.session_id)
        .await
        .unwrap();

    // Discarded session, with a comment that should still round-trip.
    let discarded = storage
        .open_or_create_session(&repo, &review, &bob)
        .await
        .unwrap();
    let drafted = Comment {
        schema_version: SCHEMA_VERSION,
        comment_id: kata_storage::ids::new_comment_id(),
        session_id: discarded.session_id.clone(),
        review_id: review.clone(),
        author: bob.clone(),
        created_at: Utc::now(),
        patchset: 1,
        anchor_change_id: ChangeId::new("tipchange"),
        anchor_commit_id: CommitId::new("tipcommit"),
        file: None,
        side: None,
        lines: None,
        flag: Flag::Other,
        body: "review-wide thought\n".into(),
    };
    storage.upsert_draft_comment(&repo, &drafted).await.unwrap();
    storage
        .discard_session(&repo, &review, &discarded.session_id)
        .await
        .unwrap();

    // Open draft session — preserved across the round-trip too.
    storage
        .open_or_create_session(&repo, &review, &alice)
        .await
        .unwrap();
}

async fn collect_state(storage: &SqliteStorage) -> Snapshot {
    let mut repos = storage.list_all_repos().await.unwrap();
    repos.sort_by(|a, b| a.repo_id.as_str().cmp(b.repo_id.as_str()));
    let mut sessions = Vec::new();
    let mut comments = Vec::new();
    let mut responses = Vec::new();
    let mut reviews = Vec::new();
    for repo in &repos {
        let mut summaries = storage.list_reviews(&repo.repo_id).await.unwrap();
        summaries.sort_by(|a, b| {
            a.manifest.review_id.as_str().cmp(b.manifest.review_id.as_str())
        });
        for s in summaries {
            reviews.push((repo.repo_id.clone(), s.manifest.clone()));
            for session in storage
                .list_all_sessions(&repo.repo_id, &s.manifest.review_id)
                .await
                .unwrap()
            {
                let session_comments = storage
                    .list_all_comments_for_session(&session.session_id)
                    .await
                    .unwrap();
                let session_responses = storage
                    .list_all_responses_for_session(&session.session_id)
                    .await
                    .unwrap();
                sessions.push((repo.repo_id.clone(), session));
                for c in session_comments {
                    comments.push((repo.repo_id.clone(), c));
                }
                for r in session_responses {
                    responses.push((repo.repo_id.clone(), r));
                }
            }
        }
    }
    sessions.sort_by(|a, b| a.1.session_id.as_str().cmp(b.1.session_id.as_str()));
    comments.sort_by(|a, b| a.1.comment_id.as_str().cmp(b.1.comment_id.as_str()));
    responses.sort_by(|a, b| a.1.response_id.as_str().cmp(b.1.response_id.as_str()));
    Snapshot {
        repos,
        reviews,
        sessions,
        comments,
        responses,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    repos: Vec<RepoManifest>,
    reviews: Vec<(RepoId, ReviewManifest)>,
    sessions: Vec<(RepoId, kata_core::Session)>,
    comments: Vec<(RepoId, Comment)>,
    responses: Vec<(RepoId, Response)>,
}

#[tokio::test]
async fn export_import_round_trip() {
    let source = SqliteStorage::open_in_memory().await.unwrap();
    seed_database(&source).await;
    let before = collect_state(&source).await;

    let tmp = TempDir::new().unwrap();
    archive::export(&source, tmp.path()).await.unwrap();

    let dest = SqliteStorage::open_in_memory().await.unwrap();
    archive::import(tmp.path(), &dest).await.unwrap();
    let after = collect_state(&dest).await;

    assert_eq!(
        before, after,
        "archive round-trip should preserve every entity bit-for-bit"
    );
}

#[tokio::test]
async fn import_handles_cross_session_responses() {
    // The reviewer-replies-to-author pattern: bob's session B holds a
    // response targeting alice's comment in session A. Whether
    // `list_subdirs` returns A or B first depends on filesystem order;
    // if import inserted responses inline with their session, B-first
    // would hit the FK before A's comment existed. The deferred
    // second-pass keeps the import order-independent.
    let source = SqliteStorage::open_in_memory().await.unwrap();
    let repo = fake_repo_id();
    let review = ReviewId::new("feature-xyz");
    let alice = Author::new("alice@example.com");
    let bob = Author::new("bob@example.com");

    source.ensure_repo(&manifest_for(&repo)).await.unwrap();
    source
        .create_review(&repo, &review_manifest(&review, &alice))
        .await
        .unwrap();

    let session_a = source
        .open_or_create_session(&repo, &review, &alice)
        .await
        .unwrap();
    let target_comment = Comment {
        schema_version: SCHEMA_VERSION,
        comment_id: kata_storage::ids::new_comment_id(),
        session_id: session_a.session_id.clone(),
        review_id: review.clone(),
        author: alice.clone(),
        created_at: Utc::now(),
        patchset: 1,
        anchor_change_id: ChangeId::new("tipchange"),
        anchor_commit_id: CommitId::new("tipcommit"),
        file: Some("src/foo.rs".into()),
        side: Some(Side::Tip),
        lines: Some(LineRange::new(10, 15)),
        flag: Flag::MustDo,
        body: "fix this\n".into(),
    };
    source.upsert_draft_comment(&repo, &target_comment).await.unwrap();
    source
        .publish_session(&repo, &review, &session_a.session_id)
        .await
        .unwrap();

    let session_b = source
        .open_or_create_session(&repo, &review, &bob)
        .await
        .unwrap();
    let cross_session_response = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_storage::ids::new_response_id(),
        in_reply_to: target_comment.comment_id.clone(),
        session_id: session_b.session_id.clone(),
        author: bob.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Resolve,
        body: "done\n".into(),
    };
    source
        .upsert_draft_response(&repo, &cross_session_response)
        .await
        .unwrap();
    source
        .publish_session(&repo, &review, &session_b.session_id)
        .await
        .unwrap();

    let tmp = TempDir::new().unwrap();
    archive::export(&source, tmp.path()).await.unwrap();
    let dest = SqliteStorage::open_in_memory().await.unwrap();
    archive::import(tmp.path(), &dest).await.unwrap();

    let responses = dest
        .list_all_responses_for_session(&session_b.session_id)
        .await
        .unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].in_reply_to, target_comment.comment_id);
}

#[tokio::test]
async fn import_rejects_non_archive_directory() {
    // Pointing at a directory that has no `<repo-id>/repo.toml` is
    // almost always a typo — without this check the import would just
    // silently succeed with zero rows, leaving the user wondering why
    // nothing showed up.
    let dest = SqliteStorage::open_in_memory().await.unwrap();
    let tmp = TempDir::new().unwrap();
    // A subdir with a stray file but no repo.toml — exactly what an
    // unrelated directory would look like.
    std::fs::create_dir_all(tmp.path().join("nope")).unwrap();
    std::fs::write(tmp.path().join("nope").join("something.txt"), b"x").unwrap();
    let err = archive::import(tmp.path(), &dest).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not a kata archive"), "unexpected error: {msg}");
}

#[tokio::test]
async fn export_writes_expected_layout() {
    // The archive *is* the contract — keep the directory shape pinned
    // so a sibling tool that scans the export can rely on it.
    let storage = SqliteStorage::open_in_memory().await.unwrap();
    seed_database(&storage).await;
    let tmp = TempDir::new().unwrap();
    archive::export(&storage, tmp.path()).await.unwrap();
    let repo_dir = tmp.path().join(fake_repo_id().as_str());
    assert!(repo_dir.join("repo.toml").is_file());
    assert!(repo_dir.join("reviews").join("feature-xyz").join("review.toml").is_file());
    let sessions = repo_dir.join("reviews").join("feature-xyz").join("sessions");
    assert!(
        list_dirs(&sessions).contains(&"alice@example.com".to_string())
            && list_dirs(&sessions).contains(&"bob@example.com".to_string()),
        "both authors should have their own session directory",
    );
}

fn list_dirs(parent: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| {
            let e = e.ok()?;
            if e.file_type().ok()?.is_dir() {
                Some(e.file_name().to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}
