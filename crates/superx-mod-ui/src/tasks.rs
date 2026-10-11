//! The join (#434, epic #433): what each of the operator's turns set going,
//! and what became of it.
//!
//! A TASK is one operator turn and everything the agent did until the next
//! one: the models that replied, what they spent, the files they wrote, and
//! the git output they saw. Every commit this machine made is matched to
//! the task that made it, by the best evidence there is, and its lines are
//! followed to their fate. The model view, the operator view and the
//! overall view are all cuts of this one table.
//!
//! # Matching a commit to its task
//!
//! 1. **exact**: the task's own tool output printed the commit's hash next
//!    to its subject, while the commit was being made or soon after. An
//!    agent prints what it just committed; a `git log` days later prints
//!    it too, which is why the print has to be near the commit.
//! 2. **files**: the commit changes files the task wrote, in the task's
//!    repository, while the task was open.
//! 3. **time**: exactly one task was open in that repository when the
//!    commit was made.
//!
//! Two tasks with equal evidence leave the commit uncredited. A guess would
//! be a number that lies, and the coverage says how many were left.
//!
//! # Who drove it
//!
//! - **directed**: an operator turn asked for the work;
//! - **corrected**: the turn pulled the previous work back, in the
//!   operator's own measured phrasings (#406); the previous task is marked
//!   as having needed correcting;
//! - **unsupervised**: work with no operator turn opening it, such as a
//!   subagent or a loop. Within a task, the burn more than the leash after
//!   the turn is counted apart.
//!
//! # Age is work, not days
//!
//! Code nobody works around is never tested, so its survival says nothing
//! (#434, operator). Each commit carries its EXPOSURE: the working minutes
//! every later task spent in the same repository.
//!
//! # The operator's time
//!
//! Each operator turn counts the time since the operator's previous turn,
//! in any session, as work, up to the break threshold. Parallel sessions
//! are never counted twice. A SITTING is a stretch of turns with no break.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, Result};
use ts_rs::TS;

use crate::landed::{RepoWork, WorkCommit};
use crate::thrown::{family, key_of, obj, str_of, time_of};

/// How long past the turn that opened it a task's work still counts as
/// steered. A parameter on the ui module's registry entity.
pub const LEASH_SECS_PARAM: &str = "attr_ui_leash_secs";
const DEFAULT_LEASH_SECS: i64 = 1_800; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The longest gap between two operator turns that still counts as work;
/// a longer one is a break. A parameter on the ui module's registry entity.
pub const BREAK_SECS_PARAM: &str = "attr_ui_break_secs";
const DEFAULT_BREAK_SECS: i64 = 1_800; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// How far ahead of the commit its print may come and still be the commit
/// being made: author time and capture time come from the same clock, but
/// not from the same instant.
const PRINT_SKEW_SECS: i64 = 60; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How much of a task's opening turn the table carries.
const OPENING_CHARS: usize = 140; // skill-allow: §9-const — display bound, not a policy tunable

/// How much text after a printed hash is kept to compare with a subject.
const AFTER_CHARS: usize = 240; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How much of a commit's subject has to follow its printed hash.
const SUBJECT_CHARS: usize = 40; // skill-allow: §9-const — read-path bound, not a policy tunable

/// The tools whose input names a file they wrote.
const WRITERS: [&str; 3] = ["Edit", "Write", "MultiEdit"];

/// Who drove a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum Driver {
    Directed,
    Corrected,
    Unsupervised,
}

/// The evidence a commit was matched to its task by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum Rule {
    Exact,
    Files,
    Time,
}

// ---------------------------------------------------------------------------
// One session, as read from the substrate
// ---------------------------------------------------------------------------

/// Everything the join needs from one session, read once.
#[derive(Debug, Default, Clone)]
pub struct SessionLog {
    pub name: String,
    /// The operator's turns: when, and what they said.
    pub turns: Vec<(DateTime<Utc>, String)>,
    /// One per reply, folded from its rows.
    pub replies: Vec<LoggedReply>,
    /// Files the agent wrote, with when and where.
    pub writes: Vec<LoggedWrite>,
    /// Hashes git printed to the agent, with the text after each.
    pub printed: Vec<Printed>,
}

#[derive(Debug, Clone)]
pub struct LoggedReply {
    pub at: DateTime<Utc>,
    /// The model family (`opus 5.5`).
    pub model: String,
    pub cwd: Option<String>,
    pub out_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct LoggedWrite {
    pub at: DateTime<Utc>,
    pub cwd: Option<String>,
    /// Absolute paths.
    pub paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Printed {
    pub at: DateTime<Utc>,
    /// A run of 7 to 40 hex digits.
    pub hash: String,
    /// The text after it, lower-cased.
    pub after: String,
}

/// Read one session. Four bounded queries, each over this session's rows
/// only, projecting what the join uses: a `Write`'s whole file body never
/// leaves the engine.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn read_session(kernel: &Kernel, session: RecordId, name: &str) -> Result<SessionLog> {
    let mut log = SessionLog { name: name.to_string(), ..SessionLog::default() };

    let turns: Vec<Value> = crate::answered("task turns", || async {
        Ok(kernel
            .db()
            .query(
                "SELECT (emitted_at ?? valid_from) AS at, content, valid_from FROM message \
                 WHERE session = $sess AND role = 'user' ORDER BY valid_from ASC",
            )
            .bind(("sess", session.clone()))
            .await?
            .take(0)?)
    })
    .await?;
    log.turns = turns
        .iter()
        .filter_map(obj)
        .filter_map(|o| Some((time_of(o, "at")?, str_of(o, "content").unwrap_or("").to_string())))
        .collect();

    let replies_query = format!(
        "SELECT (emitted_at ?? valid_from) AS at, (raw.message.model ?? raw.model) AS model, \
             {} AS k, raw.cwd AS cwd, raw.message.usage AS cu, raw.tokens AS gu, valid_from \
         FROM message WHERE session = $sess \
             AND (raw.message.model != NONE OR raw.model != NONE) \
         ORDER BY valid_from ASC",
        crate::stats::REPLY_KEY_SQL
    );
    let rows: Vec<Value> = crate::answered("task replies", || async {
        Ok(kernel.db().query(replies_query.as_str()).bind(("sess", session.clone())).await?.take(0)?)
    })
    .await?;
    log.replies = fold_replies(&rows);

    // What the agent wrote and which of its calls ran git. Only the path
    // of a write is projected, and only the id of a git call.
    let rows: Vec<Value> = crate::answered("task tool calls", || async {
        Ok(kernel
            .db()
            .query(
                "SELECT (emitted_at ?? valid_from) AS at, raw.cwd AS cwd, \
                     raw.message.content[WHERE type = 'tool_use' AND name INSIDE $writers].input.file_path AS paths, \
                     raw.message.content[WHERE type = 'tool_use' AND name = 'NotebookEdit'].input.notebook_path AS notebooks, \
                     raw.message.content[WHERE type = 'tool_use' AND name = 'Bash' AND input.command CONTAINS 'git'].id AS git, \
                     valid_from \
                 FROM message WHERE session = $sess AND role = 'assistant' ORDER BY valid_from ASC",
            )
            .bind(("sess", session.clone()))
            .bind(("writers", WRITERS.iter().map(|w| (*w).to_string()).collect::<Vec<String>>()))
            .await?
            .take(0)?)
    })
    .await?;
    let mut git_ids: Vec<String> = Vec::new();
    for o in rows.iter().filter_map(obj) {
        let Some(at) = time_of(o, "at") else { continue };
        let mut paths = strings(o.get("paths"));
        paths.extend(strings(o.get("notebooks")));
        if !paths.is_empty() {
            log.writes.push(LoggedWrite { at, cwd: str_of(o, "cwd").map(str::to_string), paths });
        }
        git_ids.extend(strings(o.get("git")));
    }

    // What those git calls printed. Only their results are read.
    if !git_ids.is_empty() {
        let rows: Vec<Value> = crate::answered("task git output", || async {
            Ok(kernel
                .db()
                .query(
                    "SELECT (emitted_at ?? valid_from) AS at, \
                         raw.message.content[WHERE type = 'tool_result' AND tool_use_id INSIDE $ids] AS blocks, \
                         valid_from \
                     FROM message WHERE session = $sess AND role = 'tool' \
                         AND raw.message.content[WHERE type = 'tool_result' AND tool_use_id INSIDE $ids] != [] \
                     ORDER BY valid_from ASC",
                )
                .bind(("sess", session.clone()))
                .bind(("ids", git_ids.clone()))
                .await?
                .take(0)?)
        })
        .await?;
        for o in rows.iter().filter_map(obj) {
            let Some(at) = time_of(o, "at") else { continue };
            let Some(Value::Array(blocks)) = o.get("blocks") else { continue };
            for text in blocks.iter().filter_map(obj).filter_map(crate::stats::tool_result_text) {
                for (hash, after) in printed_hashes(&text) {
                    log.printed.push(Printed { at, hash, after });
                }
            }
        }
    }
    Ok(log)
}

/// Every session's log. Uncapped by design, as the model runs are: a
/// sample would credit one model's commits to another.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_logs(kernel: &Kernel) -> Result<Vec<SessionLog>> {
    let sessions =
        crate::answered("sessions", || kernel.list_named_entities("node_session", "attr_session_descriptor")).await?;
    let mut out = Vec::new();
    for s in sessions {
        let Some(name) = obj(&s.payload).and_then(|o| str_of(o, "name")).map(str::to_string) else { continue };
        if let Ok(log) = read_session(kernel, s.entity_id.clone(), &name).await {
            out.push(log);
        }
    }
    Ok(out)
}

/// The strings of an array value; anything else is none.
fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A session's replies, one per reply: a reply's rows share a key, and the
/// last of them is the fullest.
fn fold_replies(rows: &[Value]) -> Vec<LoggedReply> {
    let mut out: Vec<LoggedReply> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for o in rows.iter().filter_map(obj) {
        // `<synthetic>` and friends are the runtime's own markers (#367).
        let (Some(model), Some(at), Some(key)) =
            (str_of(o, "model").filter(|m| !m.starts_with('<')), time_of(o, "at"), key_of(o))
        else {
            continue;
        };
        let out_tokens =
            crate::stats::usage_of(o.get("cu").and_then(obj), o.get("gu").and_then(obj)).map_or(0, |u| u.out);
        let reply = LoggedReply { at, model: family(model), cwd: str_of(o, "cwd").map(str::to_string), out_tokens };
        match index.get(&key) {
            Some(&i) => {
                let prev = &mut out[i];
                prev.at = prev.at.max(reply.at);
                prev.out_tokens = prev.out_tokens.max(reply.out_tokens);
                prev.cwd = reply.cwd.or(prev.cwd.take());
            }
            None => {
                index.insert(key, out.len());
                out.push(reply);
            }
        }
    }
    out.sort_by_key(|r| r.at);
    out
}

/// Every run of 7 to 40 lower-case hex digits standing alone in `text`,
/// with the text after it, lower-cased. What git prints for a commit is
/// one of these followed by its subject (`[main 1a2b3c4] subject`,
/// `1a2b3c4 subject`).
#[must_use]
pub fn printed_hashes(text: &str) -> Vec<(String, String)> {
    let bytes = text.as_bytes();
    let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts = hex(bytes[i]) && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
        if !starts {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && hex(bytes[i]) {
            i += 1;
        }
        let ends = i == bytes.len() || !bytes[i].is_ascii_alphanumeric();
        // A word spelled in a to f (`defaced`) is not a hash; one hash in a
        // thousand has no digit in its first seven.
        let digit = bytes[start..i].iter().any(u8::is_ascii_digit);
        // Both ends of the run are ASCII, so both are char boundaries.
        if ends && digit && (7..=40).contains(&(i - start)) {
            let after: String = text[i..].chars().take(AFTER_CHARS).collect::<String>().to_lowercase();
            out.push((text[start..i].to_string(), after));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

/// One operator turn and everything the agent did until the next.
#[derive(Debug, Clone)]
pub struct Task {
    pub session: String,
    /// The turn's time, or the first activity of unsupervised work.
    pub opened: DateTime<Utc>,
    /// The next turn's time; for the last, the last activity plus the idle
    /// threshold.
    pub until: DateTime<Utc>,
    pub driver: Driver,
    /// The next turn pulled this work back.
    pub needed_correcting: bool,
    pub opening: String,
    /// Model families that replied, in order of first reply.
    pub models: Vec<String>,
    pub out_tokens: i64,
    /// Output tokens spent more than the leash after the turn.
    pub unsupervised_out: i64,
    /// The agent's working time: gaps between its replies, each capped at
    /// the idle threshold.
    pub working_secs: i64,
    pub cwds: Vec<String>,
    /// Absolute paths written.
    pub files: Vec<String>,
    pub printed: Vec<Printed>,
}

/// A session's tasks, oldest first.
///
/// Activity before the first turn is unsupervised work of its own; a
/// session with no turns at all is one unsupervised task.
#[must_use]
pub fn build_tasks(log: &SessionLog, idle_secs: i64, leash_secs: i64) -> Vec<Task> {
    let mut first: Option<DateTime<Utc>> = None;
    let mut last: Option<DateTime<Utc>> = None;
    let mut see = |t: DateTime<Utc>| {
        first = Some(first.map_or(t, |f| f.min(t)));
        last = Some(last.map_or(t, |l| l.max(t)));
    };
    log.replies.iter().for_each(|r| see(r.at));
    log.writes.iter().for_each(|w| see(w.at));
    log.printed.iter().for_each(|p| see(p.at));
    let (Some(first), Some(last)) = (first, last) else {
        return Vec::new();
    };

    // Boundaries: an unsupervised opening when work came first, then one
    // per turn.
    let mut opens: Vec<(DateTime<Utc>, Option<&str>)> = Vec::new();
    let first_turn = log.turns.first().map(|t| t.0);
    if first_turn.is_none_or(|t| first < t) {
        opens.push((first, None));
    }
    opens.extend(log.turns.iter().map(|(at, text)| (*at, Some(text.as_str()))));

    let idle = Duration::seconds(idle_secs);
    let mut tasks: Vec<Task> = opens
        .iter()
        .enumerate()
        .map(|(i, (at, text))| {
            let until = opens.get(i + 1).map_or(last.max(*at) + idle, |n| n.0);
            let driver = match text {
                None => Driver::Unsupervised,
                Some(t) if crate::stats::says_word(t, &crate::stats::CORRECTIONS, false) => Driver::Corrected,
                Some(_) => Driver::Directed,
            };
            let opening: String = text
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(OPENING_CHARS)
                .collect();
            Task {
                session: log.name.clone(),
                opened: *at,
                until,
                driver,
                needed_correcting: false,
                opening,
                models: Vec::new(),
                out_tokens: 0,
                unsupervised_out: 0,
                working_secs: 0,
                cwds: Vec::new(),
                files: Vec::new(),
                printed: Vec::new(),
            }
        })
        .collect();
    for i in 1..tasks.len() {
        if tasks[i].driver == Driver::Corrected {
            tasks[i - 1].needed_correcting = true;
        }
    }

    // Each piece of activity belongs to the last task opened at or before it.
    let starts: Vec<DateTime<Utc>> = tasks.iter().map(|t| t.opened).collect();
    let owner = |at: DateTime<Utc>| starts.partition_point(|s| *s <= at).saturating_sub(1);
    let mut prev_reply: Vec<Option<DateTime<Utc>>> = vec![None; tasks.len()];
    for r in &log.replies {
        let i = owner(r.at);
        let t = &mut tasks[i];
        if !t.models.contains(&r.model) {
            t.models.push(r.model.clone());
        }
        t.out_tokens += r.out_tokens;
        if t.driver == Driver::Unsupervised || r.at > t.opened + Duration::seconds(leash_secs) {
            t.unsupervised_out += r.out_tokens;
        }
        let since = prev_reply[i].unwrap_or(t.opened);
        t.working_secs += (r.at - since).num_seconds().clamp(0, idle_secs);
        prev_reply[i] = Some(r.at);
        if let Some(cwd) = &r.cwd {
            if !t.cwds.contains(cwd) {
                t.cwds.push(cwd.clone());
            }
        }
    }
    for w in &log.writes {
        let t = &mut tasks[owner(w.at)];
        if let Some(cwd) = &w.cwd {
            if !t.cwds.contains(cwd) {
                t.cwds.push(cwd.clone());
            }
        }
        for p in &w.paths {
            if !t.files.contains(p) {
                t.files.push(p.clone());
            }
        }
    }
    for p in &log.printed {
        tasks[owner(p.at)].printed.push(p.clone());
    }
    tasks
}

// ---------------------------------------------------------------------------
// The operator's time
// ---------------------------------------------------------------------------

/// One operator turn, as the operator's time is counted.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkTurn {
    pub at: DateTime<Utc>,
    pub session: String,
    /// The model family that answered it, if any did.
    pub model: Option<String>,
    /// The time since the operator's previous turn in any session, capped
    /// at the break threshold.
    pub work_secs: i64,
}

/// A stretch of turns with no break over the threshold.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct Sitting {
    pub start: String,
    pub end: String,
    pub turns: i64,
    pub work_minutes: i64,
}

/// Every operator turn across all sessions, oldest first, with its work
/// gap; and the sittings they form.
#[must_use]
pub fn operator_time(logs: &[SessionLog], break_secs: i64) -> (Vec<WorkTurn>, Vec<Sitting>) {
    let mut turns: Vec<WorkTurn> = Vec::new();
    for log in logs {
        for (at, _) in &log.turns {
            // Replies are oldest first: the first at or after the turn
            // answered it.
            let answered = log.replies.get(log.replies.partition_point(|r| r.at < *at)).map(|r| r.model.clone());
            turns.push(WorkTurn { at: *at, session: log.name.clone(), model: answered, work_secs: 0 });
        }
    }
    turns.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.session.cmp(&b.session)));
    let mut sittings: Vec<Sitting> = Vec::new();
    // (start, end, turns, working seconds) of the sitting under way.
    let mut open: Option<(DateTime<Utc>, DateTime<Utc>, i64, i64)> = None;
    for i in 0..turns.len() {
        let gap = if i == 0 { None } else { Some((turns[i].at - turns[i - 1].at).num_seconds().max(0)) };
        let worked = gap.map_or(0, |g| g.min(break_secs));
        turns[i].work_secs = worked;
        let goes_on = open.is_some() && gap.is_some_and(|g| g <= break_secs);
        if goes_on {
            if let Some(s) = open.as_mut() {
                s.1 = turns[i].at;
                s.2 += 1;
                s.3 += worked;
            }
        } else {
            if let Some(s) = open.take() {
                sittings.push(sitting(s));
            }
            open = Some((turns[i].at, turns[i].at, 1, 0));
        }
    }
    if let Some(s) = open {
        sittings.push(sitting(s));
    }
    (turns, sittings)
}

fn sitting((start, end, turns, secs): (DateTime<Utc>, DateTime<Utc>, i64, i64)) -> Sitting {
    Sitting { start: start.to_rfc3339(), end: end.to_rfc3339(), turns, work_minutes: secs / 60 }
}

// ---------------------------------------------------------------------------
// Matching commits to tasks
// ---------------------------------------------------------------------------

/// What matching needs to know of a task in one repository.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub task: usize,
    pub opened: DateTime<Utc>,
    pub until: DateTime<Utc>,
    /// Paths the task wrote, relative to this repository's checkout.
    pub files: HashSet<String>,
}

/// Find the task that made `c`, and by what evidence.
///
/// `printed` is every hash any task printed, with the task and the time;
/// `candidates` the tasks that worked in the commit's repository.
#[must_use]
pub fn match_commit(
    c: &WorkCommit,
    printed: &[(usize, &Printed)],
    candidates: &[Candidate],
    spans: &[(DateTime<Utc>, DateTime<Utc>)],
    idle_secs: i64,
) -> Option<(usize, Rule)> {
    // 1. exact: its hash and subject, printed while it was being made: in
    //    the task open at that moment, or within the idle threshold after.
    let subject: String = c.subject.chars().take(SUBJECT_CHARS).collect();
    let near = |task: usize, at: DateTime<Utc>| {
        let (opened, until) = spans[task];
        (opened <= c.at && c.at < until)
            || (at >= c.at - Duration::seconds(PRINT_SKEW_SECS) && at <= c.at + Duration::seconds(idle_secs))
    };
    let exact = printed
        .iter()
        .filter(|(task, p)| {
            c.hash.starts_with(p.hash.as_str()) && !subject.is_empty() && p.after.contains(&subject) && near(*task, p.at)
        })
        .min_by_key(|(_, p)| p.at);
    if let Some((task, _)) = exact {
        return Some((*task, Rule::Exact));
    }

    let open: Vec<&Candidate> = candidates.iter().filter(|k| k.opened <= c.at && c.at < k.until).collect();
    // 2. files: the open task that wrote the most of its files, if one did
    //    so alone.
    let overlap = |k: &Candidate| c.files.iter().filter(|(p, _, _)| k.files.contains(p)).count();
    let best = open.iter().map(|k| overlap(k)).max().unwrap_or(0);
    if best > 0 {
        let top: Vec<&&Candidate> = open.iter().filter(|k| overlap(k) == best).collect();
        return match top.as_slice() {
            [one] => Some((one.task, Rule::Files)),
            _ => None,
        };
    }
    // 3. time: the only task open there.
    match open.as_slice() {
        [one] => Some((one.task, Rule::Time)),
        _ => None,
    }
}

/// `path` relative to the first of `toplevels` it lies under.
fn relative<'a>(path: &'a str, toplevels: &[&str]) -> Option<&'a str> {
    toplevels
        .iter()
        .filter_map(|top| path.strip_prefix(top).and_then(|rest| rest.strip_prefix('/')))
        .min_by_key(|rest| rest.len())
}

// ---------------------------------------------------------------------------
// The join
// ---------------------------------------------------------------------------

/// A task, as the views read it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TaskRow {
    pub session: String,
    pub opened_at: String,
    pub driver: Driver,
    pub needed_correcting: bool,
    pub opening: String,
    pub models: Vec<String>,
    pub out_tokens: i64,
    pub unsupervised_out_tokens: i64,
    pub working_minutes: i64,
    /// The operator's time on the turn that opened it, in seconds.
    pub operator_secs: i64,
    pub repos: Vec<String>,
    pub files_written: i64,
    pub commits: i64,
    pub lines_written: i64,
    pub lines_standing: i64,
    pub lines_replaced: i64,
    pub lines_never_landed: i64,
    pub lines_in_flight: i64,
}

/// A commit this machine made, matched or not.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct CommitRow {
    pub repo: String,
    pub hash: String,
    pub subject: String,
    pub at: String,
    /// Index into the tasks; none when no evidence settled it.
    pub task: Option<i64>,
    pub rule: Option<Rule>,
    pub written: i64,
    pub standing: i64,
    pub replaced: i64,
    pub never_landed: i64,
    pub in_flight: i64,
    /// Working minutes later tasks spent in the same repository.
    pub exposure_minutes: i64,
}

/// How much of the work the join could place, and how.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct Coverage {
    pub commits: i64,
    pub exact: i64,
    pub files: i64,
    pub time: i64,
    pub uncredited: i64,
    /// Too big to be authored work: a vendor drop or a generated purge.
    pub bulk: i64,
    /// Repositories whose main line git could not read.
    pub unreadable_repos: Vec<String>,
}

/// The whole answer of `/api/tasks`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TasksSummary {
    pub tasks: Vec<TaskRow>,
    pub commits: Vec<CommitRow>,
    pub coverage: Coverage,
    pub sittings: Vec<Sitting>,
    pub computed_at: String,
}

/// The lines of one commit, followed to their fate.
#[derive(Debug, Default, Clone, Copy)]
struct Fate {
    written: i64,
    standing: i64,
    replaced: i64,
    never_landed: i64,
    in_flight: i64,
}

/// Fates of this machine's commits in one repository: landed ones by what
/// blame still credits, branch work by the main-line commit that landed
/// each file and that commit's surviving lines split by weight, as the
/// model comparison splits them; the rest in flight or never landed.
fn fates(work: &RepoWork, ours: &dyn Fn(&str) -> bool) -> HashMap<String, Fate> {
    let mut out: HashMap<String, Fate> = HashMap::new();
    // Branch work that landed through a main-line commit.
    let mut shares: HashMap<&str, Vec<(&str, i64, i64)>> = HashMap::new();
    for c in work.off_mainline.iter().filter(|c| ours(&c.author) && c.added() + c.removed() <= crate::compare::BULK_COMMIT) {
        // Per main-line commit this one landed through: (weight, lines).
        let mut via: HashMap<&str, (i64, i64)> = HashMap::new();
        let mut rest = 0i64;
        for (path, added, removed) in &c.files {
            match work.landed_via.get(&(c.hash.clone(), path.clone())) {
                Some(v) => {
                    let e = via.entry(v.as_str()).or_default();
                    e.0 += (added + removed).max(1);
                    e.1 += added;
                }
                None => rest += added,
            }
        }
        let landed: i64 = via.values().map(|v| v.1).sum();
        for (v, (weight, lines)) in via {
            shares.entry(v).or_default().push((c.hash.as_str(), weight, lines));
        }
        let f = out.entry(c.hash.clone()).or_default();
        if work.recovered.contains(&c.hash) {
            // A reflog keeps amended and rebased-away versions too: what
            // did not land is nobody's work (#415 review), and counting it
            // as written counted an amended commit's lines twice.
            f.written += landed;
        } else if work.in_flight.contains(&c.hash) {
            f.written += c.added();
            f.in_flight += rest;
        } else {
            f.written += c.added();
            f.never_landed += rest;
        }
    }
    for (via, parts) in &shares {
        let alive = work.alive.get(*via).copied().unwrap_or(0);
        let weights: Vec<i64> = parts.iter().map(|p| p.1).collect();
        let standing = crate::compare::split(alive, &weights);
        for (k, (hash, _, landed)) in parts.iter().enumerate() {
            let f = out.entry((*hash).to_string()).or_default();
            let s = standing[k].min(*landed);
            f.standing += s;
            f.replaced += landed - s;
        }
    }
    // Main-line commits made here, that no branch work landed through.
    for c in work.landed.iter().filter(|c| ours(&c.author) && !shares.contains_key(c.hash.as_str())) {
        let added = c.added();
        let standing = work.alive.get(&c.hash).copied().unwrap_or(0).min(added);
        out.insert(c.hash.clone(), Fate { written: added, standing, replaced: added - standing, ..Fate::default() });
    }
    out
}

/// The join over every session's log. `mainlines` is the operator's
/// `attr_ui_mainline_refs`.
pub async fn join(
    logs: &[SessionLog],
    mainlines: &HashMap<String, String>,
    idle_secs: i64,
    leash_secs: i64,
    break_secs: i64,
) -> TasksSummary {
    let mut tasks: Vec<Task> = logs.iter().flat_map(|l| build_tasks(l, idle_secs, leash_secs)).collect();
    tasks.sort_by(|a, b| a.opened.cmp(&b.opened).then_with(|| a.session.cmp(&b.session)));
    let (work_turns, sittings) = operator_time(logs, break_secs);
    let operator: HashMap<(&str, DateTime<Utc>), i64> =
        work_turns.iter().map(|t| ((t.session.as_str(), t.at), t.work_secs)).collect();

    let cwds: HashSet<String> = tasks.iter().flat_map(|t| t.cwds.iter().cloned()).collect();
    let checkouts = crate::checkout::Checkouts::resolve(cwds.iter()).await;
    let spans: Vec<(DateTime<Utc>, DateTime<Utc>)> = tasks.iter().map(|t| (t.opened, t.until)).collect();
    let printed: Vec<(usize, &Printed)> =
        tasks.iter().enumerate().flat_map(|(i, t)| t.printed.iter().map(move |p| (i, p))).collect();

    // Per repository: its name, a checkout to read it from, its candidates.
    struct Repo {
        name: String,
        dir: String,
        toplevels: Vec<String>,
        earliest: DateTime<Utc>,
        tasks: Vec<usize>,
    }
    let mut repos: HashMap<String, Repo> = HashMap::new();
    let mut task_repos: Vec<Vec<String>> = vec![Vec::new(); tasks.len()];
    for (i, t) in tasks.iter().enumerate() {
        for cwd in &t.cwds {
            let Some(co) = checkouts.of(cwd) else { continue };
            let r = repos.entry(co.common.clone()).or_insert_with(|| Repo {
                name: co.repo.clone(),
                dir: co.toplevel.clone(),
                toplevels: Vec::new(),
                earliest: t.opened,
                tasks: Vec::new(),
            });
            r.earliest = r.earliest.min(t.opened);
            if !r.toplevels.contains(&co.toplevel) {
                r.toplevels.push(co.toplevel.clone());
            }
            if r.tasks.last() != Some(&i) {
                r.tasks.push(i);
            }
            if !task_repos[i].contains(&co.repo) {
                task_repos[i].push(co.repo.clone());
            }
        }
    }

    let mut coverage = Coverage::default();
    let mut commits: Vec<CommitRow> = Vec::new();
    let mut keys: Vec<&String> = repos.keys().collect();
    keys.sort_by_key(|k| &repos[*k].name);
    for key in keys {
        let r = &repos[key];
        let dir = std::path::Path::new(&r.dir);
        let mainline = crate::landed::mainline_of(dir, &r.name, mainlines).await;
        let Some(work) = crate::landed::repo_work(dir, &mainline, Some(r.earliest)).await else {
            coverage.unreadable_repos.push(r.name.clone());
            continue;
        };
        let identity = work.identity.clone();
        let ours = move |author: &str| identity.as_deref() == Some(author);
        let tops: Vec<&str> = r.toplevels.iter().map(String::as_str).collect();
        let candidates: Vec<Candidate> = r
            .tasks
            .iter()
            .map(|&i| Candidate {
                task: i,
                opened: tasks[i].opened,
                until: tasks[i].until,
                files: tasks[i].files.iter().filter_map(|p| relative(p, &tops)).map(str::to_string).collect(),
            })
            .collect();
        let fate = fates(&work, &ours);
        let mut seen: HashSet<&str> = HashSet::new();
        for c in work.landed.iter().chain(work.off_mainline.iter()) {
            let Some(f) = fate.get(&c.hash) else { continue };
            if !seen.insert(c.hash.as_str()) {
                continue;
            }
            coverage.commits += 1;
            if c.added() + c.removed() > crate::compare::BULK_COMMIT {
                coverage.bulk += 1;
                continue;
            }
            let found = match_commit(c, &printed, &candidates, &spans, idle_secs);
            match found.map(|f| f.1) {
                Some(Rule::Exact) => coverage.exact += 1,
                Some(Rule::Files) => coverage.files += 1,
                Some(Rule::Time) => coverage.time += 1,
                None => coverage.uncredited += 1,
            }
            let exposure: i64 =
                candidates.iter().filter(|k| k.opened > c.at).map(|k| tasks[k.task].working_secs).sum::<i64>() / 60;
            commits.push(CommitRow {
                repo: r.name.clone(),
                hash: c.hash.chars().take(10).collect(),
                subject: c.subject.clone(),
                at: c.at.to_rfc3339(),
                task: found.map(|f| f.0 as i64),
                rule: found.map(|f| f.1),
                written: f.written,
                standing: f.standing,
                replaced: f.replaced,
                never_landed: f.never_landed,
                in_flight: f.in_flight,
                exposure_minutes: exposure,
            });
        }
    }
    commits.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.hash.cmp(&b.hash)));

    let mut rows: Vec<TaskRow> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| TaskRow {
            session: t.session.clone(),
            opened_at: t.opened.to_rfc3339(),
            driver: t.driver,
            needed_correcting: t.needed_correcting,
            opening: t.opening.clone(),
            models: t.models.clone(),
            out_tokens: t.out_tokens,
            unsupervised_out_tokens: t.unsupervised_out,
            working_minutes: t.working_secs / 60,
            operator_secs: operator.get(&(t.session.as_str(), t.opened)).copied().unwrap_or(0),
            repos: task_repos[i].clone(),
            files_written: t.files.len() as i64,
            commits: 0,
            lines_written: 0,
            lines_standing: 0,
            lines_replaced: 0,
            lines_never_landed: 0,
            lines_in_flight: 0,
        })
        .collect();
    for c in &commits {
        let Some(i) = c.task.and_then(|i| usize::try_from(i).ok()) else { continue };
        let row = &mut rows[i];
        row.commits += 1;
        row.lines_written += c.written;
        row.lines_standing += c.standing;
        row.lines_replaced += c.replaced;
        row.lines_never_landed += c.never_landed;
        row.lines_in_flight += c.in_flight;
    }
    TasksSummary { tasks: rows, commits, coverage, sittings, computed_at: Utc::now().to_rfc3339() }
}

/// Resolve a positive seconds parameter on the ui module, else `default`.
async fn resolved_secs(kernel: &Kernel, param: &str, default: i64) -> i64 {
    match crate::module_parameter(kernel, param).await {
        Some(Value::Number(n)) => n.to_int().filter(|&v| v > 0).unwrap_or(default),
        _ => default,
    }
}

/// The leash: how long after a turn the work still counts as steered.
pub async fn resolved_leash_secs(kernel: &Kernel) -> i64 {
    resolved_secs(kernel, LEASH_SECS_PARAM, DEFAULT_LEASH_SECS).await
}

/// The break threshold: the longest gap between turns that is still work.
pub async fn resolved_break_secs(kernel: &Kernel) -> i64 {
    resolved_secs(kernel, BREAK_SECS_PARAM, DEFAULT_BREAK_SECS).await
}

/// Everything `/api/tasks` serves, read and joined.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn tasks(kernel: &Kernel) -> Result<TasksSummary> {
    let logs = session_logs(kernel).await?;
    let mainlines = crate::resolved_mainline_refs(kernel).await;
    let idle = crate::stats::resolved_active_secs(kernel).await;
    let leash = resolved_leash_secs(kernel).await;
    let brk = resolved_break_secs(kernel).await;
    Ok(join(&logs, &mainlines, idle, leash, brk).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).expect("time").with_timezone(&Utc)
    }

    fn reply(at: &str, model: &str, out: i64) -> LoggedReply {
        LoggedReply { at: t(at), model: model.to_string(), cwd: Some("/r".to_string()), out_tokens: out }
    }

    #[test]
    fn a_hash_is_a_run_of_hex_with_its_subject_after() {
        let found = printed_hashes("[main 1a2b3c4] Fix the thing\n9f8e7d6c5b feat: more\nnot deadbee here, nor 123456");
        let hashes: Vec<&str> = found.iter().map(|(h, _)| h.as_str()).collect();
        assert_eq!(hashes, vec!["1a2b3c4", "9f8e7d6c5b"], "a word or a short number is not one");
        assert!(found[0].1.starts_with("] fix the thing"), "the text after it, lower-cased");
    }

    #[test]
    fn a_hash_inside_a_word_is_not_one() {
        assert!(printed_hashes("sha=abc1234def0x and id_1a2b3c4d").iter().all(|(h, _)| h != "abc1234def0"));
        assert!(printed_hashes("x1a2b3c4d").is_empty(), "preceded by a letter");
        assert!(printed_hashes("café 1a2b3c4 ok").iter().any(|(h, a)| h == "1a2b3c4" && a == " ok"));
    }

    #[test]
    fn each_turn_opens_a_task_and_a_correction_marks_the_one_before() {
        let log = SessionLog {
            name: "s".into(),
            turns: vec![
                (t("2026-10-01T10:00:00Z"), "build the join".into()),
                (t("2026-10-01T11:00:00Z"), "stop, that is wrong".into()),
            ],
            replies: vec![
                reply("2026-10-01T10:01:00Z", "opus 5.5", 100),
                reply("2026-10-01T10:45:00Z", "opus 5.5", 50),
                reply("2026-10-01T11:02:00Z", "fable 5.1", 10),
            ],
            ..SessionLog::default()
        };
        let tasks = build_tasks(&log, 300, 1_800);
        assert_eq!(tasks.len(), 2);
        assert_eq!((tasks[0].driver, tasks[0].needed_correcting), (Driver::Directed, true));
        assert_eq!((tasks[1].driver, tasks[1].needed_correcting), (Driver::Corrected, false));
        assert_eq!(tasks[0].out_tokens, 150);
        assert_eq!(tasks[0].unsupervised_out, 50, "the reply 45 minutes in is past a 30-minute leash");
        assert_eq!(tasks[0].working_secs, 60 + 300, "gaps capped at the idle threshold");
        assert_eq!(tasks[0].until, t("2026-10-01T11:00:00Z"));
        assert_eq!(tasks[1].models, vec!["fable 5.1".to_string()]);
    }

    #[test]
    fn a_session_with_no_turns_is_unsupervised_work() {
        let log = SessionLog {
            name: "agent".into(),
            replies: vec![reply("2026-10-01T10:00:00Z", "haiku 5.5", 7)],
            ..SessionLog::default()
        };
        let tasks = build_tasks(&log, 300, 1_800);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].driver, Driver::Unsupervised);
        assert_eq!(tasks[0].unsupervised_out, 7);
        assert_eq!(tasks[0].until, t("2026-10-01T10:05:00Z"), "last activity plus the idle threshold");
    }

    #[test]
    fn operator_time_counts_each_gap_once_and_breaks_into_sittings() {
        let a = SessionLog {
            name: "a".into(),
            turns: vec![(t("2026-10-01T09:00:00Z"), "go".into()), (t("2026-10-01T09:20:00Z"), "next".into())],
            replies: vec![reply("2026-10-01T09:00:30Z", "opus 5", 1)],
            ..SessionLog::default()
        };
        let b = SessionLog {
            name: "b".into(),
            // In parallel with a, then after a two-hour break.
            turns: vec![(t("2026-10-01T09:10:00Z"), "and this".into()), (t("2026-10-01T11:30:00Z"), "back".into())],
            ..SessionLog::default()
        };
        let (turns, sittings) = operator_time(&[a, b], 1_800);
        let secs: Vec<i64> = turns.iter().map(|w| w.work_secs).collect();
        assert_eq!(secs, vec![0, 600, 600, 1_800], "gaps across sessions, the break capped");
        assert_eq!(turns[0].model.as_deref(), Some("opus 5"));
        assert_eq!(sittings.len(), 2);
        assert_eq!((sittings[0].turns, sittings[0].work_minutes), (3, 20));
        assert_eq!(sittings[1].turns, 1);
    }

    fn commit(hash: &str, at: &str, subject: &str, files: &[&str]) -> WorkCommit {
        WorkCommit {
            hash: hash.to_string(),
            at: t(at),
            author: "t@t".into(),
            subject: subject.to_string(),
            files: files.iter().map(|f| ((*f).to_string(), 3, 0)).collect(),
            binary: Vec::new(),
        }
    }

    fn cand(task: usize, from: &str, to: &str, files: &[&str]) -> Candidate {
        Candidate { task, opened: t(from), until: t(to), files: files.iter().map(|f| (*f).to_string()).collect() }
    }

    #[test]
    fn the_print_of_a_commit_beats_the_clock() {
        let c = commit(&"ab12cd3".repeat(6)[..40], "2026-10-01T10:30:00Z", "fix: the join", &["a.rs"]);
        let spans = vec![
            (t("2026-10-01T10:00:00Z"), t("2026-10-01T11:00:00Z")),
            (t("2026-10-01T10:20:00Z"), t("2026-10-01T10:40:00Z")),
        ];
        let p = Printed { at: t("2026-10-01T10:30:05Z"), hash: "ab12cd3".into(), after: "] fix: the join".into() };
        let candidates = vec![cand(0, "2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z", &[]),
                              cand(1, "2026-10-01T10:20:00Z", "2026-10-01T10:40:00Z", &[])];
        assert_eq!(match_commit(&c, &[(1, &p)], &candidates, &spans, 300), Some((1, Rule::Exact)),
                   "two tasks were open; the one that printed it made it");
        // The same hash printed by a `git log` a day later, elsewhere, does not count.
        let later = Printed { at: t("2026-10-02T10:00:00Z"), hash: "ab12cd3".into(), after: " fix: the join".into() };
        let spans3 = vec![spans[0], spans[1], (t("2026-10-02T09:00:00Z"), t("2026-10-02T11:00:00Z"))];
        assert_eq!(match_commit(&c, &[(2, &later)], &candidates, &spans3, 300), None,
                   "printed long after, by another task, and two tasks open: left uncredited");
    }

    #[test]
    fn files_settle_what_time_cannot_and_a_tie_settles_nothing() {
        let c = commit(&"e".repeat(40), "2026-10-01T10:30:00Z", "feat: x", &["src/a.rs", "src/b.rs"]);
        let spans = vec![(t("2026-10-01T10:00:00Z"), t("2026-10-01T11:00:00Z")); 2];
        let both = vec![cand(0, "2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z", &["src/a.rs"]),
                        cand(1, "2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z", &["src/a.rs", "src/b.rs"])];
        assert_eq!(match_commit(&c, &[], &both, &spans, 300), Some((1, Rule::Files)));
        let tie = vec![cand(0, "2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z", &["src/a.rs"]),
                       cand(1, "2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z", &["src/b.rs"])];
        assert_eq!(match_commit(&c, &[], &tie, &spans, 300), None);
        let alone = vec![cand(0, "2026-10-01T10:00:00Z", "2026-10-01T11:00:00Z", &[])];
        assert_eq!(match_commit(&c, &[], &alone, &spans[..1], 300), Some((0, Rule::Time)));
    }

    #[test]
    fn an_amended_away_version_is_not_written_twice() {
        // `old` was amended into `new`; the branch was squashed as `sq`.
        // The reflog still holds `old`.
        let old = commit(&"1".repeat(40), "2026-10-01T10:00:00Z", "fix: x", &["a.rs"]);
        let new = commit(&"2".repeat(40), "2026-10-01T10:00:00Z", "fix: x", &["a.rs"]);
        let mut sq = commit(&"3".repeat(40), "2026-10-01T12:00:00Z", "fix: x (#1)", &["a.rs"]);
        sq.author = "github@noreply".into();
        let mut work = RepoWork { identity: Some("t@t".into()), ..RepoWork::default() };
        work.landed.push(sq.clone());
        work.off_mainline.extend([old.clone(), new.clone()]);
        work.recovered.insert(old.hash.clone());
        work.landed_via.insert((new.hash.clone(), "a.rs".into()), sq.hash.clone());
        work.alive.insert(sq.hash.clone(), 2);
        let f = fates(&work, &|a: &str| a == "t@t");
        assert_eq!((f[&new.hash].written, f[&new.hash].standing, f[&new.hash].replaced), (3, 2, 1));
        assert_eq!((f[&old.hash].written, f[&old.hash].never_landed), (0, 0), "nobody's work");
        assert!(!f.contains_key(&sq.hash), "the squash is the branch's work, credited there");
    }

    #[test]
    fn a_path_is_relative_to_the_checkout_it_lies_under() {
        let tops = ["/w/superx", "/w/superx/.claude/worktrees/x"];
        assert_eq!(relative("/w/superx/.claude/worktrees/x/src/a.rs", &tops), Some("src/a.rs"));
        assert_eq!(relative("/w/superx/src/a.rs", &tops), Some("src/a.rs"));
        assert_eq!(relative("/w/superxyz/a.rs", &tops), None);
    }
}
