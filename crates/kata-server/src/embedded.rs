//! Static-asset serving for the bundled Svelte build.
//!
//! `rust-embed` reads `web/dist` at compile time. `build.rs` makes sure the
//! directory is populated before this module compiles.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
struct WebAssets;

/// Handler that serves a file from the embedded bundle, falling back to
/// `index.html` for SPA-style routes.
pub async fn handler(req: Request<Body>) -> Response {
    let raw = req.uri().path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    if let Some(file) = WebAssets::get(path) {
        return serve(path, file.data.as_ref());
    }

    // SPA fallback — any non-asset path falls back to index.html so client
    // routing works on hard reload.
    if let Some(file) = WebAssets::get("index.html") {
        return serve("index.html", file.data.as_ref());
    }

    StatusCode::NOT_FOUND.into_response()
}

fn serve(path: &str, bytes: &[u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref())],
        bytes.to_vec(),
    )
        .into_response()
}
