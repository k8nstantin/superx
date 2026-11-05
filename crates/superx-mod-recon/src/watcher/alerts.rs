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

use std::collections::{BTreeMap, HashSet};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use super::line::FILE_TOOLS;
use super::plan;
use super::track::{Run, Track};
use crate::params::Params;

/// How long after a stop an act still counts as not following it.
const STOP_WINDOW_SECS: i64 = 600; // skill-allow: §9-const — one turn's reach, not a policy tunable

/// How many times its bound a job with no notice is still judged.
const OPEN_HORIZON: i64 = 4; // skill-allow: §9-const — past it an unseen end is likelier than a run, not a policy tunable

/// Paths an agent writes for itself, its memory and its scratch: writing
/// there after a stop is noting it, not acting.
const ITS_OWN: [&str; 3] = ["/.claude/", "/private/tmp/", "/tmp/"];

/// The git commands that change a repository.
const GIT_CHANGES: [&str; 7] = ["commit", "push", "merge", "reset", "rebase", "checkout", "branch"];

/// What an alert is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RunAgain,
    LongRun,
    StopNotFollowed,
    PlanRepeats,
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
            evidence: vec![launch.to_string()],
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
        });
    }

    /// Forget alerts before `from`.
    pub fn trim(&mut self, from: DateTime<Utc>) {
        self.list.retain(|a| a.at >= from);
    }

    /// The alerts, newest first.
    #[must_use]
    pub fn newest_first(&self) -> Vec<Alert> {
        let mut v = self.list.clone();
        v.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        v
    }

    /// The alerts by kind, for the measures.
    #[must_use]
    pub fn by_kind(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for a in &self.list {
            *m.entry(format!("{:?}", a.kind)).or_insert(0) += 1;
        }
        m
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
pub fn duration(secs: i64) -> String {
    match secs {
        s if s >= 3600 => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}
