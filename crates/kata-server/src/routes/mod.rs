use std::path::Path;

use axum::Router;
use axum::routing::{get, get_service, post};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod annotations;
mod author;
mod browse;
mod comments;
mod events;
mod github;
mod responses;
mod reviews;
mod sessions;

pub use author::{Actor, ViewerAuthor};

pub fn router(state: AppState) -> Router {
    attach_github_routes(attach_oidc_routes(api_routes(), &state))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

pub fn router_with_assets(state: AppState, web_dir: &Path) -> Router {
    let index = web_dir.join("index.html");
    let serve_dir = ServeDir::new(web_dir).not_found_service(ServeFile::new(index));
    attach_github_routes(attach_oidc_routes(api_routes(), &state))
        .fallback_service(get_service(serve_dir))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

pub fn router_with_embedded_assets(state: AppState) -> Router {
    attach_github_routes(attach_oidc_routes(api_routes(), &state))
        .fallback(axum::routing::get(crate::embedded::handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// `/auth/login`, `/auth/callback`, and `/auth/logout` ride alongside
/// the API routes only when OIDC mode is active. Mounting them in
/// other modes would be dead code (no `OidcRuntime` to dispatch
/// against) AND a footgun (`/auth/login` returning 500 on a misclick
/// when an operator didn't intend OIDC). The OIDC routes live in
/// their own sub-router so they can hold `OidcRuntime` as their
/// state without entangling `AppState`.
fn attach_oidc_routes(api: Router<AppState>, state: &AppState) -> Router<AppState> {
    let Some(rt) = state.oidc.clone() else {
        return api;
    };
    let oidc = Router::new()
        .route("/auth/login", get(crate::oidc::login))
        .route("/auth/callback", get(crate::oidc::callback))
        .route("/auth/logout", get(crate::oidc::logout))
        .with_state(rt);
    api.merge(oidc)
}

/// `/api/github/{status,import}`. All GitHub I/O delegates to the
/// `gh` CLI (see [`kata_service::github`]) — there is no per-user
/// OAuth state and no callback URL.
fn attach_github_routes(api: Router<AppState>) -> Router<AppState> {
    api.route("/api/github/status", get(github::status))
        .route("/api/github/import", post(github::import))
}

fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/api/whoami", get(reviews::whoami))
        .route("/api/repos", get(reviews::list_repos))
        .route("/api/events", get(events::stream))
        .route("/api/repos/{repo_name}/bookmarks", get(reviews::list_bookmarks))
        .route("/api/repos/{repo_name}/revset/preview", get(reviews::preview_revset))
        .route("/api/repos/{repo_name}/files", get(reviews::read_file))
        // Repository browser. Reads only — no review created, no
        // diff baseline; just topo-ordered commits + per-row
        // decoration (bookmarks, working-copy marker).
        .route("/api/repos/{repo_name}/browse/log", get(browse::log))
        .route(
            "/api/repos/{repo_name}/browse/commits/{commit_id}",
            get(browse::commit),
        )
        .route(
            "/api/repos/{repo_name}/browse/commits/{commit_id}/diff",
            get(browse::commit_diff),
        )
        .route(
            "/api/repos/{repo_name}/browse/changes/{change_id}",
            get(browse::change),
        )
        .route("/api/repos/{repo_name}/browse/file", get(browse::file))
        .route(
            "/api/repos/{repo_name}/browse/file-history",
            get(browse::file_history),
        )
        .route(
            "/api/repos/{repo_name}/reviews",
            get(reviews::list_reviews).post(reviews::create_review),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}",
            get(reviews::open_review),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/refresh",
            post(reviews::refresh_review),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/summary",
            axum::routing::put(reviews::update_summary),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/revset",
            axum::routing::put(reviews::update_revset),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/archive",
            post(reviews::archive_review).delete(reviews::unarchive_review),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}",
            axum::routing::delete(reviews::delete_review),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/commits/{change_id}/diff",
            get(reviews::commit_diff),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/file-diff",
            get(reviews::file_diff),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/compare",
            get(reviews::compare_patchsets),
        )
        .route(
            "/api/repos/{repo_name}/diff",
            get(reviews::diff_commits),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions",
            post(sessions::start_session),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions/{session_id}/publish",
            post(sessions::publish_session),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions/{session_id}/discard",
            post(sessions::discard_session),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions/{session_id}/comments",
            post(comments::create_comment),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions/{session_id}/comments/{comment_id}",
            axum::routing::put(comments::update_comment).delete(comments::delete_comment),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions/{session_id}/responses",
            post(responses::create_response),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/sessions/{session_id}/responses/{response_id}",
            axum::routing::put(responses::update_response).delete(responses::delete_response),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/annotations",
            post(annotations::create_annotation),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/annotations/{annotation_id}",
            axum::routing::patch(annotations::update_annotation)
                .delete(annotations::delete_annotation),
        )
}
