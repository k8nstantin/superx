//! The watcher's route (#573): `GET /api/watch?offset=<minutes east of
//! UTC>`, its snapshot with the measures in the operator's own days.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::measure::{self, DayRow, ModelRow};
use super::{Snapshot, Watched};

/// What `GET /api/watch` is asked: the operator's time zone, as minutes
/// east of UTC, as `/api/stats` is.
#[derive(Debug, Deserialize)]
pub struct Asked {
    pub offset: Option<i32>,
}

/// What `GET /api/watch` answers.
#[derive(Debug, Serialize)]
pub struct View {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    pub models: Vec<ModelRow>,
    pub days: Vec<DayRow>,
}

/// The snapshot, with the measures by model and by day.
pub async fn watch(State(watched): State<Watched>, Query(q): Query<Asked>) -> Response {
    let minutes = q.offset.unwrap_or(0);
    let Some(zone) = minutes.checked_mul(60).and_then(chrono::FixedOffset::east_opt) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("offset wants minutes east of UTC within a day, not {minutes}") })))
            .into_response();
    };
    let snapshot = watched.snapshot();
    let models = measure::models(&snapshot);
    let days = measure::days(&snapshot, zone);
    Json(View { snapshot, models, days }).into_response()
}
