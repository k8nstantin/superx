//! Every query the reader issues against the module's own database, in one
//! place: the newest pass, what a session and a reply already hold, and
//! the rows a pass writes. Nothing here emits UPDATE or DELETE (§10). Row
//! ids are UUIDv7, set here (§11). Of several rows for one thing, the
//! newest is picked here by `valid_from`, never by an index's order:
//! SurrealDB 3.1's index on a datetime orders rows within a second wrongly
//! (#486).

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use superx_kernel::types::{Datetime, Number, Object, RecordId, Uuid, Value};
use superx_kernel::{Db, Result};

use crate::reader::facts::Counts;
use crate::reader::value::{at, int, text};

/// Statements in one write: a page of rows goes in a few round trips.
const STATEMENTS_PER_WRITE: usize = 200; // skill-allow: §9-const — write-path bound, not a policy tunable

/// The last line of the newest pass that read one, where the next pass
/// starts. Passes are seconds apart: the index's misorder within a second
/// cannot pick the wrong one.
const LAST_PASS: &str = "SELECT to_line, valid_from FROM pass WHERE to_line != NONE ORDER BY valid_from DESC LIMIT 1";

/// Where the newest pass stopped reading, when there is one.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn last_line(db: &Db) -> Result<Option<Uuid>> {
    let rows: Vec<Value> = db.query(LAST_PASS).await?.take(0)?;
    Ok(rows.first().and_then(|r| match at(r, &["to_line"]) {
        Some(Value::Uuid(u)) => Some(*u),
        _ => None,
    }))
}

/// Every session the database holds: its row, by the kernel's session.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn sessions(db: &Db) -> Result<Vec<(Uuid, RecordId)>> {
    let rows: Vec<Value> = db.query("SELECT id, kernel_session FROM session").await?.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| match (at(r, &["kernel_session"]), at(r, &["id"])) {
            (Some(Value::Uuid(u)), Some(Value::RecordId(id))) => Some((*u, id.clone())),
            _ => None,
        })
        .collect())
}

/// What a session already holds: what a line read again must not write a
/// second time.
#[derive(Debug, Default)]
pub struct SessionHolds {
    /// The session's newest place.
    pub place: Option<(String, Option<String>, DateTime<Utc>)>,
    pub replies: Vec<HeldReply>,
    /// Lines already written as a prompt, a turn, a compaction or a recap.
    pub lines: HashSet<Uuid>,
    pub calls: Vec<HeldCall>,
    /// Calls that have a result, an edit (by file), git events, a test run
    /// or a build run, by the call row's UUID: a set keyed by a record id is
    /// one clippy refuses (`mutable_key_type`).
    pub results: HashSet<Uuid>,
    pub edits: HashSet<(Uuid, String)>,
    pub git: HashSet<Uuid>,
    pub tests: HashSet<Uuid>,
    pub builds: HashSet<Uuid>,
}

#[derive(Debug, Clone)]
pub struct HeldReply {
    pub id: RecordId,
    pub agent_id: String,
    pub first: Uuid,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct HeldCall {
    pub id: RecordId,
    pub call_id: String,
    pub reply: RecordId,
    pub tool: String,
    pub command: Option<String>,
}

const SESSION_HOLDS: &str = "
    SELECT cwd, branch, at, valid_from FROM session_place WHERE session = $sess;
    SELECT id, agent_id, kernel_message, at FROM reply WHERE session = $sess;
    SELECT VALUE kernel_message FROM prompt WHERE session = $sess;
    SELECT VALUE kernel_message FROM turn WHERE session = $sess;
    SELECT VALUE kernel_message FROM compaction WHERE session = $sess;
    SELECT VALUE kernel_message FROM away WHERE session = $sess;
    LET $replies = (SELECT VALUE id FROM reply WHERE session = $sess);
    SELECT id, call_id, reply, tool, command FROM tool_call WHERE reply INSIDE $replies;
    LET $calls = (SELECT VALUE id FROM tool_call WHERE reply INSIDE $replies);
    SELECT VALUE tool_call FROM tool_result WHERE tool_call INSIDE $calls;
    SELECT tool_call, path FROM edit WHERE tool_call INSIDE $calls;
    SELECT VALUE tool_call FROM git_event WHERE tool_call INSIDE $calls;
    SELECT VALUE tool_call FROM test_run WHERE tool_call INSIDE $calls;
    SELECT VALUE tool_call FROM build_run WHERE tool_call INSIDE $calls;
";

/// What one session already holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn session_holds(db: &Db, session: &RecordId) -> Result<SessionHolds> {
    let mut res = db.query(SESSION_HOLDS).bind(("sess", session.clone())).await?;
    let places: Vec<Value> = res.take(0)?;
    let replies: Vec<Value> = res.take(1)?;
    let mut lines = HashSet::new();
    for i in 2..=5 {
        let held: Vec<Value> = res.take(i)?;
        lines.extend(held.iter().filter_map(uuid));
    }
    let calls: Vec<Value> = res.take(7)?;
    let results: Vec<Value> = res.take(9)?;
    let edits: Vec<Value> = res.take(10)?;
    let git: Vec<Value> = res.take(11)?;
    let tests: Vec<Value> = res.take(12)?;
    let builds: Vec<Value> = res.take(13)?;
    let place = places
        .iter()
        .filter_map(|p| {
            let cwd = text(p, &["cwd"])?.to_string();
            Some((time_at(p, "at")?, time_at(p, "valid_from")?, cwd, text(p, &["branch"]).map(str::to_string)))
        })
        .max_by_key(|(at, valid_from, ..)| (*at, *valid_from))
        .map(|(at, _, cwd, branch)| (cwd, branch, at));
    Ok(SessionHolds {
        place,
        replies: replies
            .iter()
            .filter_map(|r| {
                Some(HeldReply {
                    id: record(r, "id")?,
                    agent_id: text(r, &["agent_id"])?.to_string(),
                    first: match at(r, &["kernel_message"]) {
                        Some(Value::Uuid(u)) => *u,
                        _ => return None,
                    },
                    at: time_at(r, "at")?,
                })
            })
            .collect(),
        lines,
        calls: calls
            .iter()
            .filter_map(|c| {
                Some(HeldCall {
                    id: record(c, "id")?,
                    call_id: text(c, &["call_id"])?.to_string(),
                    reply: record(c, "reply")?,
                    tool: text(c, &["tool"])?.to_string(),
                    command: text(c, &["command"]).map(str::to_string),
                })
            })
            .collect(),
        results: results.iter().filter_map(record_uuid).collect(),
        edits: edits
            .iter()
            .filter_map(|e| Some((at(e, &["tool_call"]).and_then(record_uuid)?, text(e, &["path"])?.to_string())))
            .collect(),
        git: git.iter().filter_map(record_uuid).collect(),
        tests: tests.iter().filter_map(record_uuid).collect(),
        builds: builds.iter().filter_map(record_uuid).collect(),
    })
}

/// A reply's counts, as a `reply_state` row keeps them. Two states are the
/// same when these are: the whole usage object follows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub thinking_tokens: i64,
    pub tool_tokens: Option<i64>,
    pub web_searches: Option<i64>,
    pub web_fetches: Option<i64>,
    pub service_tier: Option<String>,
    pub text_chars: i64,
}

impl State {
    #[must_use]
    pub fn of(counts: &Counts, text_chars: i64) -> Self {
        Self {
            input_tokens: counts.input_tokens,
            cache_read_tokens: counts.cache_read_tokens,
            cache_write_tokens: counts.cache_write_tokens,
            output_tokens: counts.output_tokens,
            thinking_tokens: counts.thinking_tokens,
            tool_tokens: counts.tool_tokens,
            web_searches: counts.web_searches,
            web_fetches: counts.web_fetches,
            service_tier: counts.service_tier.clone(),
            text_chars,
        }
    }

    /// No count is higher than `other`'s: an earlier state of a reply that
    /// only grows.
    #[must_use]
    pub fn within(&self, other: &Self) -> bool {
        let opt = |a: Option<i64>, b: Option<i64>| a.unwrap_or(0) <= b.unwrap_or(0);
        self.input_tokens <= other.input_tokens
            && self.cache_read_tokens <= other.cache_read_tokens
            && self.cache_write_tokens <= other.cache_write_tokens
            && self.output_tokens <= other.output_tokens
            && self.thinking_tokens <= other.thinking_tokens
            && opt(self.tool_tokens, other.tool_tokens)
            && opt(self.web_searches, other.web_searches)
            && opt(self.web_fetches, other.web_fetches)
            && self.text_chars <= other.text_chars
    }
}

/// What a reply already holds.
#[derive(Debug, Default)]
pub struct ReplyHolds {
    pub newest: Option<State>,
    pub thinking: HashSet<i64>,
}

const REPLY_HOLDS: &str = "
    SELECT * FROM reply_state WHERE reply = $reply;
    SELECT VALUE position FROM thinking WHERE reply = $reply;
";

/// What one reply already holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn reply_holds(db: &Db, reply: &RecordId) -> Result<ReplyHolds> {
    let mut res = db.query(REPLY_HOLDS).bind(("reply", reply.clone())).await?;
    let states: Vec<Value> = res.take(0)?;
    let positions: Vec<Value> = res.take(1)?;
    let newest = states.iter().filter_map(|s| Some((time_at(s, "valid_from")?, s))).max_by_key(|(t, _)| *t).and_then(
        |(_, s)| {
            Some(State {
                input_tokens: int(s, &["input_tokens"])?,
                cache_read_tokens: int(s, &["cache_read_tokens"])?,
                cache_write_tokens: int(s, &["cache_write_tokens"])?,
                output_tokens: int(s, &["output_tokens"])?,
                thinking_tokens: int(s, &["thinking_tokens"])?,
                tool_tokens: int(s, &["tool_tokens"]),
                web_searches: int(s, &["web_searches"]),
                web_fetches: int(s, &["web_fetches"]),
                service_tier: text(s, &["service_tier"]).map(str::to_string),
                text_chars: int(s, &["text_chars"])?,
            })
        },
    );
    Ok(ReplyHolds {
        newest,
        thinking: positions
            .iter()
            .filter_map(|p| match p {
                Value::Number(n) => n.to_int(),
                _ => None,
            })
            .collect(),
    })
}

/// Rows to create, in the order they must be created: a row's links name
/// rows created before it, and the engine checks each link exists.
#[derive(Debug, Default)]
pub struct Batch {
    rows: Vec<(RecordId, Object)>,
    /// Rows added, by table, since the pass began.
    pub added: BTreeMap<&'static str, i64>,
}

impl Batch {
    /// A row of `table`, its id set here, its `valid_from` now.
    pub fn create(&mut self, table: &'static str, mut content: Row) -> RecordId {
        let id = RecordId::new(table, Uuid::new_v7());
        content.0.insert("valid_from", Value::Datetime(Datetime::from(Utc::now())));
        self.rows.push((id.clone(), content.0));
        *self.added.entry(table).or_default() += 1;
        id
    }

    /// Rows waiting to be written.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.rows.len()
    }

    /// Rows added in all, since the pass began.
    #[must_use]
    pub fn total(&self) -> i64 {
        self.added.values().sum()
    }
}

/// Write every waiting row, in order.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the engine refuses a row: the rows
/// before it in its write are written, the rest are not.
pub async fn flush(db: &Db, batch: &mut Batch) -> Result<()> {
    for chunk in batch.rows.chunks(STATEMENTS_PER_WRITE) {
        let mut sql = String::new();
        let mut vars = Object::new();
        for (i, (id, content)) in chunk.iter().enumerate() {
            sql.push_str(&format!("CREATE $i{i} CONTENT $c{i};\n"));
            vars.insert(format!("i{i}"), Value::RecordId(id.clone()));
            vars.insert(format!("c{i}"), Value::Object(content.clone()));
        }
        db.query(sql).bind(vars).await?.check()?;
    }
    batch.rows.clear();
    Ok(())
}

/// A row's content, built a field at a time; a field with no value is left
/// out, which the schema reads as NONE.
#[derive(Debug, Default, Clone)]
pub struct Row(Object);

impl Row {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn text(mut self, key: &str, v: impl Into<String>) -> Self {
        self.0.insert(key, Value::String(v.into()));
        self
    }

    #[must_use]
    pub fn some_text(self, key: &str, v: Option<impl Into<String>>) -> Self {
        match v {
            Some(v) => self.text(key, v),
            None => self,
        }
    }

    #[must_use]
    pub fn int(mut self, key: &str, v: i64) -> Self {
        self.0.insert(key, Value::Number(Number::Int(v)));
        self
    }

    #[must_use]
    pub fn some_int(self, key: &str, v: Option<i64>) -> Self {
        match v {
            Some(v) => self.int(key, v),
            None => self,
        }
    }

    #[must_use]
    pub fn flag(mut self, key: &str, v: bool) -> Self {
        self.0.insert(key, Value::Bool(v));
        self
    }

    #[must_use]
    pub fn time(mut self, key: &str, v: DateTime<Utc>) -> Self {
        self.0.insert(key, Value::Datetime(Datetime::from(v)));
        self
    }

    #[must_use]
    pub fn uuid(mut self, key: &str, v: Uuid) -> Self {
        self.0.insert(key, Value::Uuid(v));
        self
    }

    #[must_use]
    pub fn some_uuid(self, key: &str, v: Option<Uuid>) -> Self {
        match v {
            Some(v) => self.uuid(key, v),
            None => self,
        }
    }

    #[must_use]
    pub fn link(mut self, key: &str, v: &RecordId) -> Self {
        self.0.insert(key, Value::RecordId(v.clone()));
        self
    }

    #[must_use]
    pub fn object(mut self, key: &str, v: Object) -> Self {
        self.0.insert(key, Value::Object(v));
        self
    }
}

fn uuid(v: &Value) -> Option<Uuid> {
    match v {
        Value::Uuid(u) => Some(*u),
        _ => None,
    }
}

fn record(v: &Value, key: &str) -> Option<RecordId> {
    match at(v, &[key])? {
        Value::RecordId(id) => Some(id.clone()),
        _ => None,
    }
}

fn record_uuid(v: &Value) -> Option<Uuid> {
    match v {
        Value::RecordId(id) => crate::reader::kernel::record_uuid_of(id),
        _ => None,
    }
}

fn time_at(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match at(v, &[key])? {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}
