//! The entities module's OWN UI server (epic #216) — an axum server
//! spawned by `startup()` on this module's port parameter, serving
//! this crate's `ui/dist` (rust_embed) plus the typed JSON API in
//! [`crate::api`]. Every write flows through the module's existing
//! verbs (validation and versioning identical to the CLI) and emits
//! kernel telemetry.

use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::api;
use crate::MODULE_NAME;

/// The built entities dashboard (Vite output). Debug builds read from
/// disk (iterate with `npm run build` without recompiling Rust);
/// release builds embed the files in the binary.
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

#[derive(Clone)]
struct AppState {
    kernel: Kernel,
}

/// Bind and spawn the entities UI server.
///
/// # Errors
///
/// [`KernelError::Module`] when the port cannot be bound.
pub async fn spawn(kernel: Kernel, port: u16) -> Result<()> {
    let upload_limit = crate::resolved_upload_limit(&kernel).await;
    // Stop means stop: on `modules disable`/`restart` the kernel
    // cancels this token and axum closes the listener, releasing the
    // port. Without it the socket stayed bound after shutdown() and a
    // re-enable could not bind (M0).
    let stop = kernel.module_token(MODULE_NAME);
    let app = Router::new()
        .route("/api/ping", get(api_ping))
        .route("/api/types", get(api_types).post(api_types_add))
        .route("/api/rel-types", get(api_rel_types))
        // The dictionary, designable (#292): types → labels → entities is
        // the order everything else depends on, so it needs a surface.
        .route("/api/labels", get(api_labels).post(api_define_label))
        .route("/api/vocabulary", get(api_vocabulary))
        .route("/api/types/{name}/slots", get(api_slots).post(api_bind_slot))
        .route("/api/entities", get(api_list).post(api_create))
        .route("/api/entities/{frag}", get(api_detail))
        .route("/api/entities/{frag}/history", get(api_history))
        .route("/api/entities/{frag}/fields", get(api_fields).post(api_set_field))
        .route("/api/entities/{frag}/update", post(api_update))
        .route("/api/entities/{frag}/describe", post(api_describe))
        .route("/api/entities/{frag}/comment", post(api_comment))
        .route("/api/entities/{frag}/link", post(api_link))
        .route("/api/entities/{frag}/unlink", post(api_unlink))
        // EU5 — the graph is PER ENTITY: rooted where you opened it.
        .route("/api/entities/{frag}/graph", get(api_graph))
        // EU4 — attachments. Upload carries the bytes as the request
        // body with the name as a query parameter: one file, no
        // multipart boundary to parse, and the size cap sits on the
        // route where it belongs.
        .route(
            "/api/entities/{frag}/attach",
            post(api_attach).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route("/api/attachments/{frag}/download", get(api_download))
        .fallback(get(static_assets))
        .with_state(AppState { kernel });
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("entities ui cannot bind {addr}: {e}")))?;
    tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(stop.cancelled())
            .await;
        if let Err(e) = served {
            tracing::error!(target: "entities", error = %e, "entities ui server exited");
        } else {
            tracing::info!(target: "entities", "entities ui server closed");
        }
    });
    Ok(())
}

/// Liveness + identity + where the core dashboard lives (discovered
/// from the substrate, D-UI2 in reverse — the back link's target).
async fn api_ping(State(state): State<AppState>) -> Json<serde_json::Value> {
    let core_url = match state
        .kernel
        .find_module_by_name(NodeKind::KernelModule, "ui")
        .await
    {
        Ok(Some(entity)) => match state.kernel.get_parameter(entity, "attr_ui_port").await {
            Ok(Some(Value::Number(n))) => n
                .to_int()
                .and_then(|i| u16::try_from(i).ok())
                .map(|p| format!("http://127.0.0.1:{p}")),
            _ => Some("http://127.0.0.1:5150".to_string()), // skill-allow: §9-const — mirrors the ui module's param-overridable default
        },
        _ => None,
    };
    Json(serde_json::json!({
        "module": crate::MODULE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "core_url": core_url,
    }))
}

/// Best-effort write telemetry, attributed to this module (the same
/// event names the CLI emits) — a telemetry hiccup never fails the
/// user's write.
async fn emit(kernel: &Kernel, event: &str, detail: String) {
    let subject = kernel
        .find_module_by_name(NodeKind::KernelModule, crate::MODULE_NAME)
        .await
        .ok()
        .flatten();
    if let Err(e) = kernel
        .log_telemetry(event, Value::String(detail), subject)
        .await
    {
        tracing::warn!(target: "entities", "telemetry write failed: {e}");
    }
}

macro_rules! module_db {
    ($state:expr) => {
        match $state.kernel.module_db(crate::MODULE_NAME).await {
            Ok(db) => db,
            Err(e) => return Resp::err(e.to_string()),
        }
    };
}

async fn api_types(State(state): State<AppState>) -> Resp<Vec<api::TypeView>> {
    let db = module_db!(state);
    match api::types_list(&db).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_fields(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
) -> Resp<Vec<api::FieldView>> {
    let db = module_db!(state);
    match api::entity_fields(&db, &frag).await {
        Ok(rows) => Resp::ok(rows),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_set_field(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::FieldReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::set_field(&db, &frag, &req.key, &req.value).await {
        Ok(()) => {
            emit(&state.kernel, "entity_updated", frag.clone()).await;
            Resp::ok(serde_json::json!({ "key": req.key }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_labels(
    State(state): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Resp<Vec<api::LabelView>> {
    let db = module_db!(state);
    let archived = q.get("archived").is_some_and(|v| v == "true");
    match api::labels(&db, archived).await {
        Ok(rows) => Resp::ok(rows),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_vocabulary(State(state): State<AppState>) -> Resp<api::VocabularyView> {
    let db = module_db!(state);
    match api::vocabulary(&db).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_define_label(
    State(state): State<AppState>,
    Json(req): Json<api::LabelReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::define_label(&db, &req).await {
        Ok(()) => {
            emit(&state.kernel, "label_defined", req.key.clone()).await;
            Resp::ok(serde_json::json!({ "key": req.key }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_slots(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Resp<Vec<api::SlotView>> {
    let db = module_db!(state);
    let retired = q.get("retired").is_some_and(|v| v == "true");
    match api::slots(&db, &name, retired).await {
        Ok(rows) => Resp::ok(rows),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_bind_slot(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<api::SlotReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::bind_slot(&db, &name, &req).await {
        Ok(()) => {
            emit(&state.kernel, "type_slot_bound", format!("{name}.{}", req.label)).await;
            Resp::ok(serde_json::json!({ "type": name, "label": req.label }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_types_add(
    State(state): State<AppState>,
    Json(req): Json<api::TypeReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::types_add(&db, &req).await {
        Ok(()) => {
            emit(&state.kernel, "entity_type_added", req.name.clone()).await;
            Resp::ok(serde_json::json!({ "name": req.name }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_rel_types(State(state): State<AppState>) -> Resp<Vec<String>> {
    let db = module_db!(state);
    match api::rel_types(&db).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct ListQuery {
    r#type: Option<String>,
}

async fn api_list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Resp<Vec<api::EntityListItem>> {
    let db = module_db!(state);
    match api::list(&db, q.r#type.as_deref()).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_create(
    State(state): State<AppState>,
    Json(req): Json<api::CreateReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::create(&db, &req).await {
        Ok(id) => {
            emit(&state.kernel, "entity_created", format!("{} {}", req.entity_type, req.name)).await;
            Resp::ok(serde_json::json!({ "id": id }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_detail(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
) -> Resp<api::EntityDetail> {
    let db = module_db!(state);
    match api::detail(&db, &frag).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_history(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
) -> Resp<Vec<api::VersionView>> {
    let db = module_db!(state);
    match api::history(&db, &frag).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_update(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::UpdateReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::update(&db, &frag, &req).await {
        Ok(id) => {
            emit(&state.kernel, "entity_updated", id.clone()).await;
            Resp::ok(serde_json::json!({ "id": id }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_describe(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::TextReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::describe(&db, &frag, &req.text).await {
        Ok(id) => {
            emit(&state.kernel, "entity_described", frag).await;
            Resp::ok(serde_json::json!({ "text_id": id }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_comment(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::TextReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    let author = match req.author_kind.as_deref() {
        Some(kind) => match crate::notes::Author::claimed(
            kind,
            req.author_uid.as_deref(),
            req.via_uid.as_deref(),
        ) {
            Ok(a) => a,
            Err(e) => return Resp::err(e.to_string()),
        },
        None => crate::notes::Author::operator(),
    };
    match api::comment(&db, &frag, &req.text, &author).await {
        Ok(id) => {
            emit(&state.kernel, "entity_commented", frag).await;
            Resp::ok(serde_json::json!({ "text_id": id }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_link(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::LinkReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::link(&db, &frag, &req).await {
        Ok(edge_uid) => {
            emit(&state.kernel, "entity_linked", format!("{frag} -[{}]-> {}", req.rel, req.to)).await;
            Resp::ok(serde_json::json!({ "edge_uid": edge_uid }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

async fn api_unlink(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Json(req): Json<api::LinkReq>,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    match api::unlink(&db, &frag, &req).await {
        Ok(edge_uid) => {
            emit(&state.kernel, "entity_unlinked", format!("{frag} -[{}]-x {}", req.rel, req.to)).await;
            Resp::ok(serde_json::json!({ "edge_uid": edge_uid }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct GraphQuery {
    depth: Option<usize>,
    /// `out` | `in` | `both` (default) — which way the walk runs.
    direction: Option<String>,
}

/// EU5 — the subgraph rooted at one entity.
async fn api_graph(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Query(q): Query<GraphQuery>,
) -> Resp<api::GraphView> {
    let db = module_db!(state);
    // The module's existing depth ceiling governs; a deeper request is
    // clamped rather than refused.
    let ceiling = crate::cli::resolved_max_depth(&state.kernel).await;
    let opening = crate::resolved_graph_depth(&state.kernel).await;
    let depth = q.depth.unwrap_or(opening).clamp(1, ceiling);
    let direction = q.direction.unwrap_or_else(|| "both".to_string());
    match api::graph_view(&db, &frag, depth, &direction).await {
        Ok(v) => Resp::ok(v),
        Err(e) => Resp::err(e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct AttachQuery {
    /// The uploaded file's own name, for the document node's metadata.
    name: Option<String>,
}

/// EU4 — store an uploaded file under the module's dir and record the
/// document node + `attached` edge. The bytes never leave the module.
async fn api_attach(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
    Query(q): Query<AttachQuery>,
    body: axum::body::Bytes,
) -> Resp<serde_json::Value> {
    let db = module_db!(state);
    if body.is_empty() {
        return Resp::err("empty upload".to_string());
    }
    // The client's filename is untrusted: keep only the final
    // component so it can never climb out of the files dir.
    let raw = q.name.unwrap_or_else(|| "attachment".to_string());
    let file_name = std::path::Path::new(&raw)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .unwrap_or("attachment")
        .to_string();

    let owner = match crate::nodes::resolve_entity(&db, &frag).await {
        Ok(o) => o,
        Err(e) => return Resp::err(e.to_string()),
    };
    let files_dir = match state.kernel.module_dir(crate::MODULE_NAME) {
        Ok(d) => d.join("files"),
        Err(e) => return Resp::err(e.to_string()),
    };
    if let Err(e) = std::fs::create_dir_all(&files_dir) {
        return Resp::err(format!("cannot create files dir: {e}"));
    }
    // Same storage convention as `superx entities attach`: a uuid7
    // prefix keeps names unique and time-ordered.
    let stored = files_dir.join(format!("{}-{file_name}", uuid::Uuid::now_v7()));
    let size = body.len() as u64;
    if let Err(e) = std::fs::write(&stored, &body) {
        return Resp::err(format!("cannot store file: {e}"));
    }
    let mime = crate::documents::mime_for(&file_name);
    match crate::documents::attach_document(
        &db,
        &owner,
        &file_name,
        &stored.to_string_lossy(),
        mime,
        size,
    )
    .await
    {
        Ok(node) => {
            let uuid = superx_ops::record_uuid(&node);
            emit(&state.kernel, "document_attached", format!("{file_name} → {frag}")).await;
            Resp::ok(serde_json::json!({ "id": uuid, "name": file_name, "size": size }))
        }
        Err(e) => Resp::err(e.to_string()),
    }
}

/// EU4 — hand an attachment's bytes back.
async fn api_download(
    State(state): State<AppState>,
    AxumPath(frag): AxumPath<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let db = match state.kernel.module_db(crate::MODULE_NAME).await {
        Ok(db) => db,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let (path, name, mime) = match api::attachment_file(&db, &frag).await {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    // The stored path is a substrate fact, but serve it only if it
    // really sits under this module's files dir — a rogue path in the
    // attributes must not turn this route into an arbitrary file read.
    let files_dir = match state.kernel.module_dir(crate::MODULE_NAME) {
        Ok(d) => d.join("files"),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let (Ok(real), Ok(root)) = (
        std::fs::canonicalize(&path),
        std::fs::canonicalize(&files_dir),
    ) else {
        return (StatusCode::NOT_FOUND, "attachment file is missing").into_response();
    };
    if !real.starts_with(&root) {
        return (StatusCode::FORBIDDEN, "attachment is outside the module's files dir")
            .into_response();
    }
    match std::fs::read(&real) {
        Ok(bytes) => (
            [
                (axum::http::header::CONTENT_TYPE, mime),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", name.replace('"', "")),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, format!("cannot read attachment: {e}")).into_response(),
    }
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
            // Cache policy (issue #255): index.html names the
            // content-hashed bundle, so it must ALWAYS be revalidated
            // — a cached entry point pins the browser to a stale
            // dashboard through every rebuild. The hashed assets it
            // points at are immutable by construction.
            let cache = if name.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-store"
            };
            (
                [
                    (axum::http::header::CONTENT_TYPE, mime.to_string()),
                    (axum::http::header::CACHE_CONTROL, cache.to_string()),
                ],
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

/// Tiny ok/err JSON envelope (the ui module's pattern).
enum Resp<T: serde::Serialize> {
    Ok(Json<T>),
    Err((StatusCode, Json<serde_json::Value>)),
}

impl<T: serde::Serialize> Resp<T> {
    fn ok(value: T) -> Self {
        Self::Ok(Json(value))
    }
    fn err(msg: String) -> Self {
        Self::Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        ))
    }
}

impl<T: serde::Serialize> axum::response::IntoResponse for Resp<T> {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Ok(json) => json.into_response(),
            Self::Err(err) => err.into_response(),
        }
    }
}
