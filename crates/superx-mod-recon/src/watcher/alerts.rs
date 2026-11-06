//! The watcher's alerts (#573): the critical ones alone, each raised once
//! (operator, 2026-11-05: the first watcher "spewed too many alerts").
//!
//! - **Finished work run again**: a launch the same as one that already ran
//!   to its end, when that run took `attr_recon_watch_repeat_secs` or more.
//!   Its cost is what the first run took.
//! - **A run going long**: a background job past `attr_recon_watch_long_secs`,
//!   or past twice the run it repeats, whichever comes first.
//! - **Your stop not followed**: after a message that starts with one of
//!   `attr_recon_watch_stop_words`, or an interrupt, a file written, a job
//!   launched or a git change made before the agent's next words.
//! - **A plan that repeats full runs**: a task file a session reads or
//!   writes that launches the same job more than once.
//! - **A task that never ends** ([`super::task`]): reopened after it was
//!   merged.
//! - **Its task file broken** ([`super::contract`]): a session building from
//!   a task file changed what it forbids, ran a door gate again, went on
//!   after a door gate failed, or reached the real data outside its gates.
//! - **A build stopped short** ([`super::contract`]): a session building from
//!   a task file handed the operator its turn before it wrote every file the
//!   file builds.
//! - **A task file merged unproven** ([`super::task`]): a merge of a branch
//!   whose task file changed in its session with no spec gate passed since
//!   (operator, 2026-11-05: "merge = done"; every redo of that day followed
//!   one, and no merge a gate passed was undone). Raised already when the
//!   agent asks for that merge, before it is made; one alert for each change.
//! - **Its fix covers less than it found** ([`super::claims`]): the agent
//!   counted N of a kind of thing broken, then planned or reported a fix of
//!   fewer of the same kind (2026-11-06: 724 damaged routines, 723 fixed, one
//!   left out on the agent's own call with a false reason).
//! - **In its own words** ([`super::claims`]): the agent's own sentence that
//!   it acted on its own, without the operator; that what it said was not
//!   so; or that it got something wrong (operator, 2026-11-06: "the agent
//!   constantly lies").
//! - **No answer from the API** ([`super::track::Unanswered`]): a stretch
//!   with no answer that ended the agent's turn on Claude Code giving up, or
//!   that went `attr_recon_watch_api_wait_secs` with its request sent again;
//!   the stretches that follow within that span are the same alert, said
//!   again as it grows, with the other sessions the API left without an
//!   answer in the same minutes (2026-11-05: an outage held three sessions,
//!   one a build the operator watched "thinking" for twenty minutes). A long
//!   answer with nothing sent again is the model at work: the session's
//!   card shows it as it waits, and no alert.
//!
//! Only the kinds `attr_recon_watch_alarms` names are alarms; the page charts
//! every other one with what the watcher found (operator, 2026-11-06: "the
//! alarms will flood and the operator will ignore them").
//!
//! Each alert names the call it is about and the time its cause starts
//! from, for the page to read the session's lines between; and the alert
//! that caused it, when one did: the plan that launches a job run again, the
//! run again that went long.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};

use chrono::{DateTime, TimeDelta, Utc};
use serde::de::{value, IntoDeserializer as _};
use serde::{Deserialize, Serialize};

use super::command::ITS_OWN;
use super::line::FILE_TOOLS;
use super::plan::{self, Repeat};
use super::task::Task;
use super::claims::plural;
use super::track::{Run, Track, Unanswered};
use crate::params::Params;

/// How long after a stop an act still counts as not following it.
const STOP_WINDOW_SECS: i64 = 600; // skill-allow: §9-const — one turn's reach, not a policy tunable

/// How many times its bound a job with no notice is still judged.
const OPEN_HORIZON: i64 = 4; // skill-allow: §9-const — past it an unseen end is likelier than a run, not a policy tunable

/// The git commands that change a repository.
const GIT_CHANGES: [&str; 7] = ["commit", "push", "merge", "reset", "rebase", "checkout", "branch"];

/// What an alert is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RunAgain,
    LongRun,
    StopNotFollowed,
    PlanRepeats,
    EndlessTask,
    TaskFileBroken,
    BuildStopped,
    UnprovenMerge,
    ApiStalled,
    CountGap,
    ActedAlone,
    SaidFalse,
    GotWrong,
}

impl Kind {
    /// The kind a word names, as the page and `attr_recon_watch_alarms`
    /// name it: `count_gap`.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::deserialize(word.trim().into_deserializer()).map_err(|_: value::Error| ()).ok()
    }
}

/// One alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alert {
    /// The session's id, its kind and what it is about: the same at every
    /// look, so the page tells a new alert by it.
    pub id: String,
    pub session: String,
    pub name: String,
    pub kind: Kind,
    pub at: DateTime<Utc>,
    /// One sentence of the facts.
    pub what: String,
    /// When the run it repeats started, for the page to show in your time.
    pub since: Option<DateTime<Utc>>,
    /// The time it puts at stake, in seconds.
    pub cost_secs: Option<i64>,
    /// The launch, the operator's words or the plan's gates, whole.
    pub evidence: Vec<String>,
    /// The call it is about: the run launched again, the run gone long, the
    /// act after a stop, the plan's read or write.
    pub call: Option<String>,
    /// Where its cause starts: the session's lines from here to `at` tell
    /// it.
    pub from: DateTime<Utc>,
    /// What the run launched, for a run's alert.
    pub launch: Option<String>,
    /// The jobs a plan launches more than once, for a plan's alert.
    pub repeats: Vec<Repeat>,
    /// The alert that caused this one, by its id.
    pub cause: Option<String>,
    /// The stretches the API did not answer, for its alert.
    pub stretches: Vec<Unanswered>,
    /// The other sessions the API left without an answer in the same
    /// minutes, for its alert.
    pub alongside: Vec<Alongside>,
}

/// Another session the API left without an answer in the same minutes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alongside {
    pub session: String,
    pub name: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

/// Every alert raised in the window, each once.
#[derive(Debug, Clone, Default)]
pub struct Alerts {
    pub list: Vec<Alert>,
    ids: HashSet<String>,
}

impl Alerts {
    fn raise(&mut self, alert: Alert) {
        if self.ids.insert(alert.id.clone()) {
            self.list.push(alert);
        } else if alert.kind == Kind::LongRun {
            // A run gone long goes on costing until it ends.
            if let Some(a) = self.list.iter_mut().find(|a| a.id == alert.id) {
                a.cost_secs = alert.cost_secs;
            }
        }
    }

    /// A run that just started, launching what an earlier run launched.
    pub fn run_again<'a>(&mut self, track: &Track, run: &Run, every: impl Iterator<Item = &'a Track>, p: &Params) {
        let Some(launch) = run.launch.as_deref() else { return };
        let earlier = every
            .flat_map(|t| t.runs.iter().map(move |r| (t, r)))
            .filter(|(_, r)| {
                r.launch.as_deref() == Some(launch)
                    && r.ended_well()
                    && r.end.is_some_and(|e| e <= run.start)
                    && r.measured(run.start)
                    && r.secs(run.start) >= secs(p.watch_repeat_secs)
            })
            .max_by_key(|(_, r)| r.start);
        let Some((t, first)) = earlier else { return };
        let took = first.secs(run.start);
        let label = run.label(&p.watch_jobs);
        let whose = if t.session == track.session { String::new() } else { format!(" by {}", t.name) };
        self.raise(Alert {
            id: format!("{}/run_again/{}", track.session, run.call),
            session: track.session.clone(),
            name: track.name.clone(),
            kind: Kind::RunAgain,
            at: run.start,
            what: format!("{label} launched again: the same launch{whose} already ran to its end, in {}", duration(took)),
            since: Some(first.start),
            cost_secs: Some(took),
            evidence: vec![launch.to_string(), first.what.clone()],
            call: Some(run.call.clone()),
            from: first.end.unwrap_or(first.start).min(run.start),
            launch: Some(launch.to_string()),
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// Every background job past its bound by `now`.
    pub fn long_runs<'a>(&mut self, tracks: impl Iterator<Item = &'a Track> + Clone, now: DateTime<Utc>, p: &Params) {
        for t in tracks.clone() {
            for run in t.runs.iter().filter(|r| r.background) {
                let earlier = run.launch.as_deref().and_then(|l| {
                    tracks
                        .clone()
                        .flat_map(|t| t.runs.iter())
                        .filter(|r| r.launch.as_deref() == Some(l) && r.ended_well() && r.end.is_some_and(|e| e <= run.start))
                        .map(|r| r.secs(run.start))
                        .filter(|s| *s >= secs(p.watch_repeat_secs))
                        .max()
                });
                let bound = earlier.map_or(secs(p.watch_long_secs), |e| (2 * e).min(secs(p.watch_long_secs)));
                let crossed = run.start + TimeDelta::seconds(bound);
                // A job with no notice yet is judged while it is recent: one
                // whose end was never seen (a session closed, a shell gone)
                // is not known to run, and raises nothing.
                let running_past = run.end.map_or(crossed <= now && now - run.start <= TimeDelta::seconds(OPEN_HORIZON * bound), |e| e >= crossed);
                if !running_past {
                    continue;
                }
                let label = run.label(&p.watch_jobs);
                let why = match earlier {
                    Some(e) if 2 * e < secs(p.watch_long_secs) => format!("twice the {} the same launch took before", duration(e)),
                    _ => duration(secs(p.watch_long_secs)),
                };
                self.raise(Alert {
                    id: format!("{}/long_run/{}", t.session, run.call),
                    session: t.session.clone(),
                    name: t.name.clone(),
                    kind: Kind::LongRun,
                    at: crossed,
                    what: format!("{label} has run past {why}"),
                    since: Some(run.start),
                    cost_secs: Some(run.secs(now)),
                    evidence: vec![run.what.clone()],
                    call: Some(run.call.clone()),
                    from: run.start,
                    launch: run.launch.clone(),
                    repeats: Vec::new(),
                    cause: None,
                    stretches: Vec::new(),
                    alongside: Vec::new(),
                });
            }
        }
    }

    /// A run that just started, while a stop of the operator's stands.
    pub fn stop_not_followed(&mut self, track: &Track, run: &Run) {
        let Some((at, words)) = &track.stop else { return };
        if run.start - *at > TimeDelta::seconds(STOP_WINDOW_SECS) || !acts(run) {
            return;
        }
        self.raise(Alert {
            id: format!("{}/stop_not_followed/{}", track.session, at.to_rfc3339()),
            session: track.session.clone(),
            name: track.name.clone(),
            kind: Kind::StopNotFollowed,
            at: run.start,
            what: format!("after your stop it {} before answering you", act_words(run)),
            since: Some(*at),
            cost_secs: None,
            evidence: vec![words.clone(), run.what.clone()],
            call: Some(run.call.clone()),
            from: *at,
            launch: run.launch.clone(),
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// A run that broke the task file its session builds from: `key` names
    /// the rule and what broke it, once a session; `from` is where its cause
    /// starts.
    pub fn task_file(&mut self, track: &Track, run: &Run, key: &str, what: String, from: DateTime<Utc>) {
        let id = format!("{}/task_file_broken/{key}", track.session);
        // The same rule broken again: one alert, saying it all.
        if let Some(a) = self.list.iter_mut().find(|a| a.id == id) {
            a.what = what;
            a.at = a.at.max(run.start);
            return;
        }
        self.raise(Alert {
            id,
            session: track.session.clone(),
            name: track.name.clone(),
            kind: Kind::TaskFileBroken,
            at: run.start,
            what,
            since: None,
            cost_secs: None,
            evidence: vec![run.what.clone()],
            call: Some(run.call.clone()),
            from,
            launch: run.launch.clone(),
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// A merge of `task` at `at` whose task file changed at `changed` with no
    /// spec gate passed since; `key` names the merge.
    /// A merge, or the agent's ask for one, after a task file changed in
    /// `session` with no spec gate passed since: one alert for each change,
    /// said again at each merge.
    pub fn unproven(&mut self, session: &str, name: &str, what: String, changed: DateTime<Utc>, at: DateTime<Utc>) {
        let id = format!("{session}/unproven_merge/{}", changed.timestamp_millis());
        if let Some(a) = self.list.iter_mut().find(|a| a.id == id) {
            a.what = what;
            a.at = a.at.max(at);
            return;
        }
        self.raise(Alert {
            id,
            session: session.to_string(),
            name: name.to_string(),
            kind: Kind::UnprovenMerge,
            at,
            what,
            since: Some(changed),
            cost_secs: None,
            evidence: Vec::new(),
            call: None,
            from: changed,
            launch: None,
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// A build that handed the operator its turn at `at` before it wrote
    /// what its task file builds, from its order at `from`; `words`, the
    /// agent's last.
    pub fn build_stopped(&mut self, track: &Track, at: DateTime<Utc>, from: DateTime<Utc>, what: String, words: Option<String>) {
        self.raise(Alert {
            id: format!("{}/build_stopped", track.session),
            session: track.session.clone(),
            name: track.name.clone(),
            kind: Kind::BuildStopped,
            at,
            what,
            since: Some(from),
            cost_secs: Some((at - from).num_seconds()),
            evidence: words.into_iter().collect(),
            call: None,
            from,
            launch: None,
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// A run that read or wrote `text`, the file at its path, when the path
    /// is one of the operator's plans.
    pub fn plan(&mut self, track: &Track, run: &Run, text: &str, p: &Params) {
        let Some(path) = run.path.as_deref() else { return };
        if !p.watch_plans.iter().any(|m| !m.is_empty() && path.contains(m.as_str())) {
            return;
        }
        let repeats = plan::repeats(text, &p.watch_jobs);
        if repeats.is_empty() {
            return;
        }
        // The same plan with a job renamed repeats the same runs: what is
        // repeated, and how often, names it.
        let mut h = DefaultHasher::new();
        for r in &repeats {
            plan::label(&r.job, &p.watch_jobs).split_whitespace().skip(1).collect::<Vec<_>>().hash(&mut h);
            r.times.hash(&mut h);
        }
        let said: Vec<String> = repeats.iter().map(|r| format!("{} {} times", plan::label(&r.job, &p.watch_jobs), r.times)).collect();
        let file = path.rsplit('/').next().unwrap_or(path);
        // One alert a plan as it stands, whoever reads it.
        self.raise(Alert {
            id: format!("plan_repeats/{file}/{:x}", h.finish()),
            session: track.session.clone(),
            name: track.name.clone(),
            kind: Kind::PlanRepeats,
            at: run.end.unwrap_or(run.start),
            what: format!("the plan in {file} launches {}", said.join(" and ")),
            since: None,
            cost_secs: None,
            evidence: repeats.iter().map(|r| format!("{}: {}", r.job, r.by.join("; "))).collect(),
            call: Some(run.call.clone()),
            from: run.start,
            launch: None,
            repeats,
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// A task that never ends, shown `how` (`reopened`): raised once, then
    /// kept up to date as the task goes on.
    pub fn endless(&mut self, task: &Task, how: &str, what: String, from: DateTime<Utc>, at: DateTime<Utc>, cost: i64) {
        let id = format!("{}/endless_task/{}/{how}", task.session, task.branch);
        if let Some(a) = self.list.iter_mut().find(|a| a.id == id) {
            a.what = what;
            a.at = a.at.max(at);
            a.cost_secs = Some(cost);
            return;
        }
        self.raise(Alert {
            id,
            session: task.session.clone(),
            name: task.name.clone(),
            kind: Kind::EndlessTask,
            at,
            what,
            since: Some(task.asked),
            cost_secs: Some(cost),
            evidence: task.reopened.iter().map(|(_, how)| how.clone()).chain(task.estimates.iter().map(|e| e.words.clone())).collect(),
            call: None,
            from,
            launch: None,
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// Forget alerts before `from`.
    /// A stretch the API did not answer, as it stands at `at`: an alert once
    /// Claude Code giving up ended the agent's turn, or once it went
    /// `attr_recon_watch_api_wait_secs` with its request sent again. A
    /// stretch that starts within that span of the session's alert of the
    /// API's ending is that alert's.
    pub fn api(&mut self, track: &Track, u: &Unanswered, at: DateTime<Utc>, p: &Params) {
        let span = secs(p.watch_api_wait_secs);
        let theirs = |a: &Alert| a.kind == Kind::ApiStalled && a.session == track.session;
        let i = if let Some(i) = self.list.iter().position(|a| theirs(a) && a.stretches.iter().any(|s| s.since == u.since)) {
            i
        } else if u.turn_ended.is_none() && (u.resent == 0 || u.secs(at) < span) {
            return;
        } else if let Some(i) = self.list.iter().rposition(|a| theirs(a) && (u.since - a.at).num_seconds() <= span) {
            self.list[i].stretches.push(u.clone());
            i
        } else {
            self.raise(Alert {
                id: format!("{}/api_stalled/{}", track.session, u.since.timestamp_millis()),
                session: track.session.clone(),
                name: track.name.clone(),
                kind: Kind::ApiStalled,
                at: u.end(at),
                what: unanswered(std::slice::from_ref(u), at, 0),
                since: Some(u.since),
                cost_secs: Some(u.secs(at)),
                evidence: u.error.iter().cloned().collect(),
                call: None,
                from: u.since,
                launch: None,
                repeats: Vec::new(),
                cause: None,
                stretches: vec![u.clone()],
                alongside: Vec::new(),
            });
            return;
        };
        let a = &mut self.list[i];
        if let Some(s) = a.stretches.iter_mut().find(|s| s.since == u.since) {
            *s = u.clone();
        }
        a.at = a.at.max(u.end(at));
        a.cost_secs = Some(a.stretches.iter().map(|s| s.secs(a.at)).sum());
        a.evidence = a.stretches.iter().rev().find_map(|s| s.error.clone()).into_iter().collect();
        a.what = unanswered(&a.stretches, a.at, a.alongside.len());
    }

    /// A fix the agent plans or reports for `covers` of `unit`, after it
    /// found more of them broken: the count, when and in which words.
    pub fn count_gap(&mut self, track: &Track, at: DateTime<Utc>, unit: &str, found: (u64, DateTime<Utc>, &str), covers: (u64, &str)) {
        let ((n, found_at, found_words), (m, words)) = (found, covers);
        self.raise(Alert {
            id: format!("{}/count_gap/{unit}/{}/{m}", track.session, found_at.timestamp()),
            session: track.session.clone(),
            name: track.name.clone(),
            kind: Kind::CountGap,
            at,
            what: format!("its fix covers {m} of the {n} {} it found: «{}», after «{}»", plural(unit), cut(words), cut(found_words)),
            since: Some(found_at),
            cost_secs: None,
            evidence: vec![found_words.to_string(), words.to_string()],
            call: None,
            from: found_at,
            launch: None,
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// The agent's own sentence of `kind`: that it acted on its own, said
    /// what was not so, or got something wrong; its cause starts at the
    /// operator's message before.
    pub fn admitted(&mut self, track: &Track, at: DateTime<Utc>, kind: Kind, words: &str) {
        let name = match kind {
            Kind::SaidFalse => "said_false",
            Kind::ActedAlone => "acted_alone",
            _ => "got_wrong",
        };
        self.raise(Alert {
            id: format!("{}/{name}/{}", track.session, at.timestamp_millis()),
            session: track.session.clone(),
            name: track.name.clone(),
            kind,
            at,
            what: format!("it admits: «{}»", cut(words)),
            since: None,
            cost_secs: None,
            evidence: vec![words.to_string()],
            call: None,
            from: track.last_prompt.filter(|p| *p <= at).unwrap_or(at),
            launch: None,
            repeats: Vec::new(),
            cause: None,
            stretches: Vec::new(),
            alongside: Vec::new(),
        });
    }

    /// The requests of `tracks` still waiting on the API at `now`, each as it
    /// stands.
    pub fn api_waits<'a>(&mut self, tracks: impl Iterator<Item = &'a Track>, now: DateTime<Utc>, p: &Params) {
        for t in tracks {
            if let Some(u) = t.unanswered.as_ref().filter(|u| u.waits(now)) {
                self.api(t, u, now, p);
            }
        }
    }

    /// Each alert of the API's with the other sessions it left without an
    /// answer in the same minutes.
    pub fn api_alongside(&mut self) {
        let span = |a: &Alert| Some((a.stretches.first()?.since, a.stretches.last()?.end(a.at)));
        let spans: Vec<Alongside> = self
            .list
            .iter()
            .filter_map(|a| span(a).map(|(since, until)| Alongside { session: a.session.clone(), name: a.name.clone(), since, until }))
            .collect();
        for a in &mut self.list {
            let Some((from, to)) = span(a) else { continue };
            a.alongside = spans.iter().filter(|o| o.session != a.session && o.since <= to && o.until >= from).cloned().collect();
            a.what = unanswered(&a.stretches, a.at, a.alongside.len());
        }
    }

    pub fn trim(&mut self, from: DateTime<Utc>) {
        self.list.retain(|a| a.at >= from);
    }

    /// The alerts, newest first, each with the alert that caused it: a run
    /// gone long that was launched again, by that alert; a run launched
    /// again or gone long whose job a plan launches more than once, by the
    /// plan's alert, the first raised.
    #[must_use]
    pub fn newest_first(&self, jobs: &[String]) -> Vec<Alert> {
        let mut plans: Vec<&Alert> = self.list.iter().filter(|a| a.kind == Kind::PlanRepeats).collect();
        plans.sort_by_key(|a| a.at);
        let mut v: Vec<Alert> = self
            .list
            .iter()
            .map(|a| {
                let again = (a.kind == Kind::LongRun)
                    .then(|| self.list.iter().find(|b| b.kind == Kind::RunAgain && b.session == a.session && b.call == a.call))
                    .flatten();
                let plan = || {
                    let launch = a.launch.as_deref()?;
                    plans.iter().copied().find(|p| p.repeats.iter().any(|r| plan::scheduled(&r.job, launch, jobs)))
                };
                let cause = match a.kind {
                    Kind::LongRun => again.or_else(plan),
                    Kind::RunAgain => plan(),
                    _ => None,
                };
                Alert { cause: cause.map(|c| c.id.clone()), ..a.clone() }
            })
            .collect();
        v.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        v
    }
}

/// Whether a run changes something: a file written, a job launched, a git
/// change.
fn acts(run: &Run) -> bool {
    let own = run.path.as_deref().is_some_and(|p| ITS_OWN.iter().any(|o| p.contains(o)));
    (FILE_TOOLS.contains(&run.tool.as_str()) && !own) || (run.tool == "Bash" && (run.launch.is_some() || changes_git(&run.what)))
}

fn act_words(run: &Run) -> String {
    if FILE_TOOLS.contains(&run.tool.as_str()) {
        format!("wrote {}", run.path.as_deref().unwrap_or("a file"))
    } else if run.launch.is_some() {
        "launched a job".to_string()
    } else {
        "changed the repository".to_string()
    }
}

fn changes_git(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.windows(2).any(|w| w[0] == "git" && GIT_CHANGES.contains(&w[1]))
}

fn secs(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX / 2)
}

/// A duration in words: `1 h 04 min`, `12 min`, `40 s`.
#[must_use]
/// One sentence of the stretches the API did not answer, as they stand at
/// `at`, and how many other sessions it left without an answer in the same
/// minutes.
fn unanswered(stretches: &[Unanswered], at: DateTime<Utc>, others: usize) -> String {
    let (Some(first), Some(newest)) = (stretches.first(), stretches.last()) else { return String::new() };
    let times = |n: u32| if n == 1 { "time" } else { "times" };
    let resent: u32 = stretches.iter().map(|s| s.resent).sum();
    let gave_up: u32 = stretches.iter().map(|s| s.failures).sum();
    let mut parts = Vec::new();
    if resent > 0 {
        parts.push(format!("Claude Code sent its request again {resent} {}", times(resent)));
    }
    if stretches.iter().any(|s| s.interrupted.is_some()) {
        parts.push("you interrupted it".to_string());
    }
    if let Some(e) = stretches.iter().rev().find_map(|s| s.error.as_ref()) {
        parts.push(if gave_up > 1 { format!("Claude Code gave up {gave_up} times, the last: «{e}»") } else { format!("Claude Code gave up: «{e}»") });
    }
    let took = duration((newest.end(at) - first.since).num_seconds().max(0));
    let mut s = match stretches.len() {
        1 => format!("No answer from the API for {took}"),
        n => format!("No answer from the API {n} times in {took}"),
    };
    if !parts.is_empty() {
        s.push_str(": ");
        s.push_str(&parts.join(", "));
    }
    s.push_str(match () {
        () if newest.answered.is_some() => "; then the answer came.",
        () if newest.waiting() => "; none yet.",
        () => ".",
    });
    if others > 0 {
        let whom = if others == 1 { "session" } else { "sessions" };
        s.push_str(&format!(" {others} other {whom} had no answer in the same minutes: the connection or the API, not the agent."));
    }
    s
}

/// The agent's words as an alert quotes them: whole, or cut at a word.
fn cut(words: &str) -> String {
    const QUOTED_CHARS: usize = 220; // skill-allow: §9-const — length of a quote in an alert's sentence, not a policy tunable
    let words = words.trim();
    if words.chars().count() <= QUOTED_CHARS {
        return words.to_string();
    }
    let head: String = words.chars().take(QUOTED_CHARS).collect();
    let at = head.rfind(' ').unwrap_or(head.len());
    format!("{}…", &head[..at])
}

pub fn duration(secs: i64) -> String {
    match secs {
        s if s >= 3600 => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}
