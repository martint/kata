use std::path::Path;

use chrono::Utc;
use kata_core::{
    Author, ChangeId, Comment, CommitId, Flag, LineRange, Patchset, RepoId, RepoManifest,
    ResolutionAction, Response, ReviewId, ReviewManifest, RevSet, SCHEMA_VERSION, Session, Side,
};
use kata_storage::{
    Error, FilesystemStorage, Storage, compute_repo_id, jj_repo_canonical_path,
};
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
        summary: None,
    }
}

fn line_comment(session: &Session) -> Comment {
    Comment {
        schema_version: SCHEMA_VERSION,
        comment_id: kata_storage::ids::new_comment_id(),
        session_id: session.session_id.clone(),
        review_id: session.review_id.clone(),
        author: session.author.clone(),
        created_at: Utc::now(),
        patchset: 1,
        anchor_change_id: ChangeId::new("tipchange"),
        anchor_commit_id: CommitId::new("tipcommit"),
        file: Some("src/foo.rs".into()),
        side: Some(Side::Tip),
        lines: Some(LineRange::new(10, 15)),
        flag: Flag::MustDo,
        body: "this needs a doc comment\n".into(),
    }
}

fn fixture() -> (TempDir, FilesystemStorage, RepoId, ReviewId, Author) {
    let dir = TempDir::new().unwrap();
    let storage = FilesystemStorage::new(dir.path().to_path_buf());
    let repo = fake_repo_id();
    let review = ReviewId::new("feature-xyz");
    let author = Author::new("alice@example.com");
    (dir, storage, repo, review, author)
}

#[tokio::test]
async fn round_trip_repo_manifest() {
    let (_dir, storage, repo, _, _) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    let loaded = storage.open_repo(&repo).await.unwrap().unwrap();
    assert_eq!(loaded.repo_id, repo);
    // Idempotent.
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
}

#[tokio::test]
async fn open_repo_missing_returns_none() {
    let (_dir, storage, repo, _, _) = fixture();
    assert!(storage.open_repo(&repo).await.unwrap().is_none());
}

#[tokio::test]
async fn review_create_open_list_update() {
    let (_dir, storage, repo, review, author) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    let mut manifest = review_manifest(&review, &author);
    storage.create_review(&repo, &manifest).await.unwrap();

    let loaded = storage.open_review(&repo, &review).await.unwrap();
    assert_eq!(loaded.review_id, review);

    let list = storage.list_reviews(&repo).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].manifest.review_id, review);
    assert_eq!(list[0].session_count, 0);
    assert_eq!(list[0].published_comment_count, 0);

    // Recreate is rejected.
    let err = storage.create_review(&repo, &manifest).await.unwrap_err();
    assert!(matches!(err, Error::ReviewExists { .. }));

    // Update succeeds: appending a patchset.
    let now = Utc::now();
    manifest.patchsets.push(Patchset {
        n: 2,
        base_change: ChangeId::new("basechange"),
        base_commit: CommitId::new("basecommit"),
        tip_change: ChangeId::new("newer"),
        tip_commit: CommitId::new("newercommit"),
        recorded_at: now,
        parent_patchset: Some(1),
    });
    manifest.current_patchset = 2;
    storage.update_review(&repo, &manifest).await.unwrap();
    let loaded = storage.open_review(&repo, &review).await.unwrap();
    assert_eq!(loaded.current_patchset, 2);
    assert_eq!(loaded.current().tip_change.as_str(), "newer");
}

#[tokio::test]
async fn open_or_create_session_is_sticky_per_author() {
    let (_dir, storage, repo, review, author) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap();

    let s1 = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let s2 = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    assert_eq!(s1.session_id, s2.session_id, "should reuse open draft");

    // Different author gets a different session.
    let other = Author::new("bob@example.com");
    let s3 = storage
        .open_or_create_session(&repo, &review, &other)
        .await
        .unwrap();
    assert_ne!(s3.session_id, s1.session_id);
}

#[tokio::test]
async fn draft_comments_lifecycle() {
    let (_dir, storage, repo, review, author) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap();
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let comment = line_comment(&session);

    storage.upsert_draft_comment(&repo, &comment).await.unwrap();

    // Visible via list_drafts_for, NOT via list_published_comments.
    let drafts = storage
        .list_drafts_for(&repo, &review, &author)
        .await
        .unwrap();
    assert_eq!(drafts.comments.len(), 1);
    assert_eq!(drafts.comments[0].comment_id, comment.comment_id);
    assert_eq!(drafts.comments[0].body, comment.body);

    let published = storage
        .list_published_comments(&repo, &review)
        .await
        .unwrap();
    assert!(published.is_empty());

    // Upsert is idempotent.
    let mut edited = comment.clone();
    edited.body = "now with more detail\n".into();
    storage.upsert_draft_comment(&repo, &edited).await.unwrap();
    let drafts = storage
        .list_drafts_for(&repo, &review, &author)
        .await
        .unwrap();
    assert_eq!(drafts.comments.len(), 1);
    assert_eq!(drafts.comments[0].body, "now with more detail\n");

    // Discard removes it.
    storage
        .discard_draft_comment(&repo, &review, &session.session_id, &comment.comment_id)
        .await
        .unwrap();
    let drafts = storage
        .list_drafts_for(&repo, &review, &author)
        .await
        .unwrap();
    assert!(drafts.comments.is_empty());
}

#[tokio::test]
async fn publish_flips_visibility_and_locks_session() {
    let (_dir, storage, repo, review, author) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap();
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let comment = line_comment(&session);
    storage.upsert_draft_comment(&repo, &comment).await.unwrap();

    storage
        .publish_session(&repo, &review, &session.session_id)
        .await
        .unwrap();

    let published = storage
        .list_published_comments(&repo, &review)
        .await
        .unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].comment_id, comment.comment_id);

    // Further writes to the published session are rejected.
    let mut amend = comment.clone();
    amend.body = "trying to edit after publish\n".into();
    let err = storage.upsert_draft_comment(&repo, &amend).await.unwrap_err();
    assert!(matches!(err, Error::SessionState { .. }));

    // A fresh open_or_create_session opens a *new* session, not the
    // published one.
    let s2 = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    assert_ne!(s2.session_id, session.session_id);
}

#[tokio::test]
async fn responses_attach_to_open_session() {
    let (_dir, storage, repo, review, author) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap();
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let comment = line_comment(&session);
    storage.upsert_draft_comment(&repo, &comment).await.unwrap();
    storage
        .publish_session(&repo, &review, &session.session_id)
        .await
        .unwrap();

    // Bob now responds.
    let bob = Author::new("bob@example.com");
    let bob_session = storage
        .open_or_create_session(&repo, &review, &bob)
        .await
        .unwrap();
    let response = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_storage::ids::new_response_id(),
        in_reply_to: comment.comment_id.clone(),
        session_id: bob_session.session_id.clone(),
        author: bob.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Resolve,
        body: String::new(),
    };
    storage
        .upsert_draft_response(&repo, &response)
        .await
        .unwrap();

    // Visible only as a draft, not yet published.
    let bob_drafts = storage
        .list_drafts_for(&repo, &review, &bob)
        .await
        .unwrap();
    assert_eq!(bob_drafts.responses.len(), 1);
    let published_responses = storage
        .list_published_responses(&repo, &review)
        .await
        .unwrap();
    assert!(published_responses.is_empty());

    storage
        .publish_session(&repo, &review, &bob_session.session_id)
        .await
        .unwrap();
    let published_responses = storage
        .list_published_responses(&repo, &review)
        .await
        .unwrap();
    assert_eq!(published_responses.len(), 1);
    assert_eq!(published_responses[0].action, ResolutionAction::Resolve);
    assert_eq!(published_responses[0].in_reply_to, comment.comment_id);
}

#[tokio::test]
async fn discard_session_hides_drafts() {
    let (_dir, storage, repo, review, author) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap();
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let comment = line_comment(&session);
    storage.upsert_draft_comment(&repo, &comment).await.unwrap();

    storage
        .discard_session(&repo, &review, &session.session_id)
        .await
        .unwrap();

    // open_or_create_session should now mint a new one.
    let s2 = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    assert_ne!(s2.session_id, session.session_id);

    // The discarded comment isn't published, and isn't in the new draft.
    let published = storage
        .list_published_comments(&repo, &review)
        .await
        .unwrap();
    assert!(published.is_empty());
    let drafts = storage
        .list_drafts_for(&repo, &review, &author)
        .await
        .unwrap();
    assert!(drafts.comments.is_empty());
}

#[tokio::test]
async fn rejects_path_unsafe_ids() {
    let (_dir, storage, repo, _, _) = fixture();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    let bad_review = ReviewId::new("../escape");
    let err = storage
        .open_review(&repo, &bad_review)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidId { .. }));
}

#[test]
fn repo_id_helpers_are_deterministic_and_path_safe() {
    let p = Path::new("/tmp/example/.jj/repo");
    let id = compute_repo_id(p);
    assert_eq!(id.as_str().len(), 64);
    assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(compute_repo_id(p), id);
}

#[test]
fn jj_repo_canonical_path_uses_directory_when_present() {
    let dir = TempDir::new().unwrap();
    let jj = dir.path().join(".jj");
    std::fs::create_dir_all(jj.join("repo")).unwrap();
    let resolved = jj_repo_canonical_path(dir.path()).unwrap();
    assert_eq!(resolved, jj.join("repo").canonicalize().unwrap());
}

#[test]
fn jj_repo_canonical_path_follows_pointer_file() {
    let dir = TempDir::new().unwrap();
    let main_repo = dir.path().join("main").join(".jj").join("repo");
    std::fs::create_dir_all(&main_repo).unwrap();
    let alt_jj = dir.path().join("alt").join(".jj");
    std::fs::create_dir_all(&alt_jj).unwrap();
    std::fs::write(alt_jj.join("repo"), "../../main/.jj/repo").unwrap();

    let resolved = jj_repo_canonical_path(&dir.path().join("alt")).unwrap();
    assert_eq!(resolved, main_repo.canonicalize().unwrap());
}
