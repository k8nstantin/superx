//! The `lines` source (#508, in depth #537): every line the kernel
//! captured, of every agent and every kind, read into the tables of facts.
//!
//! Each pass reads the lines captured since the last one, starting a
//! minute before where it stopped so a line captured late is never missed:
//! the kernel's conversation lines and the telemetry rows that are lines,
//! merged in the order they were captured. Each line is read as its agent
//! writes lines ([`formats`]), its facts become rows once ([`state`]), and
//! every field it shows is counted ([`census`]). Every page read is checked
//! against what the kernel holds over the same ids: a pass that read a
//! different number of lines is partial, and says so.

pub mod census;
pub mod commands;
pub mod facts;
pub mod formats;
pub mod kernel;
pub mod state;
pub mod store;

use std::collections::{BTreeSet, HashSet, VecDeque};

use chrono::{TimeDelta, Utc};
use superx_kernel::types::Uuid;
use superx_kernel::{Db, Kernel};

use crate::crawler::lines::facts::Read;
use crate::crawler::lines::kernel::{Cursor, Line, Names, Page};
use crate::crawler::lines::state::{Applied, Sources, State};
use crate::crawler::store::{self as rows, Batch};
use crate::crawler::{Outcome, SourceRun, Stop};

/// The source's name, as `pass_source` records it.
pub const SOURCE: &str = "lines";

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

/// What one pass of the source read: its record, and the directories the
/// lines it read were written in, where the agents are working now. The
/// logs are the first to report a change; the `repos` source follows them
/// there in the same pass.
#[derive(Debug)]
pub struct Reading {
    pub run: SourceRun,
    pub worked_in: BTreeSet<String>,
}

/// What the source keeps from one pass to the next: the names of the
/// kernel's agents and sessions, what it has written, and every field it
/// has counted.
#[derive(Debug, Default)]
pub struct Memory {
    names: Names,
    state: State,
    fields: Option<census::Fields>,
}

/// Read the lines captured since the last pass into their facts.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when a database cannot answer or refuses
/// a row. Whatever the source wrote before is kept; the next pass reads
/// again from where the last recorded one stopped.
pub async fn read(kernel: &Kernel, db: &Db, memory: &mut Memory, stop: &mut Stop) -> superx_kernel::Result<Reading> {
    let mut run = SourceRun::start(SOURCE);
    let mut worked_in = BTreeSet::new();
    let from = match store::last_line(db).await? {
        Some(last) => kernel::uuid_time(last)
            .map_or_else(kernel::first_id, |t| kernel::first_id_at(t - TimeDelta::seconds(OVERLAP_SECS))),
        None => kernel::first_id(),
    };
    if memory.fields.is_none() {
        memory.fields = Some(store::fields(db).await?);
    }
    let mut stream = Stream::new(from);
    let mut batch = Batch::default();
    let mut sessions: HashSet<Uuid> = HashSet::new();
    let mut seen = census::Seen::default();
    let mut unused: i64 = 0;
    loop {
        let next = match stop.unless(stream.next(kernel, &mut memory.names)).await {
            None => {
                run.outcome = Outcome::Stopped;
                run.note = Some("stopped before it finished".to_string());
                break;
            }
            Some(next) => next?,
        };
        let Some(line) = next else { break };
        run.items += 1;
        run.from_id.get_or_insert(line.id);
        run.to_id = Some(line.id);
        if let Some(s) = line.session.as_ref().and_then(kernel::record_uuid_of) {
            sessions.insert(s);
        }
        let read = line.agent.as_deref().map_or_else(Read::unknown, |agent| formats::read(agent, &line));
        if read.facts.is_empty() {
            unused += 1;
        }
        if let Some(place) = &read.place {
            worked_in.insert(place.cwd.clone());
        }
        let mut sources = Sources { kernel, db, names: &mut memory.names };
        if memory.state.apply(&mut sources, &line, read, &mut batch).await? == Applied::Unread {
            run.unread += 1;
        }
        if let (Some(fields), Some(agent), Some(value)) = (memory.fields.as_mut(), line.agent.as_deref(), line.json()) {
            let kind = formats::kind(agent, value, line.kernel_kind.as_deref());
            let state = &memory.state;
            let call_tool = |call_id: &str| state.call_tool(line.session.as_ref(), call_id);
            let paths = census::paths(value, &formats::shapes(agent), &call_tool);
            fields.see(&mut seen, (agent, &kind), &line, paths, &mut batch);
        }
        if batch.waiting() >= WRITE_ROWS {
            rows::flush(db, &mut batch).await?;
        }
    }
    if let (Some(why), Outcome::Complete) = (stream.stuck(), run.outcome) {
        run.outcome = Outcome::Partial;
        run.note = Some(why);
    }
    rows::flush(db, &mut batch).await?;
    run.rows = batch.added.clone();
    run.counts.insert("sessions_read", i64::try_from(sessions.len()).unwrap_or(i64::MAX));
    run.counts.insert("lines_unused", unused);
    run.counts.insert("fields_seen", seen.seen());
    run.counts.insert("fields_new", seen.new_fields());
    run.counts.insert("fields_not_stored", seen.not_stored());
    check(&mut run, stream.held());
    run.finished_at = Utc::now();
    memory.state.forget_before(kernel::first_id_at(Utc::now() - TimeDelta::seconds(KEEP_SECS)));
    Ok(Reading { run, worked_in })
}

/// The pass checked against its source: the lines it read against those
/// the kernel holds over the ids of the pages it read. A pass that read a
/// different number is partial, its note naming both.
fn check(run: &mut SourceRun, held: i64) {
    run.counts.insert("lines_held", held);
    if run.outcome == Outcome::Stopped || run.items == held {
        return;
    }
    let differ = format!("{} read of {held} held", run.items);
    run.outcome = Outcome::Partial;
    run.note = Some(match run.note.take() {
        Some(note) => format!("{note}; {differ}"),
        None => differ,
    });
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
    /// The lines the kernel holds over the ids of the pages read.
    held: i64,
}

impl Stream {
    fn new(from: Uuid) -> Self {
        let source =
            |kind| Source { kind, lines: VecDeque::new(), cursor: Cursor::At(from), done: false, stuck: false, held: 0 };
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

    /// The lines the kernel holds over the ids of every page read.
    fn held(&self) -> i64 {
        self.messages.held + self.events.held
    }
}

impl Source {
    /// Read pages until a line is waiting or the lines run out, counting
    /// the lines the kernel holds over each page's ids.
    async fn fill(&mut self, kernel: &Kernel, names: &mut Names) -> superx_kernel::Result<()> {
        while self.lines.is_empty() && !self.done {
            names.may_reload();
            let (page, limit): (Page, u32) = match self.kind {
                Kind::Messages => (kernel::messages(kernel, names, self.cursor, MESSAGE_PAGE).await?, MESSAGE_PAGE),
                Kind::Events => (kernel::events(kernel, names, self.cursor, EVENT_PAGE).await?, EVENT_PAGE),
            };
            if let (Some(first), Some(last)) = (page.first, page.last) {
                self.held += match self.kind {
                    Kind::Messages => kernel::messages_held(kernel, first, last).await?,
                    Kind::Events => kernel::events_held(kernel, first, last).await?,
                };
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_whose_pages_counts_equal_the_lines_it_read_is_complete_and_one_that_read_fewer_is_partial_its_note_naming_both() {
        let read = |items| SourceRun { items, ..SourceRun::start(SOURCE) };
        let mut equal = read(12);
        check(&mut equal, 12);
        assert_eq!((equal.outcome, equal.note.as_deref(), equal.counts.get("lines_held")), (Outcome::Complete, None, Some(&12)));
        let mut fewer = read(11);
        check(&mut fewer, 12);
        assert_eq!(fewer.outcome, Outcome::Partial);
        assert_eq!(fewer.note.as_deref(), Some("11 read of 12 held"), "the note names both");
        let mut stuck = SourceRun { note: Some("a page held no UUID ids".to_string()), outcome: Outcome::Partial, ..read(3) };
        check(&mut stuck, 5);
        assert_eq!(stuck.note.as_deref(), Some("a page held no UUID ids; 3 read of 5 held"), "beside what else stopped it");
        let mut stopped = SourceRun { outcome: Outcome::Stopped, ..read(2) };
        check(&mut stopped, 9);
        assert_eq!(stopped.outcome, Outcome::Stopped, "a stopped pass read what it could");
    }
}
