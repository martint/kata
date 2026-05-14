use std::path::Path;

use axum::Router;
use axum::routing::{get, get_service, post};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod author;
mod comments;
mod events;
mod responses;
mod reviews;
mod sessions;

pub use author::ViewerAuthor;

pub fn router(state: AppState) -> Router {
    api_routes().with_state(state).layer(TraceLayer::new_for_http())
}

pub fn router_with_assets(state: AppState, web_dir: &Path) -> Router {
    let index = web_dir.join("index.html");
    let serve_dir = ServeDir::new(web_dir).not_found_service(ServeFile::new(index));
    api_routes()
        .fallback_service(get_service(serve_dir))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

pub fn router_with_embedded_assets(state: AppState) -> Router {
    api_routes()
        .fallback(axum::routing::get(crate::embedded::handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/api/whoami", get(reviews::whoami))
        .route("/api/repos", get(reviews::list_repos))
        .route("/api/events", get(events::stream))
        .route("/api/repos/{repo_name}/bookmarks", get(reviews::list_bookmarks))
        .route("/api/repos/{repo_name}/revset/preview", get(reviews::preview_revset))
        .route("/api/repos/{repo_name}/files", get(reviews::read_file))
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
            "/api/repos/{repo_name}/reviews/{review_number}/commits/{change_id}/diff",
            get(reviews::commit_diff),
        )
        .route(
            "/api/repos/{repo_name}/reviews/{review_number}/file-diff",
            get(reviews::file_diff),
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
}
