//! The entities module's OWN UI server (epic #216, EU1) — the
//! per-module-UI facility: an axum server spawned by `startup()` on
//! this module's own port parameter, serving this crate's `ui/dist`
//! (rust_embed). The module contract's pattern, proven by
//! `superx-mod-ui`; everything here lives inside the entities crate.

use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use superx_kernel::{Kernel, KernelError, Result};

/// The built entities dashboard (Vite output). Debug builds read from
/// disk (iterate with `npm run build` without recompiling Rust);
/// release builds embed the files in the binary.
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

/// Bind and spawn the entities UI server.
///
/// # Errors
///
/// [`KernelError::Module`] when the port cannot be bound.
pub async fn spawn(kernel: Kernel, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/api/ping", get(api_ping))
        .fallback(get(static_assets))
        .with_state(kernel);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("entities ui cannot bind {addr}: {e}")))?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(target: "entities", error = %e, "entities ui server exited");
        }
    });
    Ok(())
}

/// Liveness + identity — EU2 lands the real read API here.
async fn api_ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": crate::MODULE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Serve the dashboard: exact asset when it exists, index.html as the
/// SPA fallback for everything else.
async fn static_assets(uri: axum::http::Uri) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let serve = |name: &str| {
        Assets::get(name).map(|f| {
            let mime = mime_guess::from_path(name).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, mime.to_string())],
                f.data.into_owned(),
            )
                .into_response()
        })
    };
    serve(path)
        .or_else(|| serve("index.html"))
        .unwrap_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Html("entities ui not built — run npm run build in crates/superx-mod-entities/ui"),
            )
                .into_response()
        })
}
