//! The UI module's HTTP layer (epic #141 P4+P5): placeholder page,
//! typed read API, charts aggregation, ONE-poller SSE bridge, and
//! whitelisted command execution with history persisted to the
//! module's OWN database.

use std::collections::HashMap;
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
use crate::MODULE_NAME;

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

#[derive(Clone)]
struct AppState {
    kernel: Kernel,
    events: broadcast::Sender<String>,
    /// Answers already computed, by key, with when they were (#390).
    /// The long ranges cost ten seconds and the page polls every
    /// fifteen; between two polls the answer barely moves, and two
    /// pilots on the same range should not each pay for it.
    cache: std::sync::Arc<std::sync::Mutex<HashMap<String, (std::time::Instant, String)>>>,
}

impl AppState {
    /// A body computed less than `ttl` ago, if there is one.
    fn cached(&self, key: &str, ttl: u64) -> Option<String> {
        if ttl == 0 {
            return None;
        }
        let map = self.cache.lock().ok()?;
        let (at, body) = map.get(key)?;
        (at.elapsed().as_secs() < ttl).then(|| body.clone())
    }

    fn remember(&self, key: &str, body: &str) {
        if let Ok(mut map) = self.cache.lock() {
            // Bounded: one entry per range plus a handful — a runaway
            // key space would be a leak, not a cache.
            if map.len() >= CACHE_ENTRIES {
                map.clear();
            }
            map.insert(key.to_string(), (std::time::Instant::now(), body.to_string()));
        }
    }
}

/// How long the thrown-away answer is held. Git blame across every
/// repository is expensive and the answer only moves when a commit
/// lands, so this is minutes where the status page is seconds.
const THROWN_CACHE_SECS: u64 = 600; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Distinct answers held at once.
const CACHE_ENTRIES: usize = 32; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A JSON body that is already serialized.
fn json_body(body: String) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

/// Every request answers within the read budget (#415 QA). The reads
/// await the substrate without a bound, and a reply that never came left
/// the page on the last range's figures with nothing saying so: an answer
/// that does not come in time is a failure the page shows, in the
/// `{ is_error, output }` shape of every other. `None` is no bound. The
/// event stream's handler answers at once, so its stream is never cut.
async fn bounded(
    State(budget): State<Option<Duration>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let Some(budget) = budget else {
        return next.run(request).await;
    };
    match tokio::time::timeout(budget, next.run(request)).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({
                "output": format!("no answer within {budget:?} ({})", crate::READ_TIMEOUT_SECS_PARAM),
                "is_error": true,
            })),
        )
            .into_response(),
    }
}

/// Bind and spawn the server + the single SSE poller task.
pub async fn spawn(kernel: Kernel, port: u16) -> Result<()> {
    let (events, _) = broadcast::channel(1024);
    let state = AppState {
        kernel: kernel.clone(),
        events: events.clone(),
        cache: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
    };
    // Read once, here: resolving it per request would itself await the
    // substrate that might not answer.
    let secs = crate::resolved_read_timeout_secs(&kernel).await;
    let budget = (secs > 0).then(|| Duration::from_secs(secs));
    let app = Router::new()
        .route("/api/status", get(api_status))
        .route("/api/agents", get(api_agents))
        .route("/api/sessions", get(api_sessions))
        .route("/api/sessions/{id}/activity", get(api_session_activity))
        .route("/api/activity", get(api_activity))
        .route("/api/stats", get(api_stats))
        .route("/api/insights", get(api_insights))
        .route("/api/compare", get(api_compare))
        .route("/api/events", get(api_events))
        .route("/api/command", post(api_command))
        .route_layer(axum::middleware::from_fn_with_state(budget, bounded))
        .fallback(get(static_assets))
        .with_state(state);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| KernelError::Module(format!("ui cannot bind {addr}: {e}")))?;
    // Stop means stop (M0): the kernel cancels this token on
    // `modules disable`/`restart`, axum closes the listener and the
    // port is free for the next start.
    let stop = kernel.module_token(MODULE_NAME);
    let poller_stop = stop.clone();
    tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(stop.cancelled())
            .await;
        if let Err(e) = served {
            tracing::error!(target: "ui", error = %e, "ui server exited");
        } else {
            tracing::info!(target: "ui", "ui server closed");
        }
    });
    tokio::spawn(sse_poller(kernel, events, poller_stop));
    Ok(())
}

/// ONE poller for all SSE clients: bridges the kernel's live
/// primitives into the broadcast channel; idles when nobody listens.
async fn sse_poller(
    kernel: Kernel,
    tx: broadcast::Sender<String>,
    stop: superx_kernel::supervise::CancelToken,
) {
    // One cursor per stream (#413). Both reads are capped at a batch, and
    // a shared cursor advanced to the newer of the two ends skipped every
    // row the other stream had not reached yet — in a burst, whole runs of
    // the feed never arrived.
    let mut after_events = chrono::Utc::now();
    let mut after_messages = after_events;
    loop {
        let poll = superx_ops::live_poll_secs(&kernel).await;
        tokio::time::sleep(Duration::from_secs(poll)).await;
        // A stopped module leaves no tasks behind (M0): the poll
        // boundary is the natural place to notice.
        if stop.is_cancelled() {
            tracing::info!(target: "ui", "sse poller stopped");
            return;
        }
        if tx.receiver_count() == 0 {
            // Nobody watching — skip ahead.
            after_events = chrono::Utc::now();
            after_messages = after_events;
            continue;
        }
        if let Ok(actions) = kernel.telemetry_since(after_events, SSE_BATCH).await {
            for a in &actions {
                if a.valid_from > after_events {
                    after_events = a.valid_from;
                }
                let ev = crate::activity::action_event(a);
                if let Ok(json) = serde_json::to_string(&ev) {
                    let _receivers = tx.send(json);
                }
            }
        }
        if let Ok(messages) = kernel.messages_since(after_messages, SSE_BATCH).await {
            for m in &messages {
                if m.valid_from > after_messages {
                    after_messages = m.valid_from;
                }
                let ev = crate::activity::message_event(m);
                if let Ok(json) = serde_json::to_string(&ev) {
                    let _receivers = tx.send(json);
                }
            }
        }
    }
}

/// Serve the dashboard: the exact asset when it exists, index.html as
/// the SPA fallback for route-shaped paths, and a 404 for a file that
/// is not there (#352).
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
    if let Some(found) = serve(path) {
        return found;
    }
    // The SPA fallback exists for client-side routes, which carry no
    // extension. A FILE request that misses — the bundle an older
    // index.html still names, in an open tab or a back/forward-cache
    // entry — must say so. Served index.html in its place, the browser
    // fails to parse HTML as a script and the tab keeps showing what
    // it last rendered, which the operator read as "the UI reverted"
    // when the truth was "the UI failed to load" (#352).
    if names_a_file(path) {
        return (StatusCode::NOT_FOUND, format!("no such asset: /{path}")).into_response();
    }
    serve("index.html").unwrap_or_else(|| {
        (StatusCode::NOT_FOUND, Html("dashboard not built — run npm run build in crates/superx-mod-ui/ui")).into_response()
    })
}

/// A request path that names a file rather than a route: anything
/// under `assets/`, or whose last segment carries an extension.
fn names_a_file(path: &str) -> bool {
    path.starts_with("assets/") || path.rsplit('/').next().is_some_and(|last| last.contains('.'))
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
                // D-UI2: a module that serves its own UI publishes
                // its URL on its registry entity; never a link to
                // ourselves.
                let ui_url = if s.name == crate::MODULE_NAME {
                    None
                } else {
                    match kernel
                        .get_parameter(s.entity_id.clone(), "attr_module_ui_url")
                        .await
                    {
                        Ok(Some(superx_kernel::types::Value::String(u))) => Some(u),
                        _ => None,
                    }
                };
                modules.push(ModuleView {
                    name: s.name,
                    kind: kind.type_uid().trim_start_matches("node_").to_string(),
                    lifecycle: s.lifecycle.short_tag().to_string(),
                    version: s.version,
                    module_id: superx_ops::record_uuid(&s.entity_id),
                    provisioned,
                    ui_url,
                });
            }
        }
    }
    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await
        .map(|a| a.len())
        .unwrap_or(0);
    let default_range = crate::resolved_default_range(kernel).await;
    Json(StatusResponse {
        os: "running".into(),
        ui_version: env!("CARGO_PKG_VERSION").into(),
        agents,
        modules,
        default_range,
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
) -> axum::response::Response {
    let kernel = &state.kernel;
    // Four substrate round-trips per session (#380); until that is one
    // query, at least two pollers share the cost (#390).
    let ttl = crate::resolved_cache_secs(kernel).await;
    let key = format!("sessions:{}", q.agent.clone().unwrap_or_default());
    if let Some(body) = state.cached(&key, ttl) {
        return json_body(body);
    }
    let mut out = Vec::new();
    let Ok(sessions) = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await
    else {
        return json_body(serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string()));
    };
    // The context bar's denominator — resolved once per request.
    let window = crate::resolved_context_window(kernel).await;
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
        let last_active = crate::activity::session_last_emitted(kernel, s.entity_id.clone())
            .await
            .ok()
            .flatten()
            .map(|t| t.to_rfc3339());
        // Token telemetry from the captured raw events (issue #200);
        // best-effort — a session with no usage data shows nothing.
        let (context_tokens, output_tokens) =
            crate::activity::session_token_stats(kernel, s.entity_id.clone())
                .await
                .unwrap_or((None, None));
        let context_pct = context_tokens.map(|c| ((c * 100) / window).clamp(0, 100));
        let (model, effort) = crate::activity::session_model_effort(kernel, s.entity_id.clone())
            .await
            .unwrap_or((None, None));
        let uuid = superx_ops::record_uuid(&s.entity_id);
        out.push(SessionView {
            identity: format!("{agent}/{uuid}"),
            session_id: uuid,
            agent,
            src,
            actions: count,
            context_tokens,
            context_pct,
            output_tokens,
            last_active,
            model,
            effort,
        });
    }
    match serde_json::to_string(&out) {
        Ok(body) => {
            state.remember(&key, &body);
            json_body(body)
        }
        Err(_) => json_body("[]".to_string()),
    }
}

#[derive(serde::Deserialize)]
struct ActivityQuery {
    limit: Option<u32>,
    /// Walk BACKWARDS from here (RFC3339): the newest page strictly
    /// older than this instant. Absent = the present (issue #241).
    before: Option<String>,
    /// Keyword filter, matched in the engine against captured message
    /// text and action payloads — so it searches ALL history, not just
    /// the page the client happens to hold (issue #241).
    q: Option<String>,
}

/// A blank or whitespace-only keyword is NO filter, not a filter that
/// matches everything by accident.
fn keyword(raw: Option<&String>) -> Option<&str> {
    raw.map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// Session-activity backlog page default and bound (issue #172).
const ACTIVITY_BACKLOG_DEFAULT: u32 = 500; // skill-allow: §9-const — render page default, query-param overridable
const ACTIVITY_BACKLOG_MAX: u32 = 2000; // skill-allow: §9-const — render page bound

/// Parse the backwards cursor. A malformed one is an error, never a
/// silent fall back to the present — that would serve the newest page
/// again and the feed would loop forever instead of paging back.
fn parse_before(raw: Option<&String>) -> std::result::Result<crate::activity::Before, String> {
    match raw {
        None => Ok(None),
        Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(t) => Ok(Some(t.with_timezone(&chrono::Utc))),
            Err(e) => Err(format!("before must be an RFC3339 timestamp: {e}")),
        },
    }
}

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
    let before = match parse_before(q.before.as_ref()) {
        Ok(b) => b,
        Err(e) => return Response::err(e),
    };
    match crate::activity::session_activity(kernel, session, limit, before, keyword(q.q.as_ref()))
        .await
    {
        Ok(events) => Response::ok(events),
        Err(e) => Response::err(e.to_string()),
    }
}

/// The Status page's aggregation (issue #228).
#[derive(serde::Deserialize)]
struct RangeQuery {
    range: Option<String>,
    /// The viewer's offset from UTC, in minutes east (#415 review).
    tz: Option<i32>,
}

/// The viewer's clock, for the hours and days the page buckets by: the
/// offset its browser reports, in minutes east of UTC. Absent or out of
/// range, UTC — a stale bookmark still renders.
fn viewer_clock(tz: Option<i32>) -> chrono::FixedOffset {
    tz.and_then(|m| m.checked_mul(60))
        .and_then(chrono::FixedOffset::east_opt)
        .unwrap_or_else(|| chrono::Offset::fix(&chrono::Utc))
}

/// The viewer's offset alone — for the endpoints that take nothing else.
#[derive(serde::Deserialize)]
struct ClockQuery {
    tz: Option<i32>,
}

async fn api_stats(
    State(state): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let kernel = &state.kernel;
    let window = crate::resolved_stats_window(kernel).await;
    // Scroll-back (#326): a named range replaces the fixed newest-N
    // read. Unknown values fall back to the window rather than
    // erroring, so a stale bookmark still renders.
    let range = q.range.unwrap_or_else(|| "window".to_string());
    let range = if crate::RANGES.contains(&range.as_str()) {
        range
    } else {
        "window".to_string()
    };
    let clock = viewer_clock(q.tz);
    let ttl = crate::resolved_cache_secs(kernel).await;
    let key = format!("stats:{range}:{window}:{}", clock.local_minus_utc());
    if let Some(body) = state.cached(&key, ttl) {
        return json_body(body);
    }
    match crate::stats::stats_for_range_on(kernel, window, &range, clock).await {
        Ok(s) => match serde_json::to_string(&s) {
            Ok(body) => {
                state.remember(&key, &body);
                json_body(body)
            }
            Err(e) => err_response(&e.to_string()).into_response(),
        },
        Err(e) => err_response(&e.to_string()).into_response(),
    }
}

/// Deep statistics (issue #237) — all-history aggregates computed in
/// the engine. Separate from `/api/stats` so the live tiles keep their
/// fast refresh while these poll lazily.
async fn api_insights(State(state): State<AppState>, Query(q): Query<ClockQuery>) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    // Whole-history, and seconds to compute (#415 review): held for the
    // same while as the Status page's own figures, so two pilots — or two
    // tabs — pay for it once.
    let clock = viewer_clock(q.tz);
    let ttl = crate::resolved_cache_secs(&state.kernel).await;
    let key = format!("insights:{}", clock.local_minus_utc());
    if let Some(body) = state.cached(&key, ttl) {
        return json_body(body);
    }
    match crate::insights::insights_summary_on(&state.kernel, clock).await {
        Ok(s) => match serde_json::to_string(&s) {
            Ok(body) => {
                state.remember(&key, &body);
                json_body(body)
            }
            Err(e) => err_response(&e.to_string()).into_response(),
        },
        Err(e) => err_response(&e.to_string()).into_response(),
    }
}

/// Model comparison (#406): the switches, and whether each model
/// stayed on the objective. Same cache lifetime and same reason as
/// [`THROWN_CACHE_SECS`] — it walks git, not the message stream.
async fn api_compare(State(state): State<AppState>) -> axum::response::Response {
    const KEY: &str = "compare";
    if let Some(body) = state.cached(KEY, THROWN_CACHE_SECS) {
        return json_body(body);
    }
    let runs = match crate::thrown::model_runs(&state.kernel).await {
        Ok(r) => r,
        Err(e) => return json_body(format!("{{\"error\":{}}}", json_str(&e.to_string()))),
    };
    let mainlines = crate::resolved_mainline_refs(&state.kernel).await;
    let c = crate::compare::compare(&runs, &mainlines).await;
    let summary = crate::api::CompareSummary {
        handoffs: c.handoffs,
        deviations: c.deviations,
        repos: c.repos,
        unjudged: c.unjudged,
        computed_at: chrono::Utc::now().to_rfc3339(),
    };
    match serde_json::to_string(&summary) {
        Ok(body) => {
            state.remember(KEY, &body);
            json_body(body)
        }
        Err(e) => json_body(format!("{{\"error\":{}}}", json_str(&e.to_string()))),
    }
}

/// A JSON string literal, quotes and escapes included.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
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
    let before = match parse_before(q.before.as_ref()) {
        Ok(b) => b,
        Err(e) => return Response::err(e),
    };
    match crate::activity::global_activity(kernel, limit, before, keyword(q.q.as_ref())).await {
        Ok(events) => Response::ok(events),
        Err(e) => Response::err(e.to_string()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewers_clock_is_what_the_browser_reports_else_utc() {
        let utc = chrono::Offset::fix(&chrono::Utc);
        assert_eq!(viewer_clock(Some(-240)), chrono::FixedOffset::west_opt(4 * 3600).expect("UTC-4"));
        assert_eq!(viewer_clock(Some(330)), chrono::FixedOffset::east_opt(330 * 60).expect("UTC+5:30"));
        assert_eq!(viewer_clock(None), utc);
        assert_eq!(viewer_clock(Some(100_000)), utc, "past a day is no offset");
        assert_eq!(viewer_clock(Some(i32::MAX)), utc, "and never overflows");
    }

    /// An answer that never comes is a failure within the budget (#415
    /// QA); one inside it passes untouched, and no budget holds nothing.
    #[tokio::test]
    async fn a_request_that_never_answers_fails_within_the_budget() {
        use tower::ServiceExt as _;
        // `get` in this module fetches a static asset; routes need axum's.
        use axum::routing::get as route_get;
        // A budget far shorter than any real one; the slow handler takes
        // twice as long, and the generous budget a hundred times.
        const BUDGET: Duration = Duration::from_millis(20); // skill-allow: §9-duration — test fixture, not a policy
        let app = |budget: Option<Duration>| {
            Router::new()
                .route("/never", route_get(std::future::pending::<&'static str>))
                .route(
                    "/slow",
                    route_get(|| async {
                        tokio::time::sleep(BUDGET * 2).await;
                        "late"
                    }),
                )
                .route_layer(axum::middleware::from_fn_with_state(budget, bounded))
        };
        let call = |app: Router, path: &'static str| {
            app.oneshot(axum::http::Request::get(path).body(axum::body::Body::empty()).expect("request"))
        };

        let never = call(app(Some(BUDGET)), "/never").await.expect("response");
        assert_eq!(never.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(never.into_body(), usize::MAX).await.expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["is_error"], true);
        let output = body["output"].as_str().unwrap_or_default();
        assert!(
            output.contains(&format!("{BUDGET:?}")) && output.contains(crate::READ_TIMEOUT_SECS_PARAM),
            "{output}"
        );

        let within = call(app(Some(BUDGET * 100)), "/slow").await.expect("response");
        assert_eq!(within.status(), StatusCode::OK, "an answer inside the budget passes");
        let unbounded = call(app(None), "/slow").await.expect("response");
        assert_eq!(unbounded.status(), StatusCode::OK, "no budget, no bound");
    }

    async fn get(path: &str) -> axum::response::Response {
        static_assets(path.parse::<axum::http::Uri>().expect("uri")).await
    }

    fn content_type(r: &axum::response::Response) -> &str {
        r.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    #[test]
    fn a_route_has_no_extension_and_a_file_has_one() {
        assert!(names_a_file("assets/index-Bca9TEJN.js"));
        assert!(names_a_file("logo.svg"));
        assert!(names_a_file("sessions/report.json"));
        assert!(!names_a_file("status"));
        assert!(!names_a_file("sessions/0199a1b2-7c3d-4e5f-8a9b-0c1d2e3f4a5b"));
        assert!(!names_a_file(""));
    }

    /// The bundle an older index.html still names is gone: the answer
    /// is a 404 that says so, not index.html wearing a script's name
    /// (#352).
    #[tokio::test]
    async fn a_missing_file_is_a_404_not_the_index_page() {
        let gone = get("/assets/index-Bca9TEJN.js").await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        assert!(content_type(&gone).starts_with("text/plain"), "{}", content_type(&gone));
        let stray = get("/favicon.ico").await;
        assert_eq!(stray.status(), StatusCode::NOT_FOUND);
    }

    /// Everything the fallback was for still works: the root, a
    /// route-shaped path, and a real file with its own type and
    /// cache policy.
    #[tokio::test]
    async fn the_root_routes_and_real_files_still_serve() {
        for path in ["/", "/status", "/sessions/0199a1b2-7c3d-4e5f-8a9b-0c1d2e3f4a5b"] {
            let r = get(path).await;
            assert_eq!(r.status(), StatusCode::OK, "{path}");
            assert!(content_type(&r).starts_with("text/html"), "{path}: {}", content_type(&r));
            assert_eq!(
                r.headers().get(axum::http::header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
                Some("no-store"),
                "{path}"
            );
        }
        let logo = get("/logo.svg").await;
        assert_eq!(logo.status(), StatusCode::OK);
        assert!(content_type(&logo).starts_with("image/svg+xml"), "{}", content_type(&logo));
    }
}
