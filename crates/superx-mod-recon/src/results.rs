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

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use superx_kernel::types::Value;
use superx_kernel::{Db, Result};

use crate::crawler::hosts::held::{record, row_key, time};

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
const UNKNOWN: &str = "unknown";

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
             SELECT reply, valid_from, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, thinking_tokens, \
             reply.model AS model, reply.at AS at FROM reply_state;\n\
             SELECT at, ms, reply.model AS model FROM thinking;\n\
             SELECT time::format(time::floor(at, 1d), '%Y-%m-%d') AS day, reply.model AS model, count() AS n FROM tool_call GROUP BY day, model;\n\
             SELECT tool_call.at AS at, tool_call.reply.model AS model, added, removed FROM edit;\n\
             SELECT commit, commit.committed_at AS at, commit.added AS added, commit.removed AS removed, tool_call.reply.model AS model, \
             at AS tied_at FROM commit_session;",
        )
        .await?;
    let replies: Vec<Value> = answer.take(0)?;
    let pairs: Vec<Value> = answer.take(1)?;
    let mut states: Vec<Value> = answer.take(2)?;
    let thinking: Vec<Value> = answer.take(3)?;
    let tool_calls: Vec<Value> = answer.take(4)?;
    let edits: Vec<Value> = answer.take(5)?;
    let mut ties: Vec<Value> = answer.take(6)?;

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
    // A commit tied to several sessions counts once: under the model of its
    // earliest tie that names one, else its earliest tie.
    ties.sort_by_key(|t| (model_of(t) == UNKNOWN, time(t, "tied_at")));
    let mut counted: BTreeSet<String> = BTreeSet::new();
    for t in &ties {
        let Some(commit) = record(t, "commit") else { continue };
        if !counted.insert(row_key(&commit)) {
            continue;
        }
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

/// The measures of `model` on `day`.
fn slot(days: &mut BTreeMap<(String, String), Measures>, day: String, model: String) -> &mut Measures {
    days.entry((day, model)).or_default()
}

fn model_of(v: &Value) -> String {
    text(v, "model").filter(|m| !m.is_empty()).unwrap_or_else(|| UNKNOWN.to_string())
}

fn day_of(v: &Value, key: &str) -> Option<String> {
    time(v, key).map(|t: DateTime<Utc>| t.format("%Y-%m-%d").to_string())
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn int(v: &Value, key: &str) -> i64 {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int().unwrap_or(0),
            _ => 0,
        },
        _ => 0,
    }
}

fn flag(v: &Value, key: &str) -> bool {
    matches!(v, Value::Object(o) if matches!(o.get(key), Some(Value::Bool(true))))
}
