//! What the tab reads and writes (#539): the crawler's stats per day, from
//! the module's own `pass` and `pass_source` rows, and the parameters, read
//! as `status` reads them and set as `superx param set recon` sets them.
//!
//! A write must carry [`WRITE_HEADER`]: a page of another site cannot send a
//! header of its own to this server without asking first, and the server
//! answers no such question, so no page in the operator's browser can change
//! a parameter (with [`crate::server`]'s Host check against a name of its
//! own pointed at 127.0.0.1).

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use superx_kernel::types::Value;
use superx_kernel::{Db, Kernel, NodeKind};

use crate::crawler::hosts::held::{record, row_key, time};
use crate::crawler::progress::{Now, Progress};
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

/// `GET /api/now`: what the crawler is doing this second, and the last pass.
pub async fn now(State(progress): State<Progress>) -> Json<Now> {
    Json(progress.snapshot())
}

/// A repository's last crawl, as its `crawl` row holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastCrawl {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub outcome: String,
    pub note: Option<String>,
    pub mainline: String,
    pub tip: Option<String>,
    pub commits_read: i64,
    pub commits_new: i64,
    pub refs_read: i64,
    pub reflog_read: i64,
    pub files_blamed: i64,
    pub files_left: i64,
    pub git_calls: i64,
    pub git_failed: i64,
    pub git_timeouts: i64,
    pub rows_written: i64,
}

/// A host repository's last crawl, as its `host_crawl` row holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostLast {
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub outcome: String,
    pub note: Option<String>,
    pub calls: i64,
    pub failed: i64,
    pub not_modified: i64,
    pub rate_waits: i64,
    pub objects_new: i64,
    pub objects_changed: i64,
}

/// A repository: its crawls, the commits the module holds of it, its last
/// crawl and its last complete one, and the host repositories crawled beside
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Repo {
    pub name: String,
    pub common_dir: String,
    pub crawls: i64,
    pub commits_held: i64,
    pub last: Option<LastCrawl>,
    pub last_complete: Option<DateTime<Utc>>,
    pub hosts: Vec<HostLast>,
}

/// Every repository the module holds, by name, each with its last crawl.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
pub async fn repositories(db: &Db) -> superx_kernel::Result<Vec<Repo>> {
    let mut answer = db
        .query(
            "SELECT id, name, common_dir FROM repository;\n\
             SELECT repository, started_at, finished_at, outcome, note, mainline, tip, commits_read, commits_new, refs_read, \
             reflog_read, files_blamed, files_left, git_calls, git_failed, git_timeouts, rows_written FROM crawl;\n\
             SELECT repository, count() AS n FROM commit GROUP BY repository;\n\
             SELECT host_repository, host_repository.host AS host, host_repository.path AS path, crawl.repository AS repository, \
             started_at, finished_at, outcome, note, calls, failed, not_modified, rate_waits, objects_new, objects_changed FROM host_crawl;",
        )
        .await?;
    let repos: Vec<Value> = answer.take(0)?;
    let mut crawls: Vec<Value> = answer.take(1)?;
    let held: Vec<Value> = answer.take(2)?;
    let mut host_crawls: Vec<Value> = answer.take(3)?;
    // Newest first, sorted here: an engine's order on a time is not trusted.
    crawls.sort_by_key(|c| std::cmp::Reverse(time(c, "started_at")));
    host_crawls.sort_by_key(|c| std::cmp::Reverse(time(c, "started_at")));
    let held: BTreeMap<String, i64> =
        held.iter().filter_map(|h| Some((row_key(&record(h, "repository")?), int(h, "n")))).collect();
    let mut out: Vec<Repo> = repos
        .iter()
        .filter_map(|r| {
            let id = record(r, "id")?;
            let key = row_key(&id);
            let mine: Vec<&Value> = crawls.iter().filter(|c| record(c, "repository").is_some_and(|x| row_key(&x) == key)).collect();
            let last = mine.first().and_then(|c| last_crawl(c));
            let last_complete = mine.iter().find(|c| text(c, "outcome").as_deref() == Some("complete")).and_then(|c| time(c, "started_at"));
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let hosts = host_crawls
                .iter()
                .filter(|h| record(h, "repository").is_some_and(|x| row_key(&x) == key))
                .filter(|h| record(h, "host_repository").is_some_and(|x| seen.insert(row_key(&x))))
                .filter_map(host_last)
                .collect();
            Some(Repo {
                name: text(r, "name").unwrap_or_default(),
                common_dir: text(r, "common_dir").unwrap_or_default(),
                crawls: i64::try_from(mine.len()).unwrap_or(i64::MAX),
                commits_held: held.get(&key).copied().unwrap_or(0),
                last,
                last_complete,
                hosts,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.common_dir.cmp(&b.common_dir)));
    Ok(out)
}

fn last_crawl(c: &Value) -> Option<LastCrawl> {
    Some(LastCrawl {
        started_at: time(c, "started_at")?,
        finished_at: time(c, "finished_at")?,
        outcome: text(c, "outcome")?,
        note: text(c, "note"),
        mainline: text(c, "mainline").unwrap_or_default(),
        tip: text(c, "tip"),
        commits_read: int(c, "commits_read"),
        commits_new: int(c, "commits_new"),
        refs_read: int(c, "refs_read"),
        reflog_read: int(c, "reflog_read"),
        files_blamed: int(c, "files_blamed"),
        files_left: int(c, "files_left"),
        git_calls: int(c, "git_calls"),
        git_failed: int(c, "git_failed"),
        git_timeouts: int(c, "git_timeouts"),
        rows_written: int(c, "rows_written"),
    })
}

fn host_last(h: &Value) -> Option<HostLast> {
    Some(HostLast {
        name: format!("{}/{}", text(h, "host")?, text(h, "path")?),
        started_at: time(h, "started_at")?,
        finished_at: time(h, "finished_at")?,
        outcome: text(h, "outcome")?,
        note: text(h, "note"),
        calls: int(h, "calls"),
        failed: int(h, "failed"),
        not_modified: int(h, "not_modified"),
        rate_waits: int(h, "rate_waits"),
        objects_new: int(h, "objects_new"),
        objects_changed: int(h, "objects_changed"),
    })
}

/// `GET /api/repos`: [`repositories`], from the module's own database.
pub async fn repos(State(kernel): State<Kernel>) -> Response {
    let db = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => db,
        Err(e) => return unavailable(&e),
    };
    match repositories(&db).await {
        Ok(repos) => Json(serde_json::json!({ "repos": repos })).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// `GET /api/results/models`: [`crate::results::models`], from the module's
/// own database.
pub async fn models(State(kernel): State<Kernel>) -> Response {
    let db = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => db,
        Err(e) => return unavailable(&e),
    };
    match crate::results::models(&db).await {
        Ok(models) => Json(models).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// `GET /api/results/compare`: [`crate::results::compare::compare`], from the
/// module's own database, under the parameters in force.
pub async fn compare(State(kernel): State<Kernel>) -> Response {
    let resolved = match params::resolve(&kernel).await {
        Ok(r) => r,
        Err(e) => return unavailable(&e),
    };
    let db = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => db,
        Err(e) => return unavailable(&e),
    };
    match crate::results::compare::compare(&db, &resolved.params).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// What `GET /api/results/day` is asked: a model and a UTC day.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DayQuery {
    pub model: String,
    pub day: String,
}

/// `GET /api/results/day?model=…&day=YYYY-MM-DD`: [`crate::results::day`],
/// what is behind one model's bar on one day.
pub async fn drill(State(kernel): State<Kernel>, axum::extract::Query(q): axum::extract::Query<DayQuery>) -> Response {
    let Ok(day) = chrono::NaiveDate::parse_from_str(&q.day, "%Y-%m-%d") else {
        return refuse(StatusCode::BAD_REQUEST, format!("day wants YYYY-MM-DD, not {}", q.day));
    };
    let db = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => db,
        Err(e) => return unavailable(&e),
    };
    match crate::results::day(&db, &q.model, day).await {
        Ok(detail) => Json(detail).into_response(),
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
