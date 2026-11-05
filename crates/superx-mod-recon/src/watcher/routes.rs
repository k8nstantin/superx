//! The watcher's routes (#573): what it holds at each look; where the time
//! went, in the operator's own days; what it dug out; a session's lines in
//! a span, which is how an alert or a finding is read back to its cause;
//! and the calls asked for.
//!
//! - `GET /api/watch`
//! - `GET /api/watch/time?offset=<minutes east of UTC>`
//! - `GET /api/watch/found`
//! - `GET /api/watch/lines?session=<id>&from=<time>&to=<time>`
//! - `GET /api/watch/calls?session=&tool=&target=&under=&from=&to=`

use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;

use super::{measure, CallsAsked, Watched};

/// The watcher's routes, for any server whose state holds the watcher.
pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    Watched: FromRef<S>,
{
    Router::new()
        .route("/api/watch", get(watch))
        .route("/api/watch/time", get(time))
        .route("/api/watch/found", get(found))
        .route("/api/watch/lines", get(lines))
        .route("/api/watch/calls", get(calls))
}

fn refused(error: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": error }))).into_response()
}

/// What the watcher holds at its newest look.
pub async fn watch(State(watched): State<Watched>) -> Response {
    Json(watched.with(|w| w.snapshot())).into_response()
}

/// The operator's time zone, as minutes east of UTC, as `/api/stats` is
/// asked.
#[derive(Debug, Deserialize)]
pub struct Zone {
    pub offset: Option<i32>,
}

/// Where the time went, in the operator's days.
pub async fn time(State(watched): State<Watched>, Query(q): Query<Zone>) -> Response {
    let minutes = q.offset.unwrap_or(0);
    let Some(zone) = minutes.checked_mul(60).and_then(FixedOffset::east_opt) else {
        return refused(format!("offset wants minutes east of UTC within a day, not {minutes}"));
    };
    Json(watched.with(|w| measure::time(w.tracks(), zone))).into_response()
}

/// Everything the watcher dug out, newest first.
pub async fn found(State(watched): State<Watched>) -> Response {
    Json(watched.with(|w| w.found())).into_response()
}

/// A session and a span of it.
#[derive(Debug, Deserialize)]
pub struct Span {
    pub session: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// A session's lines in a span.
pub async fn lines(State(watched): State<Watched>, Query(q): Query<Span>) -> Response {
    if q.from > q.to {
        return refused(format!("from {} is after to {}", q.from, q.to));
    }
    match watched.with(|w| w.lines(&q.session, q.from, q.to)) {
        Some(lines) => Json(lines).into_response(),
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("no session {} in the window", q.session) }))).into_response(),
    }
}

/// Which calls: each left out asks for any.
#[derive(Debug, Deserialize)]
pub struct Which {
    pub session: Option<String>,
    pub tool: Option<String>,
    pub target: Option<String>,
    pub under: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

/// The calls asked for, newest first.
pub async fn calls(State(watched): State<Watched>, Query(q): Query<Which>) -> Response {
    let asked = CallsAsked { session: q.session, tool: q.tool, target: q.target, under: q.under, from: q.from, to: q.to };
    Json(watched.with(|w| w.calls(&asked))).into_response()
}
