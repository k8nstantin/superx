//! One watch per session (#566, steps 5 to 12): what the session's lines in
//! the window say, the operator's words and the orders in force, the task
//! file it is held to, and every report its lines and time make.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use super::follow::Captured;
use super::line::{Came, Happened, Reading, FILE_TOOLS};
use super::memory::{self, MemoryView};
use super::report::{Kind, Report, Words};
use super::spec::{self, SpecView, TaskFile};
use super::words::{self, Class};
use crate::params::Params;

/// What a watch judges a line with: the parameters in force and the
/// projects roots.
pub struct Context<'a> {
    pub params: &'a Params,
    pub roots: &'a [PathBuf],
}

/// A message of the operator's, with its class and marks; a shell or slash
/// command is kept whole, neither classed nor marked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub at: DateTime<Utc>,
    pub came: Came,
    pub text: String,
    pub class: Option<Class>,
    pub correction: bool,
    pub escalation: bool,
}

/// Text written at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Text {
    pub at: DateTime<Utc>,
    pub text: String,
}

/// A call running: its tool, what it was called on, since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Doing {
    pub tool: String,
    pub what: String,
    pub since: DateTime<Utc>,
}

/// The run since the operator's newest message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Run {
    pub since: Option<DateTime<Utc>>,
    pub calls: BTreeMap<String, u64>,
    pub lines: i64,
}

/// A session as the page shows it (#566, step 14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub name: String,
    pub key: String,
    pub agent: String,
    pub model: Option<String>,
    pub folder: Option<String>,
    pub state: &'static str,
    pub newest: Option<DateTime<Utc>>,
    pub doing: Option<Doing>,
    pub said: Option<Text>,
    pub message: Option<Message>,
    /// The orders in force; none when no order is known.
    pub orders: Option<String>,
    pub run: Run,
    pub jobs: Vec<Doing>,
    pub recap: Option<Text>,
    pub spec: Option<SpecView>,
    pub memory: MemoryView,
}

#[derive(Debug, Clone)]
struct Call {
    at: DateTime<Utc>,
    tool: String,
    what: String,
    background: bool,
    ended: bool,
}

#[derive(Debug, Clone)]
struct Stop {
    text: String,
    calls: u64,
    fired: bool,
}

#[derive(Debug, Clone)]
struct Asked {
    text: String,
    answered: bool,
    acted: bool,
}

/// The task file a session is held to.
#[derive(Debug, Clone)]
struct Held {
    path: String,
    file: Option<PathBuf>,
    checkout: Option<PathBuf>,
    task: Option<TaskFile>,
    first_version: Option<String>,
    modified: Option<SystemTime>,
    why: Option<String>,
    reported: BTreeSet<String>,
}

impl Held {
    fn new(named: spec::Named) -> Self {
        let file = named.checkout.as_ref().map(|c| c.join(&named.path));
        let mut held = Self {
            path: named.path,
            file,
            checkout: named.checkout,
            task: None,
            first_version: None,
            modified: None,
            why: None,
            reported: BTreeSet::new(),
        };
        held.read();
        held
    }

    /// Read the file again; the version it held first and the one it holds
    /// now, when they differ and the new one was not reported yet.
    fn read(&mut self) -> Option<(String, String)> {
        let Some(file) = self.file.clone() else {
            self.why = Some(format!("no folder of the session's holds {}", self.path));
            return None;
        };
        self.modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        let parsed = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display())).and_then(|t| TaskFile::parse(&t));
        match parsed {
            Err(why) => {
                self.task = None;
                self.why = Some(why);
                None
            }
            Ok(task) => {
                let change = match &self.first_version {
                    None => {
                        self.first_version = Some(task.version.clone());
                        None
                    }
                    Some(first) if *first != task.version && self.reported.insert(task.version.clone()) => {
                        Some((first.clone(), task.version.clone()))
                    }
                    Some(_) => None,
                };
                self.task = Some(task);
                self.why = None;
                change
            }
        }
    }

    fn changed(&self) -> bool {
        let Some(file) = &self.file else { return false };
        std::fs::metadata(file).and_then(|m| m.modified()).ok() != self.modified
    }

    fn view(&self) -> SpecView {
        SpecView {
            path: self.path.clone(),
            file: self.file.as_ref().map(|f| f.display().to_string()),
            version: self.task.as_ref().map(|t| t.version.clone()),
            first_version: self.first_version.clone(),
            owns: self.task.as_ref().map(|t| t.owns.clone()).unwrap_or_default(),
            forbid: self.task.as_ref().map(|t| t.forbid.clone()).unwrap_or_default(),
            why: self.why.clone(),
        }
    }
}

/// One session's watch.
#[derive(Debug, Clone)]
pub struct Watch {
    pub session: String,
    pub name: String,
    pub key: String,
    pub agent: String,
    model: Option<String>,
    folder: Option<String>,
    newest: Option<DateTime<Utc>>,
    transcript: Option<String>,
    messages: Vec<Message>,
    said: Vec<Text>,
    calls: BTreeMap<String, Call>,
    jobs: BTreeMap<String, DateTime<Utc>>,
    working: bool,
    turn_began: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    job_at_end: bool,
    question_at_end: bool,
    turn_words: Option<String>,
    proposal: Vec<String>,
    orders: Option<String>,
    held: Option<Held>,
    recap: Option<Text>,
    memory: MemoryView,
    reports: Vec<Report>,
    since_said: Option<DateTime<Utc>>,
    calls_since_said: u64,
    quiet_fired: bool,
    run: Run,
    run_count: u64,
    run_fired: bool,
    stop: Option<Stop>,
    asked: Option<Asked>,
    failures: Vec<String>,
    struggle_fired: bool,
    last_contact: Option<DateTime<Utc>>,
    behind_fired: Option<DateTime<Utc>>,
    stalled_fired: Option<DateTime<Utc>>,
    mood_fired: usize,
}

impl Watch {
    /// A watch of the session `line` names.
    #[must_use]
    pub fn new(line: &Captured) -> Self {
        Self {
            session: line.session.clone(),
            name: line.name.clone(),
            key: line.key.clone(),
            agent: line.agent.clone(),
            model: None,
            folder: None,
            newest: None,
            transcript: None,
            messages: Vec::new(),
            said: Vec::new(),
            calls: BTreeMap::new(),
            jobs: BTreeMap::new(),
            working: false,
            turn_began: None,
            ended_at: None,
            job_at_end: false,
            question_at_end: false,
            turn_words: None,
            proposal: Vec::new(),
            orders: None,
            held: None,
            recap: None,
            memory: MemoryView { folder: None, files: Vec::new(), why: Some("its transcript is not known yet".to_string()) },
            reports: Vec::new(),
            since_said: None,
            calls_since_said: 0,
            quiet_fired: false,
            run: Run::default(),
            run_count: 0,
            run_fired: false,
            stop: None,
            asked: None,
            failures: Vec::new(),
            struggle_fired: false,
            last_contact: None,
            behind_fired: None,
            stalled_fired: None,
            mood_fired: 0,
        }
    }

    /// The time of its newest line.
    #[must_use]
    pub fn newest(&self) -> Option<DateTime<Utc>> {
        self.newest
    }

    /// Its reports, oldest first.
    #[must_use]
    pub fn reports(&self) -> &[Report] {
        &self.reports
    }

    /// Its transcript, as its lines name it.
    #[must_use]
    pub fn transcript(&self) -> Option<&str> {
        self.transcript.as_deref()
    }

    /// Give the watch one line, read.
    pub fn feed(&mut self, line: &Captured, reading: &Reading, ctx: &Context<'_>) {
        self.newest = self.newest.max(Some(line.at));
        if reading.model.is_some() {
            self.model.clone_from(&reading.model);
        }
        if reading.folder.is_some() {
            self.folder.clone_from(&reading.folder);
        }
        if line.transcript.is_some() {
            self.transcript.clone_from(&line.transcript);
        }
        for happened in &reading.happened {
            self.happen(line.at, happened, reading.folder.as_deref(), ctx);
        }
    }

    fn report(&mut self, kind: Kind, at: DateTime<Utc>, what: String, words: Vec<Words>) {
        let report = Report::new(&self.session, kind, at, what, words);
        if !self.reports.iter().any(|r| r.id == report.id) {
            self.reports.push(report);
        }
    }

    fn work(&mut self, at: DateTime<Utc>) {
        if !self.working {
            self.working = true;
            self.turn_began = Some(at);
        }
    }

    fn happen(&mut self, at: DateTime<Utc>, happened: &Happened, folder: Option<&str>, ctx: &Context<'_>) {
        match happened {
            Happened::Prompt { came, text } => self.message(at, *came, text, folder, ctx),
            Happened::Said { text } => self.said(at, text, ctx),
            Happened::Call { id, tool, what, input, background, writes } => {
                self.call(at, id, tool, what, input, *background, writes, ctx);
            }
            Happened::Result { call, failed, error, lines } => {
                self.work(at);
                if let Some(c) = self.calls.get_mut(call) {
                    c.ended = true;
                }
                if self.run.since.is_some() {
                    self.run.lines += lines;
                }
                if *failed {
                    self.failures.push(error.clone().unwrap_or_default());
                    let p = ctx.params;
                    if !self.struggle_fired && self.failures.len() as u64 >= p.watch_failures {
                        self.struggle_fired = true;
                        let words = self.failures.iter().map(|e| Words::call(e)).collect();
                        let what = format!("{} tool calls failed in a row", self.failures.len());
                        self.report(Kind::Struggling, at, what, words);
                    }
                } else {
                    self.failures.clear();
                    self.struggle_fired = false;
                }
            }
            Happened::JobEnded { call, .. } => {
                if let Some(call) = call {
                    self.jobs.remove(call);
                }
                self.work(at);
            }
            Happened::TurnEnded => self.turn_ended(at),
            Happened::Compacted => {
                let what = "its context was compacted: your earlier orders may be gone from it".to_string();
                self.report(Kind::Compacted, at, what, Vec::new());
            }
            Happened::Recap { text } => self.recap = Some(Text { at, text: text.clone() }),
        }
    }

    fn message(&mut self, at: DateTime<Utc>, came: Came, text: &str, folder: Option<&str>, ctx: &Context<'_>) {
        let p = ctx.params;
        if matches!(came, Came::Shell | Came::Command) {
            self.messages.push(Message { at, came, text: text.to_string(), class: None, correction: false, escalation: false });
            return;
        }
        let interrupt = came == Came::Interrupt;
        let class = words::class(text, interrupt, p);
        let message = Message {
            at,
            came,
            text: text.to_string(),
            class: Some(class),
            correction: words::corrects(text, p),
            escalation: words::escalates(text, p),
        };
        if !interrupt {
            self.repeated(&message, p);
        }
        self.messages.push(message);
        if !interrupt {
            self.frustration(at, p);
        }
        if class == Class::StatusAsk {
            self.report(Kind::StatusAsked, at, "you asked for status".to_string(), vec![Words::operator(text)]);
        }
        match class {
            Class::Order => self.orders = Some(text.to_string()),
            Class::Approval => {
                let mut approved = self.proposal.clone();
                approved.push(text.to_string());
                self.orders = Some(approved.join("\n"));
            }
            Class::Stop | Class::StatusAsk | Class::Question => {}
        }
        self.asked = matches!(class, Class::Question | Class::StatusAsk).then(|| Asked {
            text: text.to_string(),
            answered: false,
            acted: false,
        });
        self.stop = (class == Class::Stop).then(|| Stop { text: text.to_string(), calls: 0, fired: false });
        self.since_said = Some(at);
        self.calls_since_said = 0;
        self.quiet_fired = false;
        self.run = Run { since: Some(at), calls: BTreeMap::new(), lines: 0 };
        self.run_count = 0;
        self.run_fired = false;
        self.proposal.clear();
        self.last_contact = Some(at);
        if interrupt {
            self.turn_ended_quietly(at);
        } else {
            self.work(at);
            if let Some(named) = spec::named(text, &p.watch_spec_paths, folder) {
                self.held = Some(Held::new(named));
            }
        }
    }

    /// An earlier message, sent within attr_recon_watch_repeat_secs, sharing
    /// at least attr_recon_watch_repeat_share percent of the shorter one's
    /// distinct words, the shorter one having attr_recon_watch_order_words.
    fn repeated(&mut self, message: &Message, p: &Params) {
        let mine: BTreeSet<String> = words::words(&message.text).into_iter().collect();
        let from = message.at - secs(p.watch_repeat_secs);
        let earlier = self.messages.iter().rev().find(|m| {
            if m.at < from || !matches!(m.came, Came::Typed | Came::Queued) {
                return false;
            }
            let theirs: BTreeSet<String> = words::words(&m.text).into_iter().collect();
            let shorter = mine.len().min(theirs.len()) as u64;
            let shared = mine.intersection(&theirs).count() as u64;
            shorter >= p.watch_order_words && shared * 100 >= p.watch_repeat_share * shorter
        });
        if let Some(earlier) = earlier.cloned() {
            let what = "you said the same thing again".to_string();
            self.report(
                Kind::RepeatedOrder,
                message.at,
                what,
                vec![Words::operator(&earlier.text), Words::operator(&message.text)],
            );
        }
    }

    /// Of the operator's newest attr_recon_watch_mood_prompts messages, once
    /// there are that many, more than half escalating.
    fn frustration(&mut self, at: DateTime<Utc>, p: &Params) {
        let n = usize::try_from(p.watch_mood_prompts).unwrap_or(usize::MAX);
        let typed: Vec<&Message> = self.messages.iter().filter(|m| matches!(m.came, Came::Typed | Came::Queued)).collect();
        if typed.len() < n {
            return;
        }
        let escalating = typed[typed.len() - n..].iter().filter(|m| m.escalation).count();
        if escalating * 2 <= n {
            self.mood_fired = 0;
            return;
        }
        if escalating > self.mood_fired {
            self.mood_fired = escalating;
            let what = format!("{escalating} of your newest {n} messages escalate");
            let words = typed[typed.len() - n..].iter().filter(|m| m.escalation).map(|m| Words::operator(&m.text)).collect();
            self.report(Kind::Frustration, at, what, words);
        }
    }

    fn said(&mut self, at: DateTime<Utc>, text: &str, ctx: &Context<'_>) {
        let p = ctx.params;
        self.work(at);
        self.said.push(Text { at, text: text.to_string() });
        self.turn_words = Some(text.to_string());
        self.proposal.push(text.to_string());
        self.since_said = Some(at);
        self.calls_since_said = 0;
        self.quiet_fired = false;
        if let Some(asked) = &mut self.asked {
            asked.answered = true;
        }
        self.stop = None;
        self.last_contact = Some(at);
        let mine = words::words(text);
        let long = words::held(&mine, &p.watch_long_words);
        if !long.is_empty() {
            let what = format!("it estimates a long time: {}", long.join(", "));
            self.report(Kind::LongEstimate, at, what, vec![Words::agent(text)]);
        }
        if let Some(orders) = self.ordered_words() {
            let unordered: Vec<&str> =
                words::held(&mine, &p.watch_scope_words).into_iter().filter(|w| !words::holds(&orders, w)).collect();
            if !unordered.is_empty() {
                let what = format!("it announces work your orders did not say: {}", unordered.join(", "));
                self.report(Kind::UnorderedWork, at, what, vec![Words::agent(text)]);
            }
        }
    }

    /// The words of the orders in force: the operator's and the task file's;
    /// none when no order is known.
    fn ordered_words(&self) -> Option<Vec<String>> {
        let spec = self.held.as_ref().and_then(|h| h.task.as_ref()).map(|t| t.words.as_str());
        match (self.orders.as_deref(), spec) {
            (None, None) => None,
            (orders, spec) => Some(words::words(&format!("{}\n{}", orders.unwrap_or_default(), spec.unwrap_or_default()))),
        }
    }

    #[allow(clippy::too_many_arguments)] // skill-allow: allow — one call, as a line gives it
    fn call(
        &mut self,
        at: DateTime<Utc>,
        id: &str,
        tool: &str,
        what: &str,
        input: &serde_json::Value,
        background: bool,
        writes: &[PathBuf],
        ctx: &Context<'_>,
    ) {
        let p = ctx.params;
        let effects: Vec<(&String, &Vec<String>)> =
            p.watch_effects.iter().filter(|(pattern, _)| tool.contains(pattern.as_str()) || what.contains(pattern.as_str())).collect();
        let act = FILE_TOOLS.contains(&tool) || !effects.is_empty();
        let spec_words = self.held.as_ref().and_then(|h| h.task.as_ref()).map(|t| words::words(&t.words));
        let spec_orders = spec_words.as_ref().is_some_and(|sw| effects.iter().any(|(_, ws)| ws.iter().any(|w| words::holds(sw, w))))
            || (FILE_TOOLS.contains(&tool) && writes.iter().any(|w| self.owned(w)));
        self.work(at);
        self.calls.insert(id.to_string(), Call { at, tool: tool.to_string(), what: what.to_string(), background, ended: false });
        if background {
            self.jobs.insert(id.to_string(), at);
        }
        self.calls_since_said += 1;
        if let Some(since) = self.since_said {
            if !self.quiet_fired && at - since >= secs(p.watch_quiet_secs) && self.calls_since_said >= p.watch_quiet_calls {
                self.quiet_fired = true;
                let sentence = format!(
                    "{} tool calls and {} minutes without a word to you",
                    self.calls_since_said,
                    (at - since).num_minutes()
                );
                self.report(Kind::Quiet, at, sentence, vec![Words::call(&format!("{tool}: {what}"))]);
            }
        }
        if let Some(since) = self.run.since {
            self.run_count += 1;
            *self.run.calls.entry(tool.to_string()).or_insert(0) += 1;
            if !self.run_fired && at - since >= secs(p.watch_run_secs) && self.run_count >= p.watch_run_calls {
                self.run_fired = true;
                let calls: Vec<String> = self.run.calls.iter().map(|(t, n)| format!("{t} {n}")).collect();
                let what = format!(
                    "since your last message: {} tool calls in {} minutes ({}), {} lines written",
                    self.run_count,
                    (at - since).num_minutes(),
                    calls.join(", "),
                    self.run.lines
                );
                self.report(Kind::LongRun, at, what, Vec::new());
            }
        }
        if let Some(stop) = &mut self.stop {
            stop.calls += 1;
            if !stop.fired && stop.calls >= p.watch_stop_calls {
                stop.fired = true;
                let (text, calls) = (stop.text.clone(), stop.calls);
                let mut sentence = format!("you said stop; it made {calls} tool calls since, without a word");
                if spec_orders {
                    sentence.push_str(" (the task file's words order the act; your stop holds)");
                }
                self.report(Kind::StopNotHonoured, at, sentence, vec![Words::operator(&text), Words::call(&format!("{tool}: {what}"))]);
            }
        }
        if let Some(asked) = &mut self.asked {
            if act && !asked.answered && !asked.acted {
                asked.acted = true;
                let question = asked.text.clone();
                let mut sentence = "you asked; it acted before answering".to_string();
                if spec_orders {
                    sentence.push_str(" (the task file's words order the act; your question holds)");
                }
                self.report(Kind::ActedOnQuestion, at, sentence, vec![Words::operator(&question), Words::call(&format!("{tool}: {what}"))]);
            }
        }
        if let Some(orders) = self.ordered_words() {
            let unordered: Vec<&str> = effects
                .iter()
                .filter(|(_, ws)| !ws.iter().any(|w| words::holds(&orders, w)))
                .map(|(pattern, _)| pattern.as_str())
                .collect();
            if !unordered.is_empty() {
                let what_ = format!("an action no order in force names: {}", unordered.join(", "));
                let orders_text = self.orders.clone().unwrap_or_default();
                self.report(Kind::UnorderedAction, at, what_, vec![Words::call(what), Words::operator(&orders_text)]);
            }
        }
        let memory: Vec<String> = writes.iter().filter(|w| memory::holds(w, ctx.roots)).map(|w| w.display().to_string()).collect();
        if !memory.is_empty() {
            let what_ = format!("it wrote to its memory: {}", memory.join(", "));
            self.report(Kind::MemoryWritten, at, what_, vec![Words::call(&memory.join("\n")), Words::call(&input.to_string())]);
        }
        self.judge_writes(at, writes);
    }

    /// Whether the task file the session is held to owns `path`.
    fn owned(&self, path: &Path) -> bool {
        let Some(held) = &self.held else { return false };
        let (Some(task), Some(checkout)) = (&held.task, &held.checkout) else { return false };
        path.strip_prefix(checkout).is_ok_and(|rel| task.owns_it(&rel.to_string_lossy()))
    }

    /// A call's writes against the task file: forbidden, or outside what it
    /// owns (#566, step 12).
    fn judge_writes(&mut self, at: DateTime<Utc>, writes: &[PathBuf]) {
        let Some(held) = &self.held else { return };
        let (Some(task), Some(checkout)) = (&held.task, &held.checkout) else { return };
        let mut forbidden = Vec::new();
        let mut outside = Vec::new();
        for w in writes {
            let Ok(rel) = w.strip_prefix(checkout) else { continue };
            let rel = rel.to_string_lossy().to_string();
            if let Some(entry) = task.forbids(&rel) {
                forbidden.push(format!("{rel} (forbidden: {entry})"));
            } else if task.outside(&rel) {
                outside.push(rel);
            }
        }
        if !forbidden.is_empty() {
            let what = format!("it wrote a file its task file forbids: {}", forbidden.join("; "));
            self.report(Kind::ForbiddenFile, at, what, Vec::new());
        }
        if !outside.is_empty() {
            let what = format!("it wrote outside what its task file owns: {}", outside.join(", "));
            self.report(Kind::OutsideOwns, at, what, Vec::new());
        }
    }

    fn turn_ended(&mut self, at: DateTime<Utc>) {
        self.working = false;
        self.ended_at = Some(at);
        self.job_at_end = !self.jobs.is_empty();
        let words_at_end = self.turn_words.take();
        self.question_at_end = words_at_end.as_deref().is_some_and(|w| w.trim_end().ends_with('?'));
        if let Some(asked) = self.asked.take() {
            if !asked.answered {
                let what = "its turn ended without answering your question".to_string();
                self.report(Kind::QuestionUnanswered, at, what, vec![Words::operator(&asked.text)]);
            }
        }
        if self.question_at_end {
            let text = words_at_end.unwrap_or_default();
            self.report(Kind::WaitingOnYou, at, "its turn ended on its question to you".to_string(), vec![Words::agent(&text)]);
        }
    }

    /// The operator stopped the agent: its turn ends, with nothing to report.
    fn turn_ended_quietly(&mut self, at: DateTime<Utc>) {
        self.working = false;
        self.ended_at = Some(at);
        self.job_at_end = false;
        self.question_at_end = false;
        self.turn_words = None;
    }

    /// No call of the turn running: each has its result, a background call
    /// aside.
    fn none_running(&self) -> bool {
        let began = self.turn_began.unwrap_or(DateTime::<Utc>::MIN_UTC);
        !self.calls.values().any(|c| c.at >= began && !c.background && !c.ended)
    }

    fn stalled_at(&self, now: DateTime<Utc>, p: &Params) -> Option<DateTime<Utc>> {
        let newest = self.newest?;
        (self.working && self.none_running() && now - newest >= secs(p.watch_quiet_secs)).then_some(newest)
    }

    /// What time alone makes, at the look's time (#566, step 8): behind a
    /// job, and stalled.
    pub fn judge(&mut self, now: DateTime<Utc>, p: &Params) {
        if let Some(end) = self.ended_at {
            let no_contact = self.last_contact.is_none_or(|c| c <= end);
            if self.job_at_end && no_contact && self.behind_fired != Some(end) && now - end >= secs(p.watch_behind_secs) {
                self.behind_fired = Some(end);
                let jobs: Vec<String> = self.calls_of_jobs();
                let what = format!("its turn ended with a background job running, and nothing said since: {}", jobs.join(", "));
                self.report(Kind::BehindAJob, end + secs(p.watch_behind_secs), what, Vec::new());
            }
        }
        if let Some(newest) = self.stalled_at(now, p) {
            if self.stalled_fired != Some(newest) {
                self.stalled_fired = Some(newest);
                let what = "mid-turn, no tool running and no line: a stuck model or a dead process".to_string();
                self.report(Kind::Stalled, newest + secs(p.watch_quiet_secs), what, Vec::new());
            }
        }
    }

    fn calls_of_jobs(&self) -> Vec<String> {
        self.jobs.keys().filter_map(|id| self.calls.get(id)).map(|c| format!("{}: {}", c.tool, c.what)).collect()
    }

    /// Read the task file again when it changed (#566, step 11), and report
    /// a version other than the one the session was first held to.
    pub fn reread(&mut self, now: DateTime<Utc>) {
        let Some(held) = &mut self.held else { return };
        if !held.changed() {
            return;
        }
        if let Some((first, now_version)) = held.read() {
            let what = format!("its task file changed version while it runs: it started on {first}, the file is now {now_version}");
            self.report(Kind::SpecVersionChanged, now, what, Vec::new());
        }
    }

    /// Take from the watch what the window leaves behind: its lines older
    /// than `from`, and what was made of them, but for the orders in force
    /// and the task file. Whether it holds any line still.
    pub fn trim(&mut self, from: DateTime<Utc>) -> bool {
        self.messages.retain(|m| m.at >= from);
        self.said.retain(|s| s.at >= from);
        self.calls.retain(|_, c| c.at >= from);
        self.jobs.retain(|_, since| *since >= from);
        self.reports.retain(|r| r.at >= from);
        if self.recap.as_ref().is_some_and(|r| r.at < from) {
            self.recap = None;
        }
        self.newest.is_some_and(|n| n >= from)
    }

    /// List its memory folder (#566, step 13).
    pub fn list_memory(&mut self, roots: &[PathBuf]) {
        self.memory = match self.transcript.as_deref() {
            None => MemoryView { folder: None, files: Vec::new(), why: Some("its transcript is not known yet".to_string()) },
            Some(t) => match memory::folder(Path::new(t), roots) {
                None => MemoryView { folder: None, files: Vec::new(), why: Some("no projects root holds its transcript".to_string()) },
                Some(folder) => match memory::list(&folder) {
                    Ok(files) => MemoryView { folder: Some(folder.display().to_string()), files, why: None },
                    Err(why) => MemoryView { folder: Some(folder.display().to_string()), files: Vec::new(), why: Some(why) },
                },
            },
        };
    }

    /// Its memory folder, when it has one.
    #[must_use]
    pub fn memory(&self) -> &MemoryView {
        &self.memory
    }

    fn state(&self, now: DateTime<Utc>, p: &Params) -> &'static str {
        if self.stalled_at(now, p).is_some() {
            "stalled"
        } else if self.working {
            "working"
        } else if !self.jobs.is_empty() {
            "behind a job"
        } else if self.question_at_end {
            "waiting on you"
        } else {
            "idle"
        }
    }

    /// The session as the page shows it.
    #[must_use]
    pub fn view(&self, now: DateTime<Utc>, p: &Params) -> SessionView {
        let doing = self
            .calls
            .values()
            .filter(|c| !c.ended && !c.background)
            .max_by_key(|c| c.at)
            .map(|c| Doing { tool: c.tool.clone(), what: c.what.clone(), since: c.at });
        let jobs = self
            .jobs
            .iter()
            .filter_map(|(id, since)| self.calls.get(id).map(|c| Doing { tool: c.tool.clone(), what: c.what.clone(), since: *since }))
            .collect();
        SessionView {
            id: self.session.clone(),
            name: self.name.clone(),
            key: self.key.clone(),
            agent: self.agent.clone(),
            model: self.model.clone(),
            folder: self.folder.clone(),
            state: self.state(now, p),
            newest: self.newest,
            doing,
            said: self.said.iter().max_by_key(|t| t.at).cloned(),
            message: self.messages.iter().max_by_key(|m| m.at).cloned(),
            orders: self.orders.clone(),
            run: self.run.clone(),
            jobs,
            recap: self.recap.clone(),
            spec: self.held.as_ref().map(Held::view),
            memory: self.memory.clone(),
        }
    }
}

fn secs(n: u64) -> TimeDelta {
    TimeDelta::seconds(i64::try_from(n).unwrap_or(i64::MAX / 1_000))
}
