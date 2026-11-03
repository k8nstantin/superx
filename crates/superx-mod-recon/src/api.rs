//! What the tab reads and writes (#539): the crawler's stats per day, from
//! the module's own `pass` and `pass_source` rows, and the parameters, read
//! as `status` reads them and set as `superx param set recon` sets them.
//!
//! A write must carry [`WRITE_HEADER`]: a page of another site cannot send a
//! header of its own to this server without asking first, and the server
//! answers no such question, so no page in the operator's browser can change
//! a parameter (with [`crate::server`]'s Host check against a name of its
//! own pointed at 127.0.0.1).

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use superx_kernel::types::Value;
use superx_kernel::{Db, Kernel, NodeKind};

use crate::params::{self, NAMES};
use crate::MODULE_NAME;

/// The header a write carries.
pub const WRITE_HEADER: &str = "x-superx-recon";

/// One UTC day of the crawler's passes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Day {
    /// The day, `YYYY-MM-DD`.
    pub day: String,
    /// Passes that finished that day, by outcome.
    pub passes: BTreeMap<String, i64>,
    /// Rows the passes wrote.
    pub rows: i64,
    /// Lines the lines source read.
    pub lines: i64,
    /// Git calls the repos source made, and those that failed.
    pub git_calls: i64,
    pub git_failed: i64,
    /// Calls to the hosts the hosts source made, and those that failed.
    pub host_calls: i64,
    pub host_failed: i64,
}

/// The crawler's passes per UTC day, oldest first, from every `pass` row and
/// every `pass_source` row the module holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
pub async fn days(db: &Db) -> superx_kernel::Result<Vec<Day>> {
    let mut answer = db
        .query(
            "SELECT time::format(time::floor(finished_at, 1d), '%Y-%m-%d') AS day, outcome, count() AS passes, \
             math::sum(rows_written) AS rows FROM pass GROUP BY day, outcome;\n\
             SELECT time::format(time::floor(finished_at, 1d), '%Y-%m-%d') AS day, source, math::sum(items) AS items, \
             math::sum(calls) AS calls, math::sum(failed) AS failed FROM pass_source GROUP BY day, source;",
        )
        .await?;
    let passes: Vec<Value> = answer.take(0)?;
    let sources: Vec<Value> = answer.take(1)?;
    let mut by_day: BTreeMap<String, Day> = BTreeMap::new();
    for row in &passes {
        let (Some(day), Some(outcome)) = (text(row, "day"), text(row, "outcome")) else { continue };
        let d = by_day.entry(day.clone()).or_insert_with(|| Day { day, ..Day::default() });
        *d.passes.entry(outcome).or_insert(0) += int(row, "passes");
        d.rows += int(row, "rows");
    }
    for row in &sources {
        let (Some(day), Some(source)) = (text(row, "day"), text(row, "source")) else { continue };
        let d = by_day.entry(day.clone()).or_insert_with(|| Day { day, ..Day::default() });
        match source.as_str() {
            crate::crawler::lines::SOURCE => d.lines += int(row, "items"),
            crate::crawler::repos::SOURCE => {
                d.git_calls += int(row, "calls");
                d.git_failed += int(row, "failed");
            }
            crate::crawler::hosts::SOURCE => {
                d.host_calls += int(row, "calls");
                d.host_failed += int(row, "failed");
            }
            _ => {}
        }
    }
    Ok(by_day.into_values().collect())
}

/// `GET /api/stats`: [`days`], from the module's own database.
pub async fn stats(State(kernel): State<Kernel>) -> Response {
    let db = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => db,
        Err(e) => return unavailable(&e),
    };
    match days(&db).await {
        Ok(days) => Json(serde_json::json!({ "days": days })).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// One parameter as the tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub name: &'static str,
    pub value: String,
    pub set: bool,
    pub wanted: &'static str,
    pub group: &'static str,
    pub decides: &'static str,
    pub input: &'static str,
}

/// A parameter set to a value of the wrong shape, as the tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    pub name: &'static str,
    pub found: String,
    pub wanted: &'static str,
}

/// Every parameter as it stands, in the order `status` lists them, and the
/// values set that are of the wrong shape.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the substrate cannot answer.
pub async fn listing(kernel: &Kernel) -> superx_kernel::Result<serde_json::Value> {
    let resolved = params::resolve(kernel).await?;
    let shown: Vec<Shown> = resolved
        .settings
        .iter()
        .map(|s| {
            let about = params::about(s.name);
            Shown {
                name: s.name,
                value: s.value.clone(),
                set: s.set,
                wanted: s.wanted,
                group: about.map_or("", |a| a.group),
                decides: about.map_or("", |a| a.decides),
                input: about.map_or("", |a| a.input),
            }
        })
        .collect();
    let refused: Vec<Refused> =
        resolved.problems.iter().map(|p| Refused { name: p.name, found: p.found.clone(), wanted: p.wanted }).collect();
    Ok(serde_json::json!({ "params": shown, "problems": refused }))
}

/// `GET /api/params`: [`listing`].
pub async fn params(State(kernel): State<Kernel>) -> Response {
    match listing(&kernel).await {
        Ok(listed) => Json(listed).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// `PUT /api/params/{name}`, the value as its JSON body: checked as the
/// module checks what it reads, then set on the module's registry entity as
/// `superx param set recon` sets it, and the parameters as they now stand.
pub async fn set(State(kernel): State<Kernel>, Path(name): Path<String>, headers: HeaderMap, Json(json): Json<serde_json::Value>) -> Response {
    if !headers.contains_key(WRITE_HEADER) {
        return refuse(StatusCode::FORBIDDEN, format!("a change carries the {WRITE_HEADER} header"));
    }
    let Some(&known) = NAMES.iter().find(|n| **n == name) else {
        return refuse(StatusCode::NOT_FOUND, format!("no parameter named {name}"));
    };
    let value = superx_kernel::message::value_from_json(&json);
    let checked = params::from_values(&BTreeMap::from([(known, value.clone())]));
    if let Some(p) = checked.problems.first() {
        return refuse(StatusCode::BAD_REQUEST, format!("{known} wants {}, not {}", p.wanted, p.found));
    }
    let entity = match kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await {
        Ok(Some(entity)) => entity,
        Ok(None) => return refuse(StatusCode::SERVICE_UNAVAILABLE, "the module is not registered yet".to_string()),
        Err(e) => return unavailable(&e),
    };
    if let Err(e) = kernel.set_parameter(entity, known, value).await {
        return unavailable(&e);
    }
    tracing::info!(target: "recon", name = known, value = %json, "parameter set from the tab");
    params(State(kernel)).await
}

fn refuse(status: StatusCode, why: String) -> Response {
    (status, Json(serde_json::json!({ "error": why }))).into_response()
}

fn unavailable(e: &superx_kernel::KernelError) -> Response {
    refuse(StatusCode::SERVICE_UNAVAILABLE, format!("the substrate did not answer: {e}"))
}

fn text(row: &Value, key: &str) -> Option<String> {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn int(row: &Value, key: &str) -> i64 {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int().unwrap_or(0),
            _ => 0,
        },
        _ => 0,
    }
}
