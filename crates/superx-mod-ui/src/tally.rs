//! The sessions list as a running tally (#576): every message and every
//! action event read once, in the order of their UUIDv7 ids, then only the
//! new ones, and each session's figures kept as they move. The list asked
//! the substrate four times for each session, one session after another, and
//! scanned the telemetry stream once per agent: 31 s before its first row
//! showed, and 13.5 s on every second poll (2026-11-07, 113 sessions).
//!
//! Each figure is the one the per-session reads of [`crate::activity`] give,
//! and the tests hold it to them:
//! - its lines: every message row of the session;
//! - when it last did anything: its newest row by `valid_from`, at its
//!   `emitted_at`, else its `valid_from`;
//! - its output: each reply's largest output tokens, a reply's rows told
//!   apart by [`crate::stats::REPLY_KEY_SQL`], summed;
//! - its context: the newest usage-bearing reply that is not `<synthetic>`
//!   and read a prompt, by `emitted_at` else `valid_from`;
//! - its model and its effort: the newest that names a real one, never a
//!   `<…>` marker;
//! - its actions: every event whose subject it is, or which its agent
//!   emitted for its source key, once.
//!
//! The per-session reads look only so far back for a context, a model and
//! an effort (20 and 12 rows); the tally looks at every row. They differ only
//! where that many of the newest rows hold none, and then the tally names the
//! newest that does. Times are compared in full here, where the substrate's
//! datetime index orders within one second wrongly (#486).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use superx_kernel::types::{Object, RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Kernel, Result};

/// Rows read at once from one table.
const PAGE: i64 = 5_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// What each message row gives the tally: its session, its times, its reply,
/// its usage, its model and its effort, never the rest of its raw line.
const MESSAGE_FIELDS: &str = "id, session, valid_from, emitted_at, \
     (raw.message.id ?? raw.id ?? id) AS reply, raw.message.usage AS cu, raw.tokens AS gu, \
     (raw.message.model ?? raw.model) AS model, raw.effort AS effort";

/// What each action event gives it: who emitted it, for which session.
const EVENT_FIELDS: &str = "id, agent, subject, payload.session AS src";

/// Who emitted an action event, its subject and its source key.
type Emitted = (Option<String>, Option<String>, Option<String>);

/// The sessions' figures, and how far each table has been read.
#[derive(Debug, Default)]
pub struct Tally {
    messages: Option<Uuid>,
    events: Option<Uuid>,
    sessions: HashMap<String, Session>,
    /// Action events by who emitted them, their subject and their source
    /// key: what [`crate::activity::action_counts`] groups them by.
    emitted: HashMap<Emitted, i64>,
}

/// One session's figures.
#[derive(Debug, Default)]
struct Session {
    lines: i64,
    /// The newest row's `valid_from`, and when it says it happened.
    newest: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// Each reply's largest output tokens.
    output: HashMap<String, i64>,
    /// The newest reply's prompt that read one, and when.
    context: Option<(DateTime<Utc>, i64)>,
    model: Option<(DateTime<Utc>, String)>,
    effort: Option<(DateTime<Utc>, String)>,
}

impl Tally {
    /// Read every row added since the last read: all of them the first time.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError::Db`] when the substrate does not answer;
    /// the rows read before it are kept.
    pub async fn refresh(&mut self, kernel: &Kernel) -> Result<()> {
        loop {
            let rows = page(kernel, "message", MESSAGE_FIELDS, self.messages).await?;
            for row in &rows {
                self.message(row);
            }
            if let Some(last) = rows.last().and_then(|r| uuid_at(r, "id")) {
                self.messages = Some(last);
            }
            if i64::try_from(rows.len()).unwrap_or(i64::MAX) < PAGE {
                break;
            }
        }
        loop {
            let rows = page(kernel, "telemetry_stream", EVENT_FIELDS, self.events).await?;
            for row in &rows {
                let Value::Object(o) = row else { continue };
                let key = (record_key(o.get("agent")), record_key(o.get("subject")), string(o.get("src")));
                *self.emitted.entry(key).or_default() += 1;
            }
            if let Some(last) = rows.last().and_then(|r| uuid_at(r, "id")) {
                self.events = Some(last);
            }
            if i64::try_from(rows.len()).unwrap_or(i64::MAX) < PAGE {
                break;
            }
        }
        Ok(())
    }

    fn message(&mut self, row: &Value) {
        let Value::Object(o) = row else { return };
        let Some(session) = record_key(o.get("session")) else { return };
        let Some(valid_from) = time(o.get("valid_from")) else { return };
        let at = time(o.get("emitted_at")).unwrap_or(valid_from);
        let s = self.sessions.entry(session).or_default();
        s.lines += 1;
        if s.newest.is_none_or(|(v, _)| valid_from >= v) {
            s.newest = Some((valid_from, at));
        }
        let (cu, gu) = (object(o.get("cu")), object(o.get("gu")));
        let model = string(o.get("model"));
        if cu.is_some() || gu.is_some() {
            // A reply's usage rides every line written for it (#409).
            let reply = match o.get("reply") {
                Some(Value::String(k)) => k.clone(),
                Some(Value::RecordId(id)) => format!("row {}", superx_ops::record_uuid(id)),
                _ => format!("row {}", record_key(o.get("id")).unwrap_or_default()),
            };
            let out = cu.and_then(|u| number(u.get("output_tokens"))).unwrap_or_else(|| {
                gu.map_or(0, |t| number(t.get("output")).unwrap_or(0) + number(t.get("thoughts")).unwrap_or(0))
            });
            let largest = s.output.entry(reply).or_insert(out);
            *largest = (*largest).max(out);
            // `<synthetic>` is the runtime's stand-in for an API error, all
            // zeros: it never sets the context bar.
            let context = crate::stats::usage_of(cu, gu).map_or(0, |u| u.context);
            if model.as_deref() != Some("<synthetic>") && context > 0 && s.context.is_none_or(|(when, _)| at >= when) {
                s.context = Some((at, context));
            }
        }
        if let Some(m) = model.filter(|m| real(m)) {
            if s.model.as_ref().is_none_or(|(when, _)| valid_from >= *when) {
                s.model = Some((valid_from, m));
            }
        }
        if let Some(e) = string(o.get("effort")).filter(|e| real(e)) {
            if s.effort.as_ref().is_none_or(|(when, _)| valid_from >= *when) {
                s.effort = Some((valid_from, e));
            }
        }
    }

    /// A session's message rows.
    #[must_use]
    pub fn lines(&self, session: &str) -> i64 {
        self.sessions.get(session).map_or(0, |s| s.lines)
    }

    /// When a session last did anything, on the agent's clock.
    #[must_use]
    pub fn last_active(&self, session: &str) -> Option<DateTime<Utc>> {
        self.sessions.get(session).and_then(|s| s.newest).map(|(_, at)| at)
    }

    /// A session's `(context, output_total)`, each `None` where it has none.
    #[must_use]
    pub fn tokens(&self, session: &str) -> (Option<i64>, Option<i64>) {
        let Some(s) = self.sessions.get(session) else { return (None, None) };
        let output: i64 = s.output.values().sum();
        (s.context.map(|(_, c)| c), (output > 0).then_some(output))
    }

    /// A session's current model and effort.
    #[must_use]
    pub fn model_effort(&self, session: &str) -> (Option<String>, Option<String>) {
        let Some(s) = self.sessions.get(session) else { return (None, None) };
        (s.model.as_ref().map(|(_, m)| m.clone()), s.effort.as_ref().map(|(_, e)| e.clone()))
    }

    /// Each listed session's action events, matched as
    /// [`crate::activity::action_counts`] matches them: by subject, or by its
    /// agent and source key, an event both ways once. `sessions` pairs each
    /// session with its `(agent, source key)` scope; the counts are keyed by
    /// the session's uuid, and a session with none is absent.
    #[must_use]
    pub fn actions(&self, sessions: &[(RecordId, Option<(RecordId, String)>)]) -> HashMap<String, i64> {
        let listed: HashSet<String> = sessions.iter().map(|(s, _)| superx_ops::record_uuid(s)).collect();
        let mut by_agent: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for (session, scope) in sessions {
            if let Some((agent, src)) = scope {
                by_agent.entry(superx_ops::record_uuid(agent)).or_default().insert(src.clone(), superx_ops::record_uuid(session));
            }
        }
        let mut counts: HashMap<String, i64> = HashMap::new();
        for ((agent, subject, src), n) in &self.emitted {
            let mut of: BTreeSet<&String> = BTreeSet::new();
            if let Some(subject) = subject.as_ref().filter(|s| listed.contains(*s)) {
                of.insert(subject);
            }
            // Only a listed agent's events count by their source key.
            if let (Some(by_src), Some(src)) = (agent.as_ref().and_then(|a| by_agent.get(a)), src) {
                if let Some(session) = by_src.get(src) {
                    of.insert(session);
                }
            }
            for session in of {
                *counts.entry(session.clone()).or_default() += n;
            }
        }
        counts
    }
}

/// Up to [`PAGE`] rows of `table` after `from`, in id order.
async fn page(kernel: &Kernel, table: &str, fields: &str, from: Option<Uuid>) -> Result<Vec<Value>> {
    let range = if from.is_some() { "$from>.." } else { "$from.." };
    let sql = format!("SELECT {fields} FROM type::record('{table}', {range}) LIMIT $limit");
    let from = from.unwrap_or_else(Uuid::nil);
    crate::answered("sessions tally", || async {
        Ok(kernel.db().query(sql.as_str()).bind(("from", from)).bind(("limit", PAGE)).await?.take(0)?)
    })
    .await
}

/// Not a marker the runtime writes for itself, `<synthetic>`, nor empty.
fn real(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('<')
}

fn uuid_at(row: &Value, key: &str) -> Option<Uuid> {
    let Value::Object(o) = row else { return None };
    match o.get(key) {
        Some(Value::RecordId(RecordId { key: RecordIdKey::Uuid(u), .. })) => Some(*u),
        _ => None,
    }
}

fn record_key(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::RecordId(id)) => Some(superx_ops::record_uuid(id)),
        _ => None,
    }
}

fn time(v: Option<&Value>) -> Option<DateTime<Utc>> {
    match v {
        Some(Value::Datetime(d)) => Some(**d),
        _ => None,
    }
}

fn string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn object(v: Option<&Value>) -> Option<&Object> {
    match v {
        Some(Value::Object(o)) => Some(o),
        _ => None,
    }
}

fn number(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.to_int(),
        _ => None,
    }
}
