//! The tab's Results (#539): what recon gathers its facts for, the models
//! measured over time. Each model, day by day (UTC): its replies, the tokens
//! of each reply's newest state, its thinking, its tool calls, the lines its
//! edits wrote, and the commits tied to its tool calls with their lines; and
//! each model over all time, with the sessions it answered in.
//!
//! Counts over a table's own fields are grouped by the database. Sums over
//! a field that may be NONE, and the newest state of each reply, are made
//! here, since the engine's handling of NONE is not to be trusted in either
//! (SurrealDB 3.1.4: `!= NONE` filters drop or keep the wrong rows).

pub mod commit;
pub mod compare;
pub mod findings;
pub mod trail;
pub mod window;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use superx_kernel::types::Value;
use superx_kernel::{Db, Result};

use crate::crawler::hosts::held::{record, row_key, time};
use crate::crawler::lines::kernel::{first_id, record_uuid_of};

/// Rows read at once from one of the module's tables.
pub(crate) const PAGE: i64 = 5_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Every row of `table`, its id and `fields`, read a page at a time in the
/// order of its UUIDv7 ids (as the crawler walks the kernel's lines), so no
/// answer is one huge message: the comparison's one query of every table
/// had the database reset the connection.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
pub(crate) async fn every(db: &Db, table: &str, fields: &str) -> Result<Vec<Value>> {
    let first = format!("SELECT id, {fields} FROM type::record('{table}', $from..) LIMIT $limit");
    let after = format!("SELECT id, {fields} FROM type::record('{table}', $from>..) LIMIT $limit");
    let mut out: Vec<Value> = Vec::new();
    let mut from = first_id();
    let mut sql = first;
    loop {
        let page: Vec<Value> = db.query(sql.as_str()).bind(("from", from)).bind(("limit", PAGE)).await?.take(0)?;
        let full = i64::try_from(page.len()).unwrap_or(i64::MAX) >= PAGE;
        let last = page.last().and_then(|r| record(r, "id")).and_then(|id| record_uuid_of(&id));
        out.extend(page);
        match last {
            Some(l) if full => {
                from = l;
                sql.clone_from(&after);
            }
            _ => return Ok(out),
        }
    }
}

/// One model on one UTC day.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModelDay {
    pub day: String,
    pub model: String,
    #[serde(flatten)]
    pub measures: Measures,
}

/// One model over all time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModelTotal {
    pub model: String,
    pub sessions: i64,
    pub first_day: String,
    pub last_day: String,
    #[serde(flatten)]
    pub measures: Measures,
}

/// What a model did, summed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Measures {
    pub replies: i64,
    pub subagent_replies: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub thinking_tokens: i64,
    pub thinking_spans: i64,
    pub thinking_ms: i64,
    pub tool_calls: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub commits: i64,
    pub commit_lines_added: i64,
    pub commit_lines_removed: i64,
}

impl Measures {
    fn add(&mut self, o: &Self) {
        self.replies += o.replies;
        self.subagent_replies += o.subagent_replies;
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_write_tokens += o.cache_write_tokens;
        self.thinking_tokens += o.thinking_tokens;
        self.thinking_spans += o.thinking_spans;
        self.thinking_ms += o.thinking_ms;
        self.tool_calls += o.tool_calls;
        self.lines_added += o.lines_added;
        self.lines_removed += o.lines_removed;
        self.commits += o.commits;
        self.commit_lines_added += o.commit_lines_added;
        self.commit_lines_removed += o.commit_lines_removed;
    }
}

/// The models, day by day and over all time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Models {
    pub days: Vec<ModelDay>,
    pub totals: Vec<ModelTotal>,
}

/// The model a link chain ends in, `unknown` when it ends in nothing.
pub(crate) const UNKNOWN: &str = "unknown";

/// Every model, from the module's own rows.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
pub async fn models(db: &Db) -> Result<Models> {
    let mut answer = db
        .query(
            "SELECT time::format(time::floor(at, 1d), '%Y-%m-%d') AS day, model, subagent, count() AS n FROM reply GROUP BY day, model, subagent;\n\
             SELECT model, session FROM reply GROUP BY model, session;\n\
             SELECT time::format(time::floor(at, 1d), '%Y-%m-%d') AS day, reply.model AS model, count() AS n FROM tool_call GROUP BY day, model;",
        )
        .await?;
    let replies: Vec<Value> = answer.take(0)?;
    let pairs: Vec<Value> = answer.take(1)?;
    let tool_calls: Vec<Value> = answer.take(2)?;
    let mut states = every(
        db,
        "reply_state",
        "reply, valid_from, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, thinking_tokens, reply.model AS model, reply.at AS at",
    )
    .await?;
    let thinking = every(db, "thinking", "at, ms, reply.model AS model").await?;
    let edits = every(db, "edit", "tool_call.at AS at, tool_call.reply.model AS model, added, removed").await?;
    let ties = every(
        db,
        "commit_session",
        "commit, commit.committed_at AS at, commit.added AS added, commit.removed AS removed, tool_call.reply.model AS model, at AS tied_at",
    )
    .await?;

    let mut days: BTreeMap<(String, String), Measures> = BTreeMap::new();
    for r in &replies {
        let (Some(day), model) = (text(r, "day"), model_of(r)) else { continue };
        let n = int(r, "n");
        let m = slot(&mut days, day, model);
        m.replies += n;
        if flag(r, "subagent") {
            m.subagent_replies += n;
        }
    }
    // Each reply's newest state: newest first, the first one seen kept.
    states.sort_by_key(|s| std::cmp::Reverse(time(s, "valid_from")));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for s in &states {
        let Some(reply) = record(s, "reply") else { continue };
        if !seen.insert(row_key(&reply)) {
            continue;
        }
        let Some(day) = day_of(s, "at") else { continue };
        let m = slot(&mut days, day, model_of(s));
        m.input_tokens += int(s, "input_tokens");
        m.output_tokens += int(s, "output_tokens");
        m.cache_read_tokens += int(s, "cache_read_tokens");
        m.cache_write_tokens += int(s, "cache_write_tokens");
        m.thinking_tokens += int(s, "thinking_tokens");
    }
    for t in &thinking {
        let Some(day) = day_of(t, "at") else { continue };
        let m = slot(&mut days, day, model_of(t));
        m.thinking_spans += 1;
        m.thinking_ms += int(t, "ms");
    }
    for c in &tool_calls {
        let Some(day) = text(c, "day") else { continue };
        slot(&mut days, day, model_of(c)).tool_calls += int(c, "n");
    }
    for e in &edits {
        let Some(day) = day_of(e, "at") else { continue };
        let m = slot(&mut days, day, model_of(e));
        m.lines_added += int(e, "added");
        m.lines_removed += int(e, "removed");
    }
    for t in &owned(ties) {
        let Some(day) = day_of(t, "at") else { continue };
        let m = slot(&mut days, day, model_of(t));
        m.commits += 1;
        m.commit_lines_added += int(t, "added");
        m.commit_lines_removed += int(t, "removed");
    }

    let mut sessions: BTreeMap<String, i64> = BTreeMap::new();
    for p in &pairs {
        if record(p, "session").is_some() {
            *sessions.entry(model_of(p)).or_default() += 1;
        }
    }
    let mut totals: BTreeMap<String, ModelTotal> = BTreeMap::new();
    for ((day, model), m) in &days {
        let t = totals.entry(model.clone()).or_insert_with(|| ModelTotal {
            model: model.clone(),
            sessions: sessions.get(model).copied().unwrap_or(0),
            first_day: day.clone(),
            last_day: day.clone(),
            measures: Measures::default(),
        });
        t.last_day.clone_from(day);
        t.measures.add(m);
    }
    let mut totals: Vec<ModelTotal> = totals.into_values().collect();
    totals.sort_by(|a, b| b.measures.replies.cmp(&a.measures.replies).then_with(|| a.model.cmp(&b.model)));
    let days = days.into_iter().map(|((day, model), measures)| ModelDay { day, model, measures }).collect();
    Ok(Models { days, totals })
}

/// One tie per commit: a commit tied to several sessions counts once, under
/// the model of its earliest tie that names one, else its earliest tie. The
/// charts and their drill-downs read commits through this alone, so a day's
/// commits are the same in both.
pub(crate) fn owned(mut ties: Vec<Value>) -> Vec<Value> {
    ties.sort_by_key(|t| (model_of(t) == UNKNOWN, time(t, "tied_at")));
    let mut counted: BTreeSet<String> = BTreeSet::new();
    ties.into_iter().filter(|t| record(t, "commit").is_some_and(|c| counted.insert(row_key(&c)))).collect()
}

/// A commit behind a model's day: where it is, whether its work landed, the
/// lines of it still alive at the newest blame, and its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayCommit {
    pub hash: String,
    pub repo: String,
    pub subject: String,
    pub committed_at: Option<DateTime<Utc>>,
    pub added: i64,
    pub removed: i64,
    /// Its newest placement: mainline, branch, in_flight or recovered.
    pub place: Option<String>,
    /// On the main line, or its files' versions carried there by another
    /// commit (a squash or a replay).
    pub landed: bool,
    /// None until a blame of its repository's main line credits it.
    pub alive: Option<i64>,
    pub session: Option<String>,
    pub files: Vec<DayFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayFile {
    pub path: String,
    pub change: String,
    pub added: i64,
    pub removed: i64,
}

/// An edit behind a model's day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayEdit {
    pub at: Option<DateTime<Utc>>,
    pub path: String,
    pub kind: String,
    pub added: i64,
    pub removed: i64,
    pub tool: String,
    pub session: Option<String>,
    pub operator_changed: bool,
}

/// A session a model answered in on that day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaySession {
    pub session: String,
    pub agent: String,
    pub replies: i64,
    pub first: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
}

/// What is behind one model's bar on one UTC day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelDayDetail {
    pub model: String,
    pub day: String,
    pub commits: Vec<DayCommit>,
    pub edits: Vec<DayEdit>,
    pub sessions: Vec<DaySession>,
}

/// The commits, edits and sessions behind `model` on `day`, each counted
/// as the charts count it.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
pub async fn day(db: &Db, model: &str, day: chrono::NaiveDate) -> Result<ModelDayDetail> {
    let from = day.and_hms_opt(0, 0, 0).map(|d| d.and_utc()).unwrap_or_default();
    let to = from + chrono::TimeDelta::days(1);
    let mut answer = db
        .query(
            "SELECT commit, commit.committed_at AS at, commit.added AS added, commit.removed AS removed, tool_call.reply.model AS model, \
             at AS tied_at, session.kernel_session AS session FROM commit_session;\n\
             SELECT tool_call.at AS at, tool_call.reply.model AS model, tool_call.tool AS tool, tool_call.reply.session.kernel_session AS session, \
             path, kind, added, removed, operator_changed FROM edit WHERE tool_call.at >= $from AND tool_call.at < $to;\n\
             SELECT session, session.kernel_session AS kernel_session, session.agent AS agent, at, model FROM reply WHERE at >= $from AND at < $to;",
        )
        .bind(("from", from))
        .bind(("to", to))
        .await?;
    let ties: Vec<Value> = answer.take(0)?;
    let edits: Vec<Value> = answer.take(1)?;
    let replies: Vec<Value> = answer.take(2)?;
    let wanted = day.format("%Y-%m-%d").to_string();
    let mine: Vec<Value> =
        owned(ties).into_iter().filter(|t| model_of(t) == model && day_of(t, "at").as_deref() == Some(wanted.as_str())).collect();
    let ids: Vec<superx_kernel::types::RecordId> = mine.iter().filter_map(|t| record(t, "commit")).collect();
    let mut details = db
        .query(
            "SELECT id, hash, repository.name AS repo, subject FROM commit WHERE id INSIDE $ids;\n\
             SELECT commit, place, valid_from FROM placement WHERE commit INSIDE $ids;\n\
             SELECT commit FROM landed_via WHERE commit INSIDE $ids;\n\
             SELECT commit, alive, valid_from FROM survival WHERE commit INSIDE $ids;\n\
             SELECT commit, path, change, added, removed FROM commit_file WHERE commit INSIDE $ids;",
        )
        .bind(("ids", ids))
        .await?;
    let commits: Vec<Value> = details.take(0)?;
    let mut places: Vec<Value> = details.take(1)?;
    let landed: Vec<Value> = details.take(2)?;
    let mut survival: Vec<Value> = details.take(3)?;
    let files: Vec<Value> = details.take(4)?;
    places.sort_by_key(|p| std::cmp::Reverse(time(p, "valid_from")));
    survival.sort_by_key(|s| std::cmp::Reverse(time(s, "valid_from")));
    let newest = |rows: &[Value], key: &str| -> Option<Value> {
        rows.iter().find(|r| record(r, "commit").is_some_and(|c| row_key(&c) == key)).cloned()
    };
    let landed_via: BTreeSet<String> = landed.iter().filter_map(|l| record(l, "commit")).map(|c| row_key(&c)).collect();
    let mut out_commits: Vec<DayCommit> = mine
        .iter()
        .filter_map(|t| {
            let id = record(t, "commit")?;
            let key = row_key(&id);
            let c = commits.iter().find(|c| record(c, "id").is_some_and(|x| row_key(&x) == key))?;
            let place = newest(&places, &key).and_then(|p| text(&p, "place"));
            let mut its_files: Vec<DayFile> = files
                .iter()
                .filter(|f| record(f, "commit").is_some_and(|x| row_key(&x) == key))
                .map(|f| DayFile {
                    path: text(f, "path").unwrap_or_default(),
                    change: text(f, "change").unwrap_or_default(),
                    added: int(f, "added"),
                    removed: int(f, "removed"),
                })
                .collect();
            its_files.sort_by(|a, b| (b.added + b.removed).cmp(&(a.added + a.removed)).then_with(|| a.path.cmp(&b.path)));
            Some(DayCommit {
                hash: text(c, "hash").unwrap_or_default(),
                repo: text(c, "repo").unwrap_or_default(),
                subject: text(c, "subject").unwrap_or_default(),
                committed_at: time(t, "at"),
                added: int(t, "added"),
                removed: int(t, "removed"),
                landed: place.as_deref() == Some("mainline") || landed_via.contains(&key),
                place,
                alive: newest(&survival, &key).map(|s| int(&s, "alive")),
                session: uuid_text(t, "session"),
                files: its_files,
            })
        })
        .collect();
    out_commits.sort_by_key(|c| c.committed_at);
    let mut out_edits: Vec<DayEdit> = edits
        .iter()
        .filter(|e| model_of(e) == model)
        .map(|e| DayEdit {
            at: time(e, "at"),
            path: text(e, "path").unwrap_or_default(),
            kind: text(e, "kind").unwrap_or_default(),
            added: int(e, "added"),
            removed: int(e, "removed"),
            tool: text(e, "tool").unwrap_or_default(),
            session: uuid_text(e, "session"),
            operator_changed: flag(e, "operator_changed"),
        })
        .collect();
    out_edits.sort_by_key(|e| e.at);
    let mut by_session: BTreeMap<String, DaySession> = BTreeMap::new();
    for r in replies.iter().filter(|r| model_of(r) == model) {
        let Some(session) = record(r, "session") else { continue };
        let at = time(r, "at");
        let s = by_session.entry(row_key(&session)).or_insert_with(|| DaySession {
            session: uuid_text(r, "kernel_session").unwrap_or_default(),
            agent: text(r, "agent").unwrap_or_default(),
            replies: 0,
            first: at,
            last: at,
        });
        s.replies += 1;
        s.first = s.first.min(at).or(at);
        s.last = s.last.max(at);
    }
    let mut sessions: Vec<DaySession> = by_session.into_values().collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.replies));
    Ok(ModelDayDetail { model: model.to_string(), day: wanted, commits: out_commits, edits: out_edits, sessions })
}

/// A uuid field as text.
pub(crate) fn uuid_text(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Uuid(u)) => Some(u.to_string()),
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The measures of `model` on `day`.
fn slot(days: &mut BTreeMap<(String, String), Measures>, day: String, model: String) -> &mut Measures {
    days.entry((day, model)).or_default()
}

pub(crate) fn model_of(v: &Value) -> String {
    text(v, "model").filter(|m| !m.is_empty()).unwrap_or_else(|| UNKNOWN.to_string())
}

fn day_of(v: &Value, key: &str) -> Option<String> {
    time(v, key).map(|t: DateTime<Utc>| t.format("%Y-%m-%d").to_string())
}

pub(crate) fn text(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn int(v: &Value, key: &str) -> i64 {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int().unwrap_or(0),
            _ => 0,
        },
        _ => 0,
    }
}

pub(crate) fn flag(v: &Value, key: &str) -> bool {
    matches!(v, Value::Object(o) if matches!(o.get(key), Some(Value::Bool(true))))
}
