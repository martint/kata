//! Stateless wrapper around the `gh` CLI for kata's GitHub API needs.
//!
//! Why `gh` instead of an in-process OAuth client: kata runs locally
//! for one user; that user already has `gh` set up. The `gh` CLI
//! ships under GitHub's own pre-approved OAuth App, which means it
//! sidesteps the org-policy gauntlet kata's custom OAuth app would
//! otherwise have to clear (SAML SSO authorization, OAuth-App
//! access restrictions, etc.). Trading those off costs us the
//! ability to be multi-tenant — every call attributes to whoever
//! ran `gh auth login` on the host — which is exactly the trade-off
//! we want for a single-user local server.
//!
//! Subprocess cost is fine at our volume (~10 calls per import,
//! ~5 + N per publish). For latency-sensitive bulk operations we
//! can pipe a single `gh api graphql` with batched variables.

use std::io;
use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;

use super::url::PullRequestRef;

/// Errors surfaced by the `gh`-backed client. Far smaller than the
/// HTTP-classified surface we used to have — `gh` itself absorbs
/// SSO, OAuth-app restrictions, and rate-limit handling.
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    /// `gh` binary not on PATH. Tell the user to install GitHub CLI
    /// (<https://cli.github.com/>). Distinct from
    /// [`Self::NotAuthenticated`] so the message can be precise.
    #[error("the `gh` CLI is not installed; install it from https://cli.github.com/")]
    NotInstalled,
    /// `gh` ran but reported it has no credentials. User should
    /// `gh auth login` once and retry.
    #[error("the `gh` CLI is not authenticated; run `gh auth login` in a terminal")]
    NotAuthenticated,
    /// The PR / repo doesn't exist or your gh identity can't see it.
    /// Kata can't tell the two cases apart (GitHub returns 404 for
    /// both for privacy), so the message is intentionally vague.
    #[error("GitHub returned 404: {what}")]
    NotFound { what: String },
    /// 422 from github.com — the request was well-formed but the
    /// API refused it. The publish path treats this as a recoverable
    /// signal (e.g. a LEFT-side inline comment whose line doesn't
    /// line up in the diff at the wrapping commit) and falls back
    /// to posting as an issue comment with file:line context.
    #[error("GitHub returned 422: {stderr}")]
    Validation { stderr: String },
    /// Any other non-zero `gh` exit. The stderr is propagated
    /// verbatim — `gh`'s error messages are usually actionable on
    /// their own.
    #[error("`gh` API call failed: {stderr}")]
    Api { stderr: String },
    /// Failed to spawn the subprocess at all (other than
    /// not-found, which is [`Self::NotInstalled`]).
    #[error("failed to invoke `gh`: {0}")]
    Spawn(#[source] io::Error),
    /// `gh` succeeded but stdout wasn't the JSON shape we expected.
    #[error("`gh` response parse error: {0}")]
    Parse(String),
}

pub type GithubResult<T> = Result<T, GithubError>;

/// The GitHub I/O surface kata's service layer depends on.
///
/// Carving this out as a trait (vs. calling the concrete `gh`
/// wrapper directly) exists for one reason: end-to-end tests of
/// the import / publish flows. The real impl shells out to `gh`,
/// which needs a logged-in user, network, and a live PR — none
/// of which belong in `cargo test`. Tests inject a fake that
/// scripts canned responses for `graphql` / `post` / etc.
///
/// All methods return `serde_json::Value`-typed shapes via the
/// concrete REST/GraphQL helpers so the trait surface stays small.
/// `Send + Sync` so axum handlers and the service layer can hold
/// it behind `Arc<dyn GithubClient>`.
#[async_trait]
pub trait GithubClient: Send + Sync {
    /// `gh api repos/{owner}/{repo}/pulls/{number}` → typed PR
    /// metadata. The REST shape; `gh` is just the transport.
    async fn fetch_pr(&self, pr: &PullRequestRef) -> GithubResult<PullRequest>;

    /// GitHub GraphQL. The result is deserialised from `data` (the
    /// envelope's `errors[]` is checked first and surfaced as
    /// [`GithubError::Api`]).
    async fn graphql_raw(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> GithubResult<serde_json::Value>;

    /// `gh auth status` shaped as "who am I currently logged in
    /// as." Used by the connect/disconnect flows in the SPA.
    async fn auth_status(&self) -> GithubResult<AuthStatus>;

    /// Typed GET against a REST endpoint.
    async fn get_raw(&self, endpoint: &str) -> GithubResult<serde_json::Value>;

    /// Typed POST against a REST endpoint. The body is JSON,
    /// the response is JSON.
    async fn post_raw(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> GithubResult<serde_json::Value>;
}

/// Convenience helpers layered over the raw `Value`-returning
/// trait methods so call sites can stay typed. Implemented for
/// every `T: GithubClient + ?Sized` so it covers both concrete
/// impls and `&dyn GithubClient`.
#[async_trait]
pub trait GithubClientExt: GithubClient {
    async fn graphql<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> GithubResult<T> {
        let v = self.graphql_raw(query, variables).await?;
        serde_json::from_value(v)
            .map_err(|e| GithubError::Parse(format!("graphql decode: {e}")))
    }

    async fn get<T: DeserializeOwned>(&self, endpoint: &str) -> GithubResult<T> {
        let v = self.get_raw(endpoint).await?;
        serde_json::from_value(v)
            .map_err(|e| GithubError::Parse(format!("get {endpoint}: {e}")))
    }

    async fn post<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> GithubResult<T> {
        let v = self.post_raw(endpoint, body).await?;
        serde_json::from_value(v)
            .map_err(|e| GithubError::Parse(format!("post {endpoint}: {e}")))
    }
}

impl<T: GithubClient + ?Sized> GithubClientExt for T {}

/// Production implementation: shells out to the `gh` CLI binary.
/// Stateless — every call spawns a subprocess. Clone is free.
#[derive(Clone, Default)]
pub struct GhCliClient;

impl GhCliClient {
    pub fn new() -> Self {
        Self
    }

    async fn api_get(&self, endpoint: &str) -> GithubResult<Vec<u8>> {
        let mut cmd = tokio::process::Command::new("gh");
        cmd.arg("api").arg(endpoint);
        run(cmd, None).await
    }

    async fn api_stdin(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> GithubResult<Vec<u8>> {
        let mut cmd = tokio::process::Command::new("gh");
        cmd.arg("api").arg(endpoint).arg("--input").arg("-");
        let bytes = serde_json::to_vec(body)
            .expect("Value -> JSON bytes is infallible");
        run(cmd, Some(bytes)).await
    }
}

#[async_trait]
impl GithubClient for GhCliClient {
    async fn fetch_pr(&self, pr: &PullRequestRef) -> GithubResult<PullRequest> {
        let endpoint = format!("repos/{}/{}/pulls/{}", pr.owner, pr.repo, pr.number);
        let bytes = self.api_get(&endpoint).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| GithubError::Parse(format!("fetch_pr: {e}")))
    }

    async fn graphql_raw(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> GithubResult<serde_json::Value> {
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });
        let bytes = self.api_stdin("graphql", &body).await?;
        let env: GraphQlEnvelope<serde_json::Value> = serde_json::from_slice(&bytes)
            .map_err(|e| GithubError::Parse(format!("graphql envelope: {e}")))?;
        if let Some(errs) = env.errors
            && !errs.is_empty()
        {
            let joined: Vec<String> = errs.into_iter().map(|e| e.message).collect();
            return Err(GithubError::Api {
                stderr: format!("GraphQL errors: {}", joined.join("; ")),
            });
        }
        env.data
            .ok_or_else(|| GithubError::Parse("graphql response missing `data`".into()))
    }

    async fn auth_status(&self) -> GithubResult<AuthStatus> {
        let bytes = self.api_get("user").await?;
        let user: AuthStatus = serde_json::from_slice(&bytes)
            .map_err(|e| GithubError::Parse(format!("auth_status: {e}")))?;
        Ok(user)
    }

    async fn get_raw(&self, endpoint: &str) -> GithubResult<serde_json::Value> {
        let bytes = self.api_get(endpoint).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| GithubError::Parse(format!("get {endpoint}: {e}")))
    }

    async fn post_raw(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> GithubResult<serde_json::Value> {
        let mut cmd = tokio::process::Command::new("gh");
        cmd.arg("api")
            .arg(endpoint)
            .arg("--method")
            .arg("POST")
            .arg("--input")
            .arg("-");
        let bytes = serde_json::to_vec(body).expect("Value -> JSON bytes is infallible");
        let out = run(cmd, Some(bytes)).await?;
        serde_json::from_slice(&out)
            .map_err(|e| GithubError::Parse(format!("post {endpoint}: {e}")))
    }
}

/// Spawn `gh`, optionally feed stdin, and capture stdout+stderr.
/// Classification of the exit is centralised here so every API
/// method reports the same error variants.
async fn run(
    mut cmd: tokio::process::Command,
    stdin_bytes: Option<Vec<u8>>,
) -> GithubResult<Vec<u8>> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(GithubError::NotInstalled);
        }
        Err(e) => return Err(GithubError::Spawn(e)),
    };
    if let Some(bytes) = stdin_bytes {
        let mut stdin = child.stdin.take().expect("stdin requested");
        stdin
            .write_all(&bytes)
            .await
            .map_err(GithubError::Spawn)?;
        drop(stdin);
    }
    let output = child.wait_with_output().await.map_err(GithubError::Spawn)?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Err(classify_gh_failure(&stderr))
}

/// `gh` doesn't expose a structured exit-status taxonomy, so we
/// match on substrings of stderr that have been stable across
/// recent releases. Easy to extend as new patterns show up.
fn classify_gh_failure(stderr: &str) -> GithubError {
    // `gh auth status` and any authenticated call gh attempts
    // without a token emit some variant of this.
    if stderr.contains("not logged into any GitHub hosts")
        || stderr.contains("authentication required")
        || stderr.contains("Try authenticating with: gh auth login")
        || stderr.contains("You are not logged into")
    {
        return GithubError::NotAuthenticated;
    }
    // `gh api repos/.../pulls/N` against a missing PR emits
    // `gh: HTTP 404: ...`. The 404 line is what we anchor on.
    if stderr.contains("HTTP 404") || stderr.contains("Not Found") {
        return GithubError::NotFound {
            what: extract_first_line(stderr),
        };
    }
    // 422 typically comes from `POST /pulls/N/reviews` or
    // `POST /pulls/N/comments` when an inline anchor doesn't line
    // up in the diff (LEFT-side line with no matching hunk, etc).
    // The publish path treats this as recoverable and falls back
    // to an issue comment.
    if stderr.contains("HTTP 422") || stderr.contains("Unprocessable") {
        return GithubError::Validation {
            stderr: stderr.trim().to_owned(),
        };
    }
    GithubError::Api {
        stderr: stderr.trim().to_owned(),
    }
}

fn extract_first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).trim().to_owned()
}

#[derive(Deserialize)]
struct GraphQlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

// ---- REST response shapes ----------------------------------------------

/// PR open/closed state. `merged` is a derived field GitHub returns
/// alongside; keep both because some endpoints set only `state`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullState {
    Open,
    Closed,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: PullState,
    /// True after the PR was merged; `state` becomes `Closed`.
    #[serde(default)]
    pub merged: bool,
    pub base: PullRef,
    pub head: PullRef,
    pub html_url: String,
    pub user: Option<GhActor>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRef {
    /// e.g. `octocat:main` — fully-qualified across forks.
    pub label: String,
    /// Branch name on the underlying repo (e.g. `main`, `feature/x`).
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
    pub repo: Option<GhRepo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GhRepo {
    pub full_name: String,
    pub clone_url: String,
    pub html_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GhActor {
    pub login: String,
    pub id: i64,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AuthStatus {
    pub login: String,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_deserialises() {
        let json = r#"{
            "number": 1347, "title": "Amazing", "body": "...",
            "state": "open", "merged": false,
            "html_url": "https://github.com/octocat/Hello-World/pull/1347",
            "user": {"login":"octocat","id":1,"avatar_url":"a","html_url":"h"},
            "base": {"label":"o:main","ref":"main","sha":"deadbeef",
                "repo":{"full_name":"o/p","clone_url":"c","html_url":"h"}},
            "head": {"label":"o:topic","ref":"topic","sha":"cafef00d",
                "repo":{"full_name":"o/p","clone_url":"c","html_url":"h"}}
        }"#;
        let pr: PullRequest = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 1347);
        assert_eq!(pr.head.sha, "cafef00d");
    }

    #[test]
    fn classify_not_authenticated() {
        let s = "Try authenticating with: gh auth login";
        assert!(matches!(classify_gh_failure(s), GithubError::NotAuthenticated));
    }

    #[test]
    fn classify_404() {
        let s = "gh: HTTP 404: Not Found (https://api.github.com/repos/o/r/pulls/9999)";
        match classify_gh_failure(s) {
            GithubError::NotFound { what } => assert!(what.contains("404")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn classify_other_falls_through_to_api() {
        let s = "gh: something else entirely";
        assert!(matches!(classify_gh_failure(s), GithubError::Api { .. }));
    }
}
