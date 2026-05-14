//! Behavioural tests for `SqliteStorage`. Mirror the most important
//! scenarios in `filesystem.rs`. The two impls share a `Storage` trait,
//! so this suite focuses on the things that depend on the SQLite layer
//! specifically (concurrency, field round-trips through SQL columns).

use chrono::Utc;
use kata_core::{
    Author, ChangeId, Comment, CommitId, Flag, LineRange, Patchset, RepoId, RepoManifest,
    ResolutionAction, Response, ReviewId, ReviewManifest, RevSet, SCHEMA_VERSION, Session, Side,
};
use kata_storage::sqlite::SqliteStorage;
use kata_storage::{Error, Storage};

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

async fn fixture() -> (SqliteStorage, RepoId, ReviewId, Author) {
    let storage = SqliteStorage::open_in_memory().await.unwrap();
    let repo = fake_repo_id();
    let review = ReviewId::new("feature-xyz");
    let author = Author::new("alice@example.com");
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap();
    (storage, repo, review, author)
}

#[tokio::test]
async fn round_trip_repo_manifest() {
    let storage = SqliteStorage::open_in_memory().await.unwrap();
    let repo = fake_repo_id();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    let loaded = storage.open_repo(&repo).await.unwrap().unwrap();
    assert_eq!(loaded.repo_id, repo);
    // Idempotent.
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    let count_reload = storage.open_repo(&repo).await.unwrap().unwrap();
    assert_eq!(count_reload.repo_id, repo);
}

#[tokio::test]
async fn create_open_list_update_review() {
    let (storage, repo, review, author) = fixture().await;
    let loaded = storage.open_review(&repo, &review).await.unwrap();
    assert_eq!(loaded.review_id, review);

    let list = storage.list_reviews(&repo).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].manifest.review_id, review);
    assert_eq!(list[0].session_count, 0);
    assert_eq!(list[0].published_comment_count, 0);

    // Recreate is rejected.
    let err = storage
        .create_review(&repo, &review_manifest(&review, &author))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ReviewExists { .. }));

    // Append a patchset.
    let mut manifest = storage.open_review(&repo, &review).await.unwrap();
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
    let (storage, repo, review, author) = fixture().await;
    let a = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let b = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    assert_eq!(
        a.session_id, b.session_id,
        "same author should reuse the open draft"
    );

    let other = Author::new("bob@example.com");
    let c = storage
        .open_or_create_session(&repo, &review, &other)
        .await
        .unwrap();
    assert_ne!(c.session_id, a.session_id, "different author gets a new one");
}

#[tokio::test]
async fn open_or_create_session_is_race_safe() {
    // The partial UNIQUE index on `sessions(repo, review, author) WHERE
    // status='draft'` plus BEGIN IMMEDIATE makes parallel `open_or_create`
    // converge on a single row. This is the property that distinguishes
    // the SQLite impl from the filesystem one — the FS path can produce
    // duplicate sessions under concurrency.
    let (storage, repo, review, author) = fixture().await;
    let storage = std::sync::Arc::new(storage);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = storage.clone();
        let r = repo.clone();
        let rv = review.clone();
        let a = author.clone();
        handles.push(tokio::spawn(async move {
            s.open_or_create_session(&r, &rv, &a).await.unwrap()
        }));
    }
    let mut ids = std::collections::HashSet::new();
    for h in handles {
        ids.insert(h.await.unwrap().session_id);
    }
    assert_eq!(ids.len(), 1, "concurrent callers should converge on one draft");
}

#[tokio::test]
async fn comment_field_round_trip() {
    // Every nullable column on `comments` exercises a different code
    // path through the row extractor. A bug in any single field's
    // bind/extract would only show up if a comment populates it, so
    // build one that uses every Side / Flag / line range / file slot.
    let (storage, repo, review, author) = fixture().await;
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();

    let line = line_comment(&session);
    storage.upsert_draft_comment(&repo, &line).await.unwrap();

    let file_level = Comment {
        comment_id: kata_storage::ids::new_comment_id(),
        file: Some("README.md".into()),
        side: None,
        lines: None,
        flag: Flag::Suggestion,
        ..line.clone()
    };
    storage.upsert_draft_comment(&repo, &file_level).await.unwrap();

    let review_wide = Comment {
        comment_id: kata_storage::ids::new_comment_id(),
        file: None,
        side: None,
        lines: None,
        flag: Flag::Other,
        ..line.clone()
    };
    storage.upsert_draft_comment(&repo, &review_wide).await.unwrap();

    storage
        .publish_session(&repo, &review, &session.session_id)
        .await
        .unwrap();
    let mut loaded = storage.list_published_comments(&repo, &review).await.unwrap();
    loaded.sort_by(|a, b| a.comment_id.as_str().cmp(b.comment_id.as_str()));
    let mut expected = vec![line, file_level, review_wide];
    expected.sort_by(|a, b| a.comment_id.as_str().cmp(b.comment_id.as_str()));
    assert_eq!(loaded, expected);
}

#[tokio::test]
async fn publish_flips_visibility() {
    let (storage, repo, review, author) = fixture().await;
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    storage
        .upsert_draft_comment(&repo, &line_comment(&session))
        .await
        .unwrap();

    // Drafts visible to the author, not to listings.
    let drafts = storage
        .list_drafts_for(&repo, &review, &author)
        .await
        .unwrap();
    assert_eq!(drafts.comments.len(), 1);
    assert!(
        storage
            .list_published_comments(&repo, &review)
            .await
            .unwrap()
            .is_empty()
    );

    storage
        .publish_session(&repo, &review, &session.session_id)
        .await
        .unwrap();

    // Flipped: not in drafts (no open session), now in published.
    assert!(
        storage
            .list_drafts_for(&repo, &review, &author)
            .await
            .unwrap()
            .session
            .is_none()
    );
    assert_eq!(
        storage
            .list_published_comments(&repo, &review)
            .await
            .unwrap()
            .len(),
        1
    );

    // Re-publishing a finalized session is rejected.
    let err = storage
        .publish_session(&repo, &review, &session.session_id)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SessionState { .. }));
}

#[tokio::test]
async fn responses_round_trip_and_attach_to_session() {
    let (storage, repo, review, author) = fixture().await;
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    let comment = line_comment(&session);
    storage.upsert_draft_comment(&repo, &comment).await.unwrap();

    let resp = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_storage::ids::new_response_id(),
        in_reply_to: comment.comment_id.clone(),
        session_id: session.session_id.clone(),
        author: author.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Resolve,
        body: "fixed in next commit\n".into(),
    };
    storage.upsert_draft_response(&repo, &resp).await.unwrap();

    storage
        .publish_session(&repo, &review, &session.session_id)
        .await
        .unwrap();
    let loaded = storage
        .list_published_responses(&repo, &review)
        .await
        .unwrap();
    assert_eq!(loaded, vec![resp]);
}

#[tokio::test]
async fn discard_session_hides_drafts() {
    let (storage, repo, review, author) = fixture().await;
    let session = storage
        .open_or_create_session(&repo, &review, &author)
        .await
        .unwrap();
    storage
        .upsert_draft_comment(&repo, &line_comment(&session))
        .await
        .unwrap();
    storage
        .discard_session(&repo, &review, &session.session_id)
        .await
        .unwrap();

    let drafts = storage
        .list_drafts_for(&repo, &review, &author)
        .await
        .unwrap();
    assert!(drafts.session.is_none(), "discarded sessions don't surface as drafts");
    assert!(
        storage
            .list_published_comments(&repo, &review)
            .await
            .unwrap()
            .is_empty(),
        "and they also don't surface as published"
    );
}

#[tokio::test]
async fn rejects_path_unsafe_ids() {
    let storage = SqliteStorage::open_in_memory().await.unwrap();
    let repo = fake_repo_id();
    storage.ensure_repo(&manifest_for(&repo)).await.unwrap();
    // Same id-validation rules as the FS store, so the two backends
    // behave identically when callers pass bad input.
    let bad_review = ReviewId::new("../escape");
    let manifest = review_manifest(&bad_review, &Author::new("a"));
    let err = storage.create_review(&repo, &manifest).await.unwrap_err();
    assert!(matches!(err, Error::InvalidId { .. }), "got {err:?}");
}
