//! The module's OWN service, on its OWN port (D17).
//!
//! An axum server spawned by `startup()`, independent of every other
//! module: its own listener, its own lifecycle, its own log target. It
//! serves the typed JSON API in [`crate::api`] and nothing else knows it
//! is there — the core dashboard finds its URL in the substrate, never
//! by importing anything.
//!
//! Every write goes through the module's verbs, so the validation and
//! versioning a browser gets are the same ones any other caller gets.
//! There is no second path into the data.

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use superx_kernel::{Kernel, KernelError, Result};

use crate::api;
use crate::author::Author;
use crate::MODULE_NAME;

/// The built dashboard (Vite output). Debug builds read from disk, so
/// `npm run build` is enough to see a change; release builds embed the
/// files in the binary, so the module ships as one artefact.
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

#[derive(Clone)]
struct AppState {
    kernel: Kernel,
}

/// Bind and spawn the entities service.
///
/// # Errors
///
/// [`KernelError::Module`] when the port cannot be bound.
pub async fn spawn(kernel: Kernel, port: u16) -> Result<()> {
    // Stop means stop: the kernel cancels this token on disable or
    // restart and axum closes the listener, releasing the port. Without
    // it the socket stays bound after shutdown and a re-enable cannot
    // bind.
    let stop = kernel.module_token(MODULE_NAME);
    let app = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/entities", get(list).post(create))
        .route("/api/entities/{frag}", get(detail))
        .route("/api/entities/{frag}/attributes", post(put_attribute))
        .route("/api/entities/{frag}/children", get(children))
        .route("/api/entities/{frag}/graph", get(graph))
        .route("/api/entities/{frag}/link", post(link))
        .route("/api/entities/{frag}/archive", post(archive))
        .fallback(get(assets))
        .with_state(AppState { kernel });

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("entities service cannot bind {addr}: {e}")))?;
    tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(stop.cancelled())
            .await;
        match served {
            Ok(()) => tracing::info!(target: "entities", "service closed"),
            Err(e) => tracing::error!(target: "entities", error = %e, "service exited"),
        }
    });
    Ok(())
}

/// The module's own db, or an honest error. Unprovisioned is a real
/// state, not a bug: the OS runs and this module waits.
async fn db(state: &AppState) -> std::result::Result<superx_kernel::Db, (StatusCode, String)> {
    state
        .kernel
        .module_db(MODULE_NAME)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, format!("not provisioned: {e}")))
}

fn fail(e: KernelError) -> (StatusCode, String) {
    // A refusal from a verb is the caller's problem to fix, not a
    // server fault — the datatype gate and the label check both land
    // here, and their messages say what to do.
    (StatusCode::BAD_REQUEST, e.to_string())
}

async fn ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": MODULE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    archived: bool,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> std::result::Result<Json<Vec<api::TreeNodeView>>, (StatusCode, String)> {
    let db = db(&state).await?;
    api::roots(&db, q.archived).await.map(Json).map_err(fail)
}

async fn create(
    State(state): State<AppState>,
    Json(req): Json<api::CreateReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = db(&state).await?;
    let uuid = api::create(&db, &req, &Author::operator()).await.map_err(fail)?;
    Ok(Json(serde_json::json!({ "uuid": uuid })))
}

async fn detail(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
) -> std::result::Result<Json<api::EntityView>, (StatusCode, String)> {
    let db = db(&state).await?;
    api::detail(&db, &frag).await.map(Json).map_err(fail)
}

#[derive(Deserialize)]
struct LabelQuery {
    label: Option<String>,
}

async fn children(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Query(q): Query<LabelQuery>,
) -> std::result::Result<Json<Vec<api::TreeNodeView>>, (StatusCode, String)> {
    let db = db(&state).await?;
    api::children(&db, &frag, q.label.as_deref()).await.map(Json).map_err(fail)
}

#[derive(Deserialize)]
struct GraphQuery {
    label: Option<String>,
    depth: Option<usize>,
}

async fn graph(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Query(q): Query<GraphQuery>,
) -> std::result::Result<Json<api::GraphView>, (StatusCode, String)> {
    let db = db(&state).await?;
    let depth = q.depth.unwrap_or(crate::DEFAULT_GRAPH_DEPTH).clamp(1, crate::MAX_GRAPH_DEPTH);
    api::graph(&db, &frag, q.label.as_deref(), depth).await.map(Json).map_err(fail)
}

async fn put_attribute(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::AttributeReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = db(&state).await?;
    let uid = api::put_attribute(&db, &frag, &req, &Author::operator()).await.map_err(fail)?;
    Ok(Json(serde_json::json!({ "uid": uid })))
}

async fn link(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::LinkReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = db(&state).await?;
    let uid = api::link(&db, &frag, &req, &Author::operator()).await.map_err(fail)?;
    Ok(Json(serde_json::json!({ "uid": uid })))
}

#[derive(Deserialize)]
struct ArchiveReq {
    archived: bool,
}

async fn archive(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<ArchiveReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = db(&state).await?;
    let id = crate::entity::resolve(&db, &frag).await.map_err(fail)?;
    let changed = if req.archived {
        crate::entity::archive(&db, &id, &Author::operator()).await
    } else {
        crate::entity::unarchive(&db, &id, &Author::operator()).await
    }
    .map_err(fail)?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

/// Serve the built UI, falling back to `index.html` so a deep link
/// reloads instead of 404ing.
async fn assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let file = if path.is_empty() { "index.html" } else { path };
    match Assets::get(file).or_else(|| Assets::get("index.html")) {
        Some(content) => {
            let mime = mime_guess::from_path(file).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "no ui built").into_response(),
    }
}
