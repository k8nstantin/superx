//! The watcher's routes (#566, step 16), behind the server's own Host check:
//! `GET /api/watch`, its snapshot, and `GET /api/watch/memory`, one file of a
//! session's memory folder, whole. Each answers from the snapshot and never
//! reads the kernel's database.

use std::path::Path;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::{Snapshot, Watched};

/// `GET /api/watch`: what the watcher holds.
pub async fn watch(State(watched): State<Watched>) -> Json<Snapshot> {
    Json(watched.snapshot())
}

/// What `GET /api/watch/memory` is asked.
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryQuery {
    pub session: String,
    pub file: String,
}

/// `GET /api/watch/memory?session=…&file=…`: that file of that session's
/// memory folder, read at the request, whole, as text.
pub async fn memory(State(watched): State<Watched>, Query(q): Query<MemoryQuery>) -> Response {
    if q.file.is_empty() || q.file.contains('/') || q.file.contains('\\') || q.file == "." || q.file == ".." {
        return refuse(StatusCode::BAD_REQUEST, format!("file wants the name of a file in a memory folder, not {:?}", q.file));
    }
    if q.session.parse::<uuid::Uuid>().is_err() {
        return refuse(StatusCode::BAD_REQUEST, format!("session wants a session's id, not {:?}", q.session));
    }
    let snapshot = watched.snapshot();
    let Some(session) = snapshot.sessions.iter().find(|s| s.id == q.session) else {
        return refuse(StatusCode::NOT_FOUND, format!("no session {} is watched", q.session));
    };
    let Some(folder) = &session.memory.folder else {
        return refuse(StatusCode::NOT_FOUND, format!("session {} has no memory folder", q.session));
    };
    if !session.memory.files.iter().any(|f| f.name == q.file) {
        return refuse(StatusCode::NOT_FOUND, format!("the memory folder of session {} lists no file {}", q.session, q.file));
    }
    match std::fs::read(Path::new(folder).join(&q.file)) {
        Ok(bytes) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes).into_response(),
        Err(e) => refuse(StatusCode::NOT_FOUND, format!("{}: {e}", q.file)),
    }
}

fn refuse(status: StatusCode, why: String) -> Response {
    (status, Json(serde_json::json!({ "error": why }))).into_response()
}
