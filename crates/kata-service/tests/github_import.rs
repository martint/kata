//! End-to-end tests for the GitHub PR discussion-import path.
//!
//! Lives in its own integration test (not next to `github_publish`)
//! so the import-specific fakes (GraphQL fixture, JjBackend stub
//! that returns no SHA resolutions) don't bloat the publish file.
//! Covers two regression-prone behaviours: import idempotency
//! across pass-1 / pass-2, and reply re-attach via the node-id
//! reverse-lookup branch.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use kata_core::{
    Author, Bookmark, ChangeId, CommitId, CommitInfo, ConflictTerm, FileChange, LogPage,
    OpId, Patchset, RepoId, RevSet, ReviewId, ReviewManifest,
    documents::{GithubPr, RepoManifest, SCHEMA_VERSION},
};
use kata_jj::{Endpoint, GitRemote, JjBackend, ReviewRange};
use kata_service::github::client::{
    AuthStatus, GithubClient, GithubError, GithubResult, PullRequest,
};
use kata_service::github::comments::import_pr_discussion;
use kata_service::github::url::PullRequestRef;
use kata_storage::sqlite::SqliteStorage;
use kata_storage::Storage;
use serde_json::{Value, json};

// ---- Fake GithubClient that returns canned GraphQL data ----------------

#[derive(Default)]
struct ImportFakeGithub {
    /// Successive replies to `graphql_raw`. Each call pops the front
    /// of the queue so the two import passes can return different
    /// PR shapes (e.g. an added reply on the second pass).
    graphql_responses: Mutex<Vec<Value>>,
}

impl ImportFakeGithub {
    fn push_graphql(&self, v: Value) {
        self.graphql_responses.lock().unwrap().push(v);
    }
}

#[async_trait]
impl GithubClient for ImportFakeGithub {
    async fn fetch_pr(&self, _pr: &PullRequestRef) -> GithubResult<PullRequest> {
        unimplemented!("import path does not call fetch_pr")
    }

    async fn graphql_raw(&self, _q: &str, _v: Value) -> GithubResult<Value> {
        let mut q = self.graphql_responses.lock().unwrap();
        if q.is_empty() {
            return Err(GithubError::Parse(
                "fake: no graphql response queued".into(),
            ));
        }
        Ok(q.remove(0))
    }

    async fn auth_status(&self) -> GithubResult<AuthStatus> {
        unimplemented!()
    }

    async fn get_raw(&self, _endpoint: &str) -> GithubResult<Value> {
        unimplemented!("import path does not call get_raw")
    }

    async fn post_raw(&self, _endpoint: &str, _body: &Value) -> GithubResult<Value> {
        unimplemented!("import path does not POST anything")
    }
}

// ---- Fake JjBackend: minimal stub for import -------------------------

/// The import path only calls `resolve_endpoint(sha)`. Returning
/// `None` makes every thread degrade to the patchset-1 tip anchor
/// (per `review_first_patchset_anchor`), which is a valid path and
/// keeps the fixture small. Every other method is `unimplemented`
/// — if import starts using anything else, the test loudly tells us.
struct StubJj;

#[async_trait]
impl JjBackend for StubJj {
    fn repo_path(&self) -> &Path {
        Path::new("/stub")
    }
    async fn list_bookmarks(&self) -> kata_jj::Result<Vec<Bookmark>> {
        unimplemented!()
    }
    async fn change_to_commit(
        &self,
        _change: &ChangeId,
    ) -> kata_jj::Result<Option<CommitId>> {
        unimplemented!()
    }
    async fn resolve_endpoint(&self, _expr: &str) -> kata_jj::Result<Option<Endpoint>> {
        Ok(None)
    }
    async fn read_file(
        &self,
        _commit: &CommitId,
        _path: &str,
    ) -> kata_jj::Result<Option<Vec<u8>>> {
        unimplemented!()
    }
    async fn read_conflict_at(
        &self,
        _commit: &CommitId,
        _path: &str,
    ) -> kata_jj::Result<Option<Vec<ConflictTerm>>> {
        Ok(None)
    }
    async fn changed_files(
        &self,
        _base: &CommitId,
        _tip: &CommitId,
    ) -> kata_jj::Result<Vec<FileChange>> {
        unimplemented!()
    }
    async fn resolve_range(&self, _r: &RevSet) -> kata_jj::Result<ReviewRange> {
        unimplemented!()
    }
    async fn list_commits(&self, _r: &RevSet) -> kata_jj::Result<Vec<CommitInfo>> {
        unimplemented!()
    }
    async fn is_ancestor(
        &self,
        _ancestor: &CommitId,
        _descendant: &CommitId,
    ) -> kata_jj::Result<bool> {
        unimplemented!()
    }
    async fn current_op_id(&self) -> kata_jj::Result<OpId> {
        unimplemented!()
    }
    async fn browse_log(
        &self,
        _revset: &RevSet,
        _max_rows: usize,
    ) -> kata_jj::Result<LogPage> {
        unimplemented!()
    }
    async fn working_copy_commit_id(&self) -> kata_jj::Result<Option<CommitId>> {
        unimplemented!()
    }
    async fn git_remotes(&self) -> kata_jj::Result<Vec<GitRemote>> {
        Ok(Vec::new())
    }
}

// ---- fixtures --------------------------------------------------------

fn fixture_pr_with_one_of_each() -> Value {
    // One issue comment, one review summary, one thread with one
    // reply. Each `id` is the GraphQL node id used as the dedup key.
    json!({
        "repository": {
            "pullRequest": {
                "comments": {
                    "nodes": [{
                        "id": "IC_1",
                        "databaseId": 101,
                        "body": "top-level conversation comment",
                        "createdAt": "2026-06-20T10:00:00Z",
                        "author": {"login": "bob", "databaseId": 7,
                                   "avatarUrl": null, "url": null},
                    }],
                    "pageInfo": {"hasNextPage": false},
                },
                "reviews": {
                    "nodes": [{
                        "id": "REV_1",
                        "databaseId": 201,
                        "body": "LGTM with a nit",
                        "state": "APPROVED",
                        "submittedAt": "2026-06-20T11:00:00Z",
                        "author": {"login": "bob", "databaseId": 7,
                                   "avatarUrl": null, "url": null},
                    }],
                    "pageInfo": {"hasNextPage": false},
                },
                "reviewThreads": {
                    "nodes": [{
                        "id": "TH_1",
                        "isResolved": false,
                        "isOutdated": false,
                        "path": "src/lib.rs",
                        "line": 12,
                        "originalLine": 12,
                        "startLine": null,
                        "originalStartLine": null,
                        "diffSide": "RIGHT",
                        "comments": {
                            "nodes": [
                                {
                                    "id": "TC_1",
                                    "databaseId": 301,
                                    "body": "anchor comment",
                                    "createdAt": "2026-06-20T12:00:00Z",
                                    "originalCommit": {"oid": "headsha"},
                                    "commit": {"oid": "headsha"},
                                    "replyTo": null,
                                    "author": {"login": "bob", "databaseId": 7,
                                               "avatarUrl": null, "url": null},
                                },
                                {
                                    "id": "TC_2",
                                    "databaseId": 302,
                                    "body": "first reply",
                                    "createdAt": "2026-06-20T12:30:00Z",
                                    "originalCommit": {"oid": "headsha"},
                                    "commit": {"oid": "headsha"},
                                    "replyTo": {"id": "TC_1"},
                                    "author": {"login": "carol", "databaseId": 8,
                                               "avatarUrl": null, "url": null},
                                },
                            ],
                            "pageInfo": {"hasNextPage": false},
                        },
                    }],
                    "pageInfo": {"hasNextPage": false},
                },
            }
        }
    })
}

fn fixture_with_added_reply() -> Value {
    // Same as the base fixture but the thread carries an additional
    // reply (`TC_3`). The first two nodes (anchor + first reply) are
    // already mapped from pass 1 — only `TC_3` should land as new.
    let mut v = fixture_pr_with_one_of_each();
    let comments = v["repository"]["pullRequest"]["reviewThreads"]["nodes"][0]
        ["comments"]["nodes"]
        .as_array_mut()
        .unwrap();
    comments.push(json!({
        "id": "TC_3",
        "databaseId": 303,
        "body": "second reply",
        "createdAt": "2026-06-20T13:00:00Z",
        "originalCommit": {"oid": "headsha"},
        "commit": {"oid": "headsha"},
        // GitHub threads replies under the anchor — every reply's
        // `replyTo` points at the anchor (TC_1), not at the
        // previous reply. The import path's reverse lookup expects
        // this shape.
        "replyTo": {"id": "TC_1"},
        "author": {"login": "dave", "databaseId": 9,
                   "avatarUrl": null, "url": null},
    }));
    v
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

async fn setup() -> (Arc<SqliteStorage>, RepoId, ReviewManifest) {
    let storage = Arc::new(SqliteStorage::open_in_memory().await.unwrap());
    let repo = RepoId::new("repo-1");
    let manifest = fake_manifest(&ReviewId::new("rev-1"));
    storage
        .ensure_repo(&RepoManifest {
            schema_version: SCHEMA_VERSION,
            repo_id: repo.clone(),
            canonical_path: "/test".into(),
        })
        .await
        .unwrap();
    let stored = storage.create_review(&repo, &manifest).await.unwrap();
    (storage, repo, stored)
}

fn pr_ref() -> PullRequestRef {
    PullRequestRef {
        owner: "octo".into(),
        repo: "repo".into(),
        number: 42,
    }
}

// ---- tests ---------------------------------------------------------

#[tokio::test]
async fn import_is_idempotent_across_two_passes() {
    let (storage, repo, manifest) = setup().await;
    let jj = StubJj;

    let fake = ImportFakeGithub::default();
    // Two passes return the SAME PR shape — pass 2 must insert
    // nothing because every node id is already in the mapping
    // table from pass 1.
    fake.push_graphql(fixture_pr_with_one_of_each());
    fake.push_graphql(fixture_pr_with_one_of_each());

    // Pass 1: everything lands.
    let counts_1 = import_pr_discussion(storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref())
        .await
        .unwrap();
    assert_eq!(counts_1.issue_comments, 1);
    assert_eq!(counts_1.review_summaries, 1);
    assert_eq!(counts_1.threads, 1);
    assert_eq!(counts_1.thread_replies, 1);
    assert_eq!(counts_1.skipped_already_mapped, 0);

    // Mapping rows persisted across the four GH node ids.
    for node in ["IC_1", "REV_1", "TC_1", "TC_2"] {
        assert!(
            storage
                .lookup_github_mapping_by_node_id(&repo, node)
                .await
                .unwrap()
                .is_some(),
            "mapping for {node} should exist after pass 1",
        );
    }

    // Pass 2: zero new inserts. Everything is skipped via the
    // is_github_comment_mapped dedup.
    let counts_2 = import_pr_discussion(storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref())
        .await
        .unwrap();
    assert_eq!(counts_2.issue_comments, 0, "pass 2 must not re-insert issue comments");
    assert_eq!(counts_2.review_summaries, 0, "pass 2 must not re-insert review summaries");
    assert_eq!(counts_2.threads, 0, "pass 2 must not re-insert thread anchors");
    assert_eq!(counts_2.thread_replies, 0, "pass 2 must not re-insert thread replies");
    assert!(
        counts_2.skipped_already_mapped >= 4,
        "all 4 nodes must register as skipped, got {}",
        counts_2.skipped_already_mapped,
    );
}

#[tokio::test]
async fn reply_on_existing_thread_reattaches_via_node_id_lookup() {
    // Second pass sees the same thread plus one new reply
    // (`TC_3`). The new reply must be attached as a kata Response
    // under the parent that was imported on pass 1 — exercised via
    // the lookup_github_mapping_by_node_id reverse lookup that
    // turns the parent's GH node id into the kata comment id.
    let (storage, repo, manifest) = setup().await;
    let jj = StubJj;

    let fake = ImportFakeGithub::default();
    fake.push_graphql(fixture_pr_with_one_of_each());
    fake.push_graphql(fixture_with_added_reply());

    import_pr_discussion(storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref())
        .await
        .unwrap();

    let counts_2 = import_pr_discussion(storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref())
        .await
        .unwrap();
    // Only TC_3 is new — everything else is skipped.
    assert_eq!(counts_2.thread_replies, 1, "exactly one new reply: TC_3");
    assert_eq!(counts_2.issue_comments, 0);
    assert_eq!(counts_2.review_summaries, 0);
    assert_eq!(counts_2.threads, 0);

    // The new reply's mapping row now exists.
    let m_tc3 = storage
        .lookup_github_mapping_by_node_id(&repo, "TC_3")
        .await
        .unwrap()
        .expect("TC_3 should be mapped after pass 2");
    assert_eq!(m_tc3.kind, "thread_reply");
    let kata_response_id = m_tc3
        .kata_response_id
        .expect("TC_3 must be mapped as a kata Response");

    // Verify the response landed under the same anchor comment as
    // TC_2 (i.e. the parent's kata comment from pass 1), not as a
    // dangling top-level item.
    let parent_mapping = storage
        .lookup_github_mapping_by_node_id(&repo, "TC_1")
        .await
        .unwrap()
        .expect("TC_1 anchor should be mapped");
    let parent_kata_id = parent_mapping.kata_comment_id.expect("TC_1 → kata comment");
    let responses = storage
        .list_published_responses(&repo, &manifest.review_id)
        .await
        .unwrap();
    let tc3_response = responses
        .iter()
        .find(|r| r.response_id == kata_response_id)
        .expect("TC_3 should appear in published responses");
    assert_eq!(
        tc3_response.in_reply_to, parent_kata_id,
        "TC_3 must thread under the same anchor as TC_2",
    );
    assert_eq!(tc3_response.body, "second reply");
}

/// Fixture: one thread that's already resolved on GitHub, with a
/// `resolvedBy` user. Drives the synthetic-Resolve-response
/// translation.
fn fixture_with_resolved_thread() -> Value {
    json!({
        "repository": {
            "pullRequest": {
                "comments": {"nodes": [], "pageInfo": {"hasNextPage": false}},
                "reviews": {"nodes": [], "pageInfo": {"hasNextPage": false}},
                "reviewThreads": {
                    "nodes": [{
                        "id": "TH_R",
                        "isResolved": true,
                        "isOutdated": false,
                        "path": "src/lib.rs",
                        "line": 5,
                        "originalLine": 5,
                        "startLine": null,
                        "originalStartLine": null,
                        "diffSide": "RIGHT",
                        "resolvedBy": {"login": "carol", "databaseId": 8,
                                       "avatarUrl": null, "url": null},
                        "comments": {
                            "nodes": [{
                                "id": "TCR_1",
                                "databaseId": 401,
                                "body": "please rename",
                                "createdAt": "2026-06-20T12:00:00Z",
                                "originalCommit": {"oid": "headsha"},
                                "commit": {"oid": "headsha"},
                                "replyTo": null,
                                "author": {"login": "bob", "databaseId": 7,
                                           "avatarUrl": null, "url": null},
                            }],
                            "pageInfo": {"hasNextPage": false},
                        },
                    }],
                    "pageInfo": {"hasNextPage": false},
                },
            }
        }
    })
}

#[tokio::test]
async fn resolved_thread_imports_synthetic_resolve_response() {
    // A GitHub thread with `isResolved: true` should translate into
    // a synthetic kata `Resolve` response on the anchor comment —
    // not a `_(resolved)_` body prefix. The response is authored by
    // the GitHub user who resolved the conversation.
    let (storage, repo, manifest) = setup().await;
    let jj = StubJj;
    let fake = ImportFakeGithub::default();
    fake.push_graphql(fixture_with_resolved_thread());

    let counts = import_pr_discussion(
        storage.as_ref(),
        &jj,
        &fake,
        &repo,
        &manifest,
        &pr_ref(),
    )
    .await
    .unwrap();

    assert_eq!(counts.threads, 1);
    assert_eq!(counts.thread_resolutions, 1);

    // Anchor body has NO _(resolved)_ prefix anymore.
    let anchor_mapping = storage
        .lookup_github_mapping_by_node_id(&repo, "TCR_1")
        .await
        .unwrap()
        .expect("anchor should be mapped");
    let anchor_id = anchor_mapping.kata_comment_id.expect("anchor → kata id");
    let anchor = storage
        .get_comment_by_id(&repo, &anchor_id)
        .await
        .unwrap()
        .expect("anchor comment should exist");
    assert_eq!(
        anchor.body, "please rename",
        "resolved threads should no longer carry a body prefix",
    );

    // Synthetic resolve response landed under the anchor, attributed
    // to carol (the GitHub `resolvedBy` user).
    let resolutions = storage
        .list_published_responses(&repo, &manifest.review_id)
        .await
        .unwrap();
    let synth = resolutions
        .iter()
        .find(|r| matches!(r.action, kata_core::ResolutionAction::Resolve))
        .expect("a synthetic Resolve response should exist");
    assert_eq!(synth.in_reply_to, anchor_id);
    assert_eq!(synth.author.as_str(), "gh:carol");
    assert!(synth.body.is_empty(), "synthetic resolve has no body");

    // Mapping row uses the synthetic node id namespace.
    let synth_node = format!("kata-import-resolution:TH_R");
    assert!(
        storage
            .is_github_comment_mapped(&repo, &synth_node)
            .await
            .unwrap(),
        "resolution mapping should be recorded under the synthetic node id",
    );

    // A second import pass over the same already-resolved thread
    // must not duplicate the synthetic response.
    fake.push_graphql(fixture_with_resolved_thread());
    let counts_2 = import_pr_discussion(
        storage.as_ref(),
        &jj,
        &fake,
        &repo,
        &manifest,
        &pr_ref(),
    )
    .await
    .unwrap();
    assert_eq!(
        counts_2.thread_resolutions, 0,
        "second pass must not re-insert the synthetic Resolve",
    );
}

/// Variant of `fixture_with_resolved_thread` where GitHub didn't
/// surface a `resolvedBy` user — bot resolvers, deleted accounts,
/// and a handful of other edge cases produce this shape. We still
/// want the thread state to translate; the fallback is a
/// `gh:github` ghost author.
fn fixture_with_resolved_thread_no_resolver() -> Value {
    let mut v = fixture_with_resolved_thread();
    v["repository"]["pullRequest"]["reviewThreads"]["nodes"][0]["resolvedBy"] =
        Value::Null;
    v
}

#[tokio::test]
async fn resolved_thread_with_no_resolver_falls_back_to_github_ghost() {
    // When GitHub omits `resolvedBy` (bot, deleted account, etc.),
    // the import still translates the resolution — attribution
    // falls back to a `gh:github` ghost author so the thread state
    // isn't lost.
    let (storage, repo, manifest) = setup().await;
    let jj = StubJj;
    let fake = ImportFakeGithub::default();
    fake.push_graphql(fixture_with_resolved_thread_no_resolver());

    let counts = import_pr_discussion(
        storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref(),
    )
    .await
    .unwrap();
    assert_eq!(counts.thread_resolutions, 1);

    let resolutions = storage
        .list_published_responses(&repo, &manifest.review_id)
        .await
        .unwrap();
    let synth = resolutions
        .iter()
        .find(|r| matches!(r.action, kata_core::ResolutionAction::Resolve))
        .expect("a synthetic Resolve response should exist");
    assert_eq!(
        synth.author.as_str(),
        "gh:github",
        "resolvedBy: null should attribute to the gh:github fallback ghost",
    );
}

fn fixture_with_open_thread() -> Value {
    let mut v = fixture_with_resolved_thread();
    v["repository"]["pullRequest"]["reviewThreads"]["nodes"][0]["isResolved"] =
        Value::Bool(false);
    v["repository"]["pullRequest"]["reviewThreads"]["nodes"][0]["resolvedBy"] =
        Value::Null;
    v
}

#[tokio::test]
async fn thread_flipping_to_resolved_on_a_later_import_pass_writes_the_resolve() {
    // Pass 1 imports an unresolved thread — no synthetic Resolve.
    // Pass 2 sees the same thread but now `isResolved: true` — the
    // already-mapped-anchor branch must notice the flip and write
    // the synthetic Resolve response.
    let (storage, repo, manifest) = setup().await;
    let jj = StubJj;
    let fake = ImportFakeGithub::default();
    fake.push_graphql(fixture_with_open_thread());
    let counts_1 = import_pr_discussion(
        storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref(),
    )
    .await
    .unwrap();
    assert_eq!(counts_1.threads, 1);
    assert_eq!(counts_1.thread_resolutions, 0);

    // Now the same thread comes back resolved.
    fake.push_graphql(fixture_with_resolved_thread());
    let counts_2 = import_pr_discussion(
        storage.as_ref(), &jj, &fake, &repo, &manifest, &pr_ref(),
    )
    .await
    .unwrap();
    assert_eq!(
        counts_2.threads, 0,
        "anchor already mapped, no new thread",
    );
    assert_eq!(
        counts_2.thread_resolutions, 1,
        "the flip to isResolved:true must translate on the second pass",
    );

    // Attribution came from `resolvedBy` on pass 2, not the fallback.
    let resolutions = storage
        .list_published_responses(&repo, &manifest.review_id)
        .await
        .unwrap();
    let synth = resolutions
        .iter()
        .find(|r| matches!(r.action, kata_core::ResolutionAction::Resolve))
        .expect("a synthetic Resolve response should exist after the flip");
    assert_eq!(synth.author.as_str(), "gh:carol");
}
