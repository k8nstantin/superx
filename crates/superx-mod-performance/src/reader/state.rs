//! What the reader has written, kept while it runs, and the facts of each
//! line turned into rows exactly once.
//!
//! A session is looked up in the database the first time a line of it is
//! read; from then on what it holds is kept here as rows are added. A line
//! read again (each pass reads the minute before the last one stopped, and
//! a pass that did not finish is read again) finds what it wrote and
//! writes nothing. A reply read again from the middle is first rebuilt
//! from all of its lines, so its counts are never added twice. Whatever
//! goes wrong, the caller drops this state and the next pass rebuilds it
//! from the database.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Uuid};
use superx_kernel::{Db, Kernel, Result};

use crate::reader::commands;
use crate::reader::facts::{Call, Fact, GitEvent, Outcome, Prompt, Read, ReplyLine, ToolResult};
use crate::reader::formats;
use crate::reader::kernel::{self as kernel_lines, Cursor, Line, Names};
use crate::reader::store::{self, Batch, Row, State as Counted};

/// Lines read per page when a reply is read again.
const REPLY_PAGE: u32 = 200; // skill-allow: §9-const — read-path bound, not a policy tunable

/// What the reader has written, by session.
#[derive(Debug, Default)]
pub struct State {
    /// Every session row the database holds, by the kernel's session.
    rows: Option<HashMap<Uuid, RecordId>>,
    /// The sessions read since this state began.
    sessions: HashMap<Uuid, Session>,
}

#[derive(Debug)]
struct Session {
    row: RecordId,
    kernel: RecordId,
    agent: String,
    place: Option<(String, Option<String>, DateTime<Utc>)>,
    replies: HashMap<String, Reply>,
    /// The reply last begun: the one an interrupt stops.
    newest: Option<String>,
    calls: HashMap<String, CallState>,
    lines: HashSet<Uuid>,
    /// The last line read, by when it was captured.
    last: Uuid,
}

#[derive(Debug)]
struct Reply {
    row: Option<RecordId>,
    first: (Uuid, DateTime<Utc>),
    /// What the database holds of it has been read.
    held: bool,
    lines: HashSet<Uuid>,
    text: HashMap<Uuid, i64>,
    /// The blocks of the reply before each of its lines.
    offsets: HashMap<Uuid, i64>,
    total: i64,
    blocks: i64,
    counts: Option<crate::reader::facts::Counts>,
    newest: Option<Counted>,
    thinking: HashSet<i64>,
    calls: Vec<String>,
}

impl Reply {
    fn new(row: Option<RecordId>, first: (Uuid, DateTime<Utc>), held: bool) -> Self {
        Self {
            row,
            first,
            held,
            lines: HashSet::new(),
            text: HashMap::new(),
            offsets: HashMap::new(),
            total: 0,
            blocks: 0,
            counts: None,
            newest: None,
            thinking: HashSet::new(),
            calls: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct CallState {
    row: RecordId,
    tool: String,
    command: Option<String>,
    dir: Option<String>,
    has_result: bool,
    edits: HashSet<String>,
    git: bool,
    test: bool,
    build: bool,
}

/// What reading one line came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Its facts are written, or were already.
    Read,
    /// It belongs to no session the kernel names, or is in a shape the
    /// reader does not know: counted as not read.
    Unread,
}

/// The kernel and the module's database, for reading a session or a reply
/// again.
pub struct Sources<'a> {
    pub kernel: &'a Kernel,
    pub db: &'a Db,
    pub names: &'a mut Names,
}

impl State {
    /// Forget the sessions not read since `before`: what they hold is read
    /// from the database again if a line of theirs comes.
    pub fn forget_before(&mut self, before: Uuid) {
        self.sessions.retain(|_, s| s.last >= before);
    }

    /// Turn one line's facts into rows.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when a database cannot answer.
    pub async fn apply(&mut self, src: &mut Sources<'_>, line: &Line, read: Read, batch: &mut Batch) -> Result<Applied> {
        let Some(key) = line.session.as_ref().and_then(kernel_lines::record_uuid_of) else {
            return Ok(Applied::Unread);
        };
        if line.agent.is_none() {
            return Ok(Applied::Unread);
        }
        if !self.sessions.contains_key(&key) {
            let session = self.open(src, line, &read, batch).await?;
            self.sessions.insert(key, session);
        }
        let Some(session) = self.sessions.get_mut(&key) else {
            return Ok(Applied::Unread);
        };
        session.last = session.last.max(line.id);
        if let Some(place) = &read.place {
            let branch = if place.says_branch {
                place.branch.clone()
            } else {
                session.place.as_ref().and_then(|(_, branch, _)| branch.clone())
            };
            let newer = session.place.as_ref().is_none_or(|(cwd, held, when)| {
                (cwd != &place.cwd || held != &branch) && line.at >= *when
            });
            if newer {
                batch.create(
                    "session_place",
                    Row::new()
                        .link("session", &session.row)
                        .text("cwd", place.cwd.clone())
                        .some_text("branch", branch.clone())
                        .time("at", line.at),
                );
                session.place = Some((place.cwd.clone(), branch, line.at));
            }
        }
        let known = read.known;
        for fact in read.facts {
            match fact {
                Fact::Prompt(p) => session.prompt(line, &p, batch),
                Fact::Reply(r) => session.reply(src, line, *r, batch).await?,
                Fact::Result(r) => session.result(line.id, &r, batch),
                Fact::Turn(t) => {
                    if session.lines.insert(line.id) {
                        batch.create(
                            "turn",
                            session.keyed(line).int("ms", t.ms).int("lines", t.lines),
                        );
                    }
                }
                Fact::Compaction(c) => {
                    if session.lines.insert(line.id) {
                        batch.create(
                            "compaction",
                            session
                                .keyed(line)
                                .text("trigger", c.trigger)
                                .int("tokens_before", c.tokens_before)
                                .some_int("tokens_after", c.tokens_after)
                                .some_int("tokens_dropped", c.tokens_dropped)
                                .some_int("ms", c.ms),
                        );
                    }
                }
                Fact::Away(a) => {
                    if session.lines.insert(line.id) {
                        batch.create("away", session.keyed(line).text("text", a.text));
                    }
                }
            }
        }
        Ok(if known { Applied::Read } else { Applied::Unread })
    }

    /// The session of `line`, first read since this state began: what the
    /// database holds of it, or a new row.
    async fn open(&mut self, src: &mut Sources<'_>, line: &Line, read: &Read, batch: &mut Batch) -> Result<Session> {
        let (Some(kernel_session), Some(agent)) = (line.session.as_ref(), line.agent.as_deref()) else {
            return Err(superx_kernel::KernelError::Config("a line with no session or agent".to_string()));
        };
        let Some(key) = kernel_lines::record_uuid_of(kernel_session) else {
            return Err(superx_kernel::KernelError::Config("a session whose id is not a UUID".to_string()));
        };
        if self.rows.is_none() {
            self.rows = Some(store::sessions(src.db).await?.into_iter().collect());
        }
        let rows = self.rows.get_or_insert_with(HashMap::new);
        let mut session = Session {
            row: RecordId::new("session", Uuid::new_v7()),
            kernel: kernel_session.clone(),
            agent: agent.to_string(),
            place: None,
            replies: HashMap::new(),
            newest: None,
            calls: HashMap::new(),
            lines: HashSet::new(),
            last: line.id,
        };
        let Some(row) = rows.get(&key).cloned() else {
            let agent_id = src.names.session_of(src.kernel, kernel_session).await?.map(|s| s.key);
            session.row = batch.create(
                "session",
                Row::new()
                    .uuid("kernel_session", key)
                    .text("agent", agent)
                    .some_text("agent_id", agent_id)
                    .some_text("entrypoint", read.header.entrypoint.clone())
                    .some_text("client_version", read.header.client_version.clone())
                    .time("first_at", line.at),
            );
            rows.insert(key, session.row.clone());
            return Ok(session);
        };
        session.row = row;
        let held = store::session_holds(src.db, &session.row).await?;
        session.place = held.place;
        session.lines = held.lines;
        // By the reply row's UUID: a map keyed by a record id is one clippy
        // refuses (`mutable_key_type`).
        let mut by_row: HashMap<Uuid, String> = HashMap::new();
        let mut newest: Option<(DateTime<Utc>, String)> = None;
        for r in held.replies {
            if let Some(u) = kernel_lines::record_uuid_of(&r.id) {
                by_row.insert(u, r.agent_id.clone());
            }
            if newest.as_ref().is_none_or(|(at, _)| r.at > *at) {
                newest = Some((r.at, r.agent_id.clone()));
            }
            session.replies.insert(r.agent_id, Reply::new(Some(r.id), (r.first, r.at), false));
        }
        session.newest = newest.map(|(_, id)| id);
        for c in held.calls {
            let reply_key = kernel_lines::record_uuid_of(&c.reply);
            if let Some(reply) = reply_key.and_then(|k| by_row.get(&k)).and_then(|a| session.replies.get_mut(a)) {
                reply.calls.push(c.call_id.clone());
            }
            let call_key = kernel_lines::record_uuid_of(&c.id);
            let edits = held
                .edits
                .iter()
                .filter(|(call, _)| Some(*call) == call_key)
                .map(|(_, path)| path.clone())
                .collect();
            let held_by = |set: &std::collections::HashSet<Uuid>| call_key.is_some_and(|k| set.contains(&k));
            session.calls.insert(
                c.call_id,
                CallState {
                    has_result: held_by(&held.results),
                    git: held_by(&held.git),
                    test: held_by(&held.tests),
                    build: held_by(&held.builds),
                    row: c.id,
                    tool: c.tool,
                    command: c.command,
                    dir: session.place.as_ref().map(|p| p.0.clone()),
                    edits,
                },
            );
        }
        Ok(session)
    }
}

impl Session {
    /// A row keyed by the line it was read from.
    fn keyed(&self, line: &Line) -> Row {
        Row::new().link("session", &self.row).uuid("kernel_message", line.id).time("at", line.at)
    }

    fn prompt(&mut self, line: &Line, p: &Prompt, batch: &mut Batch) {
        if !self.lines.insert(line.id) {
            return;
        }
        batch.create(
            "prompt",
            self.keyed(line)
                .text("kind", p.kind.as_str())
                .text("text", p.text.clone())
                .int("chars", crate::reader::value::chars(&p.text))
                .int("images", p.images),
        );
        if p.stops_tools {
            let open: Vec<String> = self
                .newest
                .as_ref()
                .and_then(|n| self.replies.get(n))
                .map(|r| r.calls.clone())
                .unwrap_or_default();
            for call_id in open {
                if let Some(call) = self.calls.get_mut(&call_id) {
                    if !call.has_result {
                        call.has_result = true;
                        batch.create(
                            "tool_result",
                            Row::new()
                                .link("tool_call", &call.row)
                                .uuid("kernel_message", line.id)
                                .time("at", line.at)
                                .text("outcome", Outcome::Interrupted.as_str())
                                .int("bytes", 0),
                        );
                    }
                }
            }
        }
    }

    async fn reply(&mut self, src: &mut Sources<'_>, line: &Line, r: ReplyLine, batch: &mut Batch) -> Result<()> {
        if !self.replies.contains_key(&r.agent_id) {
            self.replies.insert(r.agent_id.clone(), Reply::new(None, (line.id, line.at), true));
        }
        let created = self.replies.get(&r.agent_id).is_some_and(|reply| reply.row.is_some());
        if !created {
            // A reply is written once its model is named: at once for
            // Claude Code, at a later line of a Gemini reply.
            let Some(model) = r.model.clone() else { return Ok(()) };
            let first = self.replies.get(&r.agent_id).map_or((line.id, line.at), |reply| reply.first);
            let row = batch.create(
                "reply",
                Row::new()
                    .link("session", &self.row)
                    .text("agent_id", r.agent_id.clone())
                    .some_text("request_id", r.request_id.clone())
                    .uuid("kernel_message", first.0)
                    .time("at", first.1)
                    .text("model", model)
                    .some_text("requested_model", r.requested_model.clone())
                    .some_text("advisor_model", r.advisor_model.clone())
                    .some_text("effort", r.effort.clone())
                    .some_text("turn_effort", r.turn_effort.clone())
                    .some_text("stop_reason", r.stop_reason.clone())
                    .flag("subagent", r.subagent),
            );
            if let Some(reply) = self.replies.get_mut(&r.agent_id) {
                reply.row = Some(row);
            }
            self.newest = Some(r.agent_id.clone());
        }
        let held = self.replies.get(&r.agent_id).is_some_and(|reply| reply.held);
        if !held {
            self.hold(src, &r.agent_id, r.cumulative).await?;
        }
        self.reply_line(line, &r, batch);
        Ok(())
    }

    /// What the database holds of a reply written before this state began,
    /// and, for a reply that adds a block per line, every line of it read
    /// again, so what this line adds is counted once.
    async fn hold(&mut self, src: &mut Sources<'_>, agent_id: &str, cumulative: bool) -> Result<()> {
        let Some(reply) = self.replies.get_mut(agent_id) else { return Ok(()) };
        reply.held = true;
        let Some(row) = reply.row.clone() else { return Ok(()) };
        let holds = store::reply_holds(src.db, &row).await?;
        reply.newest = holds.newest;
        reply.thinking = holds.thinking;
        if cumulative {
            return Ok(());
        }
        let mut cursor = Cursor::At(reply.first.0);
        let mut again: Vec<(Line, ReplyLine)> = Vec::new();
        'pages: loop {
            let page = kernel_lines::session_messages(src.kernel, src.names, &self.kernel, cursor, REPLY_PAGE).await?;
            for l in &page.lines {
                let read = formats::read(&self.agent, &l.raw, l.at);
                let mine = read.facts.into_iter().find_map(|f| match f {
                    Fact::Reply(r) if r.agent_id == agent_id => Some(*r),
                    _ => None,
                });
                match mine {
                    Some(r) => again.push((l.clone(), r)),
                    // A reply's lines follow one another; the first line of
                    // the session that is not one of them ends it.
                    None => break 'pages,
                }
            }
            match page.last {
                Some(last) if page.scanned == REPLY_PAGE as usize => cursor = Cursor::After(last),
                _ => break,
            }
        }
        // Lines read again are counted, not written: what they hold was
        // written when they were first read, and what is missing is
        // written when the pass reaches them.
        let Some(reply) = self.replies.get_mut(agent_id) else { return Ok(()) };
        for (l, r) in again {
            if reply.lines.insert(l.id) {
                if let crate::reader::facts::Text::Added(n) = r.text {
                    reply.text.insert(l.id, n);
                }
                reply.offsets.insert(l.id, reply.blocks);
                reply.blocks += r.blocks;
                if r.counts.is_some() {
                    reply.counts = r.counts;
                }
            }
        }
        Ok(())
    }

    /// One line of a reply: its thinking, its calls, the results it
    /// carries, and the reply's counts when they changed.
    fn reply_line(&mut self, line: &Line, r: &ReplyLine, batch: &mut Batch) {
        let cwd = self.place.as_ref().map(|p| p.0.clone());
        let Some(reply) = self.replies.get_mut(&r.agent_id) else { return };
        let Some(row) = reply.row.clone() else { return };
        let first_time = reply.lines.insert(line.id);
        if first_time && !r.cumulative {
            if let crate::reader::facts::Text::Added(n) = r.text {
                reply.text.insert(line.id, n);
            }
            reply.offsets.insert(line.id, reply.blocks);
            reply.blocks += r.blocks;
        }
        let blocks_before = reply.offsets.get(&line.id).copied().unwrap_or(reply.blocks);
        for t in &r.thinking {
            let position = t.position.unwrap_or(blocks_before + t.on_line);
            if reply.thinking.insert(position) {
                batch.create(
                    "thinking",
                    Row::new()
                        .link("reply", &row)
                        .int("position", position)
                        .uuid("kernel_message", line.id)
                        .time("at", t.at)
                        .some_int("ms", t.ms)
                        .int("chars", t.chars)
                        .some_text("display", t.display.clone()),
                );
            }
        }
        let mut new_calls = Vec::new();
        for c in &r.calls {
            if !self.calls.contains_key(&c.call_id) {
                new_calls.push(c);
            }
        }
        for c in new_calls {
            let call_row = create_call(batch, &row, line.id, c);
            reply.calls.push(c.call_id.clone());
            self.calls.insert(
                c.call_id.clone(),
                CallState {
                    row: call_row,
                    tool: c.tool.clone(),
                    command: c.command.clone(),
                    dir: c.dir.clone().or_else(|| cwd.clone()),
                    has_result: false,
                    edits: HashSet::new(),
                    git: false,
                    test: false,
                    build: false,
                },
            );
        }
        if let Some(c) = &r.counts {
            if first_time || r.cumulative {
                reply.counts = Some(c.clone());
            }
        }
        if let crate::reader::facts::Text::Total(n) = r.text {
            reply.total = reply.total.max(n);
        }
        if let Some(counts) = reply.counts.clone() {
            let text_chars = if r.cumulative { reply.total } else { reply.text.values().sum() };
            let state = Counted::of(&counts, text_chars);
            let changed = match &reply.newest {
                None => true,
                Some(newest) if r.cumulative => state != *newest && !state.within(newest),
                Some(newest) => state != *newest,
            };
            if changed {
                batch.create(
                    "reply_state",
                    Row::new()
                        .link("reply", &row)
                        .int("input_tokens", state.input_tokens)
                        .int("cache_read_tokens", state.cache_read_tokens)
                        .int("cache_write_tokens", state.cache_write_tokens)
                        .int("output_tokens", state.output_tokens)
                        .int("thinking_tokens", state.thinking_tokens)
                        .some_int("tool_tokens", state.tool_tokens)
                        .some_int("web_searches", state.web_searches)
                        .some_int("web_fetches", state.web_fetches)
                        .some_text("service_tier", state.service_tier.clone())
                        .int("text_chars", state.text_chars)
                        .object("usage", counts.usage.clone()),
                );
                reply.newest = Some(state);
            }
        }
        for result in &r.results {
            self.result(line.id, result, batch);
        }
    }

    /// How a call ended, and what it did: the files it changed, and the
    /// git actions, tests and builds of a shell command that finished.
    fn result(&mut self, line_id: Uuid, r: &ToolResult, batch: &mut Batch) {
        let Some(call) = self.calls.get_mut(&r.call_id) else { return };
        if !call.has_result {
            call.has_result = true;
            batch.create(
                "tool_result",
                Row::new()
                    .link("tool_call", &call.row)
                    .uuid("kernel_message", line_id)
                    .time("at", r.at)
                    .text("outcome", r.outcome.as_str())
                    .some_int("ms", r.ms)
                    .int("bytes", r.bytes),
            );
        }
        for e in &r.edits {
            if call.edits.insert(e.path.clone()) {
                batch.create(
                    "edit",
                    Row::new()
                        .link("tool_call", &call.row)
                        .text("path", e.path.clone())
                        .text("kind", e.kind.as_str())
                        .int("added", e.added)
                        .int("removed", e.removed)
                        .text("counted_from", e.counted_from.as_str())
                        .flag("operator_changed", e.operator_changed),
                );
            }
        }
        let finished = matches!(r.outcome, Outcome::Ok | Outcome::Error);
        let ran = match (&call.command, finished && formats::is_shell(&call.tool)) {
            (Some(command), true) => {
                commands::read(command, r.output.as_deref().unwrap_or_default(), r.outcome == Outcome::Ok, call.dir.as_deref())
            }
            _ => commands::Ran::default(),
        };
        if !call.git {
            let events = with_noted(ran.git, &r.noted, call.dir.as_deref());
            if !events.is_empty() {
                call.git = true;
                for e in events {
                    batch.create(
                        "git_event",
                        Row::new()
                            .link("tool_call", &call.row)
                            .time("at", r.at)
                            .text("action", e.action.as_str())
                            .some_text("dir", e.dir)
                            .some_text("hash", e.hash)
                            .some_text("branch", e.branch)
                            .some_text("subject", e.subject)
                            .some_int("pr_number", e.pr_number)
                            .some_text("url", e.url),
                    );
                }
            }
        }
        if let (false, Some(t)) = (call.test, ran.test) {
            call.test = true;
            batch.create(
                "test_run",
                Row::new()
                    .link("tool_call", &call.row)
                    .time("at", r.at)
                    .text("runner", t.runner.as_str())
                    .int("passed", t.passed)
                    .int("failed", t.failed)
                    .int("ignored", t.ignored),
            );
        }
        if let (false, Some(b)) = (call.build, ran.build) {
            call.build = true;
            batch.create(
                "build_run",
                Row::new()
                    .link("tool_call", &call.row)
                    .time("at", r.at)
                    .text("builder", b.builder.as_str())
                    .flag("ok", b.ok)
                    .int("errors", b.errors)
                    .int("warnings", b.warnings),
            );
        }
    }
}

fn create_call(batch: &mut Batch, reply: &RecordId, line_id: Uuid, c: &Call) -> RecordId {
    batch.create(
        "tool_call",
        Row::new()
            .link("reply", reply)
            .text("call_id", c.call_id.clone())
            .uuid("kernel_message", line_id)
            .time("at", c.at)
            .text("tool", c.tool.clone())
            .some_text("path", c.path.clone())
            .some_text("command", c.command.clone())
            .some_text("description", c.description.clone())
            .some_text("pattern", c.pattern.clone())
            .some_text("url", c.url.clone())
            .some_text("query", c.query.clone())
            .some_text("subagent_type", c.subagent_type.clone())
            .flag("background", c.background)
            .int("input_bytes", c.input_bytes),
    )
}

/// The git events a command's output shows, with the ones the agent noted
/// itself that its output does not: one event per action and branch or
/// pull request.
fn with_noted(mut events: Vec<GitEvent>, noted: &[GitEvent], dir: Option<&str>) -> Vec<GitEvent> {
    for n in noted {
        let shown = events.iter().any(|e| {
            e.action == n.action
                && (e.branch == n.branch || n.branch.is_none())
                && (e.pr_number == n.pr_number || n.pr_number.is_none())
        });
        if !shown {
            let mut n = n.clone();
            if n.dir.is_none() {
                n.dir = dir.map(str::to_string);
            }
            events.push(n);
        }
    }
    events
}
