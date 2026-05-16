use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kata_core::{Author, RepoManifest, SCHEMA_VERSION};
use kata_jj::JjCli;
use kata_server::{AppState, ReviewService, router};
use kata_storage::sqlite::SqliteStorage;
use kata_storage::{Storage, compute_repo_id, jj_repo_canonical_path};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

struct Harness {
    _storage_root: TempDir,
    _workspace: TempDir,
    workspace_path: std::path::PathBuf,
    router: Router,
}

impl Harness {
    async fn new() -> Self {
        let workspace = TempDir::new().unwrap();
        let storage_root = TempDir::new().unwrap();
        run_jj(workspace.path(), &["git", "init", "."]);

        // Two commits, with the second on a bookmark.
        std::fs::write(workspace.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        run_jj(workspace.path(), &["describe", "-m", "initial"]);
        run_jj(workspace.path(), &["new", "-m", "tweak"]);
        std::fs::write(workspace.path().join("a.txt"), "one\nTWO\nthree\n").unwrap();
        run_jj(workspace.path(), &["bookmark", "create", "feature", "-r", "@"]);

        let canonical = jj_repo_canonical_path(workspace.path()).unwrap();
        let repo_id = compute_repo_id(&canonical);
        let storage = Arc::new(
            SqliteStorage::open(storage_root.path().join("kata.db"))
                .await
                .unwrap(),
        );
        storage
            .ensure_repo(&RepoManifest {
                schema_version: SCHEMA_VERSION,
                repo_id: repo_id.clone(),
                canonical_path: canonical.to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let jj = Arc::new(JjCli::new(workspace.path().to_path_buf()));
        let mut builder = ReviewService::builder(storage.clone());
        builder
            .add_repo(
                "main".into(),
                repo_id.clone(),
                canonical.to_string_lossy().into_owned(),
                jj,
            )
            .unwrap();
        let service = Arc::new(builder.build());
        let state = AppState {
            service,
            default_author: Author::new("alice@example.com"),
        };
        let router = router(state);
        Self {
            _storage_root: storage_root,
            workspace_path: workspace.path().to_path_buf(),
            _workspace: workspace,
            router,
        }
    }

    async fn json(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(serde_json::to_vec(&v).unwrap())
            }
            None => Body::empty(),
        };
        let resp = self
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&bytes).into_owned())
            })
        };
        (status, value)
    }
}

fn run_jj(cwd: &Path, args: &[&str]) {
    let status = StdCommand::new("jj")
        .args(args)
        .current_dir(cwd)
        .env("JJ_USER", "Tester")
        .env("JJ_EMAIL", "test@example.com")
        .status()
        .expect("jj");
    assert!(status.success(), "jj {:?} failed", args);
}

#[tokio::test]
async fn full_review_lifecycle_over_http() {
    let h = Harness::new().await;
    assert!(h.workspace_path.exists());

    // List bookmarks.
    let (status, value) = h.json("GET", "/api/repos/main/bookmarks", None).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, vec!["feature"]);

    // Create a review against `feature`. The URL identifier is the
    // server-assigned per-repo `number`; the response carries it
    // alongside the human `name`.
    let create_body = json!({
        "name": "feature",
        "revset": "@-..feature",
        "bookmark": "feature",
        "created_by": "alice@example.com",
    });
    let (status, value) = h
        .json("POST", "/api/repos/main/reviews", Some(create_body))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["name"], "feature");
    let review_number = value["number"].as_u64().unwrap();
    assert_eq!(review_number, 1);
    let review_url = format!("/api/repos/main/reviews/{review_number}");

    // Listing returns it.
    let (status, value) = h.json("GET", "/api/repos/main/reviews", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value.as_array().unwrap().len(), 1);

    // Open it — gets diff and (empty) comments.
    let (status, view) = h.json("GET", &review_url, None).await;
    assert_eq!(status, StatusCode::OK);
    let file_paths: Vec<&str> = view["diff"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(file_paths, vec!["a.txt"]);
    let current_n = view["manifest"]["current_patchset"].as_u64().unwrap();
    let current = view["manifest"]["patchsets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["n"].as_u64() == Some(current_n))
        .unwrap();
    let anchor_change = current["tip_change"].as_str().unwrap().to_owned();
    let anchor_commit = current["tip_commit"].as_str().unwrap().to_owned();
    assert!(view["comments"].as_array().unwrap().is_empty());
    assert!(view["drafts"]["comments"].as_array().unwrap().is_empty());

    // Start a session.
    let (status, session) = h
        .json("POST", &format!("{review_url}/sessions"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let session_id = session["session_id"].as_str().unwrap().to_owned();
    assert_eq!(session["status"], "draft");

    // Draft a line comment.
    let comment_body = json!({
        "anchor_change_id": anchor_change,
        "anchor_commit_id": anchor_commit,
        "file": "a.txt",
        "side": "tip",
        "lines": {"start": 2, "end": 2},
        "flag": "must-do",
        "body": "Please lowercase this.\n",
    });
    let (status, comment) = h
        .json(
            "POST",
            &format!("{review_url}/sessions/{session_id}/comments"),
            Some(comment_body),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = comment["comment_id"].as_str().unwrap().to_owned();
    assert_eq!(comment["body"], "Please lowercase this.\n");

    // Drafts visible only to author until publish.
    let (_, view) = h.json("GET", &review_url, None).await;
    assert_eq!(view["drafts"]["comments"].as_array().unwrap().len(), 1);
    assert!(view["comments"].as_array().unwrap().is_empty());

    // Publish.
    let (status, _) = h
        .json(
            "POST",
            &format!("{review_url}/sessions/{session_id}/publish"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Now visible in published list, and drafts cleared.
    let (_, view) = h.json("GET", &review_url, None).await;
    assert_eq!(view["comments"].as_array().unwrap().len(), 1);
    assert_eq!(view["comments"][0]["comment_id"], comment_id);
    assert!(view["drafts"]["comments"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn second_review_on_same_branch_gets_next_number() {
    // Two reviews can share a bookmark / name — each gets its own
    // per-repo `number` and so its own URL. This is the property the
    // number scheme exists for: a follow-up review on a branch that
    // already has one (active or archived) doesn't collide.
    let h = Harness::new().await;
    let body = json!({
        "name": "feature",
        "revset": "@-..feature",
        "bookmark": "feature",
        "created_by": "alice@example.com",
    });
    let (status, first) = h.json("POST", "/api/repos/main/reviews", Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["number"].as_u64().unwrap(), 1);
    let (status, second) = h.json("POST", "/api/repos/main/reviews", Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["number"].as_u64().unwrap(), 2);
    assert_ne!(first["review_id"], second["review_id"]);
}

#[tokio::test]
async fn writes_after_publish_are_rejected() {
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );
    let (_, session) = h
        .json("POST", &format!("{review_url}/sessions"), None)
        .await;
    let session_id = session["session_id"].as_str().unwrap().to_owned();
    let (_, view) = h.json("GET", &review_url, None).await;
    let current_n = view["manifest"]["current_patchset"].as_u64().unwrap();
    let current = view["manifest"]["patchsets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["n"].as_u64() == Some(current_n))
        .unwrap();
    let anchor_change = current["tip_change"].as_str().unwrap().to_owned();
    let anchor_commit = current["tip_commit"].as_str().unwrap().to_owned();

    // Draft + publish.
    let (status, _) = h
        .json(
            "POST",
            &format!("{review_url}/sessions/{session_id}/comments"),
            Some(json!({
                "anchor_change_id": anchor_change,
                "anchor_commit_id": anchor_commit,
                "flag": "question",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let _ = h
        .json(
            "POST",
            &format!("{review_url}/sessions/{session_id}/publish"),
            None,
        )
        .await;

    // Attempting another draft in the same (now published) session fails.
    let (status, value) = h
        .json(
            "POST",
            &format!("{review_url}/sessions/{session_id}/comments"),
            Some(json!({
                "anchor_change_id": anchor_change,
                "anchor_commit_id": anchor_commit,
                "flag": "question",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(value["error"].as_str().unwrap().contains("published"));
}

#[tokio::test]
async fn x_review_author_header_overrides_default_identity() {
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // Bob starts a session using the header.
    let req = Request::builder()
        .method("POST")
        .uri(format!("{review_url}/sessions"))
        .header("x-review-author", "bob@example.com")
        .body(Body::empty())
        .unwrap();
    let resp = h.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let session: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(session["author"], "bob@example.com");
}

#[tokio::test]
async fn archive_round_trip_and_blocks_new_sessions() {
    // Archive flips the manifest's `archived_at`, blocks new sessions
    // (the home-screen Archive button's contract), and unarchive
    // restores writes. Non-creators can't archive.
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // Non-creator can't archive — server-side gate, not just UI.
    let req = Request::builder()
        .method("POST")
        .uri(format!("{review_url}/archive"))
        .header("x-review-author", "bob@example.com")
        .body(Body::empty())
        .unwrap();
    let resp = h.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Creator archives. The returned manifest carries archived_at.
    let (status, archived) = h
        .json("POST", &format!("{review_url}/archive"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(archived["archived_at"].is_string());

    // New session is rejected while archived.
    let (status, value) = h.json("POST", &format!("{review_url}/sessions"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(value["error"].as_str().unwrap().contains("archived"));

    // Unarchive clears the timestamp and re-enables writes.
    let (status, restored) = h
        .json("DELETE", &format!("{review_url}/archive"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(restored["archived_at"].is_null());
    let (status, _) = h.json("POST", &format!("{review_url}/sessions"), None).await;
    assert_eq!(status, StatusCode::OK);
}

/// `refresh_review` should record the moved bookmark as a new patchset
/// rather than just rewriting the current one. Locks in the contract
/// the comment-anchoring story relies on (comments stay anchored to
/// the patchset they were authored against — that only works if each
/// round is a distinct entry in the manifest).
#[tokio::test]
async fn refresh_review_appends_a_new_patchset_when_the_tip_moves() {
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // Amend the feature commit so the bookmark points at a new commit
    // ID. Refresh should pick that up and create PS2.
    std::fs::write(h.workspace_path.join("a.txt"), "one\nTwo\nThree\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);

    let (status, manifest) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let patchsets = manifest["patchsets"].as_array().unwrap();
    assert_eq!(patchsets.len(), 2);
    assert_eq!(manifest["current_patchset"], 2);
    assert_ne!(patchsets[0]["tip_commit"], patchsets[1]["tip_commit"]);
}

/// Compare-mode regression: `GET /file-diff?ps=N&compare=M` must
/// return hunks computed against patchset M's tip (not patchset N's
/// base). The frontend's syntax-highlight pass indexes
/// `highlightsBase` by line numbers from this side of the diff — if
/// the backend returned the wrong base, removed-side rows would
/// render with HTML pulled from a file that has nothing to do with
/// the diff (a real bug the UI hit on the named-args review at one
/// point).
#[tokio::test]
async fn file_diff_in_compare_mode_uses_compared_patchset_tip_as_base() {
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // PS1 ships `TWO` on line 2 (set up by the harness). Amend so PS2
    // has `Two` on line 2 and `Three` on line 3 — that way the
    // PS1→PS2 compare diff has a clear, easy-to-assert shape.
    std::fs::write(h.workspace_path.join("a.txt"), "one\nTwo\nThree\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);
    let (status, manifest) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let ps1_tip = manifest["patchsets"][0]["tip_commit"]
        .as_str()
        .unwrap()
        .to_owned();
    let ps2_tip = manifest["patchsets"][1]["tip_commit"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(ps1_tip, ps2_tip);

    // Compare PS2 (selected) against PS1 (compare-with).
    let (status, change) = h
        .json(
            "GET",
            &format!("{review_url}/file-diff?path=a.txt&ps=2&compare=1"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let hunks = change["hunks"].as_array().expect("hunks");
    assert_eq!(hunks.len(), 1, "one hunk for the case-flip changes");
    let lines = hunks[0]["lines"].as_array().unwrap();
    // Walk the hunk and collect (origin, base_line, tip_line, content)
    // tuples for the removed/added rows so the assertions read in
    // diff order.
    let removed: Vec<(u64, String)> = lines
        .iter()
        .filter(|l| l["origin"] == "removed")
        .map(|l| {
            (
                l["base_line"].as_u64().unwrap(),
                l["content"].as_str().unwrap().trim_end().to_owned(),
            )
        })
        .collect();
    let added: Vec<(u64, String)> = lines
        .iter()
        .filter(|l| l["origin"] == "added")
        .map(|l| {
            (
                l["tip_line"].as_u64().unwrap(),
                l["content"].as_str().unwrap().trim_end().to_owned(),
            )
        })
        .collect();
    // The removed-side content must match PS1.tip's lines, not the
    // bookmark's base. PS1.tip had `TWO` + `three`; PS2.tip has
    // `Two` + `Three`. If the backend mistakenly diffed against
    // PS2's base, we'd see different removed-side content (and
    // different counts).
    assert_eq!(
        removed,
        vec![(2, "TWO".to_string()), (3, "three".to_string())],
        "removed rows should index into PS1.tip and carry its content"
    );
    assert_eq!(
        added,
        vec![(2, "Two".to_string()), (3, "Three".to_string())],
        "added rows should index into PS2.tip and carry its content"
    );
}

/// `open_review`'s `compare` query parameter feeds the same compare
/// pipeline as `file-diff`. Make sure its metadata response (the
/// file list with per-file +/- counts) reflects the compared-patchset
/// diff, not the patchset-base diff.
#[tokio::test]
async fn open_review_in_compare_mode_reports_compared_patchset_diff_metadata() {
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );
    std::fs::write(h.workspace_path.join("a.txt"), "one\nTwo\nThree\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);
    let (_, _) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;

    // Non-compare PS2 view: a.txt has 3 changes from the base
    // (one→one, two→Two, three→Three — actually just the case flips
    // on lines 2/3, so 2 additions + 2 removals against the
    // single-line-mid-case bookmark base).
    let (_, view) = h.json("GET", &format!("{review_url}?ps=2"), None).await;
    let plain_added = view["diff"]["files"][0]["added"].as_u64().unwrap();
    let plain_removed = view["diff"]["files"][0]["removed"].as_u64().unwrap();

    // Compare against PS1: PS1.tip had `TWO` + `three`, PS2.tip has
    // `Two` + `Three`. That's a 2-line case-flip — strictly smaller
    // than the bookmark-base diff (which also flips line 2 from
    // `two` to whatever).
    let (_, compare_view) = h
        .json("GET", &format!("{review_url}?ps=2&compare=1"), None)
        .await;
    let compare_added = compare_view["diff"]["files"][0]["added"].as_u64().unwrap();
    let compare_removed = compare_view["diff"]["files"][0]["removed"].as_u64().unwrap();
    assert_eq!(compare_added, 2);
    assert_eq!(compare_removed, 2);
    // The plain view counts should be at least as large as compare's
    // (the bookmark base differs from PS1.tip too), and at minimum
    // different — if compare were silently falling back to the
    // plain view, both would be identical.
    assert!(plain_added >= compare_added);
    assert!(plain_removed >= compare_removed);
}

#[tokio::test]
async fn first_open_records_a_visit_baseline_with_no_history() {
    // On the very first open for a given (review, viewer) pair the
    // service records the current jj op as the baseline but cannot
    // produce a "since you were here" list yet — so `last_visit_at`
    // is null and `ops_since` is empty.
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    let (status, view) = h.json("GET", &review_url, None).await;
    assert_eq!(status, StatusCode::OK);
    // Both fields are `#[serde(skip_serializing_if = ...)]`, so on a
    // truly-empty first open they're missing from the JSON entirely.
    // `Value::Null` is what indexing a missing key returns, which is
    // what we expect here.
    assert!(view["last_visit_at"].is_null());
    assert!(view["ops_since"].is_null());
}

#[tokio::test]
async fn second_open_reports_last_visit_at_and_ops_landed_since() {
    // Open the review once to set a baseline, perform a jj operation
    // (a `describe` against the bookmark tip), then open again — the
    // second open must surface a non-null `last_visit_at` and the
    // intervening op in `ops_since`.
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // Baseline visit.
    let (_, first) = h.json("GET", &review_url, None).await;
    assert!(first["last_visit_at"].is_null());

    // Intervening jj op: rewrite the bookmark's description.
    run_jj(
        &h.workspace_path,
        &["describe", "-r", "feature", "-m", "tweak (edited)"],
    );

    let (_, second) = h.json("GET", &review_url, None).await;
    assert!(
        second["last_visit_at"].is_string(),
        "expected last_visit_at to be set on second open, got {:?}",
        second["last_visit_at"],
    );
    let ops = second["ops_since"].as_array().unwrap();
    assert!(
        !ops.is_empty(),
        "expected ops_since to contain the intervening describe, got empty",
    );
}

#[tokio::test]
async fn ops_since_stays_empty_when_nothing_happened_between_visits() {
    // Two back-to-back opens with no jj activity in between: the
    // second open should see `last_visit_at` populated (so the UI
    // knows there was a prior visit) but `ops_since` empty — there
    // is genuinely nothing to surface.
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    let (_, _) = h.json("GET", &review_url, None).await;
    let (_, second) = h.json("GET", &review_url, None).await;
    assert!(second["last_visit_at"].is_string());
    // `ops_since` is `#[serde(skip_serializing_if = "Vec::is_empty")]`,
    // so an empty list is omitted entirely — index returns Null.
    assert!(second["ops_since"].is_null());
}

#[tokio::test]
async fn compare_patchsets_returns_pair_list_and_cumulative_diff() {
    // End-to-end smoke for the patchset-compare v2 endpoint. We don't
    // exhaustively cover the bucketing logic here (`compare_tests` in
    // kata-service does that as unit tests against the helper) — this
    // is just "the route exists, JSON shape is what the frontend
    // expects, and the cumulative diff matches what `?compare=` would
    // have returned via open_review".
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );
    // Build PS2 by amending the bookmark tip.
    std::fs::write(h.workspace_path.join("a.txt"), "one\nTwo\nThree\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);
    let (_, _) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;

    let (status, view) = h
        .json("GET", &format!("{review_url}/compare?from=1&to=2"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["from"]["n"], 1);
    assert_eq!(view["to"]["n"], 2);
    // Same base for both patchsets in the harness setup, so the
    // mismatch flag is false.
    assert_eq!(view["compare_base_mismatch"], false);
    // The single feature commit appears in both patchsets — same
    // change-id, different commit-id (we rewrote the tip).
    let pairs = view["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0]["status"], "changed");
    assert!(pairs[0]["from_commit"].is_string());
    assert!(pairs[0]["to_commit"].is_string());
    assert_ne!(pairs[0]["from_commit"], pairs[0]["to_commit"]);

    // Cumulative diff covers the single file the rewrite touched.
    let files = view["cumulative"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "a.txt");
}

#[tokio::test]
async fn compare_patchsets_rejects_self_comparison() {
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );
    let (status, _) = h
        .json("GET", &format!("{review_url}/compare?from=1&to=1"), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compare_patchsets_populates_parent_commit_for_one_sided_pairs() {
    // Construct a PS1 with two commits and a PS2 that drops the top
    // commit. The dropped commit becomes a `removed-from-from` pair,
    // and the service must stamp its `parent_commit` so the UI can
    // render the dropped commit's own diff against its parent. The
    // surviving commit is `same` and shouldn't have a parent attached.
    let h = Harness::new().await;

    // The harness starts on `feature` (a single commit). Stack one
    // more commit on top and re-point the bookmark to it; that's PS1.
    run_jj(&h.workspace_path, &["new", "-m", "top of stack"]);
    std::fs::write(h.workspace_path.join("b.txt"), "alpha\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);

    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@--..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // Drop the top commit → bookmark moves back to its parent → PS2
    // contains only the bottom commit. `--allow-backwards` because
    // jj refuses to retreat a bookmark by default.
    run_jj(
        &h.workspace_path,
        &["bookmark", "set", "feature", "-r", "@-", "--allow-backwards"],
    );
    let (_, _) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;

    let (status, view) = h
        .json("GET", &format!("{review_url}/compare?from=1&to=2"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let pairs = view["pairs"].as_array().unwrap();
    // The pair list should have at least one `removed-from-from` and
    // one `same`. (Depending on how jj resolves the revsets there
    // could be more, but those two have to be present.)
    let removed = pairs
        .iter()
        .find(|p| p["status"] == "removed-from-from")
        .expect("expected a removed-from-from pair");
    let same = pairs.iter().find(|p| p["status"] == "same");

    // Removed-side: parent_commit must be populated so the UI can
    // render the commit's own diff against its parent.
    assert!(
        removed["parent_commit"].is_string(),
        "removed pair should have parent_commit populated, got {removed:?}",
    );
    // Same-side: nothing for the UI to fetch, so parent_commit should
    // be absent from the response (skip_serializing_if elides it).
    if let Some(same) = same {
        assert!(
            same["parent_commit"].is_null(),
            "same pair should not have parent_commit, got {same:?}",
        );
    }
}

#[tokio::test]
async fn diff_commits_returns_metadata_without_path_and_file_with_path() {
    // The generic commit-pair diff endpoint backs the per-commit
    // interdiff view. Verify both shapes: no `path` returns file-level
    // metadata; a `path` returns the hunks for that one file.
    let h = Harness::new().await;
    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@-..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let (_, view) = h
        .json(
            "GET",
            &format!(
                "/api/repos/main/reviews/{}",
                created["number"].as_u64().unwrap()
            ),
            None,
        )
        .await;
    let current_n = view["manifest"]["current_patchset"].as_u64().unwrap();
    let ps = view["manifest"]["patchsets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["n"].as_u64() == Some(current_n))
        .unwrap();
    let base = ps["base_commit"].as_str().unwrap();
    let tip = ps["tip_commit"].as_str().unwrap();

    let metadata_uri = format!("/api/repos/main/diff?from={base}&to={tip}");
    let (status, meta) = h.json("GET", &metadata_uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["kind"], "diff");
    let files = meta["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "a.txt");
    // No hunks at the metadata level — they ship lazily.
    assert!(files[0]["hunks"].is_null());

    let file_uri = format!("/api/repos/main/diff?from={base}&to={tip}&path=a.txt");
    let (status, file) = h.json("GET", &file_uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(file["kind"], "file");
    assert_eq!(file["path"], "a.txt");
    // Hunks now populated.
    assert!(file["hunks"].as_array().unwrap().len() > 0);
}

/// Regression test for the libjj rebased-interdiff path. Builds a
/// 2-commit stack, rewrites the BOTTOM commit's content between
/// patchsets, and verifies:
///
/// - The top commit's pair classification is `changed` (commit-id
///   differs because its parent changed), but the libjj-computed
///   `diff_counts.file_count` is 0 — the commit didn't contribute
///   new changes itself.
/// - The literal `/diff` (no `interdiff` flag) for the top pair
///   returns a non-empty file list — it's a tree-vs-tree diff that
///   includes the inherited downstream delta.
/// - The libjj `/diff?interdiff=true` for the same pair returns 0
///   files — the rebase-based interdiff correctly identifies the
///   commit as a pure rebase.
///
/// This is the exact bug the libjj integration was added to fix
/// (review #1 PS4→PS5 in the trino corpus): downstream-of-rewrite
/// commits all reported identical large diffs via the naive path.
#[tokio::test]
async fn libjj_interdiff_returns_zero_for_pure_rebase_pairs() {
    let h = Harness::new().await;

    // Stack a second commit on top of `tweak`. PS1 = (tweak, top).
    run_jj(&h.workspace_path, &["new", "-m", "top of stack"]);
    std::fs::write(h.workspace_path.join("b.txt"), "alpha\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);

    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@--..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    // Rewrite the BOTTOM commit (`tweak`, currently `feature-`) by
    // editing it and changing a.txt. jj auto-rebases the top commit
    // onto the rewritten bottom, giving it a new commit-id.
    run_jj(&h.workspace_path, &["edit", "feature-"]);
    std::fs::write(
        h.workspace_path.join("a.txt"),
        "one\nREWRITTEN-TWO\nthree\n",
    )
    .unwrap();
    // Move @ to the rebased top, then point the bookmark there. The
    // rebased top is @+ (the auto-rebased child of the rewritten
    // bottom). `--allow-backwards` because the bookmark was at the
    // now-abandoned original top.
    run_jj(&h.workspace_path, &["edit", "@+"]);
    run_jj(
        &h.workspace_path,
        &["bookmark", "set", "feature", "-r", "@", "--allow-backwards"],
    );
    let (_, _) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;

    // /compare returns the pair list with libjj-computed
    // diff_counts for `changed` rows. Two pairs: the bottom
    // (real content change) and the top (pure rebase).
    let (status, view) = h
        .json("GET", &format!("{review_url}/compare?from=1&to=2"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let pairs = view["pairs"].as_array().unwrap();
    let changed: Vec<_> = pairs
        .iter()
        .filter(|p| p["status"] == "changed")
        .collect();
    assert_eq!(
        changed.len(),
        2,
        "expected 2 changed pairs, got {pairs:?}"
    );

    // Identify which pair is which by description.
    let bottom_pair = changed
        .iter()
        .find(|p| {
            p["to_description"].as_str() == Some("tweak")
                || p["from_description"].as_str() == Some("tweak")
        })
        .expect("expected a pair with the `tweak` description");
    let top_pair = changed
        .iter()
        .find(|p| {
            p["to_description"].as_str() == Some("top of stack")
                || p["from_description"].as_str() == Some("top of stack")
        })
        .expect("expected a pair with the `top of stack` description");

    let bottom_counts = &bottom_pair["diff_counts"];
    let top_counts = &top_pair["diff_counts"];
    assert!(
        bottom_counts["file_count"].as_u64().unwrap() > 0,
        "bottom pair (real edit) should have non-zero file_count, got {bottom_counts:?}",
    );
    assert_eq!(
        top_counts["file_count"].as_u64().unwrap(),
        0,
        "top pair (pure rebase) should have zero file_count via libjj, got {top_counts:?}",
    );

    // Cross-check at the /diff route: literal commit-to-commit diff
    // for the top pair shows the inherited delta...
    let top_from = top_pair["from_commit"].as_str().unwrap();
    let top_to = top_pair["to_commit"].as_str().unwrap();
    let (_, literal) = h
        .json(
            "GET",
            &format!("/api/repos/main/diff?from={top_from}&to={top_to}"),
            None,
        )
        .await;
    let literal_files = literal["files"].as_array().unwrap();
    assert!(
        !literal_files.is_empty(),
        "literal diff(from, to) for the top pair should be non-empty (includes inherited rewrite), got {literal:?}",
    );

    // ...while the libjj-routed `?interdiff=true` correctly returns
    // an empty file list for the same pair.
    let (_, interdiff) = h
        .json(
            "GET",
            &format!(
                "/api/repos/main/diff?from={top_from}&to={top_to}&interdiff=true"
            ),
            None,
        )
        .await;
    assert_eq!(interdiff["kind"], "diff");
    let interdiff_files = interdiff["files"].as_array().unwrap();
    assert!(
        interdiff_files.is_empty(),
        "libjj interdiff for the top pair (pure rebase) should be empty, got {interdiff:?}",
    );
}

/// Companion to the rebased-only test: confirms the libjj path
/// produces a *non-empty* diff for the pair whose content actually
/// changed, matching the literal diff but not necessarily equal to
/// it (the literal includes inherited downstream changes, the
/// rebased path doesn't — but for the bottom-of-stack pair there's
/// nothing inherited, so they should look similar).
#[tokio::test]
async fn libjj_interdiff_matches_real_edit_at_bottom_of_stack() {
    let h = Harness::new().await;
    run_jj(&h.workspace_path, &["new", "-m", "top of stack"]);
    std::fs::write(h.workspace_path.join("b.txt"), "alpha\n").unwrap();
    run_jj(&h.workspace_path, &["bookmark", "set", "feature", "-r", "@"]);

    let (_, created) = h
        .json(
            "POST",
            "/api/repos/main/reviews",
            Some(json!({
                "name": "feature",
                "revset": "@--..feature",
                "bookmark": "feature",
                "created_by": "alice@example.com",
            })),
        )
        .await;
    let review_url = format!(
        "/api/repos/main/reviews/{}",
        created["number"].as_u64().unwrap()
    );

    run_jj(&h.workspace_path, &["edit", "feature-"]);
    std::fs::write(
        h.workspace_path.join("a.txt"),
        "one\nREWRITTEN-TWO\nthree\n",
    )
    .unwrap();
    run_jj(&h.workspace_path, &["edit", "@+"]);
    run_jj(
        &h.workspace_path,
        &["bookmark", "set", "feature", "-r", "@", "--allow-backwards"],
    );
    let (_, _) = h
        .json("POST", &format!("{review_url}/refresh"), None)
        .await;

    let (_, view) = h
        .json("GET", &format!("{review_url}/compare?from=1&to=2"), None)
        .await;
    let bottom_pair = view["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| {
            p["status"] == "changed"
                && p["to_description"].as_str() == Some("tweak")
        })
        .expect("expected a changed pair for the bottom commit");
    let from = bottom_pair["from_commit"].as_str().unwrap();
    let to = bottom_pair["to_commit"].as_str().unwrap();

    let (_, interdiff) = h
        .json(
            "GET",
            &format!("/api/repos/main/diff?from={from}&to={to}&interdiff=true"),
            None,
        )
        .await;
    let files = interdiff["files"].as_array().unwrap();
    assert_eq!(
        files.len(),
        1,
        "bottom-pair interdiff should touch exactly a.txt, got {interdiff:?}",
    );
    assert_eq!(files[0]["path"], "a.txt");

    // Per-file hunks via the libjj path: requesting `path=a.txt`
    // routes through compute_rebased_file_hunks which populates
    // the `hunks` array.
    let (_, file) = h
        .json(
            "GET",
            &format!(
                "/api/repos/main/diff?from={from}&to={to}&interdiff=true&path=a.txt"
            ),
            None,
        )
        .await;
    assert_eq!(file["kind"], "file");
    assert!(file["hunks"].as_array().unwrap().len() > 0);
}
