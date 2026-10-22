//! The activity feed — ONE merged, chronologically ordered stream of
//! everything the OS captures: `message` rows AND `telemetry_stream`
//! events, in one row shape ([`SseEvent`]) end-to-end (issues #172,
//! #187).
//!
//! The same feed serves two scopes (operator directive 2026-08-19:
//! the feeds are identical, only scope differs):
//! - [`global_activity`] — everyone and everything, one place;
//! - [`session_activity`] — the feed filtered to one session
//!   (sessions are the top-level grouping of activity).
//!
//! Action rows are matched to a session two ways:
//! - `subject = <session entity>` — e.g. `message_captured`, whose
//!   emitter IS the session;
//! - `agent = <session's agent> AND payload.session = <source key>` —
//!   e.g. `transcript_event` / `tool_call`, which are attributed to a
//!   source entity but stamp the source-native session key on their
//!   payload. The agent scope prevents cross-session bleed when two
//!   agents' sessions share a fallback key such as `unknown-session`.
//!
//! Known gap (#186): `transcript_raw` events carry no session key, so
//! unparseable transcript lines surface only in the global feed.
//!
//! Everything here is pure SELECT — readers must not mutate the
//! stream they observe.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, MessageRecord, Result, TelemetryRecord};

use crate::api::SseEvent;

/// The source-session key an adapter stamped on a per-session event
/// (`payload.session`), when present. `None` for global events and
/// non-object payloads.
#[must_use]
pub fn session_key_of(payload: &Value) -> Option<String> {
    match payload {
        Value::Object(o) => match o.get("session") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// One feed row from a captured message — the SAME shape the SSE
/// bridge broadcasts, so backlog and live rows are interchangeable.
#[must_use]
pub fn message_event(m: &MessageRecord) -> SseEvent {
    let (rendered, request) = message_row(m);
    let (request, request_line) = request.unzip();
    SseEvent {
        id: superx_ops::record_uuid(&m.id),
        kind: "message".to_string(),
        rendered,
        role: Some(m.role.clone()),
        agent_id: Some(superx_ops::record_uuid(&m.agent)),
        session_id: Some(superx_ops::record_uuid(&m.session)),
        session_src: None,
        valid_from: m.valid_from.to_rfc3339(),
        request,
        request_line,
        heading: None,
        said: None,
    }
}

/// One feed row from a telemetry event — the SAME shape the SSE
/// bridge broadcasts.
#[must_use]
pub fn action_event(a: &TelemetryRecord) -> SseEvent {
    let (rendered, line) = action_row(a);
    let (heading, said) = line.unzip();
    SseEvent {
        id: superx_ops::record_uuid(&a.id),
        kind: "action".to_string(),
        rendered,
        role: None,
        agent_id: a.agent.as_ref().map(superx_ops::record_uuid),
        session_id: None,
        session_src: session_key_of(&a.payload),
        valid_from: a.valid_from.to_rfc3339(),
        request: None,
        request_line: None,
        heading,
        said,
    }
}

/// A message's feed row (#479): its stamp and role, then what the export
/// writes for it, whole — on the same line when that is one line — and its
/// request with the request's line, which the page shows once per request
/// (#481). The CLI's one-line summary (`superx_ops::render_message`) cut a
/// tool call's hint at 60 characters and wrote thinking and tool output as
/// a word.
fn message_row(m: &MessageRecord) -> (String, Option<(String, String)>) {
    let ts = m.emitted_at.unwrap_or(m.valid_from).format("%Y-%m-%d %H:%M:%S");
    let head = format!("[{ts}] {:>9}:", m.role);
    let (body, request) = crate::export::message_body(m);
    let row = match body.lines().count() {
        0 => head,
        1 => format!("{head} {}", body.trim_start()),
        _ => format!("{head}\n{body}"),
    };
    (row, request)
}

/// An action's feed row: a transcript line kept whole (#471) as the export
/// writes it (#479), whose payload `superx_ops::render_event` cut at 80
/// characters, with its heading and a digest of what it says (#481); any
/// other event as the CLI renders it.
fn action_row(a: &TelemetryRecord) -> (String, Option<(String, String)>) {
    match crate::export::line_body(a) {
        Some((heading, body, said)) => {
            let head = format!("[{}] {heading}", a.valid_from.format("%H:%M:%S"));
            let row = if body.is_empty() { head } else { format!("{head}\n{body}") };
            (row, Some((heading, said)))
        }
        None => (superx_ops::render_event(a).trim_end().to_string(), None),
    }
}

/// What a feed read selects from `table`: every field, except a message's
/// `raw.toolUseResult`, Claude Code's structured copy of a tool's output.
/// The row already carries that output, and the feed never renders the
/// copy, which was a fifth of every byte a session's messages read (#461).
pub(crate) fn projection(table: &str) -> &'static str {
    if table == "message" {
        "* OMIT raw.toolUseResult"
    } else {
        "*"
    }
}

/// The session a request names (#461). The page names a session by its id,
/// and one read checks that it is a session; `superx_ops::resolve_session`
/// matches a name against every session (2 + 2×sessions round trips), and
/// every page of a long session's feed paid for that. A name or a prefix
/// still goes through it.
///
/// # Errors
///
/// The resolver's message when nothing matches, or the engine's.
pub async fn session_named(kernel: &Kernel, id: &str) -> std::result::Result<RecordId, String> {
    if let Ok(uuid) = id.parse::<superx_kernel::types::Uuid>() {
        let session = RecordId::new("entity", uuid);
        let descriptor = crate::answered("session lookup", || {
            kernel.current_state(session.clone(), "attr_session_descriptor")
        })
        .await
        .map_err(|e| e.to_string())?;
        if descriptor.is_some() {
            return Ok(session);
        }
    }
    superx_ops::resolve_session(kernel, id).await
}

/// How far back each live poll looks again (#183). A row is stamped with
/// its time before it is written, so one stamped before a poll and
/// committed after it lands behind the cursor, where a read of newer rows
/// never looks. Rows already sent are skipped by id.
const LATE_COMMIT_SECS: i64 = 10; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Batches one live poll reads, at most; the rest follow on the next.
const LIVE_PAGES: usize = 5; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A row the live stream carries.
pub trait LiveRow: superx_kernel::types::SurrealValue + Send {
    /// Its id and capture time: the stream's order.
    fn key(&self) -> (&RecordId, chrono::DateTime<chrono::Utc>);
    /// The feed event it renders as.
    fn event(&self) -> SseEvent;
}

impl LiveRow for TelemetryRecord {
    fn key(&self) -> (&RecordId, chrono::DateTime<chrono::Utc>) {
        (&self.id, self.valid_from)
    }
    fn event(&self) -> SseEvent {
        action_event(self)
    }
}

impl LiveRow for MessageRecord {
    fn key(&self) -> (&RecordId, chrono::DateTime<chrono::Utc>) {
        (&self.id, self.valid_from)
    }
    fn event(&self) -> SseEvent {
        message_event(self)
    }
}

/// One live stream's read position (#183). The newest row sent is held as
/// (capture time, id), a total order: a batch cut inside a run of rows
/// written in one instant resumes inside that run, where a time alone
/// resumed past it. The ids sent in the last `LATE_COMMIT_SECS` are held
/// too, so each poll looks back over that window for a row that committed
/// late, and sends nothing twice.
pub struct LiveCursor {
    table: &'static str,
    at: chrono::DateTime<chrono::Utc>,
    id: Option<RecordId>,
    /// Where the stream started: nothing before it is sent, late or not.
    floor: chrono::DateTime<chrono::Utc>,
    sent: std::collections::HashMap<RecordId, chrono::DateTime<chrono::Utc>>,
}

impl LiveCursor {
    /// A stream over `table` from `at` onwards.
    #[must_use]
    pub fn new(table: &'static str, at: chrono::DateTime<chrono::Utc>) -> Self {
        Self { table, at, id: None, floor: at, sent: std::collections::HashMap::new() }
    }

    /// Start again from `at`, forgetting what was sent: nobody watched.
    pub fn skip_to(&mut self, at: chrono::DateTime<chrono::Utc>) {
        *self = Self::new(self.table, at);
    }

    /// The rows written since the last read, as feed events: the new rows
    /// oldest first, then any that committed late behind the cursor. The
    /// feed orders by capture time, so a late row lands where it belongs.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] for engine errors or a read that got
    /// no answer, twice.
    pub async fn read<T: LiveRow>(&mut self, kernel: &Kernel, batch: u32) -> Result<Vec<SseEvent>> {
        let mut out = Vec::new();
        for _ in 0..LIVE_PAGES {
            // Forward from the newest row sent, in (time, id) order. `>=`
            // first, so the time index is walked and the tie is a filter.
            let forward = match self.id {
                Some(_) => format!(
                    "SELECT {} FROM {} WHERE valid_from >= $at AND (valid_from > $at OR id > $id) \
                     ORDER BY valid_from ASC, id ASC LIMIT $batch",
                    projection(self.table),
                    self.table
                ),
                None => format!(
                    "SELECT {} FROM {} WHERE valid_from > $at ORDER BY valid_from ASC, id ASC LIMIT $batch",
                    projection(self.table),
                    self.table
                ),
            };
            let (at, id) = (self.at, self.id.clone());
            let rows: Vec<T> = crate::answered("live stream", || {
                let (sql, id) = (forward.clone(), id.clone());
                async move {
                    let mut q = kernel.db().query(sql).bind(("at", at)).bind(("batch", batch));
                    if let Some(id) = id {
                        q = q.bind(("id", id));
                    }
                    Ok(q.await?.take(0)?)
                }
            })
            .await?;
            for r in &rows {
                let (id, t) = r.key();
                self.at = t;
                self.id = Some(id.clone());
                if self.sent.insert(id.clone(), t).is_none() {
                    out.push(r.event());
                }
            }
            if rows.len() < batch as usize {
                break;
            }
        }
        // Behind the cursor in (time, id) order, for a row that committed
        // after the cursor passed it; what is ahead of the cursor is the
        // next poll's forward read. The rows already sent are excluded in
        // the engine: counted against the limit, the ones nearest the
        // cursor crowded out a late row further back.
        let lo = (self.at - chrono::Duration::seconds(LATE_COMMIT_SECS)).max(self.floor);
        let (at, id) = (self.at, self.id.clone());
        let sent: Vec<RecordId> = self.sent.keys().cloned().collect();
        let behind = match self.id {
            Some(_) => format!(
                "SELECT {} FROM {} WHERE valid_from > $lo AND valid_from <= $at \
                 AND (valid_from < $at OR id <= $id) AND id NOTINSIDE $sent \
                 ORDER BY valid_from DESC LIMIT $batch",
                projection(self.table),
                self.table
            ),
            None => format!(
                "SELECT {} FROM {} WHERE valid_from > $lo AND valid_from <= $at \
                 AND id NOTINSIDE $sent ORDER BY valid_from DESC LIMIT $batch",
                projection(self.table),
                self.table
            ),
        };
        let rows: Vec<T> = crate::answered("live stream, late rows", || {
            let (sql, id, sent) = (behind.clone(), id.clone(), sent.clone());
            async move {
                let mut q = kernel
                    .db()
                    .query(sql)
                    .bind(("lo", lo))
                    .bind(("at", at))
                    .bind(("sent", sent))
                    .bind(("batch", batch));
                if let Some(id) = id {
                    q = q.bind(("id", id));
                }
                Ok(q.await?.take(0)?)
            }
        })
        .await?;
        for r in &rows {
            let (id, t) = r.key();
            if self.sent.insert(id.clone(), t).is_none() {
                out.push(r.event());
            }
        }
        // Older than the window, a row is never read again: forget it.
        self.sent.retain(|_, t| *t > lo);
        Ok(out)
    }
}

/// The session's source key and agent name from its current
/// `attr_session_descriptor` payload (`{name, session, locator}` —
/// written by the capture engine's `ensure_session`; `name` is
/// `<agent>/<key>`).
pub(crate) async fn descriptor_parts(
    kernel: &Kernel,
    session: RecordId,
) -> Result<(Option<String>, Option<String>)> {
    let desc = crate::answered("session descriptor", || {
        kernel.current_state(session.clone(), "attr_session_descriptor")
    })
    .await?;
    let Some(Value::Object(o)) = desc else {
        return Ok((None, None));
    };
    let src = match o.get("session") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    let agent_name = match o.get("name") {
        Some(Value::String(n)) => n.split('/').next().map(str::to_string),
        _ => None,
    };
    Ok((src, agent_name))
}

/// Resolve a session's scoping handles for action matching: the agent
/// entity + source key. `None` parts drop the payload.session arm —
/// it must match nothing, never everything.
pub(crate) async fn session_scope(
    kernel: &Kernel,
    session: RecordId,
) -> Result<Option<(RecordId, String)>> {
    let (src, agent_name) = descriptor_parts(kernel, session).await?;
    let agent = match agent_name {
        Some(ref name) => {
            crate::answered("session agent", || {
                kernel.find_entity_by_name("node_agent", "attr_agent_descriptor", name)
            })
            .await?
        }
        None => None,
    };
    Ok(match (agent, src) {
        (Some(agent), Some(src)) => Some((agent, src)),
        _ => None,
    })
}

/// A raw captured row awaiting rendering — rendering happens ONLY for
/// rows that survive truncation.
enum Raw<'a> {
    Msg(&'a MessageRecord),
    Act(&'a TelemetryRecord),
}

/// Merge message + action rows by capture time and keep the NEWEST
/// `limit`, rendered oldest-first — per-stream limits alone would
/// return 2×limit. This function is the SOLE ordering authority: the
/// input slices may arrive in any order. Rows of one instant order by
/// id, the order the page cursor walks (#273).
fn merge_newest(
    messages: &[MessageRecord],
    actions: &[TelemetryRecord],
    limit: u32,
) -> Vec<SseEvent> {
    let mut rows: Vec<(chrono::DateTime<chrono::Utc>, String, Raw)> =
        Vec::with_capacity(messages.len() + actions.len());
    for m in messages {
        rows.push((m.valid_from, superx_ops::record_uuid(&m.id), Raw::Msg(m)));
    }
    for a in actions {
        rows.push((a.valid_from, superx_ops::record_uuid(&a.id), Raw::Act(a)));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let excess = rows.len().saturating_sub(limit as usize);
    rows.split_off(excess)
        .into_iter()
        .map(|(_, _, r)| match r {
            Raw::Msg(m) => message_event(m),
            Raw::Act(a) => action_event(a),
        })
        .collect()
}

/// Where a backwards page starts: the rows before this one — the oldest
/// row the client holds — in (capture time, id) order. `None` means the
/// present, the first page.
///
/// The time alone is not a total order: two rows can share an instant,
/// and a page edge between them skipped the other one for good, because
/// strict `<` never returned it (#273). The UUIDv7 id breaks the tie, as
/// the entities chains already do. Without an id (an older client) the
/// cursor is the time alone. The engine is asked for the cursor's second
/// whole and the rows older than the cursor are picked here (#487).
pub type Before = Option<(chrono::DateTime<chrono::Utc>, Option<superx_kernel::types::Uuid>)>;

/// A keyword the feed is filtered to, lowercased. `None` = no filter.
pub type Query<'a> = Option<&'a str>;

/// The message-side keyword clause. Searching runs over the captured
/// text itself, in the engine, so it reaches ALL history rather than
/// whatever the client happens to be holding.
pub(crate) const MSG_MATCH: &str = "string::contains(string::lowercase(content), $q)";

/// The payload fields a keyword search looks inside. A telemetry
/// payload is `any`, so there is no schema to enumerate — this is the
/// vocabulary the adapters and modules actually emit (the same kind of
/// captured-shape knowledge as `stats::WRITE_TOOLS`). Each is read as
/// the string it already is: no `<string>` cast, which §14 forbids.
const SEARCHED_PAYLOAD_FIELDS: &[&str] = &[
    "tool", "name", "file", "line", "session", "error", "source", "kind", "url", "detail",
    "snippet", "adapter", "reason", "status",
];

/// The action-side keyword clause: the event name, plus every payload
/// field above that holds a string. A field the payload lacks, or holds
/// as anything else, matches nothing: `line` is a whole transcript line,
/// an object, since #471, and `string::lowercase` refuses an object, which
/// failed the whole search (#477). `AND` stops at the check, so the
/// function never sees one.
pub(crate) fn act_match() -> String {
    let fields: Vec<String> = SEARCHED_PAYLOAD_FIELDS
        .iter()
        .map(|f| format!("(type::is_string(payload.{f}) AND string::contains(string::lowercase(payload.{f}), $q))"))
        .collect();
    format!(
        "(string::contains(string::lowercase(lifecycle_event), $q) OR {})",
        fields.join(" OR ")
    )
}

/// Assemble one page query. Every fragment spliced in here is a
/// compile-time constant; the operator's keyword and the cursor reach
/// the engine ONLY as bound parameters, never as query text.
fn page_query(table: &str, scope: Option<&str>, before: Before, q: Query, keyword: &str) -> String {
    let mut wheres: Vec<&str> = Vec::new();
    if let Some(s) = scope {
        wheres.push(s);
    }
    // One range on the time index, walked backwards, through the end of
    // the cursor's second: the index keeps whole seconds in order but not
    // the rows within one, so the rows older than the cursor are picked
    // after the read (#487).
    if before.is_some() {
        wheres.push("valid_from < $before");
    }
    if q.is_some() {
        wheres.push(keyword);
    }
    let clause = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {} ", wheres.join(" AND "))
    };
    format!("SELECT {} FROM {table} {clause}ORDER BY valid_from DESC, id DESC LIMIT $limit", projection(table))
}

/// Run a page query with only the bindings its clauses actually use.
/// The cursor's id is bound as a record of `$table`, so the engine
/// compares it with each row's id as the record it is (§14).
macro_rules! page {
    ($kernel:expr, $table:expr, $sql:expr, $limit:expr, $before:expr, $q:expr $(, $extra:expr)*) => {{
        crate::answered("activity page", || async {
            let mut stmt = $kernel.db().query(&$sql).bind(("limit", $limit));
            $( stmt = stmt.bind($extra); )*
            if let Some((cut, _)) = $before {
                stmt = stmt.bind(("before", second_after(cut)));
            }
            if let Some(k) = $q {
                stmt = stmt.bind(("q", k.to_lowercase()));
            }
            Ok(stmt.await?.take(0)?)
        })
        .await?
    }};
}

/// The bound a backward page reads to: just past the cursor's second, so
/// the page holds that second whole (#487).
fn second_after(cut: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(cut.timestamp() + 1, 1_000).unwrap_or(cut)
}

/// One table's rows for a backward page, ending on a whole second (#487).
/// SurrealDB 3.1's index on a datetime orders the rows within a second
/// wrongly (`.178` after `.178501`), so a page cut by its limit inside a
/// second could hold one row and miss another captured after it, which
/// the next page, asked for rows older than the oldest held, never
/// returned. So a page holds whole seconds only: a full read's oldest
/// second may be partial and is left for the next page, and the read grows
/// until the whole seconds older than the cursor fill the page, or history
/// ends. The rows older than the cursor are picked first: a page whose read
/// held only rows at or past the cursor and a partial older second came
/// back empty, and the page took it for the beginning of history; and a
/// page cut short of its limit by a dropped second left the feed's first
/// page with seven rows of ten (#496).
async fn whole_seconds<T, F, Fut>(limit: u32, before: Before, mut read: F) -> Result<Vec<T>>
where
    T: LiveRow,
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>>>,
{
    let mut ask = limit.max(1);
    loop {
        let rows = read(ask).await?;
        let full = rows.len() >= ask as usize;
        // The oldest second the engine reached: a full read may hold only
        // part of it.
        let oldest = rows.iter().map(|r| r.key().1.timestamp()).min();
        let rows = older_than(rows, before);
        if !full {
            return Ok(rows);
        }
        let whole: Vec<T> = rows.into_iter().filter(|r| Some(r.key().1.timestamp()) != oldest).collect();
        if whole.len() >= limit as usize {
            return Ok(whole);
        }
        ask = ask.saturating_mul(2);
    }
}

/// The rows of a backward page older than its cursor, by capture time and
/// then id (#273), compared here rather than by the index (#487).
fn older_than<T: LiveRow>(rows: Vec<T>, before: Before) -> Vec<T> {
    let Some((cut, id)) = before else { return rows };
    let id = id.map(|u| u.to_string());
    rows.into_iter()
        .filter(|r| {
            let (rid, at) = r.key();
            match &id {
                Some(id) => at < cut || (at == cut && superx_ops::record_uuid(rid) < *id),
                None => at < cut,
            }
        })
        .collect()
}

/// The NEWEST `limit` messages across ALL sessions, optionally walking
/// backwards from a cursor and/or filtered to a keyword (any order —
/// merge_newest sorts).
async fn recent_messages(
    kernel: &Kernel,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<MessageRecord>> {
    if before.is_none() && q.is_none() {
        return whole_seconds(limit, None, |ask| async move {
            crate::answered("recent messages", || async {
                Ok(kernel
                    .db()
                    .query(format!("SELECT {} FROM message ORDER BY valid_from DESC LIMIT $limit", projection("message")))
                    .bind(("limit", ask))
                    .await?
                    .take(0)?)
            })
            .await
        })
        .await;
    }
    let sql = page_query("message", None, before, q, MSG_MATCH);
    let rows: Vec<MessageRecord> = whole_seconds(limit, before, |ask| {
        let sql = sql.clone();
        async move { Ok(page!(kernel, "message", sql, ask, before, q)) }
    })
    .await?;
    Ok(rows)
}

/// The NEWEST `limit` telemetry events, optionally older than a cursor
/// and/or filtered to a keyword. The kernel's own `recent_telemetry`
/// has no cursor or search form, and adding one is a kernel change —
/// so the paged read lives here, in the module, over the same table
/// and index.
async fn recent_actions(
    kernel: &Kernel,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<TelemetryRecord>> {
    if before.is_none() && q.is_none() {
        return whole_seconds(limit, None, |ask| async move {
            crate::answered("recent actions", || kernel.recent_telemetry(ask)).await
        })
        .await;
    }
    let sql = page_query("telemetry_stream", None, before, q, &act_match());
    let rows: Vec<TelemetryRecord> = whole_seconds(limit, before, |ask| {
        let sql = sql.clone();
        async move { Ok(page!(kernel, "telemetry_stream", sql, ask, before, q)) }
    })
    .await?;
    Ok(rows)
}

/// The GLOBAL feed: everything the OS captured, everyone and every
/// session in one place — the NEWEST `limit` rows of the merged
/// stream, oldest first (issue #187: Activity's historical backlog).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn global_activity(
    kernel: &Kernel,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<SseEvent>> {
    let actions = recent_actions(kernel, limit, before, q).await?;
    let messages = recent_messages(kernel, limit, before, q).await?;
    Ok(merge_newest(&messages, &actions, limit))
}

/// The feed filtered to ONE session: the NEWEST `limit` rows of its
/// messages + its action events, oldest first. Newest-N so the
/// pinned-to-bottom view always ends at the session's present. Global
/// events (module lifecycle, boot) are not a session's activity and
/// never appear here.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_activity(
    kernel: &Kernel,
    session: RecordId,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<SseEvent>> {
    let scope = session_scope(kernel, session.clone()).await?;

    let msg_sql = page_query("message", Some("session = $sess"), before, q, MSG_MATCH);
    let messages: Vec<MessageRecord> = whole_seconds(limit, before, |ask| {
        let (sql, session) = (msg_sql.clone(), session.clone());
        async move { Ok(page!(kernel, "message", sql, ask, before, q, ("sess", session.clone()))) }
    })
    .await?;

    // Two ways an action belongs to a session (see the module doc);
    // without a resolved scope only the subject arm can match — it must
    // never widen to everything.
    let act_sql = page_query(
        "telemetry_stream",
        Some(match scope {
            Some(_) => "(subject = $sess OR (agent = $agent AND payload.session = $src))",
            None => "subject = $sess",
        }),
        before,
        q,
        &act_match(),
    );
    let actions: Vec<TelemetryRecord> = whole_seconds(limit, before, |ask| {
        let (sql, session, scope) = (act_sql.clone(), session.clone(), scope.clone());
        async move {
            Ok(match scope {
                Some((agent, src_key)) => page!(
                    kernel,
                    "telemetry_stream",
                    sql,
                    ask,
                    before,
                    q,
                    ("sess", session.clone()),
                    ("agent", agent.clone()),
                    ("src", src_key.clone())
                ),
                None => page!(kernel, "telemetry_stream", sql, ask, before, q, ("sess", session.clone())),
            })
        }
    })
    .await?;
    Ok(merge_newest(&messages, &actions, limit))
}

/// Pull an i64 field out of a dynamic row object; 0 when absent.
fn int_of(o: &superx_kernel::types::Object, key: &str) -> i64 {
    match o.get(key) {
        Some(Value::Number(n)) => n.to_int().unwrap_or(0),
        _ => 0,
    }
}


/// Rows read when looking for a session's newest real model or
/// effort — enough to see past a run of runtime-written lines.
const NEWEST_SCAN: u32 = 12; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A session's token telemetry, mined from the raw events adapters
/// already preserve (issue #200): `(context, output_total)`.
///
/// - `context` — the NEWEST usage-bearing message's prompt footprint:
///   Claude Code `raw.message.usage` (input + cache_read +
///   cache_creation) or Gemini `raw.tokens` (total; else input+cached).
/// - `output_total` — Σ output tokens across the session.
///
/// Both `None` when the session carries no usage data (e.g. telemetry-
/// only agents). Pure SELECT over the `(session, valid_from)` index.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
/// The model CURRENTLY doing this session's work: the newest message
/// that names one. A session outlives the model choice — the operator
/// switches mid-conversation — so this is a moving fact, read fresh
/// rather than stamped on the session once (issue #241).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_model(kernel: &Kernel, session: RecordId) -> Result<Option<String>> {
    Ok(session_model_effort(kernel, session).await?.0)
}

/// The session's CURRENT model and reasoning effort, in one read.
/// Effort is a separate newest-value lookup because a session can name
/// a model on one message and its effort on another.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_model_effort(
    kernel: &Kernel,
    session: RecordId,
) -> Result<(Option<String>, Option<String>)> {
    // A few rows, not one: the newest line may name `<synthetic>`, the
    // runtime's marker for a line it wrote itself. It is not a model
    // (#367) — the Status page has filtered it since, the Sessions
    // page showed it (#388). Skip the sentinels and take the newest
    // real answer behind them.
    let newest = |field: &str, guard: &str| {
        format!(
            "SELECT {field} AS v, valid_from FROM message \
             WHERE session = $sess AND {guard} \
             ORDER BY valid_from DESC LIMIT {NEWEST_SCAN}"
        )
    };
    let pick = |rows: Vec<Value>| {
        rows.iter().find_map(|row| match row {
            Value::Object(o) => match o.get("v") {
                Some(Value::String(s)) if !s.is_empty() && !s.starts_with('<') => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
    };
    let model: Vec<Value> = crate::answered("session model", || async {
        Ok(kernel
        .db()
        .query(newest(
            "raw.message.model ?? raw.model",
            "(raw.message.model != NONE OR raw.model != NONE)",
        ))
        .bind(("sess", session.clone()))
        .await?
        .take(0)?)
    })
    .await?;
    let effort: Vec<Value> = crate::answered("session effort", || async {
        Ok(kernel
        .db()
        .query(newest("raw.effort", "raw.effort != NONE"))
        .bind(("sess", session.clone()))
        .await?
        .take(0)?)
    })
    .await?;
    Ok((pick(model), pick(effort)))
}

/// When a session last did anything, on the AGENT'S clock (#413): the
/// newest captured row's own timestamp. The capture time said a session
/// last worked at the moment a backfill read it — a Gemini chat from May
/// read as active in August.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_last_emitted(
    kernel: &Kernel,
    session: RecordId,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let rows: Vec<Value> = crate::answered("session last active", || async {
        Ok(kernel
        .db()
        .query(
            "SELECT (emitted_at ?? valid_from) AS at, valid_from FROM message \
             WHERE session = $sess ORDER BY valid_from DESC LIMIT 1",
        )
        .bind(("sess", session.clone()))
        .await?
        .take(0)?)
    })
    .await?;
    Ok(rows.first().and_then(|row| match row {
        Value::Object(o) => match o.get("at") {
            Some(Value::Datetime(d)) => Some(**d),
            _ => None,
        },
        _ => None,
    }))
}

/// Newest replies read for a session's context, at most: enough to step
/// past a run of `<synthetic>` or zero-usage replies.
const CONTEXT_PROBE: u32 = 20; // skill-allow: §9-const — read-path bound, not a policy tunable

pub async fn session_token_stats(
    kernel: &Kernel,
    session: RecordId,
) -> Result<(Option<i64>, Option<i64>)> {
    // Once per reply (#409): a reply's usage rides every line Claude Code
    // writes for it, and Gemini re-emits a record as it streams.
    let rows: Vec<Value> = crate::answered("session output tokens", || async {
        Ok(kernel
        .db()
        .query(format!(
            "SELECT math::sum(o) AS toks FROM (\
                 SELECT {key} AS k, math::max({out}) AS o FROM message \
                 WHERE session = $sess AND (raw.message.usage != NONE OR raw.tokens != NONE) \
                 GROUP BY k\
             ) GROUP ALL",
            key = crate::stats::REPLY_KEY_SQL,
            out = crate::stats::OUT_TOKENS_SQL,
        ))
        .bind(("sess", session.clone()))
        .await?
        .take(0)?)
    })
    .await?;
    let output_total = rows
        .first()
        .and_then(|row| match row {
            Value::Object(o) => Some(int_of(o, "toks")),
            _ => None,
        })
        .filter(|&n| n > 0);

    // The prompt the newest reply answered, read the way the Status page
    // reads it (#415 review). A `<synthetic>` reply — the runtime's own
    // stand-in for an API error — carries all-zero usage, and taking it
    // blanked the bar; so does a reply whose usage reads zero.
    let rows: Vec<Value> = crate::answered("session context", || async {
        Ok(kernel
        .db()
        .query(format!(
            "SELECT raw.message.usage AS cu, raw.tokens AS gu, (emitted_at ?? valid_from) AS at \
             FROM message WHERE session = $sess \
               AND (raw.message.usage != NONE OR raw.tokens != NONE) \
               AND (raw.message.model ?? raw.model ?? '') != '<synthetic>' \
             ORDER BY at DESC LIMIT {CONTEXT_PROBE}"
        ))
        .bind(("sess", session.clone()))
        .await?
        .take(0)?)
    })
    .await?;
    let context = rows.iter().find_map(|row| {
        let Value::Object(o) = row else { return None };
        let usage = |key: &str| match o.get(key) {
            Some(Value::Object(u)) => Some(u),
            _ => None,
        };
        crate::stats::usage_of(usage("cu"), usage("gu"))
            .map(|u| u.context)
            .filter(|&n| n > 0)
    });
    Ok((context, output_total))
}

/// Count of a session's action events (same matching as
/// [`session_activity`]) — the sessions list shows TOTAL activity
/// (messages + actions), not messages alone (issue #187).
///
/// `scope` is the pre-resolved `(agent entity, source key)` pair —
/// callers iterating many sessions resolve agents ONCE and pass the
/// scope in, instead of paying a descriptor re-read + agent lookup
/// per session (review finding). `None` drops the payload.session
/// arm (matches nothing, never everything).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_action_count(
    kernel: &Kernel,
    session: RecordId,
    scope: Option<(RecordId, String)>,
) -> Result<i64> {
    let rows: Vec<Value> = match scope {
        Some((agent, src_key)) => {
            crate::answered("session events", || async {
                Ok(kernel
                .db()
                .query(
                    "SELECT count() AS c FROM telemetry_stream \
                     WHERE subject = $sess \
                        OR (agent = $agent AND payload.session = $src) \
                     GROUP ALL",
                )
                .bind(("sess", session.clone()))
                .bind(("agent", agent.clone()))
                .bind(("src", src_key.clone()))
                .await?
                .take(0)?)
            })
            .await?
        }
        None => {
            crate::answered("session events", || async {
                Ok(kernel
                .db()
                .query(
                    "SELECT count() AS c FROM telemetry_stream \
                     WHERE subject = $sess GROUP ALL",
                )
                .bind(("sess", session.clone()))
                .await?
                .take(0)?)
            })
            .await?
        }
    };
    // Row shape: {c: <count>} — read through the kernel's re-exported
    // Value, per the module layering rule (no direct surrealdb dep).
    Ok(rows
        .first()
        .and_then(|row| match row {
            Value::Object(o) => match o.get("c") {
                Some(Value::Number(n)) => n.to_int(),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or(0))
}
