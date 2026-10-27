//! Internal web server of step 2: the map page and, from lot 13 on, the live
//! state pushed to the browser (SSE). See `PLAN_ET2.md`.
//!
//! Behind the `server` feature, so that the library can be used without it.

mod assets;

use axum::Router;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

pub use assets::Asset;

/// The routes of the page and its static files.
pub fn router() -> Router {
    Router::new()
        .route("/", get(|| async { serve_asset("index.html") }))
        .route("/assets/{*path}", get(asset))
}

async fn asset(Path(path): Path<String>) -> Response {
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    match Asset::get(path) {
        Some(a) => (
            [
                (header::CONTENT_TYPE, a.mime),
                (header::CACHE_CONTROL, "no-cache"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            a.bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
