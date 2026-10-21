//! The session export (#460): one session as one file, for an agent to
//! review the work. It holds every row the substrate holds for the session
//! and every byte of each that is conversation: text, thinking, a tool
//! call's whole input, a tool's whole output (a string or a block array),
//! images named with their type and size, model fallbacks, and, with
//! "everything", the OS's action events with their whole payloads. A shape
//! the export does not know is written as its JSON, never dropped.
//!
//! The feed's renderer (`superx_ops::render_message`) is a one-line
//! summary: right for a feed, wrong for a record. On one session it left
//! out 2.6 MB of tool input, 100 K characters of thinking and every tool
//! output that came as a block array (#460). This one reads the captured
//! event (`raw`) whole.
//!
//! One request walks the session's messages oldest-first by the (capture
//! time, id) cursor over the existing indexes, then its action events into
//! an appendix, and streams the file as it goes, so the page writes it
//! straight into the file the reader picked. Each request to the model
//! closes with its model, effort and tokens; each thinking block says how
//! long it took, and each tool result how its call was allowed (#472). The file's
//! last line counts what it holds; the page keeps the file only when that
//! line arrived, so a failed export never leaves a short file that looks
//! whole.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write as _;

use axum::body::Bytes;
use chrono::{DateTime, Offset as _, Utc};
use chrono_tz::Tz;
use futures_core::Stream;
use serde_json::{Map, Value as Json};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, MessageRecord, Result, TelemetryRecord};

use crate::activity::LiveRow;

/// Messages read at a time. Each carries its whole captured event, so
/// fewer of them than actions.
pub const MESSAGE_PAGE: u32 = 500; // skill-allow: §9-const — read-path bound, not a policy tunable
/// Action events read at a time.
pub const ACTION_PAGE: u32 = 2000; // skill-allow: §9-const — read-path bound, not a policy tunable
/// The lines of a tool's input or output that "first lines" keeps: the
/// count the export panel names.
pub const TRIMMED_TOOL_LINES: usize = 20; // skill-allow: §9-const — render-layer bound, the choice the page names
/// How much of the file is gathered before it is sent on.
const CHUNK_BYTES: usize = 64 * 1024; // skill-allow: §9-const — transport bound, not a policy tunable

/// Fields written nowhere, by design, and named at the top of the file:
/// a thinking block's signature and a Gemini part's thought signature are
/// opaque tokens for the provider, not anything an agent can read.
const OPAQUE: &[&str] = &["signature", "thoughtSignature"];

/// The marker that opens the file's last line. The page checks for it
/// before it keeps the file.
pub const END_MARK: &str = "end of export";

/// What the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Include {
    /// The messages: user, assistant, tool and system.
    Conversation,
    /// The messages and the OS's action events.
    Everything,
}

/// How much of each tool call's input and each tool's output is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDetail {
    /// Whole.
    Full,
    /// The first [`TRIMMED_TOOL_LINES`] lines, and how many more there were.
    FirstLines,
    /// A one-line marker with the line count.
    LeftOut,
}

/// The file's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Text,
}

impl Format {
    /// The response's media type.
    #[must_use]
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown; charset=utf-8",
            Self::Text => "text/plain; charset=utf-8",
        }
    }
}

/// The export panel's choices.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub include: Include,
    pub tools: ToolDetail,
    pub format: Format,
    /// The reader's zone when the file carries times; `None` writes none.
    pub clock: Option<Tz>,
    /// Only the rows the session view's search matches; `None` is all.
    pub matching: Option<String>,
}

impl ExportOptions {
    /// The panel's choices, as the page sends them. An unknown value is
    /// refused rather than read as a default: a default would silently
    /// write a different file than the one asked for.
    ///
    /// # Errors
    ///
    /// A message naming the value that is not one of the panel's.
    pub fn parse(
        include: Option<&str>,
        tools: Option<&str>,
        format: Option<&str>,
        tz: Option<&str>,
        matching: Option<&str>,
    ) -> std::result::Result<Self, String> {
        let given = |v: Option<&str>| v.map_or_else(|| "nothing".to_string(), |s| format!("“{s}”"));
        let include = match include {
            Some("conversation") => Include::Conversation,
            Some("everything") => Include::Everything,
            other => return Err(format!("include must be conversation or everything, not {}", given(other))),
        };
        let tools = match tools {
            Some("full") => ToolDetail::Full,
            Some("trimmed") => ToolDetail::FirstLines,
            Some("none") => ToolDetail::LeftOut,
            other => return Err(format!("tools must be full, trimmed or none, not {}", given(other))),
        };
        let format = match format {
            Some("md") => Format::Markdown,
            Some("txt") => Format::Text,
            other => return Err(format!("format must be md or txt, not {}", given(other))),
        };
        let clock = match tz {
            None => None,
            Some(name) => Some(
                name.parse::<Tz>()
                    .map_err(|e| format!("tz must be an IANA time zone, not “{name}” ({e})"))?,
            ),
        };
        let matching = matching.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Self { include, tools, format, clock, matching })
    }
}

/// Rows read at a time; tests walk with small pages to cross their edges.
#[derive(Debug, Clone, Copy)]
pub struct Pages {
    pub messages: u32,
    pub actions: u32,
}

impl Default for Pages {
    fn default() -> Self {
        Self { messages: MESSAGE_PAGE, actions: ACTION_PAGE }
    }
}

/// The session as one file, in pieces as it is written: every row captured
/// for the session up to `until`, oldest first, merged by capture time and
/// id as the feed merges them. A read that fails ends the stream with the
/// error, so the file never ends with its count line.
pub fn session_export(
    kernel: Kernel,
    session: RecordId,
    options: ExportOptions,
    until: DateTime<Utc>,
    pages: Pages,
) -> impl Stream<Item = Result<Bytes>> + Send + 'static {
    async_stream::try_stream! {
        let started = std::time::Instant::now();
        let (src, agent) = crate::activity::descriptor_parts(&kernel, session.clone()).await?;
        let (model, effort) = crate::activity::session_model_effort(&kernel, session.clone()).await?;
        let identity = format!(
            "{}/{}",
            agent.as_deref().unwrap_or("unknown"),
            superx_ops::record_uuid(&session)
        );
        // The action arm is matched as the session view matches it; read
        // only when the file holds actions.
        let scope = match options.include {
            Include::Everything => Some(crate::activity::session_scope(&kernel, session.clone()).await?),
            Include::Conversation => None,
        };
        let keyword = options.matching.clone();
        let mut file = Writer::new(&options, until);
        file.header(&Facts {
            identity: &identity,
            source: src.as_deref(),
            model: model.as_deref(),
            effort: effort.as_deref(),
        });
        // The conversation, oldest first.
        let mut messages: Walk<MessageRecord> = Walk::new(pages.messages);
        loop {
            if messages.wants_page() {
                let rows = message_page(
                    &kernel, &session, keyword.as_deref(), messages.after.clone(), until, messages.page,
                )
                .await?;
                messages.take(rows);
            }
            let Some(row) = messages.pop() else { break };
            file.message(&row);
            if file.pending() >= CHUNK_BYTES {
                yield file.drain();
            }
        }
        file.close_request();
        // The OS's action events after it, in one appendix, oldest first
        // (#472): interleaved, they were most of the rows a reader had to
        // read past.
        if let Some(scope) = scope.as_ref() {
            file.appendix();
            let mut actions: Walk<TelemetryRecord> = Walk::new(pages.actions);
            loop {
                if actions.wants_page() {
                    let rows = action_page(
                        &kernel, &session, scope.as_ref(), keyword.as_deref(), actions.after.clone(), until, actions.page,
                    )
                    .await?;
                    actions.take(rows);
                }
                let Some(row) = actions.pop() else { break };
                file.action(&row);
                if file.pending() >= CHUNK_BYTES {
                    yield file.drain();
                }
            }
            file.kinds_note();
        }
        file.footer();
        tracing::info!(
            target: "ui",
            session = %identity,
            rows = file.rows(),
            bytes = file.written + file.pending(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "session exported"
        );
        yield file.drain();
    }
}

/// One table's rows, read a page at a time from the oldest.
struct Walk<T> {
    rows: VecDeque<T>,
    /// The last row read, in (capture time, id) order: the next page
    /// starts after it.
    after: Option<(DateTime<Utc>, RecordId)>,
    page: u32,
    done: bool,
}

impl<T: LiveRow> Walk<T> {
    fn new(page: u32) -> Self {
        Self { rows: VecDeque::new(), after: None, page: page.max(1), done: false }
    }

    fn wants_page(&self) -> bool {
        self.rows.is_empty() && !self.done
    }

    /// A page as read. A short one is the end.
    fn take(&mut self, rows: Vec<T>) {
        if rows.len() < self.page as usize {
            self.done = true;
        }
        if let Some(last) = rows.last() {
            let (id, at) = last.key();
            self.after = Some((at, id.clone()));
        }
        self.rows.extend(rows);
    }

    fn pop(&mut self) -> Option<T> {
        self.rows.pop_front()
    }
}

/// One forward page over a session's rows. `>=` first, so the engine walks
/// the time index and the tie is only a filter on it (#273); the snapshot
/// bound keeps rows captured after the export began out of it. The fields
/// are the feed's (`activity::projection`): a message's `toolUseResult`,
/// the agent's own structured copy of output the message already carries,
/// stays in the engine.
fn walk_query(table: &str, scope: &str, cursor: bool, keyword: Option<&str>) -> String {
    let mut wheres = vec![scope.to_string(), "valid_from <= $until".to_string()];
    if cursor {
        wheres.push("valid_from >= $at AND (valid_from > $at OR id > $id)".to_string());
    }
    if let Some(k) = keyword {
        wheres.push(k.to_string());
    }
    format!(
        "SELECT {} FROM {table} WHERE {} ORDER BY valid_from ASC, id ASC LIMIT $page",
        crate::activity::projection(table),
        wheres.join(" AND ")
    )
}

/// The next page of the session's messages.
async fn message_page(
    kernel: &Kernel,
    session: &RecordId,
    keyword: Option<&str>,
    after: Option<(DateTime<Utc>, RecordId)>,
    until: DateTime<Utc>,
    page: u32,
) -> Result<Vec<MessageRecord>> {
    let sql = walk_query(
        "message",
        "session = $sess",
        after.is_some(),
        keyword.map(|_| crate::activity::MSG_MATCH),
    );
    crate::answered("export messages", || {
        let (sql, session, after) = (sql.clone(), session.clone(), after.clone());
        let keyword = keyword.map(str::to_lowercase);
        async move {
            let mut q = kernel
                .db()
                .query(sql)
                .bind(("sess", session))
                .bind(("until", until))
                .bind(("page", page));
            if let Some((at, id)) = after {
                q = q.bind(("at", at)).bind(("id", id));
            }
            if let Some(k) = keyword {
                q = q.bind(("q", k));
            }
            Ok(q.await?.take(0)?)
        }
    })
    .await
}

/// The next page of the session's action events, matched as the session
/// view matches them (see `activity`): emitted by the session, or by its
/// agent for its source key. Without a resolved scope only the first arm
/// can match; it never widens to everything.
async fn action_page(
    kernel: &Kernel,
    session: &RecordId,
    scope: Option<&(RecordId, String)>,
    keyword: Option<&str>,
    after: Option<(DateTime<Utc>, RecordId)>,
    until: DateTime<Utc>,
    page: u32,
) -> Result<Vec<TelemetryRecord>> {
    let act_match = crate::activity::act_match();
    let sql = walk_query(
        "telemetry_stream",
        match scope {
            Some(_) => "(subject = $sess OR (agent = $agent AND payload.session = $src))",
            None => "subject = $sess",
        },
        after.is_some(),
        keyword.map(|_| act_match.as_str()),
    );
    crate::answered("export actions", || {
        let (sql, session, after, scope) = (sql.clone(), session.clone(), after.clone(), scope.cloned());
        let keyword = keyword.map(str::to_lowercase);
        async move {
            let mut q = kernel
                .db()
                .query(sql)
                .bind(("sess", session))
                .bind(("until", until))
                .bind(("page", page));
            if let Some((agent, src)) = scope {
                q = q.bind(("agent", agent)).bind(("src", src));
            }
            if let Some((at, id)) = after {
                q = q.bind(("at", at)).bind(("id", id));
            }
            if let Some(k) = keyword {
                q = q.bind(("q", k));
            }
            Ok(q.await?.take(0)?)
        }
    })
    .await
}

// ─────────────────────────────────────────────────────────────────────
// A message, taken apart: every piece of it, in the order it was written
// ─────────────────────────────────────────────────────────────────────

/// One piece of a message.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    /// Text, as the agent or the person wrote it.
    Text(String),
    /// Reasoning: Claude's thinking, Gemini's thoughts.
    Thinking { title: Option<String>, text: String },
    /// A tool call and its whole input.
    Call { name: String, id: Option<String>, more: Option<Json>, input: Json },
    /// What a tool gave back.
    Result {
        name: Option<String>,
        id: Option<String>,
        error: bool,
        status: Option<String>,
        more: Option<Json>,
        parts: Vec<Part>,
    },
    /// A one-line fact: an image, a model fallback, a tool reference.
    Note(String),
    /// A shape the export does not know, whole.
    Unknown { label: String, json: Json },
}

/// One part of a tool's output.
#[derive(Debug, Clone, PartialEq)]
enum Part {
    Text(String),
    Note(String),
    Json(Json),
}

fn str_at(v: &Json, key: &str) -> Option<String> {
    v.get(key).and_then(Json::as_str).map(str::to_string)
}

/// The fields of `v` that `known` does not cover, opaque ones aside, so a
/// field nobody anticipated is written instead of lost.
fn more_of(v: &Json, known: &[&str]) -> Option<Json> {
    let rest: Map<String, Json> = v
        .as_object()?
        .iter()
        .filter(|(k, _)| !known.contains(&k.as_str()) && !OPAQUE.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (!rest.is_empty()).then_some(Json::Object(rest))
}

/// An image, named: its type and size, never its bytes.
fn image_note(block: &Json) -> String {
    let source = block.get("source");
    let media = source
        .and_then(|s| str_at(s, "media_type"))
        .unwrap_or_else(|| "image".to_string());
    if let Some(data) = source.and_then(|s| s.get("data")).and_then(Json::as_str) {
        let pad = data.bytes().rev().take_while(|&b| b == b'=').count();
        let bytes = (data.len() * 3 / 4).saturating_sub(pad);
        return format!("image · {media} · {} bytes", grouped(bytes as u64));
    }
    match source.and_then(|s| str_at(s, "url")) {
        Some(url) => format!("image · {url}"),
        None => format!("image · {media}"),
    }
}

/// `27529` → `27,529`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The pieces of one captured message. The two captured shapes are read
/// whole — Anthropic-style `message.content` blocks (Claude Code) and
/// Gemini's `content` parts, `thoughts` and `toolCalls`; anything else is
/// its extracted text and its captured event as JSON.
fn pieces_of(raw: Option<&Json>, content: &str, calls: &mut HashMap<String, String>) -> Vec<Piece> {
    let mut out = Vec::new();
    match raw {
        Some(raw) if raw.pointer("/message/content").is_some() => {
            claude_pieces(raw.pointer("/message/content").unwrap_or(&Json::Null), calls, &mut out);
        }
        Some(raw) if ["content", "thoughts", "toolCalls"].iter().any(|k| raw.get(k).is_some()) => {
            gemini_pieces(raw, calls, &mut out);
        }
        Some(raw) => {
            if !content.is_empty() {
                out.push(Piece::Text(content.to_string()));
            }
            out.push(Piece::Unknown { label: "captured event".to_string(), json: raw.clone() });
        }
        None if !content.is_empty() => out.push(Piece::Text(content.to_string())),
        None => {}
    }
    out
}

fn claude_pieces(content: &Json, calls: &mut HashMap<String, String>, out: &mut Vec<Piece>) {
    match content {
        Json::String(s) => {
            if !s.is_empty() {
                out.push(Piece::Text(s.clone()));
            }
        }
        Json::Array(blocks) => {
            for b in blocks {
                claude_block(b, calls, out);
            }
        }
        Json::Null => {}
        other => out.push(Piece::Unknown { label: "content".to_string(), json: other.clone() }),
    }
}

/// A block's own fields beyond the ones its piece shows, as a piece of
/// their own.
fn details(kind: &str, block: &Json, known: &[&str], out: &mut Vec<Piece>) {
    if let Some(json) = more_of(block, known) {
        out.push(Piece::Unknown { label: format!("{kind} details"), json });
    }
}

fn claude_block(b: &Json, calls: &mut HashMap<String, String>, out: &mut Vec<Piece>) {
    let kind = b.get("type").and_then(Json::as_str).unwrap_or_default();
    match kind {
        "text" => {
            out.push(Piece::Text(str_at(b, "text").unwrap_or_default()));
            details(kind, b, &["type", "text"], out);
        }
        "thinking" => {
            out.push(Piece::Thinking { title: None, text: str_at(b, "thinking").unwrap_or_default() });
            details(kind, b, &["type", "thinking"], out);
        }
        "tool_use" => {
            let name = str_at(b, "name").unwrap_or_else(|| "tool".to_string());
            let id = str_at(b, "id");
            if let Some(id) = &id {
                calls.insert(id.clone(), name.clone());
            }
            out.push(Piece::Call {
                name,
                id,
                more: more_of(b, &["type", "name", "id", "input"]),
                input: b.get("input").cloned().unwrap_or(Json::Null),
            });
        }
        "tool_result" => {
            let id = str_at(b, "tool_use_id");
            out.push(Piece::Result {
                name: id.as_ref().and_then(|i| calls.get(i).cloned()),
                id,
                error: b.get("is_error").and_then(Json::as_bool).unwrap_or(false),
                status: None,
                more: more_of(b, &["type", "tool_use_id", "content", "is_error"]),
                parts: result_parts(b.get("content")),
            });
        }
        "image" => out.push(Piece::Note(image_note(b))),
        "fallback" => {
            let model = |k: &str| b.get(k).and_then(|m| str_at(m, "model"));
            match (model("from"), model("to")) {
                (Some(from), Some(to)) => {
                    out.push(Piece::Note(format!("model fallback · {from} → {to}")));
                    details(kind, b, &["type", "from", "to"], out);
                }
                _ => out.push(Piece::Unknown { label: kind.to_string(), json: b.clone() }),
            }
        }
        _ => out.push(Piece::Unknown {
            label: if kind.is_empty() { "block".to_string() } else { kind.to_string() },
            json: b.clone(),
        }),
    }
}

/// A Claude tool result's content: a string, or a block array of text,
/// images and tool references.
fn result_parts(content: Option<&Json>) -> Vec<Part> {
    match content {
        None | Some(Json::Null) => vec![],
        Some(Json::String(s)) => vec![Part::Text(s.clone())],
        Some(Json::Array(items)) => items.iter().map(result_item).collect(),
        Some(other) => vec![Part::Json(other.clone())],
    }
}

fn result_item(b: &Json) -> Part {
    let kind = b.get("type").and_then(Json::as_str);
    match kind {
        Some("text") if more_of(b, &["type", "text"]).is_none() => Part::Text(str_at(b, "text").unwrap_or_default()),
        // An image is named, never written, whatever else it carries.
        Some("image") => Part::Note(image_note(b)),
        Some("tool_reference") if more_of(b, &["type", "tool_name"]).is_none() => {
            Part::Note(format!("tool reference · {}", str_at(b, "tool_name").unwrap_or_default()))
        }
        _ => Part::Json(b.clone()),
    }
}

fn gemini_pieces(raw: &Json, calls: &mut HashMap<String, String>, out: &mut Vec<Piece>) {
    // Reasoning first: it came before the reply.
    if let Some(Json::Array(thoughts)) = raw.get("thoughts") {
        for t in thoughts {
            out.push(Piece::Thinking { title: str_at(t, "subject"), text: str_at(t, "description").unwrap_or_default() });
            details("thought", t, &["subject", "description", "timestamp"], out);
        }
    }
    gemini_content(raw.get("content"), calls, out);
    // What the person saw, when it is not what was sent (an @file
    // reference is expanded in `content`).
    if let Some(shown) = raw.get("displayContent") {
        if Some(shown) != raw.get("content") {
            out.push(Piece::Note("as the person saw it:".to_string()));
            gemini_content(Some(shown), calls, out);
        }
    }
    if let Some(Json::Array(tool_calls)) = raw.get("toolCalls") {
        for c in tool_calls {
            gemini_call(c, calls, out);
        }
    }
}

fn gemini_content(content: Option<&Json>, calls: &mut HashMap<String, String>, out: &mut Vec<Piece>) {
    match content {
        None | Some(Json::Null) => {}
        Some(Json::String(s)) => {
            if !s.is_empty() {
                out.push(Piece::Text(s.clone()));
            }
        }
        Some(Json::Array(parts)) => {
            for p in parts {
                gemini_part(p, calls, out);
            }
        }
        Some(other) => out.push(Piece::Unknown { label: "content".to_string(), json: other.clone() }),
    }
}

fn gemini_part(p: &Json, calls: &mut HashMap<String, String>, out: &mut Vec<Piece>) {
    if let Some(text) = p.get("text").and_then(Json::as_str) {
        out.push(Piece::Text(text.to_string()));
        details("part", p, &["text"], out);
    } else if let Some(call) = p.get("functionCall") {
        let name = str_at(call, "name").unwrap_or_else(|| "tool".to_string());
        let id = str_at(call, "id");
        if let Some(id) = &id {
            calls.insert(id.clone(), name.clone());
        }
        out.push(Piece::Call {
            name,
            id,
            more: more_of(call, &["name", "id", "args"]),
            input: call.get("args").cloned().unwrap_or(Json::Null),
        });
        details("part", p, &["functionCall"], out);
    } else if let Some(response) = p.get("functionResponse") {
        out.push(Piece::Result {
            name: str_at(response, "name"),
            id: str_at(response, "id"),
            error: false,
            status: None,
            more: more_of(response, &["name", "id", "response"]),
            parts: response_parts(response.get("response")),
        });
        details("part", p, &["functionResponse"], out);
    } else if let Some(data) = p.get("inlineData") {
        let mime = str_at(data, "mimeType").unwrap_or_else(|| "data".to_string());
        out.push(Piece::Note(format!("inline data · {mime}")));
        details("part", p, &["inlineData"], out);
    } else {
        out.push(Piece::Unknown { label: "part".to_string(), json: p.clone() });
    }
}

/// A Gemini function response: its `output` text, and anything else it
/// carries as JSON.
fn response_parts(response: Option<&Json>) -> Vec<Part> {
    match response {
        None | Some(Json::Null) => vec![],
        Some(Json::String(s)) => vec![Part::Text(s.clone())],
        Some(r) => match r.get("output").and_then(Json::as_str) {
            Some(output) => {
                let mut parts = vec![Part::Text(output.to_string())];
                if let Some(rest) = more_of(r, &["output"]) {
                    parts.push(Part::Json(rest));
                }
                parts
            }
            None => vec![Part::Json(r.clone())],
        },
    }
}

/// A Gemini tool call carries its result: the call, then what it gave.
fn gemini_call(c: &Json, calls: &mut HashMap<String, String>, out: &mut Vec<Piece>) {
    let name = str_at(c, "name").unwrap_or_else(|| "tool".to_string());
    let id = str_at(c, "id");
    if let Some(id) = &id {
        calls.insert(id.clone(), name.clone());
    }
    out.push(Piece::Call {
        name: name.clone(),
        id: id.clone(),
        more: more_of(c, &["name", "id", "args", "result", "status"]),
        input: c.get("args").cloned().unwrap_or(Json::Null),
    });
    let status = str_at(c, "status");
    let mut parts = Vec::new();
    match c.get("result") {
        None | Some(Json::Null) => {}
        Some(Json::Array(items)) => {
            for item in items {
                match item.get("functionResponse") {
                    Some(r) if more_of(item, &["functionResponse"]).is_none()
                        && more_of(r, &["name", "id", "response"]).is_none() =>
                    {
                        parts.extend(response_parts(r.get("response")));
                    }
                    _ => parts.push(Part::Json(item.clone())),
                }
            }
        }
        Some(other) => parts.push(Part::Json(other.clone())),
    }
    out.push(Piece::Result {
        name: Some(name),
        id,
        error: status.as_deref() == Some("error"),
        status,
        more: None,
        parts,
    });
}

/// The agent's model, effort and working place on a captured event; a
/// change is written where it happens.
#[derive(Debug, Default, Clone, PartialEq)]
struct Context {
    model: Option<String>,
    effort: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
}

fn context_of(raw: &Json) -> Context {
    let text = |v: Option<&Json>| v.and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    Context {
        model: text(raw.pointer("/message/model")).or_else(|| text(raw.get("model"))),
        effort: text(raw.get("effort")),
        cwd: text(raw.get("cwd")),
        branch: text(raw.get("gitBranch")),
    }
}

/// What a transcript line records beside its blocks (#472): how long its
/// thinking took, and how its tool call was allowed.
#[derive(Debug, Default, Clone, PartialEq)]
struct LineFacts {
    thought_ms: Option<u64>,
    permission: Option<String>,
}

fn line_facts(raw: &Json) -> LineFacts {
    LineFacts { thought_ms: raw.get("thinkingDurationMs").and_then(Json::as_u64), permission: permission_of(raw) }
}

/// How a tool call was allowed or refused, as the transcript records it
/// beside the call's result: the decision and everything said about it,
/// the classifier's request, and the kind of denial.
fn permission_of(raw: &Json) -> Option<String> {
    let decision = raw.get("permissionDecision");
    let request = raw.pointer("/serverClassifierContext/request").and_then(Json::as_str);
    let denial = raw.get("toolDenialKind").and_then(Json::as_str);
    if decision.is_none() && request.is_none() && denial.is_none() {
        return None;
    }
    let mut said = Vec::new();
    match decision.and_then(Json::as_object) {
        Some(d) => {
            if let Some(v) = d.get("decision").and_then(Json::as_str) {
                said.push(v.to_string());
            }
            for (k, v) in d.iter().filter(|(k, _)| k.as_str() != "decision") {
                let v = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                said.push(format!("{} {v}", spaced(k)));
            }
        }
        None => said.extend(decision.map(Json::to_string)),
    }
    if let Some(r) = request {
        said.push(format!("classifier request {r}"));
    }
    if let Some(k) = denial {
        said.push(format!("denied as {k}"));
    }
    Some(said.join(" · "))
}

/// `reasonType` → `reason type`.
fn spaced(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push(' ');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A duration as a reader says it: `850 ms`, `5.7 s`, `4 min 30 s`.
fn took(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{}.{} s", ms / 1_000, ms % 1_000 / 100)
    } else {
        format!("{} min {} s", ms / 60_000, ms % 60_000 / 1_000)
    }
}

/// One request to the model (#472). Claude Code writes a reply as several
/// rows, one per block, each repeating the request's counters as they stood
/// when it was written; Gemini re-emits a reply, fuller each time. So a
/// request's facts are written once, when its last row has gone by.
#[derive(Debug, Clone, PartialEq)]
struct Request {
    id: String,
    model: Option<String>,
    requested: Option<String>,
    effort: Option<String>,
    usage: Option<Json>,
}

fn request_of(raw: &Json) -> Option<Request> {
    let text = |v: Option<&Json>| v.and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    if raw.get("message").is_some() {
        let id = text(raw.get("requestId")).or_else(|| text(raw.pointer("/message/id")))?;
        return Some(Request {
            id,
            model: text(raw.pointer("/message/model")),
            requested: text(raw.get("requestedModel")),
            effort: text(raw.get("effort")),
            usage: raw.pointer("/message/usage").cloned(),
        });
    }
    // A Gemini reply is one record, its counters under `tokens`.
    let usage = raw.get("tokens")?.clone();
    Some(Request { id: text(raw.get("id"))?, model: text(raw.get("model")), requested: None, effort: None, usage: Some(usage) })
}

/// A request's token counters in words, and any counter not named here as
/// it was recorded.
fn tokens_line(usage: &Json) -> String {
    let mut said = Vec::new();
    let mut known: Vec<&str> = Vec::new();
    let named: [(&str, &str); 10] = [
        // Claude
        ("input_tokens", "in"),
        ("cache_creation_input_tokens", "cache write"),
        ("cache_read_input_tokens", "cache read"),
        ("output_tokens", "out"),
        // Gemini
        ("input", "in"),
        ("cached", "cached"),
        ("output", "out"),
        ("thoughts", "thoughts"),
        ("tool", "tool"),
        ("total", "total"),
    ];
    for (key, label) in named {
        if let Some(n) = usage.get(key).and_then(Json::as_u64) {
            said.push(format!("{} {label}", grouped(n)));
            known.push(key);
        }
    }
    if let Some(details) = usage.get("output_tokens_details") {
        if let Some(n) = details.get("thinking_tokens").and_then(Json::as_u64) {
            said.push(format!("{} of them thinking", grouped(n)));
            if more_of(details, &["thinking_tokens"]).is_none() {
                known.push("output_tokens_details");
            }
        }
    }
    let mut line = said.join(" · ");
    if let Some(rest) = more_of(usage, &known) {
        if !line.is_empty() {
            line.push_str(" · ");
        }
        line.push_str(&rest.to_string());
    }
    line
}

// ─────────────────────────────────────────────────────────────────────
// The file
// ─────────────────────────────────────────────────────────────────────

/// The session's facts, for the top of the file.
struct Facts<'a> {
    identity: &'a str,
    source: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
}

/// The file being written: options, what the walk has seen, and the text
/// not yet sent.
struct Writer {
    format: Format,
    tools: ToolDetail,
    include: Include,
    clock: Option<Tz>,
    matching: Option<String>,
    until: DateTime<Utc>,
    /// The reader's offset when the export began: a time on another offset
    /// (across a daylight-saving change) says which.
    offset: Option<chrono::FixedOffset>,
    out: String,
    written: usize,
    /// Each tool call's name, by its id, so its result names the tool.
    calls: HashMap<String, String>,
    /// The model and effort the last assistant message named.
    model: (Option<String>, Option<String>),
    /// Where the agent last worked: directory and branch.
    place: (Option<String>, Option<String>),
    roles: BTreeMap<String, u64>,
    actions: u64,
    /// The last thing written was an action line (Markdown lists need a
    /// blank line before what follows).
    in_actions: bool,
    /// The request the last rows belong to, written when it ends.
    request: Option<Request>,
    /// What the line being written records beside its blocks.
    line: LineFacts,
    /// Transcript lines the capture kept by kind only, counted (#471).
    kinds: BTreeMap<String, u64>,
}

impl Writer {
    fn new(options: &ExportOptions, until: DateTime<Utc>) -> Self {
        Self {
            format: options.format,
            tools: options.tools,
            include: options.include,
            clock: options.clock,
            matching: options.matching.clone(),
            until,
            offset: options.clock.map(|tz| until.with_timezone(&tz).offset().fix()),
            out: String::new(),
            written: 0,
            calls: HashMap::new(),
            model: (None, None),
            place: (None, None),
            roles: BTreeMap::new(),
            actions: 0,
            in_actions: false,
            request: None,
            line: LineFacts::default(),
            kinds: BTreeMap::new(),
        }
    }

    fn pending(&self) -> usize {
        self.out.len()
    }

    fn drain(&mut self) -> Bytes {
        self.written += self.out.len();
        Bytes::from(std::mem::take(&mut self.out))
    }

    fn rows(&self) -> u64 {
        self.roles.values().sum::<u64>() + self.actions
    }

    /// A moment on the reader's clock, with its offset when that differs
    /// from the offset at export time.
    fn when(&self, t: DateTime<Utc>) -> Option<String> {
        let tz = self.clock?;
        let local = t.with_timezone(&tz);
        let mut s = local.format("%Y-%m-%d %H:%M:%S").to_string();
        if Some(local.offset().fix()) != self.offset {
            let _ = write!(s, " (UTC{})", local.format("%:z"));
        }
        Some(s)
    }

    fn header(&mut self, facts: &Facts<'_>) {
        let exported = match self.clock {
            Some(tz) => {
                let local = self.until.with_timezone(&tz);
                format!("{} ({}, UTC{})", local.format("%Y-%m-%d %H:%M:%S"), tz.name(), local.format("%:z"))
            }
            None => self.until.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        };
        let model = [facts.model, facts.effort].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let holds = match self.include {
            Include::Conversation => "the conversation: every message, oldest first",
            Include::Everything => {
                "everything: every message, oldest first, then every action the OS recorded, in an appendix"
            }
        };
        let tools = match self.tools {
            ToolDetail::Full => "tool calls and output in full".to_string(),
            ToolDetail::FirstLines => format!("tool calls and output: the first {TRIMMED_TOOL_LINES} lines of each"),
            ToolDetail::LeftOut => "tool calls and output left out, their size kept".to_string(),
        };
        let times = match self.clock {
            Some(tz) => format!(
                "on your clock ({}). A message at the moment its agent wrote it, an action at the moment SuperX captured it",
                tz.name()
            ),
            None => "not written".to_string(),
        };
        let mut lines: Vec<(&str, String)> = vec![
            ("session", facts.identity.to_string()),
            ("source", facts.source.unwrap_or("—").to_string()),
            ("model", if model.is_empty() { "—".to_string() } else { model }),
            ("exported", exported),
            ("holds", format!("{holds} · {tools}")),
            ("rows", "every one captured for the session before the export began, counted on the last line".to_string()),
            ("times", times),
        ];
        if let Some(q) = &self.matching {
            lines.push(("only rows matching", format!("“{q}”")));
        }
        lines.push((
            "each request",
            "closes with its id, model, effort and token counts; each thinking block says how long it \
             took, and each tool result how its call was allowed"
                .to_string(),
        ));
        lines.push((
            "not written",
            "the transcript's bookkeeping (ids, timestamps inside a message, the agent's own structured \
             copy of each tool result), thinking signatures, and image bytes: each image is named with \
             its type and size"
                .to_string(),
        ));
        lines.push((
            "not captured",
            "the content of the transcript lines that are not messages: the system prompt, the context \
             injected into each request (CLAUDE.md, memory, reminders), the skills and tools offered, \
             attachments, turn durations and away summaries. This version of the OS records only that \
             each happened (#471); with everything, the appendix counts them by kind"
                .to_string(),
        ));
        match self.format {
            Format::Markdown => {
                let _ = writeln!(self.out, "# Session {}\n", facts.identity);
                for (k, v) in &lines {
                    let _ = writeln!(self.out, "- **{k}:** {v}");
                }
                self.out.push_str("\n---\n\n");
            }
            Format::Text => {
                let width = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
                self.out.push_str("SuperX session export\n");
                for (k, v) in &lines {
                    let _ = writeln!(self.out, "{k:width$}  {v}");
                }
                self.out.push('\n');
            }
        }
    }

    fn message(&mut self, m: &MessageRecord) {
        *self.roles.entry(m.role.clone()).or_default() += 1;
        let raw = m.raw.clone().map(|o| Value::Object(o).into_json_value());
        let context = raw.as_ref().map(context_of).unwrap_or_default();
        let pieces = pieces_of(raw.as_ref(), &m.content, &mut self.calls);
        // A row of another request, or of none, ends the one before it.
        let request = raw.as_ref().and_then(request_of);
        if request.as_ref().map(|r| &r.id) != self.request.as_ref().map(|r| &r.id) {
            self.close_request();
        }
        self.line = raw.as_ref().map(line_facts).unwrap_or_default();

        // The heading: who, when, and the model where it changes.
        let mut heading = m.role.clone();
        if let Some(t) = self.when(m.emitted_at.unwrap_or(m.valid_from)) {
            let _ = write!(heading, " · {t}");
        }
        if m.role == "assistant" {
            let named = (context.model.clone(), context.effort.clone());
            if named.0.is_some() && named != self.model {
                for v in [&named.0, &named.1].into_iter().flatten() {
                    let _ = write!(heading, " · {}", self.code(v));
                }
                self.model = named;
            }
        }
        let place = (context.cwd.clone(), context.branch.clone());
        let moved = place.0.is_some() && place != self.place;
        if moved {
            self.place = place;
        }

        match self.format {
            Format::Markdown => {
                if self.in_actions {
                    self.out.push('\n');
                    self.in_actions = false;
                }
                let _ = writeln!(self.out, "### {heading}\n");
                if moved {
                    let note = self.place_note();
                    let _ = writeln!(self.out, "*{note}*\n");
                }
                if pieces.is_empty() {
                    self.out.push_str("*(no content captured)*\n\n");
                }
                for p in &pieces {
                    self.md_piece(p);
                }
            }
            Format::Text => {
                let _ = writeln!(self.out, "[{heading}]");
                if moved {
                    let note = self.place_note();
                    let _ = writeln!(self.out, "    {note}");
                }
                if pieces.is_empty() {
                    self.out.push_str("    (no content captured)\n");
                }
                for p in &pieces {
                    self.txt_piece(p);
                }
                self.out.push('\n');
            }
        }
        // Its newest row holds the request's counters as they ended.
        self.request = request;
    }

    /// The request the rows just written belong to, closed: its id, model,
    /// effort and token counters, once.
    fn close_request(&mut self) {
        let Some(r) = self.request.take() else { return };
        let mut line = format!("request {}", self.code(&r.id));
        if let Some(model) = &r.model {
            let _ = write!(line, " · {}", self.code(model));
        }
        if let Some(asked) = r.requested.as_ref().filter(|asked| Some(*asked) != r.model.as_ref()) {
            let _ = write!(line, " (asked for {})", self.code(asked));
        }
        if let Some(effort) = &r.effort {
            let _ = write!(line, " · effort {effort}");
        }
        if let Some(usage) = &r.usage {
            let tokens = tokens_line(usage);
            if !tokens.is_empty() {
                let _ = write!(line, " · tokens: {tokens}");
            }
        }
        match self.format {
            Format::Markdown => {
                let _ = writeln!(self.out, "*{line}*\n");
            }
            Format::Text => {
                let _ = writeln!(self.out, "    [{line}]\n");
            }
        }
    }

    /// Where the OS's action events begin: after the conversation, so it
    /// reads without them (#472).
    fn appendix(&mut self) {
        match self.format {
            Format::Markdown => self.out.push_str(
                "---\n\n## Appendix: the OS's action events\n\n\
                 Every action the OS recorded for this session, oldest first, each with its whole payload.\n\n",
            ),
            Format::Text => {
                self.out.push_str("APPENDIX: the OS's action events, oldest first, each with its whole payload\n\n");
            }
        }
    }

    /// The transcript lines this OS recorded by kind alone (#471): that they
    /// happened and when, not what they held.
    fn kinds_note(&mut self) {
        if self.kinds.is_empty() {
            return;
        }
        let total: u64 = self.kinds.values().sum();
        let list = self
            .kinds
            .iter()
            .map(|(kind, n)| format!("{} ×{}", self.code(kind), grouped(*n)))
            .collect::<Vec<_>>()
            .join(" · ");
        match self.format {
            Format::Markdown => {
                if self.in_actions {
                    self.out.push('\n');
                    self.in_actions = false;
                }
                let _ = writeln!(
                    self.out,
                    "**Transcript lines kept by kind only:** {} lines the OS saw but did not keep the content of (#471): {list}\n",
                    grouped(total)
                );
            }
            Format::Text => {
                let _ = writeln!(self.out, "\ntranscript lines kept by kind only ({}, #471): {list}\n", grouped(total));
            }
        }
    }

    fn place_note(&self) -> String {
        let mut note = format!("working in {}", self.code(self.place.0.as_deref().unwrap_or_default()));
        if let Some(branch) = &self.place.1 {
            let _ = write!(note, " · branch {}", self.code(branch));
        }
        note
    }

    fn action(&mut self, a: &TelemetryRecord) {
        self.actions += 1;
        let payload = a.payload.clone().into_json_value();
        // The capture's event for a transcript line it kept by kind only.
        if a.lifecycle_event == "transcript_event" {
            if let Some(kind) = payload.get("kind").and_then(Json::as_str) {
                *self.kinds.entry(kind.to_string()).or_default() += 1;
            }
        }
        let payload = serde_json::to_string(&payload).unwrap_or_default();
        let when = self.when(a.valid_from);
        match self.format {
            Format::Markdown => {
                let mut line = "- *action*".to_string();
                if let Some(t) = when {
                    let _ = write!(line, " · {t}");
                }
                let _ = writeln!(self.out, "{line} · {} · {}", self.code(&a.lifecycle_event), self.code(&payload));
                self.in_actions = true;
            }
            Format::Text => {
                let mut line = String::new();
                if let Some(t) = when {
                    let _ = write!(line, "[{t}] ");
                }
                let _ = writeln!(self.out, "{line}action {} {payload}", a.lifecycle_event);
            }
        }
    }

    fn footer(&mut self) {
        let messages: u64 = self.roles.values().sum();
        let by_role = self
            .roles
            .iter()
            .map(|(role, n)| format!("{} {role}", grouped(*n)))
            .collect::<Vec<_>>()
            .join(", ");
        let counted = |n: u64, what: &str| format!("{} {what}{}", grouped(n), plural(usize::try_from(n).unwrap_or(usize::MAX)));
        let mut line = format!("{} · {}", counted(self.rows(), "row"), counted(messages, "message"));
        if !by_role.is_empty() {
            let _ = write!(line, " ({by_role})");
        }
        if self.include == Include::Everything {
            let _ = write!(line, " · {}", counted(self.actions, "action"));
        }
        match self.format {
            Format::Markdown => {
                if self.in_actions {
                    self.out.push('\n');
                }
                let _ = writeln!(self.out, "---\n\n**{END_MARK}** · {line}");
            }
            Format::Text => {
                let _ = writeln!(self.out, "{END_MARK} · {line}");
            }
        }
    }

    /// Inline code that holds `s` whatever backticks it carries; plain text
    /// writes it as it is.
    fn code(&self, s: &str) -> String {
        if self.format == Format::Text {
            return s.to_string();
        }
        let ticks = "`".repeat(longest_tick_run(s) + 1);
        let pad = if s.starts_with('`') || s.ends_with('`') || s.is_empty() { " " } else { "" };
        format!("{ticks}{pad}{s}{pad}{ticks}")
    }

    /// The lines of a tool's input or output the options keep, and how
    /// many were left behind.
    fn shaped<'a>(&self, text: &'a str) -> (Vec<&'a str>, usize) {
        let lines = lines_of(text);
        match self.tools {
            ToolDetail::Full => (lines, 0),
            ToolDetail::FirstLines if lines.len() > TRIMMED_TOOL_LINES => {
                let more = lines.len() - TRIMMED_TOOL_LINES;
                (lines[..TRIMMED_TOOL_LINES].to_vec(), more)
            }
            ToolDetail::FirstLines => (lines, 0),
            ToolDetail::LeftOut => (vec![], lines.len()),
        }
    }

    fn call_label(&self, name: &str, id: Option<&str>, more: Option<&Json>) -> String {
        let mut label = self.code(name);
        if let Some(id) = id {
            let _ = write!(label, " · {}", self.code(id));
        }
        if let Some(more) = more {
            let _ = write!(label, " · {}", self.code(&more.to_string()));
        }
        label
    }

    fn result_label(&self, name: Option<&str>, id: Option<&str>, error: bool, status: Option<&str>, more: Option<&Json>) -> String {
        let mut label = String::new();
        if let Some(name) = name {
            label.push_str(&self.code(name));
        }
        if let Some(id) = id {
            if !label.is_empty() {
                label.push_str(" · ");
            }
            let _ = write!(label, "for {}", self.code(id));
        }
        if let Some(status) = status {
            let _ = write!(label, " · {status}");
        } else if error {
            label.push_str(" · error");
        }
        if let Some(more) = more {
            let _ = write!(label, " · {}", self.code(&more.to_string()));
        }
        label
    }

    fn md_piece(&mut self, p: &Piece) {
        match p {
            Piece::Text(t) => {
                self.out.push_str(t.trim_end_matches('\n'));
                self.out.push_str("\n\n");
            }
            Piece::Thinking { title, text } => {
                self.out.push_str("**thinking**");
                if let Some(title) = title {
                    let _ = write!(self.out, " · {title}");
                }
                if let Some(ms) = self.line.thought_ms {
                    let _ = write!(self.out, " · {}", took(ms));
                }
                self.out.push_str("\n\n");
                if text.is_empty() {
                    self.out.push_str("> *(no thinking text in the transcript)*\n\n");
                } else {
                    for line in lines_of(text) {
                        if line.is_empty() {
                            self.out.push_str(">\n");
                        } else {
                            let _ = writeln!(self.out, "> {line}");
                        }
                    }
                    self.out.push('\n');
                }
            }
            Piece::Call { name, id, more, input } => {
                let label = self.call_label(name, id.as_deref(), more.as_ref());
                let _ = writeln!(self.out, "**tool call** {label}\n");
                let body = pretty(input);
                self.md_tool_body(&body, "json", "input");
            }
            Piece::Result { name, id, error, status, more, parts } => {
                let mut label = self.result_label(name.as_deref(), id.as_deref(), *error, status.as_deref(), more.as_ref());
                if let Some(permission) = &self.line.permission {
                    let _ = write!(label, " · permission: {permission}");
                }
                let _ = writeln!(self.out, "**tool result** {label}\n");
                self.md_parts(parts);
            }
            Piece::Note(n) => {
                let _ = writeln!(self.out, "*{n}*\n");
            }
            Piece::Unknown { label, json } => {
                let _ = writeln!(self.out, "**{label}**\n");
                let body = pretty(json);
                let fence = fence_for(&body);
                let _ = writeln!(self.out, "{fence}json\n{body}\n{fence}\n");
            }
        }
    }

    fn md_parts(&mut self, parts: &[Part]) {
        if self.tools == ToolDetail::LeftOut {
            let lines: usize = parts.iter().map(|p| lines_of(&part_text(p)).len()).sum();
            let _ = writeln!(self.out, "*(output left out, {} line{})*\n", grouped(lines as u64), plural(lines));
            return;
        }
        if parts.is_empty() {
            self.out.push_str("*(empty)*\n\n");
        }
        for part in parts {
            match part {
                Part::Text(t) => self.md_tool_body(t, "text", "output"),
                Part::Note(n) => {
                    let _ = writeln!(self.out, "*{n}*\n");
                }
                Part::Json(j) => self.md_tool_body(&pretty(j), "json", "output"),
            }
        }
    }

    /// A tool's input or output, fenced, shaped by the options.
    fn md_tool_body(&mut self, text: &str, info: &str, what: &str) {
        let (lines, more) = self.shaped(text);
        if self.tools == ToolDetail::LeftOut {
            let _ = writeln!(self.out, "*({what} left out, {} line{})*\n", grouped(more as u64), plural(more));
            return;
        }
        if lines.is_empty() {
            let _ = writeln!(self.out, "*(empty {what})*\n");
            return;
        }
        let kept = lines.join("\n");
        let fence = fence_for(&kept);
        let _ = writeln!(self.out, "{fence}{info}\n{kept}\n{fence}");
        if more > 0 {
            let _ = writeln!(self.out, "*… {} more line{}*", grouped(more as u64), plural(more));
        }
        self.out.push('\n');
    }

    fn txt_piece(&mut self, p: &Piece) {
        match p {
            Piece::Text(t) => indent(&mut self.out, t, 4),
            Piece::Thinking { title, text } => {
                self.out.push_str("    thinking");
                if let Some(title) = title {
                    let _ = write!(self.out, " · {title}");
                }
                if let Some(ms) = self.line.thought_ms {
                    let _ = write!(self.out, " · {}", took(ms));
                }
                self.out.push_str(":\n");
                if text.is_empty() {
                    self.out.push_str("        (no thinking text in the transcript)\n");
                } else {
                    indent(&mut self.out, text, 8);
                }
            }
            Piece::Call { name, id, more, input } => {
                let mut label = format!("tool call {name}");
                if let Some(id) = id {
                    let _ = write!(label, " ({id})");
                }
                if let Some(more) = more {
                    let _ = write!(label, " {more}");
                }
                let _ = writeln!(self.out, "    {label}:");
                let body = pretty(input);
                self.txt_tool_body(&body, "input");
            }
            Piece::Result { name, id, error, status, more, parts } => {
                let mut label = "tool result".to_string();
                if let Some(name) = name {
                    let _ = write!(label, " {name}");
                }
                if let Some(id) = id {
                    let _ = write!(label, " (for {id})");
                }
                if let Some(status) = status {
                    let _ = write!(label, ", {status}");
                } else if *error {
                    label.push_str(", error");
                }
                if let Some(more) = more {
                    let _ = write!(label, " {more}");
                }
                if let Some(permission) = &self.line.permission {
                    let _ = write!(label, ", permission: {permission}");
                }
                let _ = writeln!(self.out, "    {label}:");
                if self.tools == ToolDetail::LeftOut {
                    let lines: usize = parts.iter().map(|p| lines_of(&part_text(p)).len()).sum();
                    let _ = writeln!(self.out, "        (output left out, {} line{})", grouped(lines as u64), plural(lines));
                    return;
                }
                if parts.is_empty() {
                    self.out.push_str("        (empty)\n");
                }
                for part in parts {
                    match part {
                        Part::Text(t) => self.txt_tool_body(t, "output"),
                        Part::Note(n) => {
                            let _ = writeln!(self.out, "        [{n}]");
                        }
                        Part::Json(j) => self.txt_tool_body(&pretty(j), "output"),
                    }
                }
            }
            Piece::Note(n) => {
                let _ = writeln!(self.out, "    [{n}]");
            }
            Piece::Unknown { label, json } => {
                let _ = writeln!(self.out, "    {label}:");
                indent(&mut self.out, &pretty(json), 8);
            }
        }
    }

    fn txt_tool_body(&mut self, text: &str, what: &str) {
        let (lines, more) = self.shaped(text);
        if self.tools == ToolDetail::LeftOut {
            let _ = writeln!(self.out, "        ({what} left out, {} line{})", grouped(more as u64), plural(more));
            return;
        }
        if lines.is_empty() {
            let _ = writeln!(self.out, "        (empty {what})");
            return;
        }
        indent(&mut self.out, &lines.join("\n"), 8);
        if more > 0 {
            let _ = writeln!(self.out, "        … {} more line{}", grouped(more as u64), plural(more));
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The text a part counts as, for a left-out line count.
fn part_text(p: &Part) -> String {
    match p {
        Part::Text(t) => t.clone(),
        Part::Note(n) => n.clone(),
        Part::Json(j) => pretty(j),
    }
}

fn pretty(j: &Json) -> String {
    serde_json::to_string_pretty(j).unwrap_or_else(|_| j.to_string())
}

fn longest_tick_run(s: &str) -> usize {
    let (mut best, mut run) = (0, 0);
    for c in s.chars() {
        if c == '`' {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// A fence longer than any run of backticks in what it wraps.
fn fence_for(s: &str) -> String {
    "`".repeat((longest_tick_run(s) + 1).max(3))
}

/// The lines of `text`, split on `\n` alone: a `\r` before it is the
/// text's own byte and stays (`str::lines` would drop it). One trailing
/// newline ends the last line rather than adding an empty one.
fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![];
    }
    text.strip_suffix('\n').unwrap_or(text).split('\n').collect()
}

/// Every line of `text`, indented, blank lines kept blank.
fn indent(out: &mut String, text: &str, by: usize) {
    for line in lines_of(text.trim_end_matches('\n')) {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{:by$}{line}", "");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pieces(raw: Json, content: &str) -> Vec<Piece> {
        pieces_of(Some(&raw), content, &mut HashMap::new())
    }

    fn options(format: Format, tools: ToolDetail) -> ExportOptions {
        ExportOptions { include: Include::Everything, tools, format, clock: None, matching: None }
    }

    fn message(role: &str, raw: Option<Json>, content: &str) -> MessageRecord {
        let raw = raw.map(|j| superx_kernel::message::json_to_object(&j));
        MessageRecord {
            id: RecordId::new("message", superx_kernel::types::Uuid::new_v7()),
            session: RecordId::new("entity", "s"),
            agent: RecordId::new("entity", "a"),
            role: role.to_string(),
            content: content.to_string(),
            raw,
            seq: None,
            emitted_at: None,
            valid_from: "2026-10-09T13:15:42Z".parse().expect("ts"),
        }
    }

    fn written(options: &ExportOptions, rows: &[MessageRecord]) -> String {
        let mut w = Writer::new(options, "2026-10-20T13:53:25Z".parse().expect("ts"));
        for m in rows {
            w.message(m);
        }
        w.close_request();
        w.footer();
        w.out
    }

    /// One row of a Claude Code reply, as the transcript writes it: a block,
    /// and the request's counters as they stood.
    fn reply_row(request: &str, block: Json, output_tokens: u64) -> MessageRecord {
        message(
            "assistant",
            Some(json!({
                "type": "assistant", "requestId": request, "effort": "max", "requestedModel": "claude-opus-5-5",
                "message": {
                    "id": format!("msg_{request}"), "model": "claude-sonnet-5-5", "content": [block],
                    "usage": {
                        "input_tokens": 2, "cache_creation_input_tokens": 8_843, "cache_read_input_tokens": 42_806,
                        "output_tokens": output_tokens, "output_tokens_details": { "thinking_tokens": 7 },
                        "service_tier": "standard"
                    }
                }
            })),
            "",
        )
    }

    #[test]
    fn a_request_is_written_once_with_the_counters_its_last_row_holds() {
        let rows = [
            reply_row("req_1", json!({ "type": "thinking", "thinking": "plan", "signature": "s" }), 10),
            reply_row("req_1", json!({ "type": "text", "text": "Running the tests." }), 50),
            reply_row("req_1", json!({ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {} }), 120),
            message(
                "tool",
                Some(json!({ "type": "user", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "ok" }
                ] } })),
                "",
            ),
            reply_row("req_2", json!({ "type": "text", "text": "Done." }), 3),
        ];
        let out = written(&options(Format::Markdown, ToolDetail::Full), &rows);
        assert_eq!(out.matches("*request ").count(), 2, "one line per request: {out}");
        let first = out.find("*request `req_1`").expect("the first request is closed");
        assert!(out[..first].contains("Running the tests.") && out[first..].contains("**tool result**"), "after its rows: {out}");
        assert!(
            out.contains(
                "*request `req_1` · `claude-sonnet-5-5` (asked for `claude-opus-5-5`) · effort max · tokens: \
                 2 in · 8,843 cache write · 42,806 cache read · 120 out · 7 of them thinking · {\"service_tier\":\"standard\"}*"
            ),
            "the last row's counters, every one of them: {out}"
        );
        assert!(out.contains("*request `req_2`") && out.contains("3 out"), "the last request closes at the end: {out}");
    }

    #[test]
    fn a_thinking_block_says_how_long_it_took_even_without_its_text() {
        let mut row = reply_row("req_1", json!({ "type": "thinking", "thinking": "", "signature": "s" }), 1);
        let raw = json!({ "type": "assistant", "thinkingDurationMs": 270_000, "requestId": "req_1",
            "message": { "id": "m", "content": [{ "type": "thinking", "thinking": "", "signature": "s" }] } });
        row.raw = Some(superx_kernel::message::json_to_object(&raw));
        let md = written(&options(Format::Markdown, ToolDetail::Full), std::slice::from_ref(&row));
        assert!(md.contains("**thinking** · 4 min 30 s\n\n> *(no thinking text in the transcript)*"), "{md}");
        let txt = written(&options(Format::Text, ToolDetail::Full), &[row]);
        assert!(txt.contains("    thinking · 4 min 30 s:\n        (no thinking text in the transcript)"), "{txt}");
        assert_eq!((took(850), took(5_700), took(270_000)), ("850 ms".into(), "5.7 s".into(), "4 min 30 s".into()));
    }

    #[test]
    fn a_tool_result_says_how_its_call_was_allowed() {
        let result = |extra: Json| {
            let mut raw = json!({ "type": "user", "message": { "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": "out", "is_error": true }
            ] } });
            for (k, v) in extra.as_object().expect("object") {
                raw[k] = v.clone();
            }
            message("tool", Some(raw), "")
        };
        let allowed = result(json!({
            "permissionDecision": { "decision": "accept", "source": "config", "reasonType": "classifier" },
            "serverClassifierContext": { "request": "3b48cd21", "context": { "git_state": {} } }
        }));
        let denied = result(json!({ "permissionDecision": { "decision": "deny" }, "toolDenialKind": "classifier" }));
        let md = written(&options(Format::Markdown, ToolDetail::Full), &[allowed, denied.clone()]);
        // The decision first, then what was said about it, in key order.
        assert!(
            md.contains("· error · permission: accept · reason type classifier · source config · classifier request 3b48cd21\n"),
            "{md}"
        );
        assert!(md.contains("permission: deny · denied as classifier"), "{md}");
        let txt = written(&options(Format::Text, ToolDetail::Full), &[denied]);
        assert!(txt.contains(", error, permission: deny · denied as classifier:"), "{txt}");
    }

    #[test]
    fn a_gemini_reply_closes_with_its_counters() {
        let reply = message(
            "assistant",
            Some(json!({ "id": "g1", "type": "gemini", "model": "gemini-3-pro", "content": "Hello",
                "tokens": { "input": 1_200, "output": 40, "cached": 1_000, "thoughts": 25, "tool": 0, "total": 1_265 } })),
            "Hello",
        );
        let out = written(&options(Format::Markdown, ToolDetail::Full), &[reply]);
        assert!(
            out.contains("*request `g1` · `gemini-3-pro` · tokens: 1,200 in · 1,000 cached · 40 out · 25 thoughts · 0 tool · 1,265 total*"),
            "{out}"
        );
    }

    #[test]
    fn the_lines_kept_by_kind_only_are_counted_after_the_actions() {
        let mut w = Writer::new(&options(Format::Markdown, ToolDetail::Full), Utc::now());
        w.appendix();
        for kind in ["attachment", "api-request", "attachment"] {
            w.action(&TelemetryRecord {
                id: RecordId::new("telemetry_stream", superx_kernel::types::Uuid::new_v7()),
                lifecycle_event: "transcript_event".into(),
                payload: <Json as superx_kernel::types::SurrealValue>::into_value(json!({ "kind": kind, "session": "s" })),
                subject: None,
                agent: None,
                valid_from: Utc::now(),
            });
        }
        w.kinds_note();
        assert!(w.out.starts_with("---\n\n## Appendix: the OS's action events\n\n"), "{}", w.out);
        assert!(
            w.out.contains("**Transcript lines kept by kind only:** 3 lines the OS saw but did not keep the content of (#471): `api-request` ×1 · `attachment` ×2"),
            "{}",
            w.out
        );
    }

    #[test]
    fn every_claude_block_is_a_piece_in_order() {
        let input = json!({ "command": "cargo test --workspace", "description": "Run the tests" });
        let got = pieces(
            json!({ "message": { "content": [
                { "type": "thinking", "thinking": "Let me run them.", "signature": "opaque" },
                { "type": "text", "text": "Running the tests." },
                { "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": input, "caller": { "type": "direct" } },
            ] } }),
            "Running the tests.",
        );
        assert_eq!(
            got,
            vec![
                Piece::Thinking { title: None, text: "Let me run them.".into() },
                Piece::Text("Running the tests.".into()),
                Piece::Call {
                    name: "Bash".into(),
                    id: Some("toolu_1".into()),
                    more: Some(json!({ "caller": { "type": "direct" } })),
                    input,
                },
            ],
            "thinking and the call written alongside the text, the signature nowhere"
        );
    }

    #[test]
    fn a_tool_result_names_its_call_and_keeps_every_part() {
        let mut calls = HashMap::new();
        let call = json!({ "message": { "content": [{ "type": "tool_use", "id": "toolu_9", "name": "Read", "input": {} }] } });
        assert_eq!(pieces_of(Some(&call), "", &mut calls).len(), 1, "the call is one piece, and its name is kept");
        let result = json!({ "message": { "content": [{
            "type": "tool_result", "tool_use_id": "toolu_9", "is_error": true,
            "content": [
                { "type": "text", "text": "line one\nline two" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGVsbG8=" } },
                { "type": "tool_reference", "tool_name": "TaskCreate" },
                { "type": "brand_new", "x": 1 },
            ],
        }] } });
        let got = pieces_of(Some(&result), "", &mut calls);
        assert_eq!(
            got,
            vec![Piece::Result {
                name: Some("Read".into()),
                id: Some("toolu_9".into()),
                error: true,
                status: None,
                more: None,
                parts: vec![
                    Part::Text("line one\nline two".into()),
                    Part::Note("image · image/png · 5 bytes".into()),
                    Part::Note("tool reference · TaskCreate".into()),
                    Part::Json(json!({ "type": "brand_new", "x": 1 })),
                ],
            }]
        );
    }

    #[test]
    fn a_model_fallback_and_an_unknown_block_are_written_not_dropped() {
        let got = pieces(
            json!({ "message": { "content": [
                { "type": "fallback", "from": { "model": "claude-fable-5" }, "to": { "model": "claude-opus-5" } },
                { "type": "server_tool_use", "id": "s1", "input": { "q": 1 } },
            ] } }),
            "",
        );
        assert_eq!(got[0], Piece::Note("model fallback · claude-fable-5 → claude-opus-5".into()));
        assert_eq!(
            got[1],
            Piece::Unknown { label: "server_tool_use".into(), json: json!({ "type": "server_tool_use", "id": "s1", "input": { "q": 1 } }) }
        );
    }

    #[test]
    fn a_pasted_screenshot_is_named_beside_the_prompt() {
        let got = pieces(
            json!({ "message": { "content": [
                { "type": "text", "text": "what is wrong here?" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "AAAA" } },
            ] } }),
            "what is wrong here?",
        );
        assert_eq!(got[1], Piece::Note("image · image/jpeg · 3 bytes".into()));
    }

    #[test]
    fn every_gemini_shape_is_a_piece() {
        let mut calls = HashMap::new();
        let got = pieces_of(
            Some(&json!({
                "type": "gemini", "model": "gemini-3-pro",
                "thoughts": [{ "subject": "Planning", "description": "Read the file first.", "timestamp": "t" }],
                "content": "Reading it.",
                "toolCalls": [{
                    "id": "c1", "name": "run_shell_command", "displayName": "Shell", "status": "success",
                    "args": { "command": "ls" },
                    "result": [{ "functionResponse": { "id": "c1", "name": "run_shell_command", "response": { "output": "a\nb" } } }],
                }],
            })),
            "Reading it.",
            &mut calls,
        );
        assert_eq!(got[0], Piece::Thinking { title: Some("Planning".into()), text: "Read the file first.".into() });
        assert_eq!(got[1], Piece::Text("Reading it.".into()));
        assert_eq!(
            got[2],
            Piece::Call {
                name: "run_shell_command".into(),
                id: Some("c1".into()),
                more: Some(json!({ "displayName": "Shell" })),
                input: json!({ "command": "ls" }),
            }
        );
        assert_eq!(
            got[3],
            Piece::Result {
                name: Some("run_shell_command".into()),
                id: Some("c1".into()),
                error: false,
                status: Some("success".into()),
                more: None,
                parts: vec![Part::Text("a\nb".into())],
            }
        );
        // A tool's output sent back as a user turn, with its signature-free call.
        let back = pieces_of(
            Some(&json!({ "type": "user", "content": [
                { "functionCall": { "name": "replace", "args": { "file": "x" } }, "thoughtSignature": "opaque" },
                { "functionResponse": { "id": "r1", "name": "replace", "response": { "output": "done" } } },
            ] })),
            "",
            &mut calls,
        );
        assert!(matches!(&back[0], Piece::Call { name, more: None, .. } if name == "replace"), "{back:?}");
        assert!(matches!(&back[1], Piece::Result { parts, .. } if parts == &vec![Part::Text("done".into())]), "{back:?}");
    }

    #[test]
    fn a_row_with_no_known_shape_keeps_its_text_and_its_event() {
        let got = pieces(json!({ "kind": "desktop", "body": 1 }), "said this");
        assert_eq!(got[0], Piece::Text("said this".into()));
        assert_eq!(got[1], Piece::Unknown { label: "captured event".into(), json: json!({ "kind": "desktop", "body": 1 }) });
        assert_eq!(pieces_of(None, "plain", &mut HashMap::new()), vec![Piece::Text("plain".into())]);
    }

    #[test]
    fn full_writes_the_whole_input_and_output_and_fences_hold_backticks() {
        let long = "x".repeat(37_647);
        let rows = [
            message("assistant", Some(json!({ "message": { "model": "claude-opus-5-5", "content": [
                { "type": "tool_use", "id": "toolu_w", "name": "Write", "input": { "file_path": "/a.rs", "content": long } },
            ] } })), ""),
            message("tool", Some(json!({ "message": { "content": [
                { "type": "tool_result", "tool_use_id": "toolu_w", "content": "has ``` inside" },
            ] } })), "has ``` inside"),
        ];
        let md = written(&options(Format::Markdown, ToolDetail::Full), &rows);
        assert!(md.contains(&long), "the whole 37,647-char write is in the file");
        assert!(md.contains("### assistant · `claude-opus-5-5`"), "{md}");
        assert!(md.contains("**tool result** `Write` · for `toolu_w`"), "the result names its call: {md}");
        assert!(md.contains("````text\nhas ``` inside\n````"), "the fence outruns the backticks: {md}");
        assert!(md.ends_with("**end of export** · 2 rows · 2 messages (1 assistant, 1 tool) · 0 actions\n"), "{md}");
    }

    #[test]
    fn a_carriage_return_is_the_outputs_own_byte() {
        let rows = [message("tool", Some(json!({ "message": { "content": [
            { "type": "tool_result", "tool_use_id": "t", "content": "one\r\ntwo\r\n" },
        ] } })), "one\r\ntwo\r\n")];
        let md = written(&options(Format::Markdown, ToolDetail::Full), &rows);
        assert!(md.contains("```text\none\r\ntwo\r\n```"), "{md:?}");
    }

    #[test]
    fn first_lines_and_left_out_shape_calls_and_output_alike() {
        let output = (1..=25).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let rows = [message("tool", Some(json!({ "message": { "content": [
            { "type": "tool_result", "tool_use_id": "t", "content": output },
        ] } })), &output)];
        let trimmed = written(&options(Format::Markdown, ToolDetail::FirstLines), &rows);
        assert!(trimmed.contains("line 20\n```") && !trimmed.contains("line 21"), "{trimmed}");
        assert!(trimmed.contains("*… 5 more lines*"), "{trimmed}");
        let out = written(&options(Format::Text, ToolDetail::LeftOut), &rows);
        assert!(out.contains("(output left out, 25 lines)") && !out.contains("line 1\n"), "{out}");
    }

    #[test]
    fn plain_text_carries_no_markdown() {
        let rows = [message("assistant", Some(json!({ "cwd": "/repo", "gitBranch": "main", "message": {
            "model": "claude-opus-5-5", "content": [{ "type": "text", "text": "done" }],
        } })), "done")];
        let out = written(&options(Format::Text, ToolDetail::Full), &rows);
        assert!(out.contains("[assistant · claude-opus-5-5]\n    working in /repo · branch main\n"), "{out}");
        assert!(!out.contains('`'), "{out}");
    }

    #[test]
    fn an_action_is_written_whole() {
        let path = format!("/Users/someone/.claude/projects/{}/session.jsonl", "d".repeat(240));
        let mut w = Writer::new(&options(Format::Markdown, ToolDetail::Full), Utc::now());
        let payload = json!({ "file": path, "kind": "attachment", "session": "s" });
        w.action(&TelemetryRecord {
            id: RecordId::new("telemetry_stream", superx_kernel::types::Uuid::new_v7()),
            lifecycle_event: "transcript_event".into(),
            payload: <Json as superx_kernel::types::SurrealValue>::into_value(payload),
            subject: None,
            agent: None,
            valid_from: Utc::now(),
        });
        assert!(w.out.contains(&path), "no value is cut: {}", w.out);
        assert!(w.out.starts_with("- *action* · `transcript_event` · `{"), "{}", w.out);
    }

    #[test]
    fn times_are_on_the_readers_clock_across_a_daylight_saving_change() {
        let mut o = options(Format::Text, ToolDetail::Full);
        o.clock = Some("America/New_York".parse().expect("zone"));
        // Exported in summer (UTC-4); one row from before the March change (UTC-5).
        let w = Writer::new(&o, "2026-07-01T12:00:00Z".parse().expect("ts"));
        assert_eq!(w.when("2026-07-01T12:00:00Z".parse().expect("ts")).as_deref(), Some("2026-07-01 08:00:00"));
        assert_eq!(
            w.when("2026-03-08T06:59:59Z".parse().expect("ts")).as_deref(),
            Some("2026-03-08 01:59:59 (UTC-05:00)"),
            "the winter row says its own offset"
        );
        assert!(Writer::new(&options(Format::Text, ToolDetail::Full), Utc::now()).when(Utc::now()).is_none());
    }

    #[test]
    fn the_panels_choices_parse_and_nothing_else_does() {
        let o = ExportOptions::parse(Some("everything"), Some("trimmed"), Some("md"), Some("Europe/London"), Some("  bash "))
            .expect("valid");
        assert_eq!((o.include, o.tools, o.format), (Include::Everything, ToolDetail::FirstLines, Format::Markdown));
        assert_eq!(o.clock.map(|t| t.name()), Some("Europe/London"));
        assert_eq!(o.matching.as_deref(), Some("bash"));
        assert_eq!(
            ExportOptions::parse(Some("all"), Some("full"), Some("md"), None, None).err().as_deref(),
            Some("include must be conversation or everything, not “all”"),
            "the refusal reads as words, not as a Rust value"
        );
        assert!(ExportOptions::parse(Some("everything"), Some("full"), Some("pdf"), None, None).is_err());
        assert!(ExportOptions::parse(Some("everything"), Some("full"), Some("md"), Some("Mars/Base"), None).is_err());
        assert!(ExportOptions::parse(None, Some("full"), Some("md"), None, None).is_err(), "nothing is assumed");
    }

    #[test]
    fn grouped_counts_read_like_the_page() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(27_529), "27,529");
        assert_eq!(grouped(2_599_464), "2,599,464");
    }
}
