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
    db: superx_kernel::Db,
    /// The core dashboard, resolved from the substrate at startup so the
    /// module's header can offer a way back to it.
    core_url: Option<String>,
}

/// Bind and spawn the entities service.
///
/// # Errors
///
/// [`KernelError::Module`] when the port cannot be bound.
pub async fn spawn(kernel: &Kernel, db: superx_kernel::Db, port: u16) -> Result<()> {
    // Stop means stop: the kernel cancels this token on disable or
    // restart and axum closes the listener, releasing the port. Without
    // it the socket stays bound after shutdown and a re-enable cannot
    // bind.
    let stop = kernel.module_token(MODULE_NAME);
    let core_url = crate::core_dashboard_url(kernel).await;
    let app = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/entities", get(list).post(create))
        .route("/api/entities/search", get(search))
        .route("/api/entities/all", get(all_entities))
        .route("/api/entities/{frag}", get(detail))
        .route("/api/entities/{frag}/attributes", post(put_attribute))
        .route("/api/entities/{frag}/children", get(children))
        .route("/api/entities/{frag}/graph", get(graph))
        .route("/api/entities/{frag}/link", post(link))
        .route("/api/entities/{frag}/archive", post(archive))
        .fallback(get(assets))
        .with_state(AppState { db, core_url });

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

fn fail(e: KernelError) -> (StatusCode, String) {
    // A refusal from a verb is the caller's problem to fix, not a
    // server fault — the datatype gate and the label check both land
    // here, and their messages say what to do.
    (StatusCode::BAD_REQUEST, e.to_string())
}

async fn ping(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": MODULE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "core_url": state.core_url,
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    archived: bool,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    archived: bool,
}

async fn all_entities(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> std::result::Result<Json<Vec<api::TreeNodeView>>, (StatusCode, String)> {
    api::all(&state.db, q.archived).await.map(Json).map_err(fail)
}

async fn search(
    State(state): State<AppState>,
    Query(p): Query<SearchQuery>,
) -> std::result::Result<Json<Vec<api::TreeNodeView>>, (StatusCode, String)> {
    api::search(&state.db, &p.q, p.archived).await.map(Json).map_err(fail)
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> std::result::Result<Json<Vec<api::TreeNodeView>>, (StatusCode, String)> {
    let db = &state.db;
    api::roots(db, q.archived).await.map(Json).map_err(fail)
}

async fn create(
    State(state): State<AppState>,
    Json(req): Json<api::CreateReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = &state.db;
    let uuid = api::create(db, &req, &Author::operator()).await.map_err(fail)?;
    Ok(Json(serde_json::json!({ "uuid": uuid })))
}

async fn detail(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
) -> std::result::Result<Json<api::EntityView>, (StatusCode, String)> {
    let db = &state.db;
    api::detail(db, &frag).await.map(Json).map_err(fail)
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
    let db = &state.db;
    api::children(db, &frag, q.label.as_deref()).await.map(Json).map_err(fail)
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
    let db = &state.db;
    let depth = q.depth.unwrap_or(crate::DEFAULT_GRAPH_DEPTH).clamp(1, crate::MAX_GRAPH_DEPTH);
    api::graph(db, &frag, q.label.as_deref(), depth).await.map(Json).map_err(fail)
}

async fn put_attribute(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::AttributeReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = &state.db;
    let uid = api::put_attribute(db, &frag, &req, &Author::operator()).await.map_err(fail)?;
    Ok(Json(serde_json::json!({ "uid": uid })))
}

async fn link(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::LinkReq>,
) -> std::result::Result<Json<serde_json::Value>, (StatusCode, String)> {
    let db = &state.db;
    let uid = api::link(db, &frag, &req, &Author::operator()).await.map_err(fail)?;
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
    let db = &state.db;
    let id = crate::entity::resolve(db, &frag).await.map_err(fail)?;
    let changed = if req.archived {
        crate::entity::archive(db, &id, &Author::operator()).await
    } else {
        crate::entity::unarchive(db, &id, &Author::operator()).await
    }
    .map_err(fail)?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

/// Serve the built UI, falling back to `index.html` so a deep link
/// reloads instead of 404ing.
async fn assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let file = if path.is_empty() { "index.html" } else { path };
    // GUESS FROM WHAT IS ACTUALLY SERVED, not from what was asked for.
    // Falling back to index.html while labelling it from the URL sent
    // HTML as `application/octet-stream` for `/entity/abc` — the browser
    // downloaded a file instead of rendering the app, which is the exact
    // case this fallback exists for.
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
