//! One session as the watcher follows it (#573): where its time goes, hour
//! by hour; every call it made as a run, with its start, its end and how it
//! ended; the jobs it has running; and the stop of the operator's it has not
//! answered yet.
//!
//! Every gap between two of its lines is given to what ended it: a thought
//! to thinking, words or a call to writing, a call's result to the tool's
//! run, a background job's notice to waiting on jobs, and a message of the
//! operator's to waiting on the operator, or on a job when one was running.
//! A foreground call cannot run past Claude Code's command limit, so a
//! longer wait for its result was the operator's: a permission prompt, or a
//! machine that slept. A reply does not take a quarter of an hour, so a
//! longer gap before one is a session left, and also the operator's.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use serde::Serialize;

use super::line::{Came, Happened, Reading, ASK_TOOLS};
use super::plan;

/// The longest a foreground call runs: Claude Code's 10-minute command
/// limit, and a minute for the call to arrive and its result to be written.
pub const FOREGROUND_LIMIT_SECS: i64 = 660; // skill-allow: §9-const — Claude Code's own command limit, not a policy tunable

/// How long a background job with no notice yet counts as running: past it,
/// an end that was never seen (a shell gone, a session closed) is likelier.
pub const UNSEEN_END_SECS: i64 = 14_400; // skill-allow: §9-const — how long an unseen job is trusted to run, not a policy tunable

/// The longest a reply takes to arrive.
const REPLY_LIMIT_SECS: i64 = 900; // skill-allow: §9-const — no model reply takes this long; past it the session was left

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

/// What a session did in an hour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub prompts: u32,
    pub calls: u32,
    pub launches: u32,
}

/// An hour of a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Hour {
    pub split: Split,
    pub counts: Counts,
}

/// One call, as a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub call: String,
    pub tool: String,
    pub what: String,
    /// What the agent said the call does, when it said.
    pub about: Option<String>,
    /// The job it launched ([`plan::launch`]); a background command's
    /// whole text when it launched none.
    pub launch: Option<String>,
    pub path: Option<String>,
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

/// What feeding a line told the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fed {
    /// A run started: its index in [`Track::runs`].
    Started(usize),
    /// A run ended, with the text a plan is read from: a Write's, or a
    /// Read's result.
    Ended(usize, Option<String>),
    Prompt { stop: bool },
    Said,
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
    pub hours: BTreeMap<DateTime<Utc>, Hour>,
    pub runs: Vec<Run>,
    open: HashMap<String, usize>,
    /// Background jobs by Claude Code's id for them.
    jobs: HashMap<String, usize>,
    /// The operator's stop the agent has not answered with words yet.
    pub stop: Option<(DateTime<Utc>, String)>,
    /// Its newest turn ended, and no message of the operator's came since.
    pub turn_over: bool,
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
            hours: BTreeMap::new(),
            runs: Vec::new(),
            open: HashMap::new(),
            jobs: HashMap::new(),
            stop: None,
            turn_over: false,
        }
    }

    /// Feed one line, written at `at`; `jobs` and `stop_words` are the
    /// operator's lists.
    pub fn feed(&mut self, at: DateTime<Utc>, reading: Reading, jobs: &[String], stop_words: &[String]) -> Vec<Fed> {
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
                Happened::Thought => self.spend(at, Spent::Thinking),
                Happened::Said => {
                    self.spend(at, Spent::Writing);
                    self.stop = None;
                    fed.push(Fed::Said);
                }
                Happened::Call { id, tool, what, about, path, background, content, stops } => {
                    self.spend(at, Spent::Writing);
                    // A job stopped by the agent ended there.
                    if let Some(i) = stops.as_ref().and_then(|j| self.jobs.get(j)).copied() {
                        let run = &mut self.runs[i];
                        if run.end.is_none() {
                            run.end = Some(at);
                            run.status = Some("killed".to_string());
                            let call = run.call.clone();
                            self.open.remove(&call);
                            fed.push(Fed::Ended(i, None));
                        }
                    }
                    let launch = if tool == "Bash" {
                        plan::launch(&what, jobs).or_else(|| background.then(|| what.split_whitespace().collect::<Vec<_>>().join(" ")))
                    } else {
                        None
                    };
                    let hour = self.hour(at);
                    hour.counts.calls += 1;
                    hour.counts.launches += u32::from(launch.is_some());
                    let i = self.runs.len();
                    self.runs.push(Run { call: id.clone(), tool, what, about, launch, path, start: at, end: None, background, failed: false, status: None });
                    self.open.insert(id, i);
                    fed.push(Fed::Started(i));
                    if content.is_some() {
                        // A Write's text is the file's whole: a plan is read from it at once.
                        fed.push(Fed::Ended(i, content));
                    }
                }
                Happened::Result { call, failed, job, text } => {
                    let Some(&i) = self.open.get(&call) else {
                        self.spend(at, Spent::Tools);
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
                    self.spend(at, spent);
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
                    fed.push(Fed::Ended(i, (run.tool == "Read").then_some(text)));
                }
                Happened::JobEnded { call, status } => {
                    // Only a job of its own that this notice ends was waited
                    // on; any other notice (a watch's, a subagent's) ends a
                    // wait that was the operator's.
                    let i = call.as_ref().and_then(|c| self.open.get(c).copied()).filter(|&i| self.runs[i].background);
                    let span = i.map(|i| (self.runs[i].start, at));
                    self.spend_waiting(at, span.into_iter().collect());
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
                    let running: Vec<(DateTime<Utc>, DateTime<Utc>)> = self
                        .runs
                        .iter()
                        .filter(|r| r.background)
                        .map(|r| (r.start, r.end.unwrap_or(r.start + TimeDelta::seconds(UNSEEN_END_SECS))))
                        .collect();
                    self.spend_waiting(at, running);
                    if matches!(came, Came::Shell | Came::Command) {
                        continue;
                    }
                    self.hour(at).counts.prompts += 1;
                    self.turn_over = false;
                    let stop = came == Came::Interrupt || starts_with_any(&text, stop_words);
                    self.stop = stop.then_some((at, text));
                    fed.push(Fed::Prompt { stop });
                }
                Happened::TurnEnded => self.turn_over = true,
            }
        }
        fed
    }

    /// The gap since the line before, given to what ended it.
    fn spend(&mut self, at: DateTime<Utc>, spent: Spent) {
        let gap = self.prev.map_or(0, |p| (at - p).num_seconds());
        if gap > 0 {
            let spent = if matches!(spent, Spent::Thinking | Spent::Writing) && gap > REPLY_LIMIT_SECS { Spent::Operator } else { spent };
            self.hour(at).split.add(spent, gap);
        }
        self.prev = Some(self.prev.map_or(at, |p| p.max(at)));
    }

    fn hour(&mut self, at: DateTime<Utc>) -> &mut Hour {
        let hour = at.duration_trunc(TimeDelta::hours(1)).unwrap_or(at);
        self.hours.entry(hour).or_default()
    }

    /// A wait that ended at `at`: the part of it some job of `running` (each
    /// from its start to its notice, or [`UNSEEN_END_SECS`] past its start
    /// when none came) ran through was a wait on that job, the rest the
    /// operator's.
    fn spend_waiting(&mut self, at: DateTime<Utc>, running: Vec<(DateTime<Utc>, DateTime<Utc>)>) {
        let Some(prev) = self.prev else {
            self.prev = Some(at);
            return;
        };
        let gap = (at - prev).num_seconds();
        if gap > 0 {
            let on_jobs = running.iter().map(|(a, b)| ((*b).min(at) - (*a).max(prev)).num_seconds().max(0)).max().unwrap_or(0).min(gap);
            let hour = self.hour(at);
            hour.split.add(Spent::Jobs, on_jobs);
            hour.split.add(Spent::Operator, gap - on_jobs);
        }
        self.prev = Some(prev.max(at));
    }

    /// The runs still running.
    pub fn running(&self) -> impl Iterator<Item = &Run> {
        let mut open: Vec<usize> = self.open.values().copied().collect();
        open.sort_unstable();
        open.into_iter().map(|i| &self.runs[i])
    }

    /// Forget what ended before `from`.
    pub fn trim(&mut self, from: DateTime<Utc>) {
        self.hours.retain(|h, _| *h + TimeDelta::hours(1) > from);
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

/// Whether `text` starts with one of `words`, as a whole word.
#[must_use]
pub fn starts_with_any(text: &str, words: &[String]) -> bool {
    let t = text.trim_start().to_lowercase();
    words.iter().map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).any(|w| {
        t.strip_prefix(&w).is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '\''))
    })
}
