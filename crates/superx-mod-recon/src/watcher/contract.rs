//! The task file a session builds from (#573): the operator's message
//! `Build <path>` names it, and the agent's read of it gives its text. Every
//! act after is judged against what the file decides, and a break of it is
//! one critical alert (operator, 2026-11-05: "very defined alarms, not a
//! flood"):
//!
//! - a change to a file its `<forbid>` names, a path or a folder written
//!   `<folder>/**`, that its `<owns>` does not;
//! - a door gate's command launched more often than the file's door gates
//!   run it: each `<cmd>` of a `<gate kind="door">` runs once;
//! - after a door gate whose `<expect>` ends in `exit 0` failed, the build
//!   going on before the operator's next message: a file of the checkout
//!   changed, a job launched, a commit or a push; a failed gate stops it;
//! - when its `<forbid>` names the checkout, its verbs (the task's `command`)
//!   run from the checkout; when it names the bucket, a write to a bucket;
//! - against its `<start>`: a branch cut other than the one it names (`Cut
//!   the branch ENG-8080-…-v2-<model>-<effort>`), a commit whose message
//!   does not start with its word or carries an attribution trailer it
//!   forbids, and a pull request opened or merged by the agent where the
//!   file has the operator open it.
//!
//! Each of its local gates is followed too, by the gate's own command: when
//! it ran and whether its expected last line came, for the page to show a
//! build against its file as it goes.
//!
//! And a build that hands the operator its turn, no job of its own running,
//! before it wrote every file the task's `builds` names has stopped short:
//! the file did not build as written (2026-11-05: ENG-8080 at 11:33, on four
//! silences of its task file; three such stops in all the builds before,
//! each a real one). The task merged with that file, in the session that
//! wrote it, is reopened by it ([`super::task::Tasks::build_stopped`]).
//!
//! A launch that started no execution ran nothing and takes no gate. Over
//! every build of 2026-10-09 to 2026-11-05 these rules found nothing: the
//! reruns of those builds were their task files' own gates.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::alerts::Alerts;
use super::command::{self, simple_commands};
use super::line::FILE_TOOLS;
use super::plan;
use super::task;
use super::track::{Run, StepKind, Track, UNSEEN_END_SECS};
use crate::params::Params;

/// What gcloud prints when it starts an execution, and when one failed.
const EXECUTED: [&str; 2] = ["Creating execution", "Execution ["];
const EXECUTION_FAILED: &str = "The execution failed";
const EXECUTION_SUCCEEDED: &str = "has successfully completed";

/// What Python writes beside the code: no file of the repository.
const GENERATED: [&str; 2] = ["/__pycache__/", ".pyc"]; // skill-allow: §9-const — the interpreter's own caches, not a policy tunable

/// How much of the agent's last words a stopped build's alert keeps.
const STOP_WORDS_CHARS: usize = 600; // skill-allow: §9-const — the evidence's length on the page, not a policy tunable

/// The programs that write a bucket, by the word after them.
const BUCKET_WRITES: [&str; 4] = ["cp", "mv", "rm", "rsync"];

/// One door gate's command: the gate it is of, the work it launches, and
/// whether the gate expects its run to end well.
#[derive(Debug, Clone)]
struct Slot {
    gate: String,
    /// The work its command launches; none for a door gate whose text
    /// gives no command of its own (`G5 repeats G3 on purpose`), which
    /// takes any launch once.
    work: Option<String>,
    ends_well: bool,
    /// The call that took it, when it started, and whether its execution
    /// ended well, once it ended.
    taken: Option<String>,
    ran: Option<DateTime<Utc>>,
    ended_well: Option<bool>,
    /// The job its launch runs, and whether the launch runs in the
    /// background: its end is then read where the agent reads it.
    job: Option<String>,
    background: bool,
}

/// One local gate: its command's words, the last line it expects, and its
/// newest run.
#[derive(Debug, Clone)]
struct Local {
    gate: String,
    words: Vec<String>,
    last_line: Option<String>,
    call: Option<String>,
    ran: Option<DateTime<Utc>>,
    passed: Option<bool>,
}

/// What a task file's `<start>` decides of the build's git.
#[derive(Debug, Clone, Default)]
struct Rules {
    /// The branch it cuts, its placeholders whole: `ENG-8080-…-v2-<model>-<effort>`.
    branch: Option<String>,
    /// The word every commit message starts with.
    prefix: Option<String>,
    /// Whether a commit may carry no attribution trailer.
    no_trailer: bool,
    /// Whether the operator opens the pull request.
    pull_is_yours: bool,
}

/// A commit of the build: when, its first line, and whether it keeps the
/// file's rules; the call that made it, which a refusal unmakes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitRow {
    pub at: DateTime<Utc>,
    pub line: String,
    pub ok: bool,
    #[serde(skip)]
    pub call: String,
}

/// A local gate and its newest run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalRun {
    pub gate: String,
    pub ran: Option<DateTime<Utc>>,
    pub passed: Option<bool>,
}

/// A build of a task file, as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildRow {
    pub session: String,
    /// The task file's name, and its ticket.
    pub file: String,
    pub ticket: Option<String>,
    /// When the operator ordered it, and when it handed back its turn short
    /// of what it builds.
    pub ordered: DateTime<Utc>,
    pub stopped: Option<DateTime<Utc>>,
    /// The files it builds, and those of them it wrote.
    pub builds: Vec<String>,
    pub written: Vec<String>,
    /// The forbidden files it changed.
    pub forbidden: Vec<String>,
    /// Each door gate's command, in the file's order, with its run.
    pub gates: Vec<GateRun>,
    /// Each local gate with its newest run.
    pub locals: Vec<LocalRun>,
    /// What the file owns and forbids, as it names them.
    pub owns: Vec<String>,
    pub forbid: Vec<String>,
    /// The branch the file names, the one cut, and the commits made.
    pub branch_rule: Option<String>,
    pub branch: Option<String>,
    pub commits: Vec<CommitRow>,
}

/// A door gate's command and its run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateRun {
    pub gate: String,
    /// Whether the gate expects its run to end well (`exit 0`).
    pub ends_well: bool,
    pub ran: Option<DateTime<Utc>>,
    pub ended_well: Option<bool>,
}

/// What a task file decides, as the session building from it was given it.
#[derive(Debug, Clone)]
pub struct Contract {
    /// The task file's name.
    file: String,
    /// The checkout: the session's folder when the build was ordered.
    root: String,
    owns: Vec<String>,
    forbid: Vec<String>,
    slots: Vec<Slot>,
    /// The task's verbs, when the file forbids them from the checkout.
    verbs: Vec<Vec<String>>,
    /// Whether the file forbids a write to the bucket.
    bucket: bool,
    /// The files the task builds, and those of the checkout the session
    /// changed since it was told to build.
    builds: Vec<String>,
    written: HashSet<String>,
    /// Its ticket, and when the build was ordered.
    ticket: Option<String>,
    ordered: DateTime<Utc>,
    /// The forbidden files it changed, in order.
    forbidden: Vec<String>,
    /// When it stopped short.
    stopped: Option<DateTime<Utc>>,
    locals: Vec<Local>,
    rules: Rules,
    branch: Option<String>,
    commits: Vec<CommitRow>,
    /// The door gate that failed, and when, until the operator's next
    /// message.
    failed: Option<(String, DateTime<Utc>)>,
    /// An act past that failure, until its call is answered: a call refused
    /// or failed did nothing. Its call, the break's key, its words and when
    /// the gate failed.
    past_failure: Option<(String, String, String, DateTime<Utc>)>,
}

/// Every session's task file, once read.
#[derive(Debug, Default)]
pub struct Contracts {
    by_session: HashMap<String, Contract>,
    /// The task file each session was told to build, until it reads it:
    /// its path, the checkout and when.
    ordered: HashMap<String, (String, String, DateTime<Utc>)>,
    /// What of it each session read so far, when it read it in parts.
    parts: HashMap<String, String>,
}

/// A build that stopped short: its ticket, when, and how it reads.
#[derive(Debug, Clone)]
pub struct Stopped {
    pub ticket: String,
    pub at: DateTime<Utc>,
    pub how: String,
}

impl Contracts {
    /// The operator's message to `t`: the end of a failed gate's stop, and
    /// an order to build a task file.
    pub fn prompt(&mut self, t: &Track, at: DateTime<Utc>, text: &str, first: bool, p: &Params) {
        if let Some(c) = self.by_session.get_mut(&t.session) {
            c.failed = None;
        }
        // The order to build is the session's first message, as a task
        // file's start says it must be: the same line later in a session is
        // talk of it.
        if !first {
            return;
        }
        let Some(path) = text.trim().strip_prefix("Build ").map(|r| r.split_whitespace().next().unwrap_or_default().trim_end_matches('.')) else { return };
        if path.ends_with(".xml") && p.watch_plans.iter().any(|m| !m.is_empty() && path.contains(m.as_str())) {
            self.ordered.insert(t.session.clone(), (path.to_string(), t.folder.clone().unwrap_or_default(), at));
        }
    }

    /// A call of `t`'s Claude Code answered runs in the background: an act
    /// past a failed gate it was is done.
    pub fn running(&mut self, t: &Track, run: &Run, alerts: &mut Alerts) {
        if let Some(c) = self.by_session.get_mut(&t.session) {
            c.answered(t, run, alerts);
        }
    }

    /// A call of `t`'s that ended, with its result's text: the read of the
    /// task file it was told to build, which makes its contract, or a door
    /// gate's run.
    pub fn ended(&mut self, t: &Track, run: &Run, text: &str, alerts: &mut Alerts, p: &Params) {
        if run.tool == "Read" {
            let read = run.path.as_deref().unwrap_or_default();
            if let Some((path, root, at)) = self.ordered.get(&t.session).filter(|(path, _, _)| read.ends_with(path.as_str())) {
                // A file read in parts is whole once its parts hold its
                // task's head and its end.
                let whole = self.parts.remove(&t.session).unwrap_or_default() + &super::numbers_set_aside(text) + "\n";
                if !(whole.contains("<task") && whole.contains("</task>")) {
                    self.parts.insert(t.session.clone(), whole);
                    return;
                }
                let file = path.rsplit('/').next().unwrap_or(path).to_string();
                let contract = Contract::of(&whole, file, root.clone(), *at, p);
                self.by_session.insert(t.session.clone(), contract);
                self.ordered.remove(&t.session);
            }
            return;
        }
        let Some(c) = self.by_session.get_mut(&t.session) else { return };
        c.answered(t, run, alerts);
        // A commit refused or failed made none.
        if run.failed {
            c.commits.retain(|r| r.call != run.call);
        }
        // A local gate passed when its expected last line came and nothing
        // failed: one command may run several gates.
        let failed = run.failed || text.lines().any(|l| ["FAILED", "ERROR", "Traceback"].iter().any(|w| l.trim_start().starts_with(w)));
        for l in c.locals.iter_mut().filter(|l| l.call.as_deref() == Some(run.call.as_str())) {
            l.passed = Some(!failed && l.last_line.as_deref().is_none_or(|want| text.lines().any(|line| line.trim() == want)));
        }
        // A door gate run in the background ends where the agent reads its
        // execution's end, in any result that names the execution
        // (`<job>-<id>`): ended well, or failed.
        let (succeeded, failed) = (text.contains(EXECUTION_SUCCEEDED), text.contains(EXECUTION_FAILED));
        if EXECUTED.iter().any(|e| text.contains(e)) && succeeded != failed {
            let at = run.end.unwrap_or(run.start);
            for slot in c.slots.iter_mut().filter(|s| s.background && s.taken.is_some() && s.ended_well.is_none()) {
                if slot.job.as_deref().is_some_and(|job| text.contains(&format!("{job}-"))) {
                    slot.ended_well = Some(succeeded);
                    if slot.ends_well && failed {
                        c.failed = Some((slot.gate.clone(), at));
                    }
                }
            }
        }
        let Some(slot) = c.slots.iter_mut().find(|s| s.taken.as_deref() == Some(run.call.as_str())) else { return };
        if !(EXECUTED.iter().any(|e| text.contains(e)) || (text.trim().is_empty() && !run.failed)) {
            slot.taken = None;
            slot.ran = None;
            return;
        }
        slot.ended_well = Some(!(text.contains(EXECUTION_FAILED) || run.failed));
        if slot.ends_well && slot.ended_well == Some(false) {
            c.failed = Some((slot.gate.clone(), run.end.unwrap_or(run.start)));
        }
    }

    /// A call of `t`'s that started, judged against its task file. Whether
    /// it ran a door gate again, which says all a run again would.
    pub fn started(&mut self, t: &Track, run: &Run, alerts: &mut Alerts, p: &Params) -> bool {
        let Some(c) = self.by_session.get_mut(&t.session) else { return false };
        let changed = c.changed(t, run);
        c.written.extend(changed.iter().cloned());
        let launch = (run.tool == "Bash").then(|| plan::launch(&run.what, &p.watch_jobs)).flatten();
        let mut again = false;
        if run.tool == "Bash" {
            c.git(t, run, alerts);
            for simple in simple_commands(&run.what) {
                let words: Vec<String> = simple.split_whitespace().map(|w| w.trim_matches(['\'', '"']).replace("python3", "python")).collect();
                let best = (0..c.locals.len())
                    .filter(|&i| !c.locals[i].words.is_empty() && c.locals[i].words.iter().all(|w| words.contains(w)))
                    .max_by_key(|&i| c.locals[i].words.len());
                if let Some(i) = best {
                    c.locals[i].call = Some(run.call.clone());
                    c.locals[i].ran = Some(run.start);
                    c.locals[i].passed = None;
                }
            }
        }
        let forbidden: Vec<&String> = changed.iter().filter(|rel| !c.owns.contains(rel) && c.forbid.iter().any(|g| within(rel, g))).collect();
        if !forbidden.is_empty() {
            for rel in forbidden {
                if !c.forbidden.contains(rel) {
                    c.forbidden.push(rel.clone());
                }
            }
            let files = match c.forbidden.as_slice() {
                [one] => one.clone(),
                many => format!("{} files: {}", many.len(), many.join(", ")),
            };
            let what = format!("changed {files}, which {} forbids: it owns {}", c.file, c.owns.join(", "));
            alerts.task_file(t, run, "forbidden", what, run.start);
        }
        if let Some((gate, failed)) = c.failed.clone() {
            // Going on is work on what it builds: a file it owns, a launch, a
            // commit or a push; the gate's evidence it keeps is none.
            let act = changed.iter().find(|rel| c.owns.contains(rel)).map(|rel| format!("changed {rel}"));
            let act = act.or_else(|| launch.as_ref().map(|l| format!("launched {}", plan::titled(l, &p.watch_jobs))));
            let act = act.or_else(|| commits(&run.what).then(|| "committed or pushed".to_string()));
            if let Some(act) = act {
                let what = format!("went on after the door gate «{gate}» failed: {act}; {} stops the build on a failed gate", c.file);
                c.past_failure = Some((run.call.clone(), format!("went_on/{gate}"), what, failed));
            }
        }
        if let Some(l) = &launch {
            // The gates that run this work, in the file's order: the first
            // not yet run takes it; none left is a gate run again.
            // A gate with no command of its own takes the file's own work
            // only: a launch some door gate of the file runs.
            let ours = c.slots.iter().filter_map(|s| s.work.as_deref()).any(|w| plan::scheduled(w, l, &p.watch_jobs));
            let gates: Vec<usize> = (0..c.slots.len()).filter(|&i| c.slots[i].work.as_deref().map_or(ours, |w| plan::scheduled(w, l, &p.watch_jobs))).collect();
            match gates.iter().find(|&&i| c.slots[i].taken.is_none()) {
                Some(&free) => {
                    let slot = &mut c.slots[free];
                    slot.taken = Some(run.call.clone());
                    slot.ran = Some(run.start);
                    slot.job = plan::label(l, &p.watch_jobs).split_whitespace().next().map(str::to_string);
                    slot.background = run.background;
                }
                None if !gates.is_empty() => {
                    let mut ids: Vec<&str> = gates.iter().map(|&i| c.slots[i].gate.as_str()).collect();
                    ids.dedup();
                    let what = format!("launched the door gate «{}» again: {} runs each of its door gates once", ids.join("», «"), c.file);
                    alerts.task_file(t, run, &format!("again/{}", ids.join(",")), what, run.start);
                    again = true;
                }
                None => {}
            }
        }
        if run.tool == "Bash" {
            for simple in simple_commands(&run.what) {
                let words = command::from_program(&simple);
                let named: Vec<String> = words.iter().map(|w| command::program(w).replace("python3", "python")).collect();
                if let Some(verb) = c.verbs.iter().find(|v| named.len() >= v.len() && named[..v.len()] == v[..]) {
                    let what = format!("ran «{}» from the checkout: {} runs its verbs only through its door gates", verb.join(" "), c.file);
                    alerts.task_file(t, run, &format!("verb/{}", verb.join(" ")), what, run.start);
                }
                if c.bucket && writes_bucket(&words) {
                    let what = format!("wrote to the bucket, which {} forbids: {simple}", c.file);
                    alerts.task_file(t, run, "bucket", what, run.start);
                }
            }
        }
        again
    }

    /// The end of `t`'s turn at `at`: a build that stopped short, when it
    /// is one.
    pub fn turn_ended(&mut self, t: &Track, at: DateTime<Utc>, alerts: &mut Alerts) -> Option<Stopped> {
        let c = self.by_session.get_mut(&t.session)?;
        let missing: Vec<&str> = c.builds.iter().filter(|b| !c.written.contains(*b)).map(String::as_str).collect();
        let waits = t.running().any(|r| r.background && (at - r.start).num_seconds() < UNSEEN_END_SECS);
        if missing.is_empty() || waits || c.stopped.is_some() {
            return None;
        }
        c.stopped = Some(at);
        let words = t.steps.iter().rev().find(|s| s.kind == StepKind::Said).map(|s| s.text.chars().take(STOP_WORDS_CHARS).collect::<String>());
        let what = format!("the build of {} handed you its turn before it wrote {}, which it builds", c.file, missing.join(" and "));
        alerts.build_stopped(t, at, c.ordered, what, words);
        let ticket = c.ticket.clone()?;
        Some(Stopped { ticket, at, how: format!("its build stopped short in {}: {} not written", t.name, missing.join(" and ")) })
    }

    /// Every build read, as the page lists it, the newest ordered first.
    #[must_use]
    pub fn rows(&self) -> Vec<BuildRow> {
        let mut rows: Vec<BuildRow> = self
            .by_session
            .iter()
            .map(|(session, c)| BuildRow {
                session: session.clone(),
                file: c.file.clone(),
                ticket: c.ticket.clone(),
                ordered: c.ordered,
                stopped: c.stopped,
                builds: c.builds.clone(),
                written: c.builds.iter().filter(|b| c.written.contains(*b)).cloned().collect(),
                forbidden: c.forbidden.clone(),
                gates: c.slots.iter().map(|s| GateRun { gate: s.gate.clone(), ends_well: s.ends_well, ran: s.ran, ended_well: s.ended_well }).collect(),
                locals: c.locals.iter().map(|l| LocalRun { gate: l.gate.clone(), ran: l.ran, passed: l.passed }).collect(),
                owns: c.owns.clone(),
                forbid: c.forbid.clone(),
                branch_rule: c.rules.branch.clone(),
                branch: c.branch.clone(),
                commits: c.commits.clone(),
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.ordered));
        rows
    }

    /// Forget the sessions no longer followed.
    pub fn trim(&mut self, sessions: &std::collections::HashSet<&String>) {
        self.by_session.retain(|s, _| sessions.contains(s));
        self.ordered.retain(|s, _| sessions.contains(s));
        self.parts.retain(|s, _| sessions.contains(s));
    }
}

impl Contract {
    /// What `xml`, the task file named `file`, decides for a build in
    /// `root`.
    fn of(xml: &str, file: String, root: String, ordered: DateTime<Utc>, p: &Params) -> Self {
        let forbid_text = element(xml, "forbid").unwrap_or_default();
        let mut slots = Vec::new();
        for (head, body) in elements(xml, "gate") {
            if plan::attribute(head, "kind").as_deref() != Some("door") {
                continue;
            }
            let gate = plan::attribute(head, "id").unwrap_or_default();
            let expect = element(body, "expect").unwrap_or_default();
            let ends_well = expect.contains("exit 0") && !expect.contains("exit 1");
            let launches: Vec<String> = elements(body, "cmd").into_iter().filter_map(|(_, cmd)| plan::launch(&plan::placeholders_whole(&plan::unescape(cmd)), &p.watch_jobs)).collect();
            if launches.is_empty() {
                slots.push(Slot { gate: gate.clone(), work: None, ends_well, taken: None, ran: None, ended_well: None, job: None, background: false });
            }
            for l in launches {
                slots.push(Slot {
                    gate: gate.clone(),
                    work: Some(plan::same_work(&l)),
                    ends_well,
                    taken: None,
                    ran: None,
                    ended_well: None,
                    job: None,
                    background: false,
                });
            }
        }
        let mut locals = Vec::new();
        for (head, body) in elements(xml, "gate") {
            if plan::attribute(head, "kind").as_deref() != Some("local") {
                continue;
            }
            let expect = plan::unescape(element(body, "expect").unwrap_or_default());
            let last_line = expect.split_once("the last line is \"").and_then(|(_, rest)| rest.split_once('"')).map(|(line, _)| line.to_string());
            if let Some((_, cmd)) = elements(body, "cmd").first() {
                let words = plan::unescape(cmd).split_whitespace().filter(|w| *w != "-v").map(|w| w.replace("python3", "python")).collect();
                locals.push(Local { gate: plan::attribute(head, "id").unwrap_or_default(), words, last_line, call: None, ran: None, passed: None });
            }
        }
        let start = plan::unescape(&element(xml, "start").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "));
        let after = |phrase: &str| start.split_once(phrase).and_then(|(_, rest)| rest.split_whitespace().next()).map(|w| w.trim_end_matches(['.', ',', ';']).to_string());
        let rules = Rules {
            branch: after("Cut the branch "),
            prefix: after("Every commit message starts with "),
            no_trailer: start.contains("no attribution trailer"),
            pull_is_yours: start.contains("the operator opens the PR"),
        };
        let task = elements(xml, "task").first().map(|(head, _)| *head).unwrap_or_default();
        let verbs = if forbid_text.contains("checkout") {
            plan::attribute(task, "command")
                .unwrap_or_default()
                .split(';')
                .map(|v| v.split_whitespace().take_while(|w| !w.starts_with('<') && !w.starts_with('[')).map(str::to_string).collect::<Vec<_>>())
                .filter(|v| v.len() > 1)
                .collect()
        } else {
            Vec::new()
        };
        let ticket = plan::attribute(task, "jira").or_else(|| task::keys(&file).into_iter().next());
        Self {
            builds: plan::attribute(task, "builds").unwrap_or_default().split_whitespace().map(str::to_string).collect(),
            written: HashSet::new(),
            ticket,
            ordered,
            forbidden: Vec::new(),
            stopped: None,
            locals,
            rules,
            branch: None,
            commits: Vec::new(),
            file,
            root,
            owns: paths(element(xml, "owns").unwrap_or_default()),
            forbid: paths(forbid_text),
            slots,
            verbs,
            bucket: forbid_text.contains("bucket"),
            failed: None,
            past_failure: None,
        }
    }

    /// A call answered: an act past a failed gate it was is a break when the
    /// call did not fail, and nothing when it was refused.
    fn answered(&mut self, t: &Track, run: &Run, alerts: &mut Alerts) {
        if self.past_failure.as_ref().is_some_and(|(call, ..)| *call == run.call) {
            let Some((_, key, what, failed)) = self.past_failure.take() else { return };
            if !run.failed {
                alerts.task_file(t, run, &key, what, failed);
                self.failed = None;
            }
        }
    }

    /// A command of the build judged against the git its file's start
    /// decides: the branch cut, each commit's message, a pull request.
    fn git(&mut self, t: &Track, run: &Run, alerts: &mut Alerts) {
        if let Some(branch) = command::new_branch(&run.what) {
            if let Some(rule) = self.rules.branch.as_deref().filter(|rule| !named(&branch, rule)) {
                let what = format!("cut the branch {branch}, not the one {} names: {rule}", self.file);
                alerts.task_file(t, run, "branch", what, run.start);
            }
            self.branch = Some(branch);
        }
        if let Some(message) = commit_message(&run.what) {
            let line = message.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
            let unprefixed = self.rules.prefix.as_deref().is_some_and(|p| !line.starts_with(p));
            let lower = message.to_lowercase();
            let trailer = self.rules.no_trailer && (lower.contains("co-authored-by:") || lower.contains("generated with"));
            if unprefixed || trailer {
                let why = if trailer { "carries an attribution trailer" } else { "does not start with its ticket" };
                let what = format!("a commit whose message {why}, which {} forbids: {line}", self.file);
                alerts.task_file(t, run, "commit", what, run.start);
            }
            self.commits.push(CommitRow { at: run.start, line: line.chars().take(200).collect(), ok: !(unprefixed || trailer), call: run.call.clone() });
        }
        if self.rules.pull_is_yours && (command::opens_pull(&run.what) || command::merges(&run.what).is_some()) {
            let what = format!("opened or merged a pull request itself: {} has you open it", self.file);
            alerts.task_file(t, run, "pull", what, run.start);
        }
    }

    /// The files of the checkout a call changes, each as the task file names
    /// it: from the checkout's top.
    fn changed(&self, t: &Track, run: &Run) -> Vec<String> {
        let mut cwd = t.folder.clone().unwrap_or_else(|| self.root.clone());
        let mut out = Vec::new();
        if FILE_TOOLS.contains(&run.tool.as_str()) && run.tool != "Read" {
            out.extend(run.path.as_deref().map(|p| resolve(&cwd, p)));
        } else if run.tool == "Bash" {
            for simple in simple_commands(&run.what) {
                let words = command::from_program(&simple);
                if words.first().map(|w| command::program(w)).as_deref() == Some("cd") {
                    if let Some(dir) = words.get(1) {
                        cwd = resolve(&cwd, dir.trim_matches(['\'', '"']));
                    }
                    continue;
                }
                out.extend(command::changed(&simple).iter().filter(|p| !p.starts_with("DDL:") && !p.contains(' ')).map(|p| resolve(&cwd, p)));
            }
        }
        let top = format!("{}/", self.root.trim_end_matches('/'));
        out.into_iter()
            .filter(|p| !GENERATED.iter().any(|g| p.contains(g) || p.ends_with(g)))
            .filter_map(|p| p.strip_prefix(&top).map(str::to_string))
            .collect()
    }
}

/// Whether `branch` is the one `rule` names: its words as written, each
/// placeholder (`<model>`) any word of letters, digits, dots and dashes.
fn named(branch: &str, rule: &str) -> bool {
    let mut rest = branch;
    let mut parts = rule.split('<').enumerate().peekable();
    while let Some((i, part)) = parts.next() {
        let (placeholder, literal) = if i == 0 { (false, part) } else { part.split_once('>').map_or((false, part), |(_, lit)| (true, lit)) };
        if placeholder {
            // A placeholder takes what comes before the next literal part.
            let taken = if literal.is_empty() && parts.peek().is_none() {
                rest.len()
            } else if literal.is_empty() {
                rest.find('-').unwrap_or(rest.len())
            } else {
                match rest.find(literal) {
                    Some(at) => at,
                    None => return false,
                }
            };
            if taken == 0 || !rest[..taken].chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
                return false;
            }
            rest = &rest[taken..];
        }
        match rest.strip_prefix(literal) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    rest.is_empty()
}

/// A `git commit`'s message, `git -C <dir> commit` too: its `-m` values, or
/// the here-document `-F -` reads.
fn commit_message(command: &str) -> Option<String> {
    let at = command.match_indices("git ").map(|(i, _)| i).find(|&i| {
        let whole = command[..i].chars().last().is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'));
        let words: Vec<&str> = command[i + 4..].split_whitespace().take(8).collect();
        whole && command::subcommand(&words).map(|j| words[j]) == Some("commit")
    })?;
    let tail = &command[at..];
    if let Some(start) = tail.find("<<") {
        let word: String = tail[start + 2..].trim_start_matches('-').trim_start().chars().take_while(|c| !c.is_whitespace()).collect();
        let word = word.trim_matches(['\'', '"']).to_string();
        let body = tail[start..].split_once('\n').map(|(_, b)| b).unwrap_or_default();
        let lines: Vec<&str> = body.lines().take_while(|l| l.trim() != word).collect();
        if !lines.is_empty() {
            return Some(lines.join("\n"));
        }
    }
    let mut out = Vec::new();
    let mut rest = tail;
    while let Some(i) = rest.find("-m ").or_else(|| rest.find("--message=")) {
        let after = rest[i..].split_once([' ', '=']).map(|(_, a)| a.trim_start()).unwrap_or_default();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'');
        let value: String = match quote {
            Some(q) => after[1..].chars().take_while(|c| *c != q).collect(),
            None => after.chars().take_while(|c| !c.is_whitespace()).collect(),
        };
        out.push(value);
        rest = &after[after.len().min(1)..];
    }
    (!out.is_empty()).then(|| out.join("\n\n"))
}

/// Whether `rel` is `glob`: a path, or anything under a folder written
/// `<folder>/**`.
fn within(rel: &str, glob: &str) -> bool {
    match glob.strip_suffix("**") {
        Some(folder) => rel.starts_with(folder),
        None => rel == glob,
    }
}

/// The paths a text names, each once: its words with a slash, the
/// punctuation after them set aside.
fn paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(|c: char| c.is_whitespace() || c == ';' || c == ',').map(|w| w.trim_end_matches(['.', ':', ')']).trim_start_matches('(')) {
        if w.contains('/') && !w.contains("://") && !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// `path` from the folder `cwd`, `.` and `..` resolved.
fn resolve(cwd: &str, path: &str) -> String {
    let joined = if path.starts_with('/') { path.to_string() } else { format!("{cwd}/{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Whether a simple command's words, from its program on, write a bucket:
/// `gcloud storage cp|mv|rm|rsync` or `gsutil` with the same, to a `gs://`
/// place (any for `rm`, the last for the others).
fn writes_bucket(words: &[String]) -> bool {
    let plain: Vec<&str> = words.iter().map(String::as_str).filter(|w| !w.starts_with('-')).collect();
    let rest = match plain.first().map(|w| command::program(w)).as_deref() {
        Some("gcloud") if plain.get(1) == Some(&"storage") => &plain[2..],
        Some("gsutil") => &plain[1..],
        _ => return false,
    };
    let Some((verb, args)) = rest.split_first() else { return false };
    BUCKET_WRITES.contains(verb)
        && if *verb == "rm" { args.iter().any(|a| a.trim_matches(['\'', '"']).starts_with("gs://")) } else { args.last().is_some_and(|a| a.trim_matches(['\'', '"']).starts_with("gs://")) }
}

/// Whether a command commits or pushes.
fn commits(command: &str) -> bool {
    !command::pushes(command).is_empty() || simple_commands(command).iter().any(|s| command::git_subcommand(s).as_deref() == Some("commit"))
}

/// The first `<tag>` of `xml`: its text.
fn element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    elements(xml, tag).into_iter().next().map(|(_, body)| body)
}

/// Every `<tag …>…</tag>` of `xml`: its head's attributes and its text.
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<(&'a str, &'a str)> {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        if !after.starts_with([' ', '>', '\n', '\t']) {
            rest = after;
            continue;
        }
        let Some(head_end) = after.find('>') else { break };
        let head = &after[..head_end];
        let body_from = &after[head_end + 1..];
        let Some(end) = body_from.find(&close) else { break };
        out.push((head, &body_from[..end]));
        rest = &body_from[end + close.len()..];
    }
    out
}
