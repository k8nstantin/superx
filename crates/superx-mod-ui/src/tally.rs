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
//! The per-session read of a context looks only at the newest 20 usage
//! rows; the tally looks at every row. They differ only where 20 newer ones
//! hold no prompt, and then the tally names the newest that does. Times are
//! compared in full here, where the substrate's datetime index orders within
//! one second wrongly (#486).
//!
//! A row's id is minted before it is written, so a row can commit after a
//! newer one was read. It commits within the late-commit window
//! ([`crate::activity::LATE_COMMIT_SECS`]) of its stamp, as the live feed
//! takes it (#183), so each read starts again at the newest id stamped
//! before that window of when the read before it began, and skips the ids it
//! holds after it. When capture is quiet, that is the newest row alone: the
//! read costs one row, not the window again at every poll.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::types::{Object, RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Kernel, Result};

use crate::thrown::{key_of, obj, str_of, time_of};

/// Rows read at once from one table.
const PAGE: i64 = 5_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// What each action event gives the tally: who emitted it, for which
/// session, and when it was captured.
const EVENT_FIELDS: &str = "id, valid_from, agent, subject, payload.session AS src";

/// Who emitted an action event, its subject and its source key.
type Emitted = (Option<String>, Option<String>, Option<String>);

/// The sessions' figures, and how far each table has been read.
#[derive(Debug, Default)]
pub struct Tally {
    messages: Cursor,
    events: Cursor,
    figures: Figures,
}

/// Where a table's next read starts: the ids read within the late-commit
/// window of the last read, and the newest from before it, each with when
/// it was captured.
#[derive(Debug, Default)]
struct Cursor {
    recent: BTreeMap<Uuid, DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct Figures {
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
    /// the rows read before it are kept, and the next read goes on from them.
    pub async fn refresh(&mut self, kernel: &Kernel) -> Result<()> {
        let fields = format!(
            "id, session, valid_from, emitted_at, {key} AS k, raw.message.usage AS cu, raw.tokens AS gu, \
             (raw.message.model ?? raw.model) AS model, raw.effort AS effort",
            key = crate::stats::REPLY_KEY_SQL,
        );
        let figures = &mut self.figures;
        read(kernel, "message", &fields, &mut self.messages, |o| figures.message(o)).await?;
        read(kernel, "telemetry_stream", EVENT_FIELDS, &mut self.events, |o| {
            let key = (record_key(o.get("agent")), record_key(o.get("subject")), str_of(o, "src").map(str::to_string));
            *figures.emitted.entry(key).or_default() += 1;
        })
        .await?;
        Ok(())
    }

    /// A session's message rows.
    #[must_use]
    pub fn lines(&self, session: &str) -> i64 {
        self.figures.sessions.get(session).map_or(0, |s| s.lines)
    }

    /// When a session last did anything, on the agent's clock.
    #[must_use]
    pub fn last_active(&self, session: &str) -> Option<DateTime<Utc>> {
        self.figures.sessions.get(session).and_then(|s| s.newest).map(|(_, at)| at)
    }

    /// A session's `(context, output_total)`, each `None` where it has none.
    #[must_use]
    pub fn tokens(&self, session: &str) -> (Option<i64>, Option<i64>) {
        let Some(s) = self.figures.sessions.get(session) else { return (None, None) };
        let output: i64 = s.output.values().sum();
        (s.context.map(|(_, c)| c), (output > 0).then_some(output))
    }

    /// A session's current model and effort.
    #[must_use]
    pub fn model_effort(&self, session: &str) -> (Option<String>, Option<String>) {
        let Some(s) = self.figures.sessions.get(session) else { return (None, None) };
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
        for ((agent, subject, src), n) in &self.figures.emitted {
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

impl Figures {
    fn message(&mut self, o: &Object) {
        let Some(session) = record_key(o.get("session")) else { return };
        let Some(valid_from) = time_of(o, "valid_from") else { return };
        let at = time_of(o, "emitted_at").unwrap_or(valid_from);
        let s = self.sessions.entry(session).or_default();
        s.lines += 1;
        if s.newest.is_none_or(|(v, _)| valid_from >= v) {
            s.newest = Some((valid_from, at));
        }
        let model = str_of(o, "model");
        // A reply's usage rides every line written for it (#409).
        if let Some(usage) = crate::stats::usage_of(o.get("cu").and_then(obj), o.get("gu").and_then(obj)) {
            let largest = s.output.entry(key_of(o).unwrap_or_default()).or_insert(usage.out);
            *largest = (*largest).max(usage.out);
            // `<synthetic>` is the runtime's stand-in for an API error, all
            // zeros: it never sets the context bar.
            if model != Some("<synthetic>") && usage.context > 0 && s.context.is_none_or(|(when, _)| at >= when) {
                s.context = Some((at, usage.context));
            }
        }
        if let Some(m) = model.filter(|m| real(m)) {
            if s.model.as_ref().is_none_or(|(when, _)| valid_from >= *when) {
                s.model = Some((valid_from, m.to_string()));
            }
        }
        if let Some(e) = str_of(o, "effort").filter(|e| real(e)) {
            if s.effort.as_ref().is_none_or(|(when, _)| valid_from >= *when) {
                s.effort = Some((valid_from, e.to_string()));
            }
        }
    }
}

impl Cursor {
    /// Keep what the next read needs. A row this read did not find commits
    /// later, so it is stamped no earlier than the late-commit window before
    /// the read `began`: the next read starts at the newest id stamped
    /// before then, which a row read inside the window alone could pass
    /// over, and skips the ids held after it.
    fn prune(&mut self, began: DateTime<Utc>) {
        let edge = began - TimeDelta::seconds(crate::activity::LATE_COMMIT_SECS);
        let start = self.recent.iter().filter(|(_, at)| **at < edge).max_by_key(|(_, at)| **at).map(|(id, _)| *id);
        self.recent.retain(|id, at| *at >= edge || Some(*id) == start);
    }
}

/// Every row of `table` from where `cursor` says, a page at a time in id
/// order, each one not read before given to `take`.
async fn read<F: FnMut(&Object)>(kernel: &Kernel, table: &str, fields: &str, cursor: &mut Cursor, mut take: F) -> Result<()> {
    // The first page starts at the oldest id held, again; the rest after
    // the page before.
    let began = Utc::now();
    let mut from = cursor.recent.keys().next().copied();
    let mut again = true;
    loop {
        let rows = page(kernel, table, fields, from, again).await?;
        again = false;
        for o in rows.iter().filter_map(obj) {
            let Some(id) = uuid_of(o.get("id")) else { continue };
            if cursor.recent.contains_key(&id) {
                continue;
            }
            take(o);
            cursor.recent.insert(id, time_of(o, "valid_from").unwrap_or_else(Utc::now));
        }
        cursor.prune(began);
        // A page that ends on an id that is no UUID cannot say where the
        // next starts: the read stops there rather than read it again.
        let last = rows.last().and_then(obj).and_then(|o| uuid_of(o.get("id")));
        match last {
            Some(last) if i64::try_from(rows.len()).unwrap_or(i64::MAX) >= PAGE => from = Some(last),
            _ => return Ok(()),
        }
    }
}

/// Up to [`PAGE`] rows of `table` from `from` (`again`: that id too), in id
/// order.
async fn page(kernel: &Kernel, table: &str, fields: &str, from: Option<Uuid>, again: bool) -> Result<Vec<Value>> {
    let range = if again || from.is_none() { "$from.." } else { "$from>.." };
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

fn uuid_of(v: Option<&Value>) -> Option<Uuid> {
    match v {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The next read starts at the newest id from before the late-commit
    /// window and holds the ones inside it; once capture is quiet past the
    /// window, it holds the newest alone (#576 review).
    #[test]
    fn a_cursor_holds_the_window_and_the_newest_row_before_it() {
        let began = Utc::now();
        let ago = |secs| began - TimeDelta::seconds(secs);
        let mut ids = [Uuid::new_v7(), Uuid::new_v7(), Uuid::new_v7(), Uuid::new_v7()];
        ids.sort();
        let [oldest, before, inside, newest] = ids;
        let mut cursor = Cursor { recent: BTreeMap::from([(oldest, ago(90)), (before, ago(30)), (inside, ago(5)), (newest, ago(1))]) };
        cursor.prune(began);
        assert_eq!(cursor.recent.keys().copied().collect::<Vec<_>>(), [before, inside, newest]);
        cursor.prune(began + TimeDelta::seconds(60));
        assert_eq!(cursor.recent.keys().copied().collect::<Vec<_>>(), [newest], "quiet: the newest alone");
    }
}
