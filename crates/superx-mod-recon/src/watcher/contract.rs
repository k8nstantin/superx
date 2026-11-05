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
//!   run from the checkout; when it names the bucket, a write to a bucket.
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
    /// The call that took it.
    taken: Option<String>,
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
    /// Whether it stopped short already.
    stopped: bool,
    /// The door gate that failed, and when, until the operator's next
    /// message.
    failed: Option<(String, DateTime<Utc>)>,
}

/// Every session's task file, once read.
#[derive(Debug, Default)]
pub struct Contracts {
    by_session: HashMap<String, Contract>,
    /// The task file each session was told to build, until it reads it:
    /// its path, the checkout and when.
    ordered: HashMap<String, (String, String, DateTime<Utc>)>,
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

    /// A call of `t`'s that ended, with its result's text: the read of the
    /// task file it was told to build, which makes its contract, or a door
    /// gate's run.
    pub fn ended(&mut self, t: &Track, run: &Run, text: &str, p: &Params) {
        if run.tool == "Read" {
            let read = run.path.as_deref().unwrap_or_default();
            if let Some((path, root, at)) = self.ordered.get(&t.session).filter(|(path, _, _)| read.ends_with(path.as_str())) {
                let file = path.rsplit('/').next().unwrap_or(path).to_string();
                let contract = Contract::of(&super::numbers_set_aside(text), file, root.clone(), *at, p);
                self.by_session.insert(t.session.clone(), contract);
                self.ordered.remove(&t.session);
            }
            return;
        }
        let Some(c) = self.by_session.get_mut(&t.session) else { return };
        let Some(slot) = c.slots.iter_mut().find(|s| s.taken.as_deref() == Some(run.call.as_str())) else { return };
        if !(EXECUTED.iter().any(|e| text.contains(e)) || (text.trim().is_empty() && !run.failed)) {
            slot.taken = None;
            return;
        }
        if slot.ends_well && (text.contains(EXECUTION_FAILED) || run.failed) {
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
            let act = changed.first().map(|rel| format!("changed {rel}")).or_else(|| launch.as_ref().map(|l| format!("launched {}", plan::titled(l, &p.watch_jobs))));
            let act = act.or_else(|| commits(&run.what).then(|| "committed or pushed".to_string()));
            if let Some(act) = act {
                let what = format!("went on after the door gate «{gate}» failed: {act}; {} stops the build on a failed gate", c.file);
                alerts.task_file(t, run, &format!("went_on/{gate}"), what, failed);
                c.failed = None;
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
                Some(&free) => c.slots[free].taken = Some(run.call.clone()),
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
        if missing.is_empty() || waits || c.stopped {
            return None;
        }
        c.stopped = true;
        let words = t.steps.iter().rev().find(|s| s.kind == StepKind::Said).map(|s| s.text.chars().take(STOP_WORDS_CHARS).collect::<String>());
        let what = format!("the build of {} handed you its turn before it wrote {}, which it builds", c.file, missing.join(" and "));
        alerts.build_stopped(t, at, c.ordered, what, words);
        let ticket = c.ticket.clone()?;
        Some(Stopped { ticket, at, how: format!("its build stopped short in {}: {} not written", t.name, missing.join(" and ")) })
    }

    /// Forget the sessions no longer followed.
    pub fn trim(&mut self, sessions: &std::collections::HashSet<&String>) {
        self.by_session.retain(|s, _| sessions.contains(s));
        self.ordered.retain(|s, _| sessions.contains(s));
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
                slots.push(Slot { gate: gate.clone(), work: None, ends_well, taken: None });
            }
            for l in launches {
                slots.push(Slot { gate: gate.clone(), work: Some(plan::same_work(&l)), ends_well, taken: None });
            }
        }
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
            stopped: false,
            file,
            root,
            owns: paths(element(xml, "owns").unwrap_or_default()),
            forbid: paths(forbid_text),
            slots,
            verbs,
            bucket: forbid_text.contains("bucket"),
            failed: None,
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

/// Whether `rel` is `glob`: a path, or anything under a folder written
/// `<folder>/**`.
fn within(rel: &str, glob: &str) -> bool {
    match glob.strip_suffix("**") {
        Some(folder) => rel.starts_with(folder),
        None => rel == glob,
    }
}

/// The paths a text names: its words with a slash, the punctuation after
/// them set aside.
fn paths(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ';' || c == ',')
        .map(|w| w.trim_end_matches(['.', ':', ')']).trim_start_matches('('))
        .filter(|w| w.contains('/') && !w.contains("://"))
        .map(str::to_string)
        .collect()
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
    !command::pushes(command).is_empty()
        || simple_commands(command).iter().any(|s| {
            let words = command::from_program(s);
            words.first().map(|w| command::program(w)).as_deref() == Some("git") && words.iter().skip(1).find(|w| !w.starts_with('-')).is_some_and(|w| w == "commit")
        })
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
