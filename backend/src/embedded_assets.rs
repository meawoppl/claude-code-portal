use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../frontend/dist"]
pub struct FrontendAssets;

/// Serve embedded frontend assets with SPA fallback
pub async fn serve_embedded_frontend(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    serve_asset(path)
}

fn serve_asset(path: &str) -> Response {
    match FrontendAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                Body::from(content.data.to_vec()),
            )
                .into_response()
        }
        None => {
            // SPA fallback: serve index.html for any unknown path
            match FrontendAssets::get("index.html") {
                Some(content) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    Body::from(content.data.to_vec()),
                )
                    .into_response(),
                None => (StatusCode::NOT_FOUND, "Frontend not found").into_response(),
            }
        }
    }
}
