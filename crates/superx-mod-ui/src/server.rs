//! The UI module's HTTP layer (P4): placeholder page, JSON status,
//! whitelisted command execution. Read API + SSE arrive in P5.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

/// Commands the UI may execute — the read surface only. Lifecycle
/// (stop/restart) is deliberately excluded until UI v1.1
/// (operator-decided): this server lives INSIDE the daemon.
const COMMAND_WHITELIST: &[&str] = &["agents", "sessions", "actions", "read", "modules"];

#[derive(Clone)]
struct AppState {
    kernel: Kernel,
}

/// Bind and spawn the server task. Fails startup loudly if the port
/// is taken (the module then shows `failed` in the registry — honest).
pub async fn spawn(kernel: Kernel, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/status", get(api_status))
        .route("/api/command", post(api_command))
        .with_state(AppState { kernel });
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("ui cannot bind {addr}: {e}")))?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(target: "ui", error = %e, "ui server exited");
        }
    });
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(crate::PLACEHOLDER_HTML)
}

async fn api_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let kernel = &state.kernel;
    let mut modules = Vec::new();
    for kind in [NodeKind::KernelModule, NodeKind::Adapter] {
        if let Ok(list) = kernel.list_with_status(kind).await {
            for s in list {
                modules.push(serde_json::json!({
                    "name": s.name,
                    "kind": kind.type_uid().trim_start_matches("node_"),
                    "lifecycle": s.lifecycle.short_tag(),
                    "version": s.version,
                    "module_id": superx_ops::record_uuid(&s.entity_id),
                }));
            }
        }
    }
    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await
        .map(|a| a.len())
        .unwrap_or(0);
    Json(serde_json::json!({
        "os": "running",
        "ui_version": env!("CARGO_PKG_VERSION"),
        "agents": agents,
        "modules": modules,
    }))
}

#[derive(serde::Deserialize)]
struct CommandRequest {
    argv: Vec<String>,
}

async fn api_command(
    State(state): State<AppState>,
    Json(req): Json<CommandRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let kernel = &state.kernel;
    let Some(cmd) = req.argv.first().map(String::as_str) else {
        return err_response("empty command");
    };
    if !COMMAND_WHITELIST.contains(&cmd) {
        return err_response(&format!(
            "'{cmd}' is not executable from the UI (allowed: {}) — lifecycle commands arrive in UI v1.1",
            COMMAND_WHITELIST.join(", ")
        ));
    }
    let result: std::result::Result<String, String> = match (cmd, req.argv.get(1).map(String::as_str)) {
        ("agents", _) => superx_ops::run_agents(kernel).await,
        ("sessions", agent) => superx_ops::run_sessions(kernel, agent).await,
        ("actions", _) => superx_ops::run_actions(kernel, 50, None).await.map(|(t, _)| t),
        ("read", Some(q)) => match superx_ops::resolve_session(kernel, q).await {
            Ok(id) => superx_ops::run_read(kernel, id).await.map(|(t, _)| t),
            Err(e) => Err(e),
        },
        ("read", None) => Err("usage: read <session fragment>".to_string()),
        ("modules", Some("list") | None) => superx_ops::run_modules_list(kernel).await,
        ("modules", Some(other)) => Err(format!(
            "'modules {other}' is not executable from the UI — use the terminal"
        )),
        _ => Err("unsupported".to_string()),
    };
    match result {
        Ok(output) => (
            StatusCode::OK,
            Json(serde_json::json!({ "output": output, "is_error": false })),
        ),
        Err(e) => err_response(&e),
    }
}

fn err_response(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "output": msg, "is_error": true })),
    )
}
