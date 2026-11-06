//! Tasks (#573): the work the operator orders, followed from the order to
//! its merge, and past it. A task is the branch the agent makes or pushes
//! for it, named with the tickets its name carries (`ENG-8078`); a session
//! works on it from then until it makes or pushes another. Each task keeps
//! what it took: when the operator last asked before it began, every
//! estimate the agent gave while on it, every merge, every reopening after
//! one, and the operator's corrections and status asks while it was worked
//! on.
//!
//! A task that never ends is critical (operator, 2026-11-05: "a task
//! becomes an infinite task, endless tailspin"): reopened after it was
//! merged, by git: a branch of it merged and pushed again, a new version of
//! it made (its branch's name but for `-v<N>` and what follows), or a shared
//! branch's history rewritten after the merge. A merge is a merge call whose
//! answer says it merged, or a merge commit the agent reads that names one
//! of the task's own branches; a call that tried and failed merged nothing.
//! The operator's word alone is not one: the operator speaks of other
//! sessions' merges too, and every merge of the operator's the history holds
//! was read back from its commit before the operator's next message. The
//! same merge read again is one merge: a number not above the newest merged,
//! or no push since the last merge. What the agent says is not done once it
//! has read a merge (`attr_recon_watch_not_done_words`) is kept in the
//! task's history and raises nothing: such words also name other work.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::alerts::{duration, Alerts};
use super::claims;
use super::command;
use super::line;
use super::track::{Run, Track};
use crate::params::Params;

/// Words before a time that make it a promise: `about 30 minutes`, `takes 8
/// minutes`, `ETA about 1 hour`.
const PROMISES: [&str; 9] = ["about", "around", "roughly", "approximately", "eta", "takes", "take", "need", "needs"];

/// Words before a time that make it a measure, not a promise: `took about
/// 12 minutes`, `the previous run took 4 minutes`.
const MEASURES: [&str; 7] = ["took", "was", "were", "ran", "last", "previous", "after"];

/// Words after a time that make it a rate or a past, not a promise: `about
/// 20 seconds per call`, `about 10 minutes ago`.
const NOT_AFTER: [&str; 3] = ["per", "ago", "each"];

/// How far before a time a promise word may stand, in words.
const PROMISE_REACH: usize = 3; // skill-allow: §9-const — grammar of an estimate, not a policy tunable

/// The longest status ask: a longer message asks more than how it stands.
const STATUS_CHARS: usize = 200; // skill-allow: §9-const — a status ask is a short message, not a policy tunable

/// One estimate the agent gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Estimate {
    pub at: DateTime<Utc>,
    pub secs: i64,
    /// The line it was given in, cut short.
    pub words: String,
}

/// One merge of a task: when it was seen, its pull request and merge commit
/// when the answer named them, and whether it was proven: when the session
/// changed a task file, whether a spec's gate passed after that change
/// (`attr_recon_watch_spec_passed`); none when it changed no task file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Merge {
    pub at: DateTime<Utc>,
    pub pull: Option<u64>,
    pub commit: Option<String>,
    pub proven: Option<bool>,
}

/// A session's newest change to a task file, and its newest gate passed.
#[derive(Debug, Clone, Copy)]
struct Spec {
    changed: DateTime<Utc>,
    passed: Option<DateTime<Utc>>,
    /// The newest merge of the session after it: a change a merge took is
    /// held against no merge after it.
    merged: Option<DateTime<Utc>>,
}

impl Spec {
    /// Whether a merge now would take a task file changed with no spec gate
    /// passed since, and no merge since either.
    fn unproven(&self) -> bool {
        self.passed.is_none_or(|p| p < self.changed) && self.merged.is_none_or(|m| m < self.changed)
    }
}

/// One task, as the watcher followed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub session: String,
    pub name: String,
    /// Its branch.
    pub branch: String,
    /// The tickets its branch names.
    pub tickets: Vec<String>,
    /// The operator's newest message before it began.
    pub asked: DateTime<Utc>,
    /// The newest act on it.
    pub last: DateTime<Utc>,
    pub estimates: Vec<Estimate>,
    pub merges: Vec<Merge>,
    /// Each reopening after a merge: when, and what did it.
    pub reopened: Vec<(DateTime<Utc>, String)>,
    pub corrections: u32,
    pub asks: u32,
    /// The session's time at work from the task's first merge to its newest
    /// act, its own jobs included: work on what was taken for done.
    pub after_merge_secs: i64,
    /// The branches made for it after a merge, newest last: its new
    /// versions.
    pub versions: Vec<String>,
    /// What the agent said was not done once it had read a merge of it,
    /// before the operator's next message.
    pub not_done: Vec<(DateTime<Utc>, String)>,
    /// Its branches merged.
    #[serde(skip)]
    merged: Vec<String>,
    /// Its newest push in the session.
    #[serde(skip)]
    pushed: Option<DateTime<Utc>>,
    /// The number of its newest pull request merged.
    #[serde(skip)]
    pull: Option<u64>,
}

impl Task {
    /// Whether `branch` is one of its branches.
    fn holds(&self, branch: &str) -> bool {
        self.branch == branch || self.versions.iter().any(|v| v == branch)
    }

    /// The branch it is worked on now: its newest version.
    fn newest(&self) -> &str {
        self.versions.last().map_or(&self.branch, String::as_str)
    }

    /// Its name on the page: its tickets, or its branch.
    #[must_use]
    pub fn title(&self) -> String {
        if self.tickets.is_empty() {
            self.branch.clone()
        } else {
            format!("{} ({})", self.tickets.join(", "), self.branch)
        }
    }
}

/// Every task of the window.
#[derive(Debug, Default)]
pub struct Tasks {
    pub list: Vec<Task>,
    /// Each session's task now, by its place in the list.
    current: HashMap<String, usize>,
    /// Each session's newest task merged.
    last_merged: HashMap<String, usize>,
    /// The pull requests each session opened, by number: the branch each
    /// carries.
    pulls: HashMap<(String, String), String>,
    /// The task each session's agent just read was merged outside the
    /// session, until the operator's next message: the agent's words after.
    after_merge: HashMap<String, usize>,
    /// Each session's task files: its newest change and gate passed.
    specs: HashMap<String, Spec>,
}

impl Tasks {
    /// The task of `branch` in `t`, made when it is new; a new version of a
    /// task merged in the session is that task again, reopened.
    fn task(&mut self, t: &Track, branch: &str, at: DateTime<Utc>, alerts: &mut Alerts) -> usize {
        if let Some(i) = self.list.iter().position(|x| x.session == t.session && x.holds(branch)) {
            return i;
        }
        let version = self.list.iter().rposition(|x| x.session == t.session && !x.merges.is_empty() && stem(&x.branch) == stem(branch));
        if let Some(i) = version {
            self.list[i].versions.push(branch.to_string());
            let since = self.list[i].merges.last().map_or(0, |m| (at - m.at).num_seconds());
            self.reopen(i, at, format!("made {branch}, a new version of it, {} after it was merged", duration(since)), alerts);
            return i;
        }
        self.list.push(Task {
            session: t.session.clone(),
            name: t.name.clone(),
            branch: branch.to_string(),
            tickets: keys(branch),
            asked: t.last_prompt.unwrap_or(at).min(at),
            last: at,
            estimates: Vec::new(),
            merges: Vec::new(),
            reopened: Vec::new(),
            corrections: 0,
            asks: 0,
            after_merge_secs: 0,
            versions: Vec::new(),
            not_done: Vec::new(),
            merged: Vec::new(),
            pushed: None,
            pull: None,
        });
        self.list.len() - 1
    }

    /// A message of the operator's to `t`: a correction or a status ask of
    /// the task in hand.
    pub fn prompt(&mut self, t: &Track, text: &str, p: &Params) {
        self.after_merge.remove(&t.session);
        let Some(&i) = self.current.get(&t.session) else { return };
        let lower = text.to_lowercase();
        let task = &mut self.list[i];
        task.corrections += u32::from(has_any(&lower, &p.correction_words));
        task.asks += u32::from(text.len() <= STATUS_CHARS && has_any(&lower, &p.watch_status_words));
    }

    /// The agent's words in `t`: what it says is not done once it has read
    /// a merge done outside the session, before the operator's next message,
    /// and an estimate for the task in hand, each kept in the task's record.
    pub fn said(&mut self, t: &Track, at: DateTime<Utc>, text: &str, alerts: &mut Alerts, p: &Params) {
        // Asked to merge with a task file changed and no spec gate passed
        // since: the merge it asks for is unproven before it is made.
        let lower = text.to_lowercase();
        if p.watch_handoff_words.iter().map(|w| w.trim().to_lowercase()).any(|w| !w.is_empty() && claims::find(&lower, &w, 0).is_some()) {
            if let Some(s) = self.specs.get(&t.session).filter(|s| s.unproven()) {
                let of = self.current.get(&t.session).map(|&i| format!(" {}", self.list[i].title())).unwrap_or_default();
                let what = format!(
                    "asks you to merge{of} with its task file changed {} before, and no spec gate passed since that change",
                    duration((at - s.changed).num_seconds().max(0))
                );
                alerts.unproven(&t.session, &t.name, what, s.changed, at);
            }
        }
        if let Some(&m) = self.after_merge.get(&t.session) {
            if has_any(&text.to_lowercase(), &p.watch_not_done_words) {
                self.after_merge.remove(&t.session);
                let line = text.lines().find(|l| has_any(&l.to_lowercase(), &p.watch_not_done_words)).unwrap_or(text);
                let line: String = line.trim().trim_start_matches(['-', '*', ' ']).chars().take(200).collect();
                self.list[m].not_done.push((at, line));
            }
        }
        let Some(&i) = self.current.get(&t.session) else { return };
        let Some(secs) = estimate(text) else { return };
        let words: String = text.lines().find(|l| estimate(l).is_some()).unwrap_or(text).trim().chars().take(240).collect();
        let task = &mut self.list[i];
        task.last = task.last.max(at);
        task.estimates.push(Estimate { at, secs, words });
    }

    /// A call of `t`'s that ended well, with its result's text: the branch
    /// it makes or pushes, a merge it did, and a reopening of a task merged
    /// before.
    pub fn ran(&mut self, t: &Track, run: &Run, text: &str, alerts: &mut Alerts, p: &Params) {
        let at = run.start;
        if let Some(&i) = self.current.get(&t.session) {
            self.list[i].last = self.list[i].last.max(at);
        }
        // A task file changed, by a file tool or a command; a spec's gate
        // passed: what a merge after them is proven by.
        let task_file = |path: &str| path.ends_with(".xml") && p.watch_plans.iter().any(|m| !m.is_empty() && path.contains(m.as_str()));
        let wrote = if run.tool == "Bash" {
            command::changed(&run.what).iter().any(|c| task_file(c)) || command::program_writes(&run.what).iter().any(|c| task_file(c))
        } else {
            line::FILE_TOOLS.contains(&run.tool.as_str()) && run.tool != "Read" && run.path.as_deref().is_some_and(task_file)
        };
        if wrote {
            let spec = self.specs.entry(t.session.clone()).or_insert(Spec { changed: at, passed: None, merged: None });
            spec.changed = spec.changed.max(at);
        }
        if text.lines().any(|l| p.watch_spec_passed.iter().any(|w| !w.trim().is_empty() && l.trim_start().starts_with(w.trim()))) {
            if let Some(spec) = self.specs.get_mut(&t.session) {
                spec.passed = Some(run.end.unwrap_or(at));
            }
        }
        if run.tool != "Bash" {
            return;
        }
        if let Some(branch) = command::new_branch(&run.what) {
            let i = self.task(t, &branch, at, alerts);
            self.current.insert(t.session.clone(), i);
        }
        for push in command::pushes(&run.what) {
            let shared = p.watch_shared_branches.contains(&push.branch);
            if shared {
                if !push.force {
                    continue;
                }
                // A shared branch rewritten undoes what was merged into it:
                // the merge whose commit it was, in whatever session it was
                // read, or else the session's newest merged.
                let moved = command::forced_updates(text).into_iter().find(|(b, _, _)| *b == push.branch);
                let undone = moved.as_ref().and_then(|(_, was, _)| self.merged_as(was));
                // A merge undone took no change: its task file waits on a
                // merge again, and the next is held to it.
                let session = undone.map_or_else(|| t.session.clone(), |(i, _)| self.list[i].session.clone());
                if let Some(spec) = self.specs.get_mut(&session) {
                    spec.merged = None;
                }
                match (undone, moved) {
                    (Some((i, pull)), Some((branch, was, is))) => {
                        let pull = pull.map(|n| format!(" #{n}")).unwrap_or_default();
                        self.reopen(i, at, format!("undid its merge{pull}: {branch} put back from {} to {}", short(&was), short(&is)), alerts);
                    }
                    _ => {
                        let Some(&i) = self.last_merged.get(&t.session) else { continue };
                        self.reopen(i, at, format!("rewrote the shared branch {}: {}", push.branch, one_line(&run.what)), alerts);
                    }
                }
                continue;
            }
            let i = if push.branch.is_empty() {
                match self.current.get(&t.session) {
                    Some(&i) => i,
                    None => continue,
                }
            } else {
                self.task(t, &push.branch, at, alerts)
            };
            self.current.insert(t.session.clone(), i);
            let task = &mut self.list[i];
            task.last = task.last.max(at);
            task.pushed = Some(at);
            // A branch merged and pushed again; a new version's first push
            // is the reopening its making already counted.
            let branch = if push.branch.is_empty() { task.newest().to_string() } else { push.branch.clone() };
            if let (Some(m), true) = (task.merges.last().map(|m| m.at), task.merged.contains(&branch)) {
                if at > m {
                    let how = format!("pushed {branch} again, {} after it was merged", duration((at - m).num_seconds()));
                    self.reopen(i, at, how, alerts);
                }
            }
        }
        if command::opens_pull(&run.what) {
            if let (Some(n), Some(&i)) = (command::pr_number(text), self.current.get(&t.session)) {
                self.pulls.insert((t.session.clone(), n), self.list[i].branch.clone());
            }
        }
        // A merge is one its answer says was done, of the branch its pull
        // request carries: the one named, or the branch checked out.
        let done = text.to_lowercase().contains("merged pull request") || text.split(|c: char| !c.is_ascii_alphabetic()).any(|w| w == "MERGED");
        if let Some(target) = command::merges(&run.what).filter(|_| done) {
            let (branch, number) = match target {
                None => (self.current.get(&t.session).map(|&i| self.list[i].branch.clone()), None),
                Some(sel) if sel.chars().all(|c| c.is_ascii_digit()) => (self.pulls.get(&(t.session.clone(), sel.clone())).cloned(), sel.parse().ok()),
                Some(sel) => (Some(sel), None),
            };
            if let Some(branch) = branch {
                let i = self.task(t, &branch, at, alerts);
                self.merged(&t.session, i, &branch, Merge { at, pull: number, commit: None, proven: None }, false, alerts);
            }
        }
        // A merge done outside the session, on the host: the merge commit
        // the agent reads names one of the session's own branches.
        for m in command::merge_commits(text) {
            if let Some(i) = self.list.iter().position(|x| x.session == t.session && x.holds(&m.branch)) {
                self.merged(&t.session, i, &m.branch, Merge { at, pull: Some(m.pull), commit: m.commit, proven: None }, true, alerts);
            }
        }
    }

    /// A merge of `branch`, of task `i`; `seen` when the agent read it, done
    /// outside the session. The same merge read again is one: a pull request
    /// not above the newest merged, or, its number unknown, no push since
    /// the last merge.
    fn merged(&mut self, session: &str, i: usize, branch: &str, mut merge: Merge, seen: bool, alerts: &mut Alerts) {
        let spec = self.specs.get(session).copied();
        let task = &mut self.list[i];
        let at = merge.at;
        let same = match (merge.pull, task.pull) {
            (Some(n), Some(p)) => n <= p,
            _ => task.merges.last().is_some_and(|m| task.pushed.is_none_or(|p| p <= m.at)),
        };
        if same {
            return;
        }
        task.pull = merge.pull.or(task.pull);
        if !task.merged.iter().any(|b| b == branch) {
            task.merged.push(branch.to_string());
        }
        // A change an earlier merge took is no part of this one.
        let spec = spec.filter(|s| s.merged.is_none_or(|m| m < s.changed));
        merge.proven = spec.map(|s| !s.unproven());
        if let Some(s) = self.specs.get_mut(session) {
            s.merged = Some(at);
        }
        let task = &mut self.list[i];
        if let (Some(false), Some(s)) = (merge.proven, spec) {
            let pull = merge.pull.map(|n| format!(" #{n}")).unwrap_or_default();
            let what = format!("merged{pull} of {} after its task file changed, and no spec gate passed since that change", task.title());
            alerts.unproven(&task.session, &task.name, what, s.changed, at);
        }
        task.merges.push(merge);
        task.last = task.last.max(at);
        self.last_merged.insert(session.to_string(), i);
        if seen {
            self.after_merge.insert(session.to_string(), i);
        }
    }

    /// The task one of whose merges is the commit `commit`, in any session,
    /// and that merge's pull request.
    fn merged_as(&self, commit: &str) -> Option<(usize, Option<u64>)> {
        self.list.iter().enumerate().rev().find_map(|(i, t)| {
            t.merges.iter().rev().find(|m| m.commit.as_deref().is_some_and(|c| command::same_commit(c, commit))).map(|m| (i, m.pull))
        })
    }

    /// A build of `ticket`'s task file, in `session`, that stopped short at
    /// `at`: the task of that ticket merged before it in another session,
    /// the newest, was taken for done and is reopened.
    pub fn build_stopped(&mut self, session: &str, ticket: &str, at: DateTime<Utc>, how: String, alerts: &mut Alerts) {
        let merged = |t: &Task| t.merges.last().map(|m| m.at).filter(|&m| m < at);
        let Some(i) = (0..self.list.len())
            .filter(|&i| self.list[i].session != session && self.list[i].tickets.iter().any(|k| k == ticket))
            .filter_map(|i| merged(&self.list[i]).map(|m| (m, i)))
            .max()
            .map(|(_, i)| i)
        else {
            return;
        };
        let since = merged(&self.list[i]).map_or(0, |m| (at - m).num_seconds());
        self.reopen(i, at, format!("{how}, {} after it was merged", duration(since)), alerts);
    }

    fn reopen(&mut self, i: usize, at: DateTime<Utc>, how: String, alerts: &mut Alerts) {
        let task = &mut self.list[i];
        task.last = task.last.max(at);
        task.reopened.push((at, how.clone()));
        let what = format!(
            "{} never ends: merged {} times and reopened {} times; the newest: {how}",
            task.title(),
            task.merges.len(),
            task.reopened.len()
        );
        let from = task.merges.last().map_or(task.asked, |m| m.at).min(at);
        alerts.endless(task, "reopened", what, from, at, (at - task.asked).num_seconds());
    }

    /// Forget the tasks untouched since `from`, and those of sessions no
    /// longer followed.
    pub fn trim(&mut self, from: DateTime<Utc>, sessions: &HashSet<&String>) {
        let keep: Vec<bool> = self.list.iter().map(|t| t.last >= from && sessions.contains(&t.session)).collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        let mut index = HashMap::new();
        let mut list = Vec::new();
        for (i, task) in std::mem::take(&mut self.list).into_iter().enumerate() {
            if keep[i] {
                index.insert(i, list.len());
                list.push(task);
            }
        }
        self.list = list;
        self.current = self.current.iter().filter_map(|(s, i)| index.get(i).map(|j| (s.clone(), *j))).collect();
        self.last_merged = self.last_merged.iter().filter_map(|(s, i)| index.get(i).map(|j| (s.clone(), *j))).collect();
        self.pulls.retain(|(s, _), _| sessions.contains(s));
        self.after_merge = self.after_merge.iter().filter_map(|(s, i)| index.get(i).map(|j| (s.clone(), *j))).collect();
    }
}

/// A branch's name without its version: what comes before its last `-v<N>`
/// that ends the name or a dash does (`ENG-8078-task-file-v2`,
/// `feat/573-watcher-v2-opus-5-5-max`); the name itself when it has none.
#[must_use]
pub fn stem(branch: &str) -> &str {
    let mut cut = branch.len();
    for (i, mark) in branch.match_indices("-v") {
        let rest = &branch[i + mark.len()..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && (rest.len() == digits || rest[digits..].starts_with('-')) {
            cut = i;
        }
    }
    &branch[..cut]
}

/// Whether `text`, lowercase, holds one of `words` as whole words.
#[must_use]
pub fn has_any(text: &str, words: &[String]) -> bool {
    words.iter().map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).any(|w| {
        text.match_indices(&w).any(|(i, _)| {
            let before = text[..i].chars().next_back();
            let after = text[i + w.len()..].chars().next();
            before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
        })
    })
}

/// The ticket keys a branch's name carries, in order: an upper-case word, a
/// dash and a number, `ENG-8078`; or the issue a branch named for one
/// carries, `feat/573-watcher` its `#573`.
#[must_use]
pub fn keys(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    if let Some((kind, rest)) = text.split_once('/') {
        let issue: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase()) && !issue.is_empty() && rest[issue.len()..].starts_with('-') {
            out.push(format!("#{issue}"));
        }
    }
    let mut i = 0;
    while i < chars.len() {
        if !(chars[i].is_ascii_uppercase() && (i == 0 || !chars[i - 1].is_alphanumeric())) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && (chars[j].is_ascii_uppercase() || chars[j].is_ascii_digit()) {
            j += 1;
        }
        let mut k = j + 1;
        while k < chars.len() && chars[k].is_ascii_digit() {
            k += 1;
        }
        let ends = k >= chars.len() || !chars[k].is_alphanumeric();
        if j - i >= 2 && j < chars.len() && chars[j] == '-' && k > j + 1 && ends {
            let key: String = chars[i..k].iter().collect();
            if !out.contains(&key) {
                out.push(key);
            }
            i = k;
        } else {
            i = j.max(i + 1);
        }
    }
    out
}

/// The longest time the agent's words promise, in seconds: `about 30 to 40
/// minutes`, `takes about 8 minutes`, `ETA about 1½ hours`, `~2 h`.
#[must_use]
pub fn estimate(words: &str) -> Option<i64> {
    let lower = words.to_lowercase().replace('½', ".5").replace(['–', '—'], " - ");
    let tokens: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | ':' | '*'))
        .filter(|t| !t.is_empty())
        .collect();
    let mut best: Option<i64> = None;
    for (i, raw) in tokens.iter().enumerate() {
        let before = &tokens[i.saturating_sub(PROMISE_REACH)..i];
        let promised = before.iter().any(|w| PROMISES.contains(&w.trim_start_matches('~')));
        let measured = tokens[i.saturating_sub(PROMISE_REACH + 1)..i].iter().any(|w| MEASURES.contains(w));
        if !promised || measured {
            continue;
        }
        let tok = raw.trim_start_matches('~');
        let (lo, hi) = tok.split_once('-').filter(|(a, b)| !a.is_empty() && !b.is_empty()).unwrap_or((tok, ""));
        let (Some(mut tenths), mut unit) = tenths_then(lo) else { continue };
        let mut next = i + 1;
        if !hi.is_empty() {
            if let (Some(n), u) = tenths_then(hi) {
                tenths = tenths.max(n);
                unit = u.or(unit);
            }
        } else if unit.is_none() && matches!(tokens.get(next).copied(), Some("to" | "-" | "or")) {
            if let Some((Some(n), u)) = tokens.get(next + 1).map(|t| tenths_then(t)) {
                tenths = tenths.max(n);
                unit = u;
                next += 2;
            }
        }
        let unit_at = if unit.is_some() { next } else { next + 1 };
        let unit = unit.or_else(|| tokens.get(next).and_then(|t| unit_secs(t.trim_end_matches('.'))));
        let rate = tokens.get(unit_at).is_some_and(|w| NOT_AFTER.contains(&w.trim_end_matches('.')));
        if let Some(u) = unit.filter(|u| *u >= 60 && !rate) {
            let secs = tenths * u / 10;
            best = Some(best.map_or(secs, |b| b.max(secs)));
        }
    }
    best
}

/// A number at the start of a token in tenths, and the unit attached to
/// it, if one is: `40`, `3.5`, `2h`, `30min`.
fn tenths_then(token: &str) -> (Option<i64>, Option<i64>) {
    let whole: String = token.chars().take_while(char::is_ascii_digit).collect();
    let mut rest = &token[whole.len()..];
    let mut tenth = 0;
    if let Some(after) = rest.strip_prefix('.') {
        if let Some(d) = after.chars().next().and_then(|c| c.to_digit(10)) {
            tenth = i64::from(d);
            rest = after.trim_start_matches(|c: char| c.is_ascii_digit());
        }
    }
    let number = whole.parse::<i64>().ok().map(|n| n * 10 + tenth).filter(|n| *n > 0);
    let rest = rest.trim_end_matches('.');
    (number, if rest.is_empty() { None } else { unit_secs(rest) })
}

/// A unit of time, in seconds.
fn unit_secs(word: &str) -> Option<i64> {
    match word {
        "s" | "sec" | "secs" | "second" | "seconds" => Some(1),
        "min" | "mins" | "minute" | "minutes" => Some(60),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(3_600),
        _ => None,
    }
}

/// A commit written short: its first 8 digits.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect()
}
