//! End-to-end test for the github publish path.
//!
//! Lives outside the unit tests in `lib.rs` because it needs to
//! script the GitHub client surface (which 3rd-pass PR review
//! flagged as untestable without a trait). Drives the routing
//! decisions in `publish::publish_session_to_github` against a
//! real `SqliteStorage` and an in-memory fake client; never
//! touches `gh`, the network, or jj.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use kata_core::{
    Author, ChangeId, CommentId, CommitId, LineRange, Patchset, RepoId, ResolutionAction,
    ReviewId, ReviewManifest, RevSet, SessionId, Side, documents::Comment,
    documents::ExternalAuthor, documents::Flag, documents::GithubPr, documents::RepoManifest,
    documents::Response, documents::SCHEMA_VERSION,
};
use kata_service::github::client::{
    AuthStatus, GithubClient, GithubError, GithubResult, PullRequest,
};
use kata_service::github::publish::{PublishEvent, publish_session_to_github};
use kata_service::github::url::PullRequestRef;
use kata_storage::sqlite::SqliteStorage;
use kata_storage::{GithubCommentMapping, Storage};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct PostedCall {
    endpoint: String,
    body: Value,
}

/// Scriptable fake. Returns `fetch_pr_result` for every `fetch_pr`
/// (the publish flow only fetches once per call), and replies to
/// every POST with a synthetic CreatedComment shape — the publish
/// path deserialises the response into a `CreatedComment` so the
/// fake needs to return `{ id, node_id, ... }`. Validation errors
/// can be queued via `validation_errors` to exercise the 422-
/// fallback paths.
#[derive(Default)]
struct FakeGithub {
    fetch_pr_result: Mutex<Option<PullRequest>>,
    posts: Mutex<Vec<PostedCall>>,
    /// Queued `Validation` errors. Each POST checks whether its
    /// endpoint+body should fail with the next-queued error before
    /// returning success; on match it pops the entry and returns
    /// `GithubError::Validation`.
    validation_errors: Mutex<Vec<ValidationRule>>,
    /// Scripted responses for `get_raw`, keyed by endpoint
    /// substring. First match wins. Default falls through to `[]`
    /// (the publish path's only GET is the post-bundle comments
    /// fetch, and an empty list is harmless when the test doesn't
    /// care about mapping rewrites).
    get_responses: Mutex<Vec<(String, Value)>>,
    /// Mirrors github.com's `commit_id` validation on
    /// `POST /pulls/N/comments` and `POST /pulls/N/reviews`: any
    /// `commit_id` in the body that isn't in this set returns a
    /// `Validation` error, exactly as the real API would.
    /// `None` (the default) means "accept anything" so existing
    /// tests don't have to set it. Opt in for the LEFT-anchor
    /// regression and anywhere else that needs to catch a wrong
    /// commit_id.
    valid_commit_shas: Mutex<Option<std::collections::HashSet<String>>>,
    next_id: Mutex<u64>,
}

#[derive(Debug, Clone)]
struct ValidationRule {
    /// Endpoint substring the call must contain. Empty matches any.
    endpoint_contains: String,
    /// Body fragment the POST body must contain (as substring of
    /// its JSON serialisation). Empty matches any.
    body_contains: String,
    stderr: String,
}

impl FakeGithub {
    fn set_pr(&self, pr: PullRequest) {
        *self.fetch_pr_result.lock().unwrap() = Some(pr);
    }

    fn posts(&self) -> Vec<PostedCall> {
        self.posts.lock().unwrap().clone()
    }

    fn set_get_response(&self, endpoint_contains: &str, body: Value) {
        self.get_responses
            .lock()
            .unwrap()
            .push((endpoint_contains.to_owned(), body));
    }

    fn set_valid_commit_shas<I, S>(&self, shas: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *self.valid_commit_shas.lock().unwrap() =
            Some(shas.into_iter().map(Into::into).collect());
    }

    fn queue_422(&self, endpoint_contains: &str, body_contains: &str, stderr: &str) {
        self.validation_errors.lock().unwrap().push(ValidationRule {
            endpoint_contains: endpoint_contains.to_owned(),
            body_contains: body_contains.to_owned(),
            stderr: stderr.to_owned(),
        });
    }

    fn synth_created(&self) -> Value {
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        json!({
            "id": *id,
            "node_id": format!("MDEx{}", *id),
            "path": null,
            "line": null,
            "original_line": null,
            "start_line": null,
            "original_start_line": null,
            "side": null,
            "body": "",
        })
    }
}

#[async_trait]
impl GithubClient for FakeGithub {
    async fn fetch_pr(&self, _pr: &PullRequestRef) -> GithubResult<PullRequest> {
        self.fetch_pr_result
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| GithubError::Parse("fake: fetch_pr not configured".into()))
    }

    async fn graphql_raw(&self, _q: &str, _v: Value) -> GithubResult<Value> {
        unimplemented!("publish path does not call graphql")
    }

    async fn auth_status(&self) -> GithubResult<AuthStatus> {
        unimplemented!("publish path does not call auth_status")
    }

    async fn get_raw(&self, endpoint: &str) -> GithubResult<Value> {
        let resps = self.get_responses.lock().unwrap();
        if let Some((_, v)) = resps.iter().find(|(pat, _)| endpoint.contains(pat)) {
            return Ok(v.clone());
        }
        // Default: the post-bundle comments fetch returns nothing.
        Ok(json!([]))
    }

    async fn post_raw(&self, endpoint: &str, body: &Value) -> GithubResult<Value> {
        // Mirror github.com's commit_id validation. Applies to
        // line-comment POSTs (`commit_id` at top level) and to
        // review POSTs (`commit_id` at top level, comments[] entries
        // inherit it). If a body carries an unknown commit_id, return
        // a Validation error exactly the way the real API would.
        if let Some(valid) = self.valid_commit_shas.lock().unwrap().as_ref() {
            let mut commit_ids: Vec<&str> = Vec::new();
            if let Some(cid) = body.get("commit_id").and_then(Value::as_str) {
                commit_ids.push(cid);
            }
            if let Some(arr) = body.get("comments").and_then(Value::as_array) {
                for c in arr {
                    if let Some(cid) = c.get("commit_id").and_then(Value::as_str) {
                        commit_ids.push(cid);
                    }
                }
            }
            for cid in commit_ids {
                if !valid.contains(cid) {
                    return Err(GithubError::Validation {
                        stderr: format!(
                            "HTTP 422: commit_id {cid:?} is not part of the pull request"
                        ),
                    });
                }
            }
        }
        let body_str = body.to_string();
        let mut rules = self.validation_errors.lock().unwrap();
        if let Some(idx) = rules.iter().position(|r| {
            (r.endpoint_contains.is_empty() || endpoint.contains(&r.endpoint_contains))
                && (r.body_contains.is_empty() || body_str.contains(&r.body_contains))
        }) {
            let rule = rules.remove(idx);
            return Err(GithubError::Validation { stderr: rule.stderr });
        }
        drop(rules);
        self.posts.lock().unwrap().push(PostedCall {
            endpoint: endpoint.to_owned(),
            body: body.clone(),
        });
        Ok(self.synth_created())
    }
}

// ---- fixtures ------------------------------------------------------------

fn fake_pull_request() -> PullRequest {
    serde_json::from_value(json!({
        "number": 42,
        "title": "test",
        "body": null,
        "state": "open",
        "merged": false,
        "html_url": "https://github.com/octo/repo/pull/42",
        "user": {"login":"octo","id":1,"avatar_url":null,"html_url":null},
        "base": {"label":"octo:main","ref":"main","sha":"basesha",
            "repo":{"full_name":"octo/repo","clone_url":"","html_url":""}},
        "head": {"label":"octo:topic","ref":"topic","sha":"headsha",
            "repo":{"full_name":"octo/repo","clone_url":"","html_url":""}},
    }))
    .unwrap()
}

fn fake_manifest(review_id: &ReviewId) -> ReviewManifest {
    let now = Utc::now();
    let base = CommitId::new("basesha");
    let tip = CommitId::new("headsha");
    let base_change = ChangeId::new("c-base");
    let tip_change = ChangeId::new("c-tip");
    ReviewManifest {
        schema_version: SCHEMA_VERSION,
        review_id: review_id.clone(),
        number: 1,
        name: "test review".into(),
        revset: RevSet::new("basesha..headsha"),
        created_at: now,
        created_by: Author::new("alice"),
        bookmark: None,
        summary: None,
        patchsets: vec![Patchset {
            n: 1,
            base_change: base_change.clone(),
            base_commit: base.clone(),
            tip_change: tip_change.clone(),
            tip_commit: tip.clone(),
            recorded_at: now,
            parent_patchset: None,
        }],
        current_patchset: 1,
        archived_at: None,
        github_pr: Some(GithubPr {
            owner: "octo".into(),
            repo: "repo".into(),
            number: 42,
            html_url: "https://github.com/octo/repo/pull/42".into(),
            original_head_sha: "headsha".into(),
            original_base_sha: "basesha".into(),
            remote_name: "origin".into(),
        }),
    }
}

fn make_draft_comment(
    review_id: &ReviewId,
    session_id: &SessionId,
    comment_id: &str,
    file: Option<&str>,
    side: Option<Side>,
    line: Option<u32>,
    review_wide: bool,
    body: &str,
) -> Comment {
    Comment {
        schema_version: SCHEMA_VERSION,
        comment_id: CommentId::new(comment_id),
        session_id: session_id.clone(),
        review_id: review_id.clone(),
        author: Author::new("alice"),
        created_at: Utc::now(),
        patchset: 1,
        anchor_change_id: ChangeId::new("c-tip"),
        anchor_commit_id: CommitId::new("headsha"),
        file: file.map(|s| s.to_string()),
        side,
        lines: line.map(|l| LineRange::single(l)),
        columns: None,
        review_wide,
        flag: Flag::Suggestion,
        body: body.into(),
        external_author: None,
    }
}

async fn setup() -> (Arc<SqliteStorage>, RepoId, ReviewManifest, SessionId) {
    let storage = Arc::new(SqliteStorage::open_in_memory().await.unwrap());
    let repo = RepoId::new("repo-1");
    let review_id = ReviewId::new("rev-1");
    let manifest = fake_manifest(&review_id);
    storage
        .ensure_repo(&RepoManifest {
            schema_version: SCHEMA_VERSION,
            repo_id: repo.clone(),
            canonical_path: "/test".into(),
        })
        .await
        .unwrap();
    let stored = storage.create_review(&repo, &manifest).await.unwrap();
    let session = storage
        .open_or_create_session(&repo, &stored.review_id, &Author::new("alice"))
        .await
        .unwrap();
    (storage, repo, stored, session.session_id)
}

// ---- tests ---------------------------------------------------------------

#[tokio::test]
async fn publish_routes_each_bucket_to_its_own_endpoint() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // RIGHT-side inline → bundled review
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-right",
                Some("src/lib.rs"), Some(Side::Tip), Some(10),
                false, "right-side note",
            ),
        )
        .await
        .unwrap();
    // LEFT-side inline → individual post against base sha
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-left",
                Some("src/lib.rs"), Some(Side::Base), Some(20),
                false, "left-side note",
            ),
        )
        .await
        .unwrap();
    // Review-wide → issue comment
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-rw",
                None, None, None, true, "overall thoughts",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    // Mirror github.com's commit_id validation: only headsha is in
    // the PR's commit list. The PR's base SHA is NOT — passing it
    // as commit_id is the bug a previous round of this code shipped,
    // so the test enforces the constraint that caught it.
    fake.set_valid_commit_shas(["headsha"]);

    publish_session_to_github(
        storage.as_ref(),
        fake.as_ref(),
        &repo,
        &manifest,
        &session,
        &author,
        PublishEvent::Comment,
        None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    // Expected: 1 issue comment, 1 LEFT-side line comment, 1 bundled review.
    assert_eq!(posts.len(), 3, "exactly three POSTs expected, got {posts:#?}");

    let issue = posts
        .iter()
        .find(|p| p.endpoint.contains("/issues/42/comments"))
        .expect("review-wide comment should land on /issues/N/comments");
    assert_eq!(issue.body["body"], "overall thoughts");

    let left = posts
        .iter()
        .find(|p| {
            p.endpoint.contains("/pulls/42/comments") && p.body["side"] == "LEFT"
        })
        .expect("LEFT-side comment should land on /pulls/N/comments with side=LEFT");
    // LEFT-side inlines anchor to the live head SHA (the PR's own
    // commit list) with `side: LEFT` — github.com maps the line
    // to the base/original side of the diff at that commit.
    assert_eq!(left.body["commit_id"], "headsha");
    // Success-path body is the raw user text; no file:line footer
    // (the footer earns its keep only when the inline POST 422s
    // and we fall back to an issue comment).
    assert_eq!(left.body["body"], "left-side note");

    let bundled = posts
        .iter()
        .find(|p| p.endpoint.contains("/pulls/42/reviews"))
        .expect("RIGHT-side inline should ride the bundled review");
    assert_eq!(bundled.body["commit_id"], "headsha");
    let comments = bundled.body["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["body"], "right-side note");
}

#[tokio::test]
async fn left_side_422_falls_back_to_quoted_issue_comment() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-left",
                Some("src/lib.rs"), Some(Side::Base), Some(99),
                false, "concern on a missing line",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    fake.queue_422(
        "/pulls/42/comments",
        "\"side\":\"LEFT\"",
        "HTTP 422: Unprocessable Entity (line not in diff)",
    );

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    // The line-comment POST 422'd; the fallback issue comment is the
    // only POST that actually landed.
    assert_eq!(posts.len(), 1);
    assert!(posts[0].endpoint.contains("/issues/42/comments"));
    let body = posts[0].body["body"].as_str().unwrap();
    assert!(body.starts_with("concern on a missing line"));
    assert!(body.contains("src/lib.rs:99"));
}

#[tokio::test]
async fn reply_with_known_thread_mapping_uses_in_reply_to() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // Seed an imported parent comment + its thread-anchor mapping
    // so the reply path finds a REST id to thread off of. The
    // parent rides a synthetic session distinct from the
    // author's open one — otherwise list_drafts_for sees it as
    // a same-session draft and re-publishes it.
    let imported_session = SessionId::new("imported-bob");
    storage
        .raw_insert_session(
            &repo,
            &kata_core::Session {
                schema_version: SCHEMA_VERSION,
                session_id: imported_session.clone(),
                review_id: manifest.review_id.clone(),
                author: Author::new("gh:bob"),
                status: kata_core::documents::SessionStatus::Published,
                created_at: Utc::now(),
                published_at: Some(Utc::now()),
            },
        )
        .await
        .unwrap();
    let parent = Comment {
        author: Author::new("gh:bob"),
        external_author: Some(ExternalAuthor {
            source: "github".into(),
            login: "bob".into(),
            id: 7,
            avatar_url: None,
            html_url: None,
        }),
        body: "original concern".into(),
        ..make_draft_comment(
            &manifest.review_id, &imported_session, "c-parent",
            Some("src/lib.rs"), Some(Side::Tip), Some(5),
            false, "original concern",
        )
    };
    storage.raw_insert_comment(&repo, &parent).await.unwrap();
    storage
        .insert_github_comment_mapping(
            &repo,
            &GithubCommentMapping {
                review_id: manifest.review_id.clone(),
                kata_comment_id: Some(parent.comment_id.clone()),
                kata_response_id: None,
                github_node_id: "MDExNODE1".into(),
                github_rest_id: Some(12345),
                pr_number: 42,
                kind: "line_comment".into(),
                thread_node_id: Some("MDExTH1".into()),
            },
        )
        .await
        .unwrap();

    // Draft a reply that targets the imported parent's kata id.
    let reply = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_core::ResponseId::new("r-1"),
        in_reply_to: parent.comment_id.clone(),
        session_id: session.clone(),
        author: author.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Comment,
        body: "reply text".into(),
    };
    storage.upsert_draft_response(&repo, &reply).await.unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    assert_eq!(posts.len(), 1, "single reply POST expected, got {posts:#?}");
    let p = &posts[0];
    assert!(p.endpoint.contains("/pulls/42/comments"));
    assert_eq!(p.body["in_reply_to"], 12345);
    assert_eq!(p.body["body"], "reply text");
}

/// Seed an imported parent comment authored by `gh:bob` and a
/// thread-anchor mapping pointing at REST id `rest_id`. Used by
/// both reply-path tests so they share one shape.
async fn seed_imported_parent_with_mapping(
    storage: &SqliteStorage,
    repo: &RepoId,
    manifest: &ReviewManifest,
    rest_id: i64,
) -> Comment {
    let imported_session = SessionId::new("imported-bob");
    storage
        .raw_insert_session(
            repo,
            &kata_core::Session {
                schema_version: SCHEMA_VERSION,
                session_id: imported_session.clone(),
                review_id: manifest.review_id.clone(),
                author: Author::new("gh:bob"),
                status: kata_core::documents::SessionStatus::Published,
                created_at: Utc::now(),
                published_at: Some(Utc::now()),
            },
        )
        .await
        .unwrap();
    let parent = Comment {
        author: Author::new("gh:bob"),
        external_author: Some(ExternalAuthor {
            source: "github".into(),
            login: "bob".into(),
            id: 7,
            avatar_url: None,
            html_url: None,
        }),
        body: "original concern".into(),
        ..make_draft_comment(
            &manifest.review_id, &imported_session, "c-parent",
            Some("src/lib.rs"), Some(Side::Tip), Some(5),
            false, "original concern",
        )
    };
    storage.raw_insert_comment(repo, &parent).await.unwrap();
    storage
        .insert_github_comment_mapping(
            repo,
            &GithubCommentMapping {
                review_id: manifest.review_id.clone(),
                kata_comment_id: Some(parent.comment_id.clone()),
                kata_response_id: None,
                github_node_id: "MDExNODE1".into(),
                github_rest_id: Some(rest_id),
                pr_number: 42,
                kind: "line_comment".into(),
                thread_node_id: Some("MDExTH1".into()),
            },
        )
        .await
        .unwrap();
    parent
}

#[tokio::test]
async fn threaded_reply_422_falls_back_to_quoted_issue_comment() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");
    let parent =
        seed_imported_parent_with_mapping(storage.as_ref(), &repo, &manifest, 99999).await;

    storage
        .upsert_draft_response(
            &repo,
            &Response {
                schema_version: SCHEMA_VERSION,
                response_id: kata_core::ResponseId::new("r-1"),
                in_reply_to: parent.comment_id.clone(),
                session_id: session.clone(),
                author: author.clone(),
                created_at: Utc::now(),
                action: ResolutionAction::Comment,
                body: "my reply".into(),
            },
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    // The threaded POST (with `in_reply_to`) 422s — match on
    // `in_reply_to` so we don't also blow up the fallback issue
    // comment, which doesn't carry that field.
    fake.queue_422(
        "/pulls/42/comments",
        "\"in_reply_to\":99999",
        "HTTP 422: pull request review thread is missing",
    );

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    // The threaded POST 422'd; the fallback is the only landed POST.
    assert_eq!(posts.len(), 1, "expected one fallback POST, got {posts:#?}");
    assert!(posts[0].endpoint.contains("/issues/42/comments"));
    let body = posts[0].body["body"].as_str().unwrap();
    // GitHub-style quote header + the parent body line-prefixed,
    // a "posted as issue comment" note, then the reply text.
    assert!(body.contains("> @bob wrote:"));
    assert!(body.contains("> original concern"));
    assert!(body.contains("threaded_422"));
    assert!(body.contains("my reply"));
}

#[tokio::test]
async fn base_drift_warns_but_publishes() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // Any draft so we don't trip the "no drafts" early-return.
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-rw",
                None, None, None, true, "review-wide",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    // Head matches; base moved. Publish should still proceed.
    let mut pr = fake_pull_request();
    pr.base.sha = "different_base".into();
    fake.set_pr(pr);

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .expect("base drift must not refuse publish");

    let posts = fake.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].endpoint.contains("/issues/42/comments"));
}

#[tokio::test]
async fn head_drift_refuses_with_conflict_and_posts_nothing() {
    use kata_service::ServiceError;

    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // Add a draft so the head-drift refusal can't be confused with
    // the no-drafts BadRequest.
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-rw",
                None, None, None, true, "review-wide",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    let mut pr = fake_pull_request();
    pr.head.sha = "moved_head".into();
    fake.set_pr(pr);

    let err = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .expect_err("head drift must refuse");

    match err {
        ServiceError::Conflict { kind, message } => {
            assert_eq!(kind, "head_drift");
            // Both old and new SHAs (truncated) appear in the message
            // so the SPA can render them without re-fetching.
            assert!(message.contains("headsha"));
            assert!(message.contains("moved_hea"));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert!(
        fake.posts().is_empty(),
        "head-drift refusal must short-circuit before any POST",
    );
}

#[tokio::test]
async fn duplicate_drafts_each_get_their_own_mapping_row() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // Two RIGHT-side drafts with identical (path, line, side, body).
    // Without the consumed-set in `match_posted_comment`, both kata
    // comments would resolve to the same posted comment and only one
    // mapping row would survive.
    for id in ["c-dup-a", "c-dup-b"] {
        storage
            .upsert_draft_comment(
                &repo,
                &make_draft_comment(
                    &manifest.review_id, &session, id,
                    Some("src/lib.rs"), Some(Side::Tip), Some(7),
                    false, "same body",
                ),
            )
            .await
            .unwrap();
    }

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    // Bundled-review submission round-trips through this GET; return
    // two posted comments matching the drafts so the consumed-set
    // dedup has something to dedup against.
    fake.set_get_response(
        "/pulls/42/reviews/",
        json!([
            {"id": 10001, "node_id": "GH_A", "path": "src/lib.rs",
             "line": 7, "side": "RIGHT", "body": "same body",
             "original_line": null, "start_line": null,
             "original_start_line": null},
            {"id": 10002, "node_id": "GH_B", "path": "src/lib.rs",
             "line": 7, "side": "RIGHT", "body": "same body",
             "original_line": null, "start_line": null,
             "original_start_line": null},
        ]),
    );

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    // Each duplicate must end up with its own mapping row pointing
    // at a distinct GitHub node id — that's the contract the
    // consumed-set protects.
    let m_a = storage
        .lookup_github_mapping_by_kata_comment(&repo, &CommentId::new("c-dup-a"))
        .await
        .unwrap()
        .expect("c-dup-a should have a mapping");
    let m_b = storage
        .lookup_github_mapping_by_kata_comment(&repo, &CommentId::new("c-dup-b"))
        .await
        .unwrap()
        .expect("c-dup-b should have a mapping");
    assert_ne!(
        m_a.github_node_id, m_b.github_node_id,
        "duplicate drafts must not collapse onto the same posted comment",
    );
    let pair = [m_a.github_node_id.as_str(), m_b.github_node_id.as_str()];
    assert!(pair.contains(&"GH_A") && pair.contains(&"GH_B"));
}

#[tokio::test]
async fn bundled_review_422_falls_back_to_per_comment_posts() {
    // Regression for the publish-side 422 the user saw on review #12
    // — a RIGHT-side draft anchored to a line outside the diff hunks
    // makes the atomic /reviews POST 422, which used to abort the
    // entire publish. The fix: catch the bundle 422 and re-post each
    // inline individually, with the same issue-comment fallback the
    // LEFT-side path uses.
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // Two RIGHT-side drafts: one will succeed individually, one will
    // 422 individually and fall back to an issue comment. Without
    // the bundle-422 catch, neither would land.
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-good",
                Some("src/lib.rs"), Some(Side::Tip), Some(10),
                false, "in-diff comment",
            ),
        )
        .await
        .unwrap();
    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-bad",
                Some("src/lib.rs"), Some(Side::Tip), Some(9999),
                false, "out-of-diff comment",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    // The atomic bundled POST 422s.
    fake.queue_422(
        "/pulls/42/reviews",
        "\"event\":\"COMMENT\"",
        "HTTP 422: Validation Failed (pull_request_review_thread.line not part of the diff)",
    );
    // The individual POST for the out-of-diff comment also 422s; the
    // good comment posts cleanly.
    fake.queue_422(
        "/pulls/42/comments",
        "out-of-diff comment",
        "HTTP 422: line not part of the diff",
    );

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .expect("bundle 422 must not escape; per-comment fallback must absorb it");

    let posts = fake.posts();
    // No review-body / non-Comment event, so no shell re-post. The
    // bundle POST and the bad individual POST were both consumed by
    // queued 422s (the fake only records successful POSTs), so what
    // landed is: the good inline + the issue-comment fallback for
    // the bad one.
    assert_eq!(posts.len(), 2, "expected 2 landed POSTs, got {posts:#?}");

    let line = posts
        .iter()
        .find(|p| p.endpoint.contains("/pulls/42/comments"))
        .expect("good inline must land individually");
    assert_eq!(line.body["side"], "RIGHT");
    assert_eq!(
        line.body["commit_id"], "headsha",
        "RIGHT-side inlines anchor to live head",
    );
    assert_eq!(line.body["body"], "in-diff comment");
    // The out-of-diff one 422'd on the individual post too, so it
    // landed as an issue comment with file:line context.
    let issue = posts
        .iter()
        .find(|p| p.endpoint.contains("/issues/42/comments"))
        .expect("out-of-diff inline must land as an issue comment");
    let body = issue.body["body"].as_str().unwrap();
    assert!(body.starts_with("out-of-diff comment"));
    assert!(body.contains("src/lib.rs:9999"));
    assert!(body.contains("head-side"));
}

#[tokio::test]
async fn bundled_review_422_with_body_resubmits_shell_then_inlines() {
    // When the bundle 422s but the review carries a body (or a
    // non-Comment event like APPROVE), we resubmit the wrapping
    // review with an empty comments[] so the body / state still
    // lands, then post the inline comments individually.
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-one",
                Some("src/lib.rs"), Some(Side::Tip), Some(7),
                false, "an inline",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    // Only the FIRST /reviews POST (with comments[]) 422s. The
    // queued rule matches on the comments[] entry, so the shell
    // re-post (comments=[]) goes through.
    fake.queue_422(
        "/pulls/42/reviews",
        "\"body\":\"an inline\"",
        "HTTP 422: comments anchor not in diff",
    );

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, Some("LGTM with a nit".into()),
    )
    .await
    .unwrap();

    let posts = fake.posts();
    // Landed: the empty-comments shell review + the one inline.
    let shell = posts
        .iter()
        .find(|p| p.endpoint.contains("/pulls/42/reviews"))
        .expect("shell review POST should land");
    assert_eq!(shell.body["body"], "LGTM with a nit");
    let comments = shell.body["comments"].as_array().unwrap();
    assert!(comments.is_empty(), "shell must have no inline comments");

    let line = posts
        .iter()
        .find(|p| p.endpoint.contains("/pulls/42/comments"))
        .expect("inline should be retried individually");
    assert_eq!(line.body["body"], "an inline");
    assert_eq!(line.body["commit_id"], "headsha");
}

#[tokio::test]
async fn quoted_reply_to_native_kata_author_uses_backticks_not_at_mention() {
    // Regression: a reply whose parent is a native kata comment (no
    // external_author, structural author like `alice`) used to render
    // `> @alice wrote:` and ping whoever owns the `alice` handle on
    // github.com. The fix backticks native-author names so the quote
    // can never accidentally mention a stranger.
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    // Native kata parent in its own (non-draft) session.
    let imported_session = SessionId::new("alice-prev-session");
    storage
        .raw_insert_session(
            &repo,
            &kata_core::Session {
                schema_version: SCHEMA_VERSION,
                session_id: imported_session.clone(),
                review_id: manifest.review_id.clone(),
                author: Author::new("alice"),
                status: kata_core::documents::SessionStatus::Published,
                created_at: Utc::now(),
                published_at: Some(Utc::now()),
            },
        )
        .await
        .unwrap();
    // No external_author + author = "alice" (no `gh:` prefix) =
    // native kata identity, which must NOT be @-mentioned.
    let parent = make_draft_comment(
        &manifest.review_id, &imported_session, "c-parent",
        Some("src/lib.rs"), Some(Side::Tip), Some(5),
        false, "original concern",
    );
    storage.raw_insert_comment(&repo, &parent).await.unwrap();
    // No mapping row → the reply hits the quoted-issue-comment
    // fallback (the path that calls build_quoted_reply_body).

    storage
        .upsert_draft_response(
            &repo,
            &Response {
                schema_version: SCHEMA_VERSION,
                response_id: kata_core::ResponseId::new("r-1"),
                in_reply_to: parent.comment_id.clone(),
                session_id: session.clone(),
                author: author.clone(),
                created_at: Utc::now(),
                action: ResolutionAction::Comment,
                body: "my reply".into(),
            },
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    assert_eq!(posts.len(), 1);
    let body = posts[0].body["body"].as_str().unwrap();
    // The native author must be backticked, never @-mentioned.
    assert!(
        body.contains("> `alice` wrote:"),
        "native-author quote must use backticks, body was:\n{body}"
    );
    assert!(
        !body.contains("@alice"),
        "native-author name must not appear as @login; body was:\n{body}"
    );
}

async fn seed_mapping_for_kata_comment(
    storage: &SqliteStorage,
    repo: &RepoId,
    review_id: &ReviewId,
    kata_comment_id: &CommentId,
    pr_number: u32,
    kind: &str,
) {
    storage
        .insert_github_comment_mapping(
            repo,
            &GithubCommentMapping {
                review_id: review_id.clone(),
                kata_comment_id: Some(kata_comment_id.clone()),
                kata_response_id: None,
                github_node_id: format!("PRE_{}", kata_comment_id.as_str()),
                github_rest_id: Some(1),
                pr_number,
                kind: kind.to_owned(),
                thread_node_id: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn retry_skips_bundled_inlines_already_in_github_comment_map() {
    // Simulates a publish that previously succeeded for one RIGHT
    // inline but partially failed afterward, leaving its mapping
    // row in storage while the surrounding session stayed Draft.
    // The retry must skip the already-mapped inline (otherwise it
    // would duplicate on github.com).
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    let already = make_draft_comment(
        &manifest.review_id, &session, "c-already",
        Some("src/lib.rs"), Some(Side::Tip), Some(10),
        false, "already posted",
    );
    let pending = make_draft_comment(
        &manifest.review_id, &session, "c-pending",
        Some("src/lib.rs"), Some(Side::Tip), Some(11),
        false, "needs posting",
    );
    storage.upsert_draft_comment(&repo, &already).await.unwrap();
    storage.upsert_draft_comment(&repo, &pending).await.unwrap();
    seed_mapping_for_kata_comment(
        storage.as_ref(), &repo, &manifest.review_id,
        &already.comment_id, 42, "line_comment",
    ).await;

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    let counts = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    assert_eq!(posts.len(), 1, "only the pending inline should hit GH: {posts:#?}");
    let bundled = &posts[0];
    assert!(bundled.endpoint.contains("/pulls/42/reviews"));
    let inlines = bundled.body["comments"].as_array().unwrap();
    assert_eq!(inlines.len(), 1);
    assert_eq!(inlines[0]["body"], "needs posting");
    assert_eq!(counts.skipped_already_published, 1);
}

#[tokio::test]
async fn retry_skips_left_inline_already_in_github_comment_map() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    let already = make_draft_comment(
        &manifest.review_id, &session, "c-left-already",
        Some("src/lib.rs"), Some(Side::Base), Some(7),
        false, "already posted",
    );
    let pending = make_draft_comment(
        &manifest.review_id, &session, "c-left-pending",
        Some("src/lib.rs"), Some(Side::Base), Some(9),
        false, "needs posting",
    );
    storage.upsert_draft_comment(&repo, &already).await.unwrap();
    storage.upsert_draft_comment(&repo, &pending).await.unwrap();
    seed_mapping_for_kata_comment(
        storage.as_ref(), &repo, &manifest.review_id,
        &already.comment_id, 42, "line_comment",
    ).await;

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    let counts = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    let left_posts: Vec<_> = posts
        .iter()
        .filter(|p| p.endpoint.contains("/pulls/42/comments") && p.body["side"] == "LEFT")
        .collect();
    assert_eq!(left_posts.len(), 1, "only the pending LEFT should land");
    assert_eq!(left_posts[0].body["body"], "needs posting");
    assert_eq!(counts.skipped_already_published, 1);
}

#[tokio::test]
async fn retry_skips_issue_comment_already_in_github_comment_map() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    let already = make_draft_comment(
        &manifest.review_id, &session, "c-rw-already",
        None, None, None, true, "already posted",
    );
    let pending = make_draft_comment(
        &manifest.review_id, &session, "c-rw-pending",
        None, None, None, true, "needs posting",
    );
    storage.upsert_draft_comment(&repo, &already).await.unwrap();
    storage.upsert_draft_comment(&repo, &pending).await.unwrap();
    seed_mapping_for_kata_comment(
        storage.as_ref(), &repo, &manifest.review_id,
        &already.comment_id, 42, "issue_comment",
    ).await;

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    let counts = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    let issue_posts: Vec<_> = posts
        .iter()
        .filter(|p| p.endpoint.contains("/issues/42/comments"))
        .collect();
    assert_eq!(issue_posts.len(), 1, "only the pending issue comment should land");
    assert_eq!(issue_posts[0].body["body"], "needs posting");
    assert_eq!(counts.skipped_already_published, 1);
}

#[tokio::test]
async fn retry_skips_reply_already_in_github_comment_map() {
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");
    let parent =
        seed_imported_parent_with_mapping(storage.as_ref(), &repo, &manifest, 12345).await;

    let already_reply = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_core::ResponseId::new("r-already"),
        in_reply_to: parent.comment_id.clone(),
        session_id: session.clone(),
        author: author.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Comment,
        body: "already posted".into(),
    };
    let pending_reply = Response {
        schema_version: SCHEMA_VERSION,
        response_id: kata_core::ResponseId::new("r-pending"),
        in_reply_to: parent.comment_id.clone(),
        session_id: session.clone(),
        author: author.clone(),
        created_at: Utc::now(),
        action: ResolutionAction::Comment,
        body: "needs posting".into(),
    };
    storage.upsert_draft_response(&repo, &already_reply).await.unwrap();
    storage.upsert_draft_response(&repo, &pending_reply).await.unwrap();
    // Seed a mapping row for the already-published reply.
    storage
        .insert_github_comment_mapping(
            &repo,
            &GithubCommentMapping {
                review_id: manifest.review_id.clone(),
                kata_comment_id: None,
                kata_response_id: Some(already_reply.response_id.clone()),
                github_node_id: "PRE_r_already".into(),
                github_rest_id: Some(99),
                pr_number: 42,
                kind: "thread_reply".into(),
                thread_node_id: Some("MDExTH1".into()),
            },
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    let counts = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment, None,
    )
    .await
    .unwrap();

    let posts = fake.posts();
    assert_eq!(posts.len(), 1, "only the pending reply should land: {posts:#?}");
    assert_eq!(posts[0].body["body"], "needs posting");
    assert_eq!(posts[0].body["in_reply_to"], 12345);
    assert_eq!(counts.skipped_already_published, 1);
}

async fn seed_review_body_mapping(
    storage: &SqliteStorage,
    repo: &RepoId,
    review_id: &ReviewId,
    pr_number: u32,
) {
    storage
        .insert_github_comment_mapping(
            repo,
            &GithubCommentMapping {
                review_id: review_id.clone(),
                kata_comment_id: None,
                kata_response_id: None,
                github_node_id: "PRE_REVIEW_BODY".into(),
                github_rest_id: Some(777),
                pr_number,
                kind: "review_body".into(),
                thread_node_id: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn retry_with_body_already_posted_strips_body_from_wrapping_review() {
    // Body landed on a prior attempt → review-body mapping exists.
    // Retry has a new inline draft + the same body. The new inline
    // needs to land, but the body must NOT be re-posted.
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-pending",
                Some("src/lib.rs"), Some(Side::Tip), Some(10),
                false, "new inline",
            ),
        )
        .await
        .unwrap();
    seed_review_body_mapping(storage.as_ref(), &repo, &manifest.review_id, 42).await;

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    let counts = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment,
        Some("body the user re-submitted on retry".into()),
    )
    .await
    .unwrap();

    let posts = fake.posts();
    let wrapping = posts
        .iter()
        .find(|p| p.endpoint.contains("/pulls/42/reviews"))
        .expect("wrapping review still needed for the new inline");
    assert!(
        wrapping.body.get("body").is_none(),
        "body must be stripped on retry; payload was {wrapping:#?}",
    );
    let inlines = wrapping.body["comments"].as_array().unwrap();
    assert_eq!(inlines.len(), 1);
    assert_eq!(inlines[0]["body"], "new inline");
    assert_eq!(counts.skipped_already_published, 1, "body skip is counted");
}

#[tokio::test]
async fn retry_with_everything_already_posted_skips_wrapping_review_entirely() {
    // Both the body AND every inline are mapped. There's no work
    // for /reviews to carry — no wrapping POST should fire.
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    let inline = make_draft_comment(
        &manifest.review_id, &session, "c-already",
        Some("src/lib.rs"), Some(Side::Tip), Some(10),
        false, "already posted",
    );
    storage.upsert_draft_comment(&repo, &inline).await.unwrap();
    seed_mapping_for_kata_comment(
        storage.as_ref(), &repo, &manifest.review_id,
        &inline.comment_id, 42, "line_comment",
    ).await;
    seed_review_body_mapping(storage.as_ref(), &repo, &manifest.review_id, 42).await;

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    let counts = publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment,
        Some("re-submitted body".into()),
    )
    .await
    .unwrap();

    assert!(
        fake.posts().is_empty(),
        "no POSTs expected when everything is mapped: {:#?}",
        fake.posts()
    );
    assert_eq!(counts.skipped_already_published, 2, "inline + body both skipped");
}

#[tokio::test]
async fn wrapping_review_records_review_body_mapping_on_success() {
    // First-attempt publish with a body: /reviews POST succeeds,
    // a review-body mapping must be written so a hypothetical
    // retry would skip the body. (Belt-and-braces — a session
    // that landed cleanly is marked Published and list_drafts_for
    // returns empty on the next call, so retry rarely re-enters
    // section 7 with the same body. But mid-publish failures on
    // a later step can leave the session Draft, and then the
    // body-mapping check is what saves us.)
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment,
        Some("LGTM, ship it".into()),
    )
    .await
    .unwrap();

    // /reviews POST landed; mapping was recorded.
    assert!(
        storage
            .lookup_review_body_mapping(&repo, &manifest.review_id, 42)
            .await
            .unwrap()
            .is_some(),
        "review-body mapping must be written after a successful /reviews POST",
    );
}

#[tokio::test]
async fn deep_shell_fallback_records_review_body_mapping_under_issue_comment() {
    // Bundle 422s, then the empty-comments shell also 422s, so
    // the body lands as an issue comment. That issue comment must
    // get a kind=review_body mapping under its node id — otherwise
    // a future import re-imports it as a ghost of ourselves, and
    // a retry double-posts the body.
    let (storage, repo, manifest, session) = setup().await;
    let author = Author::new("alice");

    storage
        .upsert_draft_comment(
            &repo,
            &make_draft_comment(
                &manifest.review_id, &session, "c-one",
                Some("src/lib.rs"), Some(Side::Tip), Some(7),
                false, "an inline",
            ),
        )
        .await
        .unwrap();

    let fake = Arc::new(FakeGithub::default());
    fake.set_pr(fake_pull_request());
    // Bundle 422 (matches the comments[] entry).
    fake.queue_422(
        "/pulls/42/reviews",
        "\"body\":\"an inline\"",
        "HTTP 422: anchor not in diff",
    );
    // Shell 422 (no comments[] entry now — match on the body alone).
    fake.queue_422(
        "/pulls/42/reviews",
        "\"comments\":[]",
        "HTTP 422: review must have a comment or body — strange edge case",
    );

    publish_session_to_github(
        storage.as_ref(), fake.as_ref(), &repo, &manifest, &session,
        &author, PublishEvent::Comment,
        Some("LGTM with a nit".into()),
    )
    .await
    .unwrap();

    // The body landed as an issue comment and a review-body
    // mapping is recorded against its node id.
    let mapping = storage
        .lookup_review_body_mapping(&repo, &manifest.review_id, 42)
        .await
        .unwrap()
        .expect("review-body mapping should be recorded for the deep-fallback issue comment");
    assert_eq!(mapping.kind, "review_body");
    // The fake assigns node ids prefixed with MDEx; the body
    // landed as the second POST (after the inline retry), so its
    // node id isn't a fixed constant — just verify it's set and
    // dedup against it works.
    assert!(!mapping.github_node_id.is_empty());

    // A second publish call against the same session would skip
    // the body via the mapping check. Verify the mapping is what
    // lookup_review_body_mapping returns.
    assert!(
        storage
            .lookup_review_body_mapping(&repo, &manifest.review_id, 42)
            .await
            .unwrap()
            .is_some(),
    );
}
