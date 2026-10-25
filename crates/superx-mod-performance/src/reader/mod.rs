//! The reader (#508): every line the kernel captured, read into the
//! module's tables of facts, pass after pass while `superx-performance`
//! runs.
//!
//! A pass reads the lines captured since the last one, starting a minute
//! before where it stopped so a line captured late is never missed: the
//! kernel's conversation lines and Claude Code's other lines, merged in
//! the order they were captured. Each line is read as its agent writes
//! lines ([`formats`]), its facts become rows once ([`state`]), and the
//! pass ends with its own row: `complete`, `partial` when lines could not
//! be read past some point, or `stopped`. A pass that fails is not
//! recorded; the next one reads again from the last one that was.
//!
//! The reader only reads the kernel (SELECT) and only writes its own
//! database (CREATE).

pub mod commands;
pub mod facts;
pub mod formats;
pub mod kernel;
pub mod state;
pub mod store;
pub mod value;

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::types::Uuid;
use superx_kernel::{Db, Kernel};
use tokio::sync::watch;

use crate::reader::facts::Read;
use crate::reader::kernel::{Cursor, Line, Names, Page};
use crate::reader::state::{Applied, Sources, State};
use crate::reader::store::{Batch, Row};
use crate::{params, status, MODULE_NAME};

/// Conversation lines read per page. On a copy of the live database the
/// id walk read every line in 1.4 s (#501).
const MESSAGE_PAGE: u32 = 500; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Telemetry rows scanned per page: most of the stream is not a line, and a
/// page reads each row it scans.
const EVENT_PAGE: u32 = 2_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How far before the last pass's last line a pass starts.
const OVERLAP_SECS: i64 = 60; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Rows held before they are written.
const WRITE_ROWS: usize = 1_000; // skill-allow: §9-const — write-path bound, not a policy tunable

/// A session no line came from for this long is forgotten until one does.
const KEEP_SECS: i64 = 600; // skill-allow: §9-const — memory bound, not a policy tunable

/// While paused or without its database, how often the reader looks again.
const RECHECK_SECS: u64 = 60; // skill-allow: §9-const — idle re-check cadence, not a policy tunable

/// A request to stop, as the reader sees it.
pub struct Stop(watch::Receiver<bool>);

impl Stop {
    #[must_use]
    pub fn new(asked: watch::Receiver<bool>) -> Self {
        Self(asked)
    }

    /// A stop was asked for.
    #[must_use]
    pub fn requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once a stop is asked for, or once nothing can ask for one
    /// any more: either way, it is time to stop.
    pub async fn wait(&mut self) {
        let _asked_or_gone = self.0.wait_for(|stopped| *stopped).await;
    }

    /// `step`, unless a stop comes first: then `None`.
    pub async fn unless<T>(&mut self, step: impl Future<Output = T>) -> Option<T> {
        if self.requested() {
            return None;
        }
        tokio::select! {
            out = step => Some(out),
            () = self.wait() => None,
        }
    }
}

/// What the reader keeps from one pass to the next: its connection to its
/// own database, the names of the kernel's agents and sessions, and what it
/// has written. A pass that fails drops all of it.
#[derive(Default)]
pub struct Reader {
    db: Option<Db>,
    names: Names,
    state: State,
}

/// What one pass read and wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pass {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub from_line: Option<Uuid>,
    pub to_line: Option<Uuid>,
    pub lines_read: i64,
    pub lines_unread: i64,
    /// Rows written, by table.
    pub added: BTreeMap<&'static str, i64>,
    pub rows_written: i64,
    pub outcome: &'static str,
    pub note: Option<String>,
}

/// The tables a pass counts, and what its row calls each count.
const COUNTED: [(&str, &str); 15] = [
    ("session", "sessions_new"),
    ("session_place", "places_new"),
    ("prompt", "prompts_new"),
    ("reply", "replies_new"),
    ("reply_state", "reply_states_new"),
    ("thinking", "thinking_new"),
    ("tool_call", "tool_calls_new"),
    ("tool_result", "tool_results_new"),
    ("edit", "edits_new"),
    ("git_event", "git_events_new"),
    ("test_run", "test_runs_new"),
    ("build_run", "build_runs_new"),
    ("turn", "turns_new"),
    ("compaction", "compactions_new"),
    ("away", "aways_new"),
];

/// Pass after pass, until asked to stop: one right away, then one every
/// `attr_performance_read_secs`, read afresh before each. Nothing is read
/// while it is 0, or while the module's database is not provisioned; both
/// are looked at again a minute later.
pub async fn run(kernel: Kernel, mut stop: Stop) {
    let mut reader = Reader::default();
    loop {
        let wait_secs = match params::resolve(&kernel).await {
            Err(e) => {
                tracing::warn!(target: "performance", error = %e, "no pass: its parameters cannot be read");
                RECHECK_SECS
            }
            Ok(resolved) if resolved.params.read_secs == 0 => RECHECK_SECS,
            Ok(resolved) => {
                if reader.db.is_none() {
                    match kernel.module_db(MODULE_NAME).await {
                        Ok(db) => reader.db = Some(db),
                        Err(e) => tracing::warn!(
                            target: "performance",
                            error = %status::own_hint(&e.to_string()),
                            "no pass: its database is not available"
                        ),
                    }
                }
                if reader.db.is_some() {
                    match pass(&kernel, &mut reader, &mut stop).await {
                        Ok(p) if p.rows_written > 0 || p.outcome != "complete" => tracing::info!(
                            target: "performance",
                            outcome = p.outcome,
                            lines_read = p.lines_read,
                            lines_unread = p.lines_unread,
                            rows_written = p.rows_written,
                            "pass"
                        ),
                        Ok(p) => tracing::debug!(target: "performance", lines_read = p.lines_read, "pass, nothing new"),
                        Err(e) => {
                            tracing::warn!(target: "performance", error = %e, "the pass failed and was not recorded");
                        }
                    }
                    resolved.params.read_secs
                } else {
                    RECHECK_SECS
                }
            }
        };
        if stop.unless(tokio::time::sleep(std::time::Duration::from_secs(wait_secs))).await.is_none() {
            break;
        }
    }
}

/// One pass, recorded when it ends. A stop ends it where it is, and it is
/// recorded as `stopped`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when a database cannot answer or refuses
/// a row. The pass is not recorded; what the reader kept is dropped, and
/// the next pass reads again from the last pass recorded.
pub async fn pass(kernel: &Kernel, reader: &mut Reader, stop: &mut Stop) -> superx_kernel::Result<Pass> {
    let Some(db) = reader.db.clone() else {
        return Err(superx_kernel::KernelError::Config("the reader has no database".to_string()));
    };
    let result = read(kernel, &db, reader, stop).await;
    if result.is_err() {
        *reader = Reader::default();
    }
    result
}

/// [`pass`] on a database the caller holds: for the tests, which hold the
/// module's database without an instance behind it.
///
/// # Errors
///
/// As [`pass`].
pub async fn pass_on(kernel: &Kernel, db: &Db, reader: &mut Reader, stop: &mut Stop) -> superx_kernel::Result<Pass> {
    reader.db = Some(db.clone());
    pass(kernel, reader, stop).await
}

async fn read(kernel: &Kernel, db: &Db, reader: &mut Reader, stop: &mut Stop) -> superx_kernel::Result<Pass> {
    let started_at = Utc::now();
    let from = match store::last_line(db).await? {
        Some(last) => kernel::uuid_time(last)
            .map_or_else(kernel::first_id, |t| kernel::first_id_at(t - TimeDelta::seconds(OVERLAP_SECS))),
        None => kernel::first_id(),
    };
    let mut stream = Stream::new(from);
    let mut batch = Batch::default();
    let mut pass = Pass {
        started_at,
        finished_at: started_at,
        from_line: None,
        to_line: None,
        lines_read: 0,
        lines_unread: 0,
        added: BTreeMap::new(),
        rows_written: 0,
        outcome: "complete",
        note: None,
    };
    loop {
        let next = match stop.unless(stream.next(kernel, &mut reader.names)).await {
            None => {
                pass.outcome = "stopped";
                pass.note = Some("stopped before it finished".to_string());
                break;
            }
            Some(next) => next?,
        };
        let Some(line) = next else { break };
        pass.lines_read += 1;
        pass.from_line.get_or_insert(line.id);
        pass.to_line = Some(line.id);
        let read = line.agent.as_deref().map_or_else(Read::unknown, |agent| formats::read(agent, &line.raw, line.at));
        let mut sources = Sources { kernel, db, names: &mut reader.names };
        if reader.state.apply(&mut sources, &line, read, &mut batch).await? == Applied::Unread {
            pass.lines_unread += 1;
        }
        if batch.waiting() >= WRITE_ROWS {
            store::flush(db, &mut batch).await?;
        }
    }
    if let (Some(why), "complete") = (stream.stuck(), pass.outcome) {
        pass.outcome = "partial";
        pass.note = Some(why);
    }
    store::flush(db, &mut batch).await?;
    pass.finished_at = Utc::now();
    pass.rows_written = batch.total();
    pass.added = batch.added.clone();
    let mut row = Row::new()
        .time("started_at", pass.started_at)
        .time("finished_at", pass.finished_at)
        .some_uuid("from_line", pass.from_line)
        .some_uuid("to_line", pass.to_line)
        .int("lines_read", pass.lines_read)
        .int("lines_unread", pass.lines_unread)
        .int("rows_written", pass.rows_written)
        .text("outcome", pass.outcome)
        .some_text("note", pass.note.clone());
    for (table, field) in COUNTED {
        row = row.int(field, pass.added.get(table).copied().unwrap_or(0));
    }
    let mut recorded = Batch::default();
    recorded.create("pass", row);
    store::flush(db, &mut recorded).await?;
    reader.state.forget_before(kernel::first_id_at(Utc::now() - TimeDelta::seconds(KEEP_SECS)));
    Ok(pass)
}

/// The kernel's two kinds of lines, merged in the order they were
/// captured.
struct Stream {
    messages: Source,
    events: Source,
}

#[derive(Clone, Copy)]
enum Kind {
    Messages,
    Events,
}

struct Source {
    kind: Kind,
    lines: VecDeque<Line>,
    cursor: Cursor,
    done: bool,
    stuck: bool,
}

impl Stream {
    fn new(from: Uuid) -> Self {
        let source = |kind| Source { kind, lines: VecDeque::new(), cursor: Cursor::At(from), done: false, stuck: false };
        Self { messages: source(Kind::Messages), events: source(Kind::Events) }
    }

    /// The next line captured, of either kind; `None` once both are read to
    /// their end.
    async fn next(&mut self, kernel: &Kernel, names: &mut Names) -> superx_kernel::Result<Option<Line>> {
        self.messages.fill(kernel, names).await?;
        self.events.fill(kernel, names).await?;
        let from = match (self.messages.lines.front(), self.events.lines.front()) {
            (None, None) => return Ok(None),
            (Some(_), None) => &mut self.messages,
            (None, Some(_)) => &mut self.events,
            (Some(m), Some(e)) => {
                if m.id <= e.id {
                    &mut self.messages
                } else {
                    &mut self.events
                }
            }
        };
        Ok(from.lines.pop_front())
    }

    /// Why reading stopped short, when it did.
    fn stuck(&self) -> Option<String> {
        (self.messages.stuck || self.events.stuck)
            .then(|| "a page of captured lines held no UUID ids; reading stopped there".to_string())
    }
}

impl Source {
    /// Read pages until a line is waiting or the lines run out.
    async fn fill(&mut self, kernel: &Kernel, names: &mut Names) -> superx_kernel::Result<()> {
        while self.lines.is_empty() && !self.done {
            names.may_reload();
            let (page, limit): (Page, u32) = match self.kind {
                Kind::Messages => (kernel::messages(kernel, names, self.cursor, MESSAGE_PAGE).await?, MESSAGE_PAGE),
                Kind::Events => (kernel::events(kernel, names, self.cursor, EVENT_PAGE).await?, EVENT_PAGE),
            };
            match page.last {
                Some(last) => self.cursor = Cursor::After(last),
                None if page.scanned > 0 => {
                    self.stuck = true;
                    self.done = true;
                }
                None => {}
            }
            if page.scanned < limit as usize {
                self.done = true;
            }
            self.lines.extend(page.lines);
        }
        Ok(())
    }
}
