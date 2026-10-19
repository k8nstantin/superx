//! The module's OWN service, on its OWN port (D17): the fault ledger as a
//! page.
//!
//! An axum server spawned by `startup()`, independent of every other
//! module. It serves the ledger as JSON (`/api/ledger`, the same
//! [`crate::ledger::build`] the CLI renders as text) and the built page,
//! and nothing else knows it is there: the core dashboard finds its URL
//! in the substrate (`attr_module_ui_url`), never through an import.
//! Read-only: no route writes anything.

use axum::extract::{Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use superx_kernel::{Kernel, KernelError, Result};

use crate::MODULE_NAME;

/// The built page (Vite output). Debug builds read from disk, so
/// `npm run build` is enough to see a change; release builds embed the
/// files in the binary, so the module ships as one artefact.
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

#[derive(Clone)]
struct AppState {
    db: superx_kernel::Db,
    /// The core dashboard, resolved at startup, for the page's way back.
    core_url: Option<String>,
}

/// The routes, over a database the caller already has.
pub fn router(db: superx_kernel::Db, core_url: Option<String>) -> Router {
    Router::new()
        .route("/api/ping", get(ping))
        .route("/api/ledger", get(ledger))
        .fallback(get(assets))
        .with_state(AppState { db, core_url })
}

/// Bind and spawn the specs service.
///
/// # Errors
///
/// [`KernelError::Module`] when the port cannot be bound.
pub async fn spawn(kernel: &Kernel, db: superx_kernel::Db, port: u16) -> Result<()> {
    // Stop means stop: the kernel cancels this token on disable or restart
    // and axum closes the listener, releasing the port.
    let stop = kernel.module_token(MODULE_NAME);
    let app = router(db, crate::core_dashboard_url(kernel).await);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("specs service cannot bind {addr}: {e}")))?;
    tokio::spawn(async move {
        match axum::serve(listener, app).with_graceful_shutdown(stop.cancelled()).await {
            Ok(()) => tracing::info!(target: "specs", "service closed"),
            Err(e) => tracing::error!(target: "specs", error = %e, "service exited"),
        }
    });
    Ok(())
}

async fn ping(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": MODULE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "core_url": state.core_url,
    }))
}

#[derive(serde::Deserialize)]
struct LedgerQuery {
    key: Option<String>,
}

async fn ledger(
    State(state): State<AppState>,
    Query(q): Query<LedgerQuery>,
) -> std::result::Result<Json<Vec<crate::ledger::TaskLedger>>, (StatusCode, String)> {
    crate::ledger::build(&state.db, q.key.as_deref())
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))
}

async fn assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let file = if path.is_empty() { "index.html" } else { path };
    // The type comes from what is SERVED: a fallback to index.html is HTML,
    // whatever the URL asked for.
    let (served, content) = match Assets::get(file) {
        Some(c) => (file, c),
        None => match Assets::get("index.html") {
            Some(c) => ("index.html", c),
            None => return (StatusCode::NOT_FOUND, "no ui built").into_response(),
        },
    };
    let mime = mime_guess::from_path(served).first_or_octet_stream();
    ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
}
