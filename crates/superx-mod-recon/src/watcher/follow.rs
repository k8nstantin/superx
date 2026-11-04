//! The lines the watcher follows (#566, step 3): its own reads of the
//! kernel's database, SELECT alone (rules kernel-read-only), a page at a time
//! in the order of the rows' ids, the UUIDv7 the kernel set when it captured
//! each line, never through a time or a datetime index. Conversation lines
//! are `message` rows; every other line is a `transcript_event` row of
//! `telemetry_stream`, whose page reads its rows' ids first, since most of
//! that stream is not a line. Each look starts a minute before the newest id
//! it has read in each table and skips every id it has read already, so a
//! line the kernel committed after a newer one is read, and every line once.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, TimeDelta, TimeZone as _, Utc};
use serde::Serialize;
use serde_json::Value as Json;
use superx_kernel::types::{RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Kernel, Result};

use super::line::CLAUDE_CODE;

/// Conversation lines read per page.
const MESSAGE_PAGE: u32 = 500; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Telemetry rows scanned per page: most of the stream is not a line.
const EVENT_PAGE: u32 = 2_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How far before the newest id it has read a look starts again.
const OVERLAP_SECS: i64 = 60; // skill-allow: §9-const — read-path bound, not a policy tunable

const MESSAGES: &str = "SELECT id, session, agent, raw_text, raw FROM type::record('message', $from>..) LIMIT $limit";
const EVENTS: &str = "SELECT id, lifecycle_event = 'transcript_event' AS line \
     FROM type::record('telemetry_stream', $from>..) LIMIT $limit";
const EVENT_LINES: &str = "SELECT id, agent, payload FROM $ids";

/// The kind capture gives a file beside the transcripts: no line.
const FILE_KIND: &str = "file"; // skill-allow: §9-const — capture's own word for a side file (adapters/claude_code.rs)

/// One captured line of Claude Code's, as the watcher reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Captured {
    pub id: Uuid,
    /// The kernel's session entity, by its UUID.
    pub session: String,
    /// `<agent>/<key>`, as capture names the session.
    pub name: String,
    /// The session's key in its agent's own files.
    pub key: String,
    pub agent: String,
    /// The timestamp its agent wrote in it, else the time its id holds.
    pub at: DateTime<Utc>,
    pub line: Json,
    /// The transcript it came from, as a `transcript_event` names it.
    pub transcript: Option<String>,
}

/// Why a row was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// Neither raw_text nor a parsed copy, or raw_text that is not JSON.
    NoLine,
    /// No session the kernel's names give.
    NoSession,
    /// Another agent's line, judged nowhere.
    Agent,
}

/// Rows not read, by why and by agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unread {
    pub why: Why,
    pub agent: String,
    pub rows: u64,
}

/// Where the watcher's following stands, and the names it has learned.
#[derive(Debug, Clone)]
pub struct Follow {
    /// Where the first look starts: the window before the watcher's start.
    start: DateTime<Utc>,
    newest_message: Option<Uuid>,
    newest_event: Option<Uuid>,
    /// The lines read within the overlap.
    read: HashSet<Uuid>,
    agents: HashMap<Uuid, String>,
    sessions: HashMap<(String, String), Uuid>,
    names: HashMap<Uuid, (String, String)>,
    lines: u64,
    unread: BTreeMap<(Why, String), u64>,
}

impl Follow {
    /// Following from `start` on.
    #[must_use]
    pub fn from(start: DateTime<Utc>) -> Self {
        Self {
            start,
            newest_message: None,
            newest_event: None,
            read: HashSet::new(),
            agents: HashMap::new(),
            sessions: HashMap::new(),
            names: HashMap::new(),
            lines: 0,
            unread: BTreeMap::new(),
        }
    }

    /// The lines read since the watcher started.
    #[must_use]
    pub fn lines(&self) -> u64 {
        self.lines
    }

    /// The rows not read, by why and by agent.
    #[must_use]
    pub fn unread(&self) -> Vec<Unread> {
        self.unread.iter().map(|((why, agent), rows)| Unread { why: *why, agent: agent.clone(), rows: *rows }).collect()
    }

    /// Every line captured since the last look, in the order of their ids. A
    /// look that cannot read changes nothing: the next starts where this one
    /// did.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel's database does not answer.
    pub async fn next(&mut self, kernel: &Kernel) -> Result<Vec<Captured>> {
        let mut follow = self.clone();
        let out = follow.read_both(kernel).await?;
        *self = follow;
        Ok(out)
    }

    async fn read_both(&mut self, kernel: &Kernel) -> Result<Vec<Captured>> {
        let mut named = false;
        let mut out = Vec::new();
        let mut from = self.start_of(self.newest_message);
        loop {
            let rows: Vec<Value> =
                kernel.db().query(MESSAGES).bind(("from", from)).bind(("limit", MESSAGE_PAGE)).await?.take(0)?;
            let full = rows.len() == usize::try_from(MESSAGE_PAGE).unwrap_or(usize::MAX);
            for row in &rows {
                let Some(id) = uuid_at(row, "id") else { continue };
                from = id;
                self.newest_message = self.newest_message.max(Some(id));
                if !self.read.insert(id) {
                    continue;
                }
                let (Some(agent), Some(session)) = (uuid_at(row, "agent"), uuid_at(row, "session")) else {
                    self.count(Why::NoSession, "?");
                    continue;
                };
                if (!self.agents.contains_key(&agent) || !self.names.contains_key(&session)) && !named {
                    self.learn_names(kernel).await?;
                    named = true;
                }
                let agent = self.agents.get(&agent).cloned().unwrap_or_else(|| "?".to_string());
                let Some((name, key)) = self.names.get(&session).cloned() else {
                    self.count(Why::NoSession, &agent);
                    continue;
                };
                if agent != CLAUDE_CODE {
                    self.count(Why::Agent, &agent);
                    continue;
                }
                let Some(line) = line_of(field(row, "raw_text"), field(row, "raw")) else {
                    self.count(Why::NoLine, &agent);
                    continue;
                };
                out.push(Captured { id, session: session.to_string(), name, key, agent, at: at(&line, id), line, transcript: None });
            }
            if !full {
                break;
            }
        }
        let mut from = self.start_of(self.newest_event);
        loop {
            let rows: Vec<Value> =
                kernel.db().query(EVENTS).bind(("from", from)).bind(("limit", EVENT_PAGE)).await?.take(0)?;
            let full = rows.len() == usize::try_from(EVENT_PAGE).unwrap_or(usize::MAX);
            let mut lines: Vec<RecordId> = Vec::new();
            for row in &rows {
                let Some(Value::RecordId(rid)) = field(row, "id") else { continue };
                let Some(id) = uuid_of(rid) else { continue };
                from = id;
                self.newest_event = self.newest_event.max(Some(id));
                if matches!(field(row, "line"), Some(Value::Bool(true))) && !self.read.contains(&id) {
                    lines.push(rid.clone());
                }
            }
            if !lines.is_empty() {
                let rows: Vec<Value> = kernel.db().query(EVENT_LINES).bind(("ids", lines)).await?.take(0)?;
                for row in &rows {
                    let Some(id) = uuid_at(row, "id") else { continue };
                    if !self.read.insert(id) {
                        continue;
                    }
                    let payload = field(row, "payload");
                    if matches!(payload.and_then(|p| field(p, "kind")), Some(Value::String(k)) if k == FILE_KIND) {
                        continue;
                    }
                    let Some(agent) = uuid_at(row, "agent") else {
                        self.count(Why::NoSession, "?");
                        continue;
                    };
                    if !self.agents.contains_key(&agent) && !named {
                        self.learn_names(kernel).await?;
                        named = true;
                    }
                    let agent = self.agents.get(&agent).cloned().unwrap_or_else(|| "?".to_string());
                    let Some(Value::String(key)) = payload.and_then(|p| field(p, "session")) else {
                        self.count(Why::NoSession, &agent);
                        continue;
                    };
                    let pair = (agent.clone(), key.clone());
                    if !self.sessions.contains_key(&pair) && !named {
                        self.learn_names(kernel).await?;
                        named = true;
                    }
                    let Some(session) = self.sessions.get(&pair).copied() else {
                        self.count(Why::NoSession, &agent);
                        continue;
                    };
                    if agent != CLAUDE_CODE {
                        self.count(Why::Agent, &agent);
                        continue;
                    }
                    let Some(line) = line_of(payload.and_then(|p| field(p, "raw_text")), payload.and_then(|p| field(p, "line")))
                    else {
                        self.count(Why::NoLine, &agent);
                        continue;
                    };
                    let transcript = match payload.and_then(|p| field(p, "file")) {
                        Some(Value::String(f)) => Some(f.clone()),
                        _ => None,
                    };
                    let name = self.names.get(&session).map_or_else(|| format!("{agent}/{key}"), |(n, _)| n.clone());
                    out.push(Captured {
                        id,
                        session: session.to_string(),
                        name,
                        key: key.clone(),
                        agent,
                        at: at(&line, id),
                        line,
                        transcript,
                    });
                }
            }
            if !full {
                break;
            }
        }
        out.sort_by_key(|c| c.id);
        self.lines += u64::try_from(out.len()).unwrap_or(u64::MAX);
        self.forget_old();
        Ok(out)
    }

    /// Where a look starts in a table: a minute before the newest id read
    /// there, or the watcher's start.
    fn start_of(&self, newest: Option<Uuid>) -> Uuid {
        newest.map_or_else(|| id_at(self.start), |id| id_at(uuid_time(id) - TimeDelta::seconds(OVERLAP_SECS)))
    }

    /// Forget the lines read before the overlap: no look reads them again.
    fn forget_old(&mut self) {
        let newest = [self.newest_message, self.newest_event].into_iter().flatten().map(uuid_time).min();
        if let Some(newest) = newest {
            let before = newest - TimeDelta::seconds(OVERLAP_SECS * 2);
            self.read.retain(|id| uuid_time(*id) >= before);
        }
    }

    fn count(&mut self, why: Why, agent: &str) {
        *self.unread.entry((why, agent.to_string())).or_insert(0) += 1;
    }

    /// Every agent and session the kernel has named, through its own verb.
    async fn learn_names(&mut self, kernel: &Kernel) -> Result<()> {
        for a in kernel.list_named_entities("node_agent", "attr_agent_descriptor").await? {
            if let (Some(Value::String(name)), Some(id)) = (field(&a.payload, "name"), uuid_of(&a.entity_id)) {
                self.agents.insert(id, name.clone());
            }
        }
        for s in kernel.list_named_entities("node_session", "attr_session_descriptor").await? {
            // `<agent>/<session key>`, as capture names a session.
            let Some(Value::String(name)) = field(&s.payload, "name") else { continue };
            let (Some((agent, key)), Some(id)) = (name.split_once('/'), uuid_of(&s.entity_id)) else { continue };
            self.sessions.insert((agent.to_string(), key.to_string()), id);
            self.names.insert(id, (name.clone(), key.to_string()));
        }
        Ok(())
    }
}

/// A row's line: its text parsed as JSON, else the parsed copy the kernel
/// kept.
fn line_of(text: Option<&Value>, copy: Option<&Value>) -> Option<Json> {
    match (text, copy) {
        (Some(Value::String(t)), _) => serde_json::from_str(t).ok(),
        (_, Some(copy @ Value::Object(_))) => Some(copy.clone().into_json_value()),
        _ => None,
    }
}

/// When a line was written: the timestamp its agent wrote in it, else the
/// time its id holds.
fn at(line: &Json, id: Uuid) -> DateTime<Utc> {
    line.get("timestamp")
        .and_then(Json::as_str)
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map_or_else(|| uuid_time(id), |t| t.with_timezone(&Utc))
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key),
        _ => None,
    }
}

fn uuid_at(row: &Value, key: &str) -> Option<Uuid> {
    match field(row, key) {
        Some(Value::RecordId(id)) => uuid_of(id),
        _ => None,
    }
}

fn uuid_of(id: &RecordId) -> Option<Uuid> {
    match &id.key {
        RecordIdKey::Uuid(u) => Some(*u),
        _ => None,
    }
}

/// When a UUIDv7 says its line was captured.
#[must_use]
pub fn uuid_time(id: Uuid) -> DateTime<Utc> {
    id.get_timestamp()
        .and_then(|t| {
            let (secs, nanos) = t.to_unix();
            Utc.timestamp_opt(i64::try_from(secs).ok()?, nanos).single()
        })
        .unwrap_or_default()
}

/// The smallest id a line captured at `t` can have.
#[must_use]
pub fn id_at(t: DateTime<Utc>) -> Uuid {
    let ms = u64::try_from(t.timestamp_millis()).unwrap_or(0);
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    Uuid::from(uuid::Uuid::from_bytes(bytes))
}
