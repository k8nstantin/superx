//! The tab (#539): the page the module's own URL serves, built with Vite from
//! `ui/` into `ui/dist/` and embedded in the binary, as the entities module
//! embeds its own. Debug builds read the files from disk, so `npm run build`
//! is enough to see a change.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

/// A file of the built page by its path; any other path is the page itself,
/// labelled by what is served, not by what was asked for.
pub async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let file = if path.is_empty() { "index.html" } else { path };
    let (served, content) = match Assets::get(file) {
        Some(c) => (file, c),
        None => match Assets::get("index.html") {
            Some(c) => ("index.html", c),
            None => return (StatusCode::NOT_FOUND, "no page built").into_response(),
        },
    };
    let mime = mime_guess::from_path(served).first_or_octet_stream();
    ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
}
