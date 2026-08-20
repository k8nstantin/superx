//! The entities module's OWN UI server (epic #216) — an axum server
//! spawned by `startup()` on this module's port parameter, serving
//! this crate's `ui/dist` (rust_embed) plus the typed JSON API in
//! [`crate::api`]. Every write flows through the module's existing
//! verbs (validation and versioning identical to the CLI) and emits
//! kernel telemetry.

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::api;

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
    let app = Router::new()
        .route("/api/ping", get(api_ping))
        .route("/api/types", get(api_types).post(api_types_add))
        .route("/api/rel-types", get(api_rel_types))
        .route("/api/entities", get(api_list).post(api_create))
        .route("/api/entities/{frag}", get(api_detail))
        .route("/api/entities/{frag}/history", get(api_history))
        .route("/api/entities/{frag}/update", post(api_update))
        .route("/api/entities/{frag}/describe", post(api_describe))
        .route("/api/entities/{frag}/comment", post(api_comment))
        .route("/api/entities/{frag}/link", post(api_link))
        .route("/api/entities/{frag}/unlink", post(api_unlink))
        .fallback(get(static_assets))
        .with_state(AppState { kernel });
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
    match api::comment(&db, &frag, &req.text).await {
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
