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
                "flag": "other",
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
                "flag": "other",
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
