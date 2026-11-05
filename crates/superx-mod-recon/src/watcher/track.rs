//! One session as the watcher follows it (#573): where every second of its
//! time goes, hour by hour, by model, and on what (the tool and what it was
//! called on); every call it made as a run, with its start, its end and how
//! it ended; the jobs it has running; its lines, as what is found in them is
//! read back to its cause; and the stop of the operator's it has not
//! answered yet.
//!
//! Every gap between two of its lines is given to what ended it, spread over
//! the hours it spans: a thought to thinking, on what the agent did next;
//! words to writing, on its words; a call to writing, on the call; a call's
//! result to the tool's run; a background job's notice to waiting on that
//! job; and a message of the operator's to waiting on the operator, or on
//! the jobs that ran through the wait. A foreground call cannot run past
//! Claude Code's command limit, so a longer wait for its result was the
//! operator's: a permission prompt, or a machine that slept. A reply does
//! not take a quarter of an hour, so a longer gap before one is a session
//! left, and also the operator's.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use serde::Serialize;

use super::command;
use super::line::{Came, Happened, Reading, ASK_TOOLS};
use super::plan;
use crate::params::Params;

/// The longest a foreground call runs: Claude Code's 10-minute command
/// limit, and a minute for the call to arrive and its result to be written.
pub const FOREGROUND_LIMIT_SECS: i64 = 660; // skill-allow: §9-const — Claude Code's own command limit, not a policy tunable

/// How long a background job with no notice yet counts as running: past it,
/// an end that was never seen (a shell gone, a session closed) is likelier.
pub const UNSEEN_END_SECS: i64 = 14_400; // skill-allow: §9-const — how long an unseen job is trusted to run, not a policy tunable

/// The longest a reply takes to arrive.
const REPLY_LIMIT_SECS: i64 = 900; // skill-allow: §9-const — no model reply takes this long; past it the session was left

/// The most of the agent's words in one turn kept to judge what the
/// operator's next message answered.
const TURN_WORDS_CHARS: usize = 20_000; // skill-allow: §9-const — memory bound of a turn's words, not a policy tunable

/// Time no call held, as the tool it is counted under.
pub const WORDS: &str = "(its words)";
pub const YOUR_MESSAGE: &str = "(your next message)";
pub const THINKING: &str = "(thinking, nothing after)";
pub const EARLIER_CALL: &str = "(a call before the window)";

/// Where a session's time went, in seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Split {
    pub thinking: i64,
    pub writing: i64,
    /// Calls running in the foreground.
    pub tools: i64,
    /// The agent waiting on its background jobs.
    pub jobs: i64,
    /// Waiting on the operator's next message or answer.
    pub operator: i64,
    /// Waiting on the operator's approval of a call.
    pub approval: i64,
}

impl Split {
    pub fn plus(&mut self, o: &Self) {
        self.thinking += o.thinking;
        self.writing += o.writing;
        self.tools += o.tools;
        self.jobs += o.jobs;
        self.operator += o.operator;
        self.approval += o.approval;
    }

    fn add(&mut self, spent: Spent, secs: i64) {
        match spent {
            Spent::Thinking => self.thinking += secs,
            Spent::Writing => self.writing += secs,
            Spent::Tools => self.tools += secs,
            Spent::Jobs => self.jobs += secs,
            Spent::Operator => self.operator += secs,
            Spent::Approval => self.approval += secs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spent {
    Thinking,
    Writing,
    Tools,
    Jobs,
    Operator,
    Approval,
}

/// What a stretch of a session's time went on: the tool, and what it was
/// called on ([`super::line`]'s target; a command's [`command::label`]; a
/// job's [`plan::label`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct On {
    pub tool: String,
    pub target: String,
}

impl On {
    fn of(run: &Run) -> Self {
        Self { tool: run.tool.clone(), target: run.target.clone() }
    }

    fn bare(tool: &str) -> Self {
        Self { tool: tool.to_string(), target: String::new() }
    }
}

/// A place in the ledger: the hour, the model at work, what the time was on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub hour: DateTime<Utc>,
    pub model: Option<String>,
    pub on: On,
}

/// The time spent at a place in the ledger, and the calls made there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Spend {
    pub split: Split,
    pub calls: u32,
    pub failed: u32,
}

/// What a session did in an hour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub prompts: u32,
    pub calls: u32,
    pub launches: u32,
}

/// One call, as a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub call: String,
    pub tool: String,
    pub what: String,
    /// What it works on, as its time is counted ([`On`]).
    pub target: String,
    /// What the agent said the call does, when it said.
    pub about: Option<String>,
    /// The job it launched ([`plan::launch`]); a background command's
    /// whole text when it launched none.
    pub launch: Option<String>,
    pub path: Option<String>,
    /// The part of its file a Read read.
    pub part: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub background: bool,
    pub failed: bool,
    /// How its background job ended, as the notice says.
    pub status: Option<String>,
}

impl Run {
    /// How long it ran, or has run by `now`.
    #[must_use]
    pub fn secs(&self, now: DateTime<Utc>) -> i64 {
        (self.end.unwrap_or(now) - self.start).num_seconds().max(0)
    }

    /// Whether it ran to its end and did not fail.
    #[must_use]
    pub fn ended_well(&self) -> bool {
        self.end.is_some() && !self.failed && self.status.as_deref().is_none_or(|s| s == "completed")
    }

    /// The run as the page names it: the job it launched, else what the
    /// agent said it does, else the start of what it was called on.
    #[must_use]
    pub fn label(&self, jobs: &[String]) -> String {
        match (&self.launch, &self.about) {
            (Some(l), _) if plan::launch(&self.what, jobs).is_some() => plan::label(l, jobs),
            (_, Some(about)) => about.clone(),
            _ => self.what.lines().next().unwrap_or_default().chars().take(120).collect(),
        }
    }

    /// Whether its time is its own: a background job, or a foreground call
    /// within the command limit.
    #[must_use]
    pub fn measured(&self, now: DateTime<Utc>) -> bool {
        self.background || self.secs(now) <= FOREGROUND_LIMIT_SECS
    }
}

/// What a line of a session is, as its cause is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// The operator's message, a `!` command or a slash command.
    You,
    Thought,
    Said,
    Call,
    /// A call that failed, with its error.
    Failed,
    /// A background job's notice that it ended.
    JobEnded,
    Compacted,
}

/// One line of a session, kept for the window: what it was, and its words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub at: DateTime<Utc>,
    pub kind: StepKind,
    pub text: String,
    pub tool: Option<String>,
    /// The call it is or ends, by which the page shows the run.
    pub call: Option<String>,
}

/// What feeding a line told the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fed {
    /// A run started: its index in [`Track::runs`].
    Started(usize),
    /// A Write's whole text, which is the file's: a plan is read from it.
    Wrote(usize, String),
    /// A run ended: with its result's text, or none when a notice or a
    /// stop ended it.
    Ended(usize, Option<String>),
    /// A message of the operator's, and when the one before it came.
    Prompt { came: Came, stop: bool, text: String, previous: Option<DateTime<Utc>> },
    Said(String),
    Compacted,
    /// The agent's turn ended: the operator is to answer.
    TurnEnded,
}

/// One session, as the watcher follows it.
#[derive(Debug, Clone)]
pub struct Track {
    /// The kernel's session entity, by its UUID.
    pub session: String,
    pub name: String,
    pub key: String,
    pub model: Option<String>,
    pub folder: Option<String>,
    pub first: DateTime<Utc>,
    pub newest: DateTime<Utc>,
    prev: Option<DateTime<Utc>>,
    /// Where its time went: by hour, model, and what it was on.
    pub ledger: BTreeMap<Key, Spend>,
    /// What it did, by hour.
    pub counts: BTreeMap<DateTime<Utc>, Counts>,
    /// Thinking not given yet to what the agent did after it: each hour's
    /// part, and the model.
    pending: Vec<(DateTime<Utc>, Option<String>, i64)>,
    pub runs: Vec<Run>,
    open: HashMap<String, usize>,
    /// Background jobs by Claude Code's id for them.
    jobs: HashMap<String, usize>,
    /// Its lines in the window.
    pub steps: Vec<Step>,
    /// The operator's stop the agent has not answered with words yet.
    pub stop: Option<(DateTime<Utc>, String)>,
    /// When the operator's newest message came.
    pub last_prompt: Option<DateTime<Utc>>,
    /// Its newest turn ended, and no message of the operator's came since.
    pub turn_over: bool,
    /// The agent thought, said or called since its turn last ended: a turn
    /// a job's notice woke ends again with no message between.
    acting: bool,
    /// The operator's newest message, and the agent's words in the turn it
    /// answered, lowercase: what the operator asked for.
    pub asked: String,
    /// The agent's words since the operator's newest message, lowercase.
    turn_words: String,
}

impl Track {
    #[must_use]
    pub fn new(session: &str, name: &str, key: &str, at: DateTime<Utc>) -> Self {
        Self {
            session: session.to_string(),
            name: name.to_string(),
            key: key.to_string(),
            model: None,
            folder: None,
            first: at,
            newest: at,
            prev: None,
            ledger: BTreeMap::new(),
            counts: BTreeMap::new(),
            pending: Vec::new(),
            runs: Vec::new(),
            open: HashMap::new(),
            jobs: HashMap::new(),
            steps: Vec::new(),
            stop: None,
            last_prompt: None,
            turn_over: false,
            acting: false,
            asked: String::new(),
            turn_words: String::new(),
        }
    }

    /// Feed one line, written at `at`.
    pub fn feed(&mut self, at: DateTime<Utc>, reading: Reading, p: &Params) -> Vec<Fed> {
        if reading.model.is_some() {
            self.model.clone_from(&reading.model);
        }
        if reading.folder.is_some() {
            self.folder.clone_from(&reading.folder);
        }
        self.newest = self.newest.max(at);
        let mut fed = Vec::new();
        for h in reading.happened {
            match h {
                Happened::Thought { text } => {
                    self.acting = true;
                    self.spend(at, Spent::Thinking, On::bare(THINKING));
                    self.step(at, StepKind::Thought, text, None, None);
                }
                Happened::Said { text } => {
                    self.acting = true;
                    self.spend(at, Spent::Writing, On::bare(WORDS));
                    self.settle_thinking(&On::bare(WORDS));
                    self.stop = None;
                    self.heard(&text);
                    self.step(at, StepKind::Said, text.clone(), None, None);
                    fed.push(Fed::Said(text));
                }
                Happened::Call { id, tool, what, about, path, target, part, background, content, stops } => {
                    self.acting = true;
                    let launch = if tool == "Bash" {
                        plan::launch(&what, &p.watch_jobs).or_else(|| background.then(|| what.split_whitespace().collect::<Vec<_>>().join(" ")))
                    } else {
                        None
                    };
                    let target = if tool == "Bash" {
                        match &launch {
                            Some(l) if plan::launch(&what, &p.watch_jobs).is_some() => plan::titled(l, &p.watch_jobs),
                            _ => command::label(&what),
                        }
                    } else {
                        target
                    };
                    let run = Run {
                        call: id.clone(),
                        tool,
                        what,
                        target,
                        about,
                        launch,
                        path,
                        part,
                        start: at,
                        end: None,
                        background,
                        failed: false,
                        status: None,
                    };
                    let on = On::of(&run);
                    self.spend(at, Spent::Writing, on.clone());
                    self.settle_thinking(&on);
                    // A job stopped by the agent ended there.
                    if let Some(i) = stops.as_ref().and_then(|j| self.jobs.get(j)).copied() {
                        let stopped = &mut self.runs[i];
                        if stopped.end.is_none() {
                            stopped.end = Some(at);
                            stopped.status = Some("killed".to_string());
                            let call = stopped.call.clone();
                            self.open.remove(&call);
                            fed.push(Fed::Ended(i, None));
                        }
                    }
                    if ASK_TOOLS.contains(&run.tool.as_str()) {
                        self.heard(&run.what);
                    }
                    let model = self.model.clone();
                    self.ledger.entry(Key { hour: hour_of(at), model, on }).or_default().calls += 1;
                    let counts = self.counts.entry(hour_of(at)).or_default();
                    counts.calls += 1;
                    counts.launches += u32::from(run.launch.is_some());
                    self.step(at, StepKind::Call, run.what.clone(), Some(run.tool.clone()), Some(id.clone()));
                    let i = self.runs.len();
                    self.runs.push(run);
                    self.open.insert(id, i);
                    fed.push(Fed::Started(i));
                    if let Some(content) = content {
                        fed.push(Fed::Wrote(i, content));
                    }
                }
                Happened::Result { call, failed, job, text } => {
                    let Some(&i) = self.open.get(&call) else {
                        self.spend(at, Spent::Tools, On::bare(EARLIER_CALL));
                        self.settle_thinking(&On::bare(THINKING));
                        continue;
                    };
                    let run = &self.runs[i];
                    let gap = self.prev.map_or(0, |p| (at - p).num_seconds());
                    let spent = if ASK_TOOLS.contains(&run.tool.as_str()) {
                        Spent::Operator
                    } else if !run.background && gap > FOREGROUND_LIMIT_SECS {
                        Spent::Approval
                    } else {
                        Spent::Tools
                    };
                    let on = On::of(run);
                    self.spend(at, spent, on.clone());
                    self.settle_thinking(&On::bare(THINKING));
                    if failed {
                        let model = self.model.clone();
                        self.ledger.entry(Key { hour: hour_of(at), model, on }).or_default().failed += 1;
                        let tool = self.runs[i].tool.clone();
                        self.step(at, StepKind::Failed, super::line::kept(&text), Some(tool), Some(call.clone()));
                    }
                    let run = &mut self.runs[i];
                    // A job started in the background runs on until its notice.
                    if run.background && !failed {
                        if let Some(job) = job {
                            self.jobs.insert(job, i);
                            continue;
                        }
                    }
                    run.end = Some(at);
                    run.failed = failed;
                    self.open.remove(&call);
                    fed.push(Fed::Ended(i, Some(text)));
                }
                Happened::JobEnded { call, status, summary } => {
                    // Only a job of its own that this notice ends was waited
                    // on; any other notice (a watch's, a subagent's) ends a
                    // wait that was the operator's.
                    let i = call.as_ref().and_then(|c| self.open.get(c).copied()).filter(|&i| self.runs[i].background);
                    let span = i.map(|i| (self.runs[i].start, at, On::of(&self.runs[i])));
                    self.spend_waiting(at, span.into_iter().collect());
                    self.settle_thinking(&On::bare(THINKING));
                    let words = [status.clone(), summary].into_iter().flatten().collect::<Vec<_>>().join(": ");
                    self.step(at, StepKind::JobEnded, words, None, call);
                    if let Some(i) = i {
                        let run = &mut self.runs[i];
                        run.end = Some(at);
                        run.status = status;
                        let call = run.call.clone();
                        self.open.remove(&call);
                        fed.push(Fed::Ended(i, None));
                    }
                }
                Happened::Prompt { came, text } => {
                    let running: Vec<(DateTime<Utc>, DateTime<Utc>, On)> = self
                        .runs
                        .iter()
                        .filter(|r| r.background)
                        .map(|r| (r.start, r.end.unwrap_or(r.start + TimeDelta::seconds(UNSEEN_END_SECS)), On::of(r)))
                        .collect();
                    self.spend_waiting(at, running);
                    self.settle_thinking(&On::bare(THINKING));
                    self.step(at, StepKind::You, text.clone(), None, None);
                    if matches!(came, Came::Shell | Came::Command) {
                        continue;
                    }
                    self.counts.entry(hour_of(at)).or_default().prompts += 1;
                    self.turn_over = false;
                    self.asked = format!("{} {}", text.to_lowercase(), std::mem::take(&mut self.turn_words));
                    let stop = came == Came::Interrupt || starts_with_any(&text, &p.watch_stop_words);
                    self.stop = stop.then(|| (at, text.clone()));
                    let previous = self.last_prompt.replace(at);
                    fed.push(Fed::Prompt { came, stop, text, previous });
                }
                Happened::TurnEnded => {
                    self.settle_thinking(&On::bare(THINKING));
                    if std::mem::take(&mut self.acting) {
                        fed.push(Fed::TurnEnded);
                    }
                    self.turn_over = true;
                }
                Happened::Compacted => {
                    self.step(at, StepKind::Compacted, String::new(), None, None);
                    fed.push(Fed::Compacted);
                }
            }
        }
        fed
    }

    fn step(&mut self, at: DateTime<Utc>, kind: StepKind, text: String, tool: Option<String>, call: Option<String>) {
        self.steps.push(Step { at, kind, text, tool, call });
    }

    /// The agent's words, heard as part of the turn the operator answers
    /// next.
    fn heard(&mut self, words: &str) {
        let room = TURN_WORDS_CHARS.saturating_sub(self.turn_words.len());
        if room > 0 {
            self.turn_words.push(' ');
            self.turn_words.extend(words.chars().take(room).flat_map(char::to_lowercase));
        }
    }

    /// The gap since the line before, given to what ended it. Thinking waits
    /// for what the agent does after it.
    fn spend(&mut self, at: DateTime<Utc>, spent: Spent, on: On) {
        let Some(prev) = self.prev else {
            self.prev = Some(at);
            return;
        };
        if at > prev {
            let gap = (at - prev).num_seconds();
            let (spent, on) = if matches!(spent, Spent::Thinking | Spent::Writing) && gap > REPLY_LIMIT_SECS {
                (Spent::Operator, On::bare(YOUR_MESSAGE))
            } else {
                (spent, on)
            };
            if spent == Spent::Thinking {
                let model = self.model.clone();
                self.pending.extend(by_hour(prev, prev, at).into_iter().map(|(h, s)| (h, model.clone(), s)));
            } else {
                self.give(prev, prev, at, spent, &on);
            }
        }
        self.prev = Some(prev.max(at));
    }

    /// The seconds from `from` to `to`, counted from `base`, to `spent` on
    /// `on`, hour by hour.
    fn give(&mut self, base: DateTime<Utc>, from: DateTime<Utc>, to: DateTime<Utc>, spent: Spent, on: &On) {
        for (hour, secs) in by_hour(base, from, to) {
            self.ledger.entry(Key { hour, model: self.model.clone(), on: on.clone() }).or_default().split.add(spent, secs);
        }
    }

    /// The thinking not given yet, given to what came after it.
    fn settle_thinking(&mut self, on: &On) {
        for (hour, model, secs) in std::mem::take(&mut self.pending) {
            self.ledger.entry(Key { hour, model, on: on.clone() }).or_default().split.add(Spent::Thinking, secs);
        }
    }

    /// A wait that ended at `at`: each stretch of it some job of `running`
    /// (each from its start to its notice, or [`UNSEEN_END_SECS`] past its
    /// start when none came) ran through was a wait on that job, the one
    /// started first where several ran; the rest the operator's.
    fn spend_waiting(&mut self, at: DateTime<Utc>, running: Vec<(DateTime<Utc>, DateTime<Utc>, On)>) {
        let Some(prev) = self.prev else {
            self.prev = Some(at);
            return;
        };
        if at > prev {
            let mut cuts: Vec<DateTime<Utc>> = vec![prev, at];
            cuts.extend(running.iter().flat_map(|(a, b, _)| [*a, *b]).filter(|t| *t > prev && *t < at));
            cuts.sort();
            cuts.dedup();
            for w in cuts.windows(2) {
                let (from, to) = (w[0], w[1]);
                match running.iter().filter(|(a, b, _)| *a <= from && *b >= to).min_by_key(|(a, _, _)| *a) {
                    Some((_, _, on)) => self.give(prev, from, to, Spent::Jobs, on),
                    None => self.give(prev, from, to, Spent::Operator, &On::bare(YOUR_MESSAGE)),
                }
            }
        }
        self.prev = Some(prev.max(at));
    }

    /// The runs still running.
    pub fn running(&self) -> impl Iterator<Item = &Run> {
        let mut open: Vec<usize> = self.open.values().copied().collect();
        open.sort_unstable();
        open.into_iter().map(|i| &self.runs[i])
    }

    /// The run of a call, by its id.
    #[must_use]
    pub fn run_of(&self, call: &str) -> Option<&Run> {
        self.runs.iter().rev().find(|r| r.call == call)
    }

    /// Forget what ended before `from`.
    pub fn trim(&mut self, from: DateTime<Utc>) {
        self.ledger.retain(|k, _| k.hour + TimeDelta::hours(1) > from);
        self.counts.retain(|h, _| *h + TimeDelta::hours(1) > from);
        self.steps.retain(|s| s.at >= from);
        let keep: Vec<bool> = self.runs.iter().map(|r| r.end.is_none_or(|e| e >= from)).collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        let mut index = HashMap::new();
        let mut runs = Vec::new();
        for (i, run) in std::mem::take(&mut self.runs).into_iter().enumerate() {
            if keep[i] {
                index.insert(i, runs.len());
                runs.push(run);
            }
        }
        self.runs = runs;
        self.open = self.open.iter().filter_map(|(c, i)| index.get(i).map(|j| (c.clone(), *j))).collect();
        self.jobs = self.jobs.iter().filter_map(|(c, i)| index.get(i).map(|j| (c.clone(), *j))).collect();
    }
}

/// The hour `at` falls in.
#[must_use]
pub fn hour_of(at: DateTime<Utc>) -> DateTime<Utc> {
    at.duration_trunc(TimeDelta::hours(1)).unwrap_or(at)
}

/// The seconds of `from..to` in each hour it spans, each counted from
/// `base`: stretches cut from one gap at any points sum to its whole
/// seconds.
fn by_hour(base: DateTime<Utc>, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<(DateTime<Utc>, i64)> {
    let secs = |t: DateTime<Utc>| (t - base).num_seconds();
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let hour = hour_of(at);
        let next = (hour + TimeDelta::hours(1)).min(to);
        let s = secs(next) - secs(at);
        if s > 0 {
            out.push((hour, s));
        }
        at = next;
    }
    out
}

/// Whether `text` starts with one of `words`, as a whole word.
#[must_use]
pub fn starts_with_any(text: &str, words: &[String]) -> bool {
    let t = text.trim_start().to_lowercase();
    words.iter().map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).any(|w| {
        t.strip_prefix(&w).is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '\''))
    })
}
