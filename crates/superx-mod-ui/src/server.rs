//! The UI module's HTTP layer (epic #141 P4+P5): placeholder page,
//! typed read API, charts aggregation, ONE-poller SSE bridge, and
//! whitelisted command execution with history persisted to the
//! module's OWN database.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_core::Stream;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};
use tokio::sync::broadcast;

use crate::api::*;

/// The built dashboard (Vite output). Debug builds read from disk
/// (iterate with `npm run build` without recompiling Rust); release
/// builds embed the files in the binary.
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

/// Commands the UI may execute — the read surface only. Lifecycle
/// (stop/restart) is deliberately excluded until UI v1.1
/// (operator-decided): this server lives INSIDE the daemon.
const COMMAND_WHITELIST: &[&str] = &["agents", "sessions", "actions", "read", "modules"];

/// SSE poller page size per tick.
const SSE_BATCH: u32 = 200; // skill-allow: §9-const — poll page bound

/// Charts aggregation source window.
const CHART_EVENT_WINDOW: u32 = 2000; // skill-allow: §9-const — aggregation page bound

#[derive(Clone)]
struct AppState {
    kernel: Kernel,
    events: broadcast::Sender<String>,
}

/// Bind and spawn the server + the single SSE poller task.
pub async fn spawn(kernel: Kernel, port: u16) -> Result<()> {
    let (events, _) = broadcast::channel(1024);
    let state = AppState {
        kernel: kernel.clone(),
        events: events.clone(),
    };
    let app = Router::new()
        .route("/api/status", get(api_status))
        .route("/api/agents", get(api_agents))
        .route("/api/sessions", get(api_sessions))
        .route("/api/sessions/{id}/activity", get(api_session_activity))
        .route("/api/activity", get(api_activity))
        .route("/api/actions", get(api_actions))
        .route("/api/charts/summary", get(api_charts))
        .route("/api/events", get(api_events))
        .route("/api/command", post(api_command))
        .fallback(get(static_assets))
        .with_state(state);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("ui cannot bind {addr}: {e}")))?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(target: "ui", error = %e, "ui server exited");
        }
    });
    tokio::spawn(sse_poller(kernel, events));
    Ok(())
}

/// ONE poller for all SSE clients: bridges the kernel's live
/// primitives into the broadcast channel; idles when nobody listens.
async fn sse_poller(kernel: Kernel, tx: broadcast::Sender<String>) {
    let mut after = chrono::Utc::now();
    loop {
        let poll = superx_ops::live_poll_secs(&kernel).await;
        tokio::time::sleep(Duration::from_secs(poll)).await;
        if tx.receiver_count() == 0 {
            after = chrono::Utc::now(); // nobody watching — skip ahead
            continue;
        }
        let mut high = after;
        if let Ok(actions) = kernel.telemetry_since(after, SSE_BATCH).await {
            for a in &actions {
                if a.valid_from > high {
                    high = a.valid_from;
                }
                let ev = crate::activity::action_event(a);
                if let Ok(json) = serde_json::to_string(&ev) {
                    let _receivers = tx.send(json);
                }
            }
        }
        if let Ok(messages) = kernel.messages_since(after, SSE_BATCH).await {
            for m in &messages {
                if m.valid_from > high {
                    high = m.valid_from;
                }
                let ev = crate::activity::message_event(m);
                if let Ok(json) = serde_json::to_string(&ev) {
                    let _receivers = tx.send(json);
                }
            }
        }
        after = high;
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
            (StatusCode::NOT_FOUND, Html("dashboard not built — run npm run build in crates/superx-mod-ui/ui")).into_response()
        })
}

async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let kernel = &state.kernel;
    let mut modules = Vec::new();
    for kind in [NodeKind::KernelModule, NodeKind::Adapter] {
        if let Ok(list) = kernel.list_with_status(kind).await {
            for s in list {
                let provisioned = kernel
                    .latest_module_record(&s.name)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.provisioned);
                modules.push(ModuleView {
                    name: s.name,
                    kind: kind.type_uid().trim_start_matches("node_").to_string(),
                    lifecycle: s.lifecycle.short_tag().to_string(),
                    version: s.version,
                    module_id: superx_ops::record_uuid(&s.entity_id),
                    provisioned,
                });
            }
        }
    }
    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await
        .map(|a| a.len())
        .unwrap_or(0);
    Json(StatusResponse {
        os: "running".into(),
        ui_version: env!("CARGO_PKG_VERSION").into(),
        agents,
        modules,
    })
}

async fn api_agents(State(state): State<AppState>) -> Json<Vec<AgentView>> {
    let kernel = &state.kernel;
    let mut out = Vec::new();
    let (Ok(agents), Ok(sessions), Ok(sources)) = (
        kernel
            .list_named_entities("node_agent", "attr_agent_descriptor")
            .await,
        kernel
            .list_named_entities("node_session", "attr_session_descriptor")
            .await,
        kernel
            .list_named_entities("node_source", "attr_source_descriptor")
            .await,
    ) else {
        return Json(out);
    };
    let name_of = |payload: &superx_kernel::types::Value| -> Option<String> {
        match payload {
            superx_kernel::types::Value::Object(o) => match o.get("name") {
                Some(superx_kernel::types::Value::String(s)) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    };
    for a in &agents {
        let Some(name) = name_of(&a.payload) else { continue };
        let prefix = format!("{name}/");
        out.push(AgentView {
            agent_id: superx_ops::record_uuid(&a.entity_id),
            sources: sources
                .iter()
                .filter(|s| name_of(&s.payload).is_some_and(|n| n.starts_with(&prefix)))
                .count(),
            sessions: sessions
                .iter()
                .filter(|s| name_of(&s.payload).is_some_and(|n| n.starts_with(&prefix)))
                .count(),
            name,
        });
    }
    Json(out)
}

#[derive(serde::Deserialize)]
struct SessionsQuery {
    agent: Option<String>,
}

async fn api_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> Json<Vec<SessionView>> {
    let kernel = &state.kernel;
    let mut out = Vec::new();
    let Ok(sessions) = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await
    else {
        return Json(out);
    };
    // Resolve every agent ONCE — the per-session action count takes a
    // pre-resolved scope instead of re-reading descriptors and
    // re-resolving agents per row (review finding, issue #187).
    let mut agent_ids: std::collections::HashMap<String, superx_kernel::types::RecordId> =
        std::collections::HashMap::new();
    if let Ok(agents) = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await
    {
        for a in &agents {
            if let superx_kernel::types::Value::Object(o) = &a.payload {
                if let Some(superx_kernel::types::Value::String(n)) = o.get("name") {
                    agent_ids.insert(n.clone(), a.entity_id.clone());
                }
            }
        }
    }
    for s in sessions {
        let name = match &s.payload {
            superx_kernel::types::Value::Object(o) => match o.get("name") {
                Some(superx_kernel::types::Value::String(v)) => v.clone(),
                _ => continue,
            },
            _ => continue,
        };
        let agent = name.split('/').next().unwrap_or("?").to_string();
        if let Some(want) = &q.agent {
            if agent != *want {
                continue;
            }
        }
        let src = name.split_once('/').map_or("", |(_, r)| r).to_string();
        // TOTAL activity — messages + the session's action events
        // (issue #187: the list counts everything the feed shows).
        let scope = if src.is_empty() {
            None
        } else {
            agent_ids.get(&agent).map(|id| (id.clone(), src.clone()))
        };
        let count = kernel
            .session_message_count(s.entity_id.clone())
            .await
            .unwrap_or(0)
            + crate::activity::session_action_count(kernel, s.entity_id.clone(), scope)
                .await
                .unwrap_or(0);
        let last_active = kernel
            .session_last_activity(s.entity_id.clone())
            .await
            .ok()
            .flatten()
            .map(|t| t.to_rfc3339());
        let uuid = superx_ops::record_uuid(&s.entity_id);
        out.push(SessionView {
            identity: format!("{agent}/{uuid}"),
            session_id: uuid,
            agent,
            src,
            actions: count,
            last_active,
        });
    }
    Json(out)
}

#[derive(serde::Deserialize)]
struct ActivityQuery {
    limit: Option<u32>,
}

/// Session-activity backlog page default and bound (issue #172).
const ACTIVITY_BACKLOG_DEFAULT: u32 = 500; // skill-allow: §9-const — render page default, query-param overridable
const ACTIVITY_BACKLOG_MAX: u32 = 2000; // skill-allow: §9-const — render page bound

/// Everything the OS captured for one session — messages + actions,
/// merged chronologically (issue #172). Same row shape as the SSE
/// bridge and the global feed (issue #187).
async fn api_session_activity(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<ActivityQuery>,
) -> Response<Vec<SseEvent>> {
    let kernel = &state.kernel;
    let session = match superx_ops::resolve_session(kernel, &id).await {
        Ok(s) => s,
        Err(e) => return Response::err(e),
    };
    let limit = q
        .limit
        .unwrap_or(ACTIVITY_BACKLOG_DEFAULT)
        .min(ACTIVITY_BACKLOG_MAX);
    match crate::activity::session_activity(kernel, session, limit).await {
        Ok(events) => Response::ok(events),
        Err(e) => Response::err(e.to_string()),
    }
}

/// The GLOBAL feed backlog — everything the OS captured, merged
/// chronologically (issue #187): the Activity page's history.
async fn api_activity(
    State(state): State<AppState>,
    Query(q): Query<ActivityQuery>,
) -> Response<Vec<SseEvent>> {
    let kernel = &state.kernel;
    let limit = q
        .limit
        .unwrap_or(ACTIVITY_BACKLOG_DEFAULT)
        .min(ACTIVITY_BACKLOG_MAX);
    match crate::activity::global_activity(kernel, limit).await {
        Ok(events) => Response::ok(events),
        Err(e) => Response::err(e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct ActionsQuery {
    limit: Option<u32>,
}

async fn api_actions(
    State(state): State<AppState>,
    Query(q): Query<ActionsQuery>,
) -> Json<Vec<ActionView>> {
    let kernel = &state.kernel;
    let limit = q.limit.unwrap_or(50).min(SSE_BATCH); // skill-allow: §9-or — render page default, query-param overridable
    let mut events = kernel.recent_telemetry(limit).await.unwrap_or_default();
    events.reverse();
    Json(
        events
            .iter()
            .map(|e| ActionView {
                event: e.lifecycle_event.clone(),
                summary: superx_ops::render_event(e).trim_end().to_string(),
                agent_id: e.agent.as_ref().map(superx_ops::record_uuid),
                valid_from: e.valid_from.to_rfc3339(),
            })
            .collect(),
    )
}

async fn api_charts(State(state): State<AppState>) -> Json<ChartsSummary> {
    let kernel = &state.kernel;
    let events = kernel
        .recent_telemetry(CHART_EVENT_WINDOW)
        .await
        .unwrap_or_default();

    // Events per minute (over the fetched window).
    let mut per_minute: std::collections::BTreeMap<String, i64> = Default::default();
    let mut per_agent_id: std::collections::BTreeMap<String, i64> = Default::default();
    let mut boots = Vec::new();
    for e in &events {
        let minute = e.valid_from.format("%H:%M").to_string();
        *per_minute.entry(minute).or_insert(0) += 1;
        if let Some(agent) = &e.agent {
            *per_agent_id.entry(superx_ops::record_uuid(agent)).or_insert(0) += 1;
        }
        if e.lifecycle_event == "boot_complete" {
            if let superx_kernel::types::Value::Object(o) = &e.payload {
                if let Some(superx_kernel::types::Value::Number(n)) = o.get("duration_ms") {
                    boots.push(TimeCount {
                        t: e.valid_from.format("%m-%d %H:%M").to_string(),
                        value: n.to_int().unwrap_or(0),
                    });
                }
            }
        }
    }
    // Agent ids → names.
    let mut per_agent = Vec::new();
    if let Ok(agents) = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await
    {
        for a in &agents {
            let uuid = superx_ops::record_uuid(&a.entity_id);
            if let Some(count) = per_agent_id.get(&uuid) {
                let name = match &a.payload {
                    superx_kernel::types::Value::Object(o) => match o.get("name") {
                        Some(superx_kernel::types::Value::String(s)) => s.clone(),
                        _ => uuid.clone(),
                    },
                    _ => uuid.clone(),
                };
                per_agent.push(NameCount {
                    name,
                    value: *count,
                });
            }
        }
    }
    // Message roles via a grouped query (kernel read handle).
    let mut message_roles = Vec::new();
    if let Ok(mut resp) = kernel
        .db()
        .query("SELECT role, count() AS c FROM message GROUP BY role")
        .await
    {
        if let Ok(rows) = resp.take::<Vec<superx_kernel::types::Value>>(0) {
            for row in rows {
                if let superx_kernel::types::Value::Object(o) = row {
                    let role = match o.get("role") {
                        Some(superx_kernel::types::Value::String(s)) => s.clone(),
                        _ => continue,
                    };
                    let c = match o.get("c") {
                        Some(superx_kernel::types::Value::Number(n)) => n.to_int().unwrap_or(0),
                        _ => 0,
                    };
                    message_roles.push(NameCount { name: role, value: c });
                }
            }
        }
    }
    Json(ChartsSummary {
        events_per_minute: per_minute
            .into_iter()
            .map(|(t, value)| TimeCount { t, value })
            .collect(),
        per_agent,
        message_roles,
        boot_durations: boots,
    })
}

async fn api_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let mut rx = state.events.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(json) => yield Ok(Event::default().data(json)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
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
    // Command history lives in the UI's OWN database (best-effort:
    // an unprovisioned ui db must never break command execution).
    let argv_line = req.argv.join(" ");
    let is_error = result.is_err();
    if let Ok(db) = kernel.module_db(crate::MODULE_NAME).await {
        let outcome = db
            .query("CREATE command_history SET argv = $argv, is_error = $is_error, valid_from = time::now()")
            .bind(("argv", argv_line))
            .bind(("is_error", is_error))
            .await;
        if let Err(e) = outcome {
            tracing::debug!(target: "ui", error = %e, "command history write skipped");
        }
    }
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

/// Tiny ok/err JSON envelope for endpoints that can fail resolution.
enum Response<T: serde::Serialize> {
    Ok(Json<T>),
    Err((StatusCode, Json<serde_json::Value>)),
}

impl<T: serde::Serialize> Response<T> {
    fn ok(value: T) -> Self {
        Self::Ok(Json(value))
    }
    fn err(msg: String) -> Self {
        Self::Err(err_response(&msg))
    }
}

impl<T: serde::Serialize> axum::response::IntoResponse for Response<T> {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Ok(json) => json.into_response(),
            Self::Err(err) => err.into_response(),
        }
    }
}
