//! What went to waste (#573): the tokens the agents' messages used and the
//! lines their calls wrote, in work that went wrong. The operator,
//! 2026-11-06: "I want the waste highlighted in all charts: lines, tokens".
//!
//! Every case of the watcher's, an alert or a finding, spans the session's
//! lines from where its cause starts to where it shows; what the messages in
//! that span used and the calls in it wrote is its cost. The span of a case
//! that shows work gone wrong is waste:
//!
//! - **Work redone:** a task that never ends, from your message that began
//!   it to its newest reopening (every version before it was thrown away);
//!   finished work run again, for as long as the first run took; work undone.
//! - **Said what was not so:** a fix short of what it found, from the count
//!   to the fix.
//! - **Acted on its own:** a protected place changed when nothing you said
//!   named it.
//! - **Broke your orders:** your stop not followed, its task file broken.
//! - **Merged unproven:** a task that never ends after a merge no spec gate
//!   proved, as above.
//! - **Stopped short:** a build, from your order to its stop.
//! - **You stepped in:** the turn you corrected or stopped, from your message
//!   before it.
//! - **Effort wasted:** a file edited over and over (the lines written to
//!   that file), read again, a call refused and tried again, a flood, a
//!   context lost.
//!
//! What the agent says (that it acted on its own, got something wrong, or
//! said what was not so), a merge asked for, a plan, your questions and the
//! time held up cost no work of their own: they are counted, and their
//! spans shown, but they are no waste. The agent's word that it acted on
//! its own names no span of the work it means. Spans of one family that overlap count once in it;
//! one span in two families counts in both, and once in all the waste.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use serde::Serialize;

use super::alerts::{Alert, Kind};
use super::found::{Found, Hidden};
use super::line::Tokens;
use super::task::Task;
use super::track::{StepKind, Track};

/// A span of a session's lines: from, to.
type Span = (DateTime<Utc>, DateTime<Utc>);
/// A session's spans of waste, by family.
type ByFamily = BTreeMap<&'static str, Vec<Span>>;
/// The files a session edited over and over, each in its span.
type Edited<'a> = Vec<(DateTime<Utc>, DateTime<Utc>, &'a str)>;

/// What a span cost: the tokens its messages used, and the lines its calls
/// wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Cost {
    pub tokens: Tokens,
    pub added: u64,
    pub removed: u64,
}

impl Cost {
    fn plus(&mut self, o: &Self) {
        self.tokens = self.tokens.plus(o.tokens);
        self.added += o.added;
        self.removed += o.removed;
    }
}

/// A case's cost: its span's, its family, and whether its span is waste.
#[derive(Debug, Clone, Serialize)]
pub struct CaseCost {
    pub cost: Cost,
    pub family: &'static str,
    pub waste: bool,
}

/// One day of one session under one model: all it cost, the part that went
/// to waste, and that part by family.
#[derive(Debug, Clone, Serialize)]
pub struct DayRow {
    pub day: String,
    pub session: String,
    pub model: Option<String>,
    pub all: Cost,
    pub wasted: Cost,
    pub families: BTreeMap<&'static str, Cost>,
}

/// What went to waste: each case's cost by its id, and every day's.
#[derive(Debug, Clone, Serialize)]
pub struct Waste {
    pub cases: HashMap<String, CaseCost>,
    pub days: Vec<DayRow>,
}

/// The family of what went wrong an alert is of.
#[must_use]
pub const fn alert_family(k: Kind) -> &'static str {
    match k {
        Kind::EndlessTask | Kind::RunAgain | Kind::PlanRepeats => "redone",
        Kind::CountGap | Kind::SaidFalse => "false",
        Kind::ActedAlone => "alone",
        Kind::StopNotFollowed | Kind::TaskFileBroken => "orders",
        Kind::UnprovenMerge => "unproven",
        Kind::BuildStopped => "stopped",
        Kind::GotWrong => "wrong",
        Kind::ApiStalled | Kind::LongRun => "held",
    }
}

/// The family a finding is of.
#[must_use]
pub const fn found_family(k: Hidden) -> &'static str {
    match k {
        Hidden::Undone => "redone",
        Hidden::Protected => "alone",
        Hidden::Corrected | Hidden::Interrupted | Hidden::Asked => "you",
        Hidden::Held | Hidden::Slept => "held",
        Hidden::Thrash | Hidden::Reread | Hidden::Refused | Hidden::Flood | Hidden::Compacted => "waste",
    }
}

/// Whether an alert's span is work that went to waste.
const fn alert_wasted(k: Kind) -> bool {
    matches!(
        k,
        Kind::EndlessTask | Kind::RunAgain | Kind::CountGap | Kind::StopNotFollowed | Kind::TaskFileBroken | Kind::BuildStopped
    )
}

/// Whether a finding's span is work that went to waste.
const fn found_wasted(k: Hidden) -> bool {
    !matches!(k, Hidden::Asked | Hidden::Held | Hidden::Slept)
}

/// The span an alert's cost is read over.
fn alert_span(a: &Alert) -> Span {
    match a.kind {
        Kind::EndlessTask | Kind::BuildStopped | Kind::LongRun | Kind::CountGap => (a.since.unwrap_or(a.from).min(a.at), a.at),
        Kind::RunAgain => (a.at, a.at + TimeDelta::seconds(a.cost_secs.unwrap_or(0).max(0))),
        _ => (a.from.min(a.at), a.at),
    }
}

/// The span a finding's cost is read over: a stop of yours, from your
/// message before it at the earliest, though the call it stopped started
/// before.
fn found_span(f: &Found, t: &Track) -> Span {
    match f.kind {
        Hidden::Thrash | Hidden::Held | Hidden::Slept => (f.from, f.from + TimeDelta::seconds(f.secs.unwrap_or(0).max(0))),
        Hidden::Interrupted => {
            let yours = t.steps[..t.steps.partition_point(|s| s.at < f.at)].iter().rev().find(|s| s.kind == StepKind::You).map(|s| s.at);
            (f.from.max(yours.unwrap_or(f.from)).min(f.at), f.at)
        }
        _ => (f.from.min(f.at), f.at),
    }
}

/// What `t` used and wrote from `from` to `to`, both ends in; of the lines,
/// only those written to `path` when one is given.
fn cost(t: &Track, from: DateTime<Utc>, to: DateTime<Utc>, path: Option<&str>) -> Cost {
    let used = &t.used[t.used.partition_point(|u| u.at < from)..t.used.partition_point(|u| u.at <= to)];
    let wrote = t.wrote[t.wrote.partition_point(|w| w.at < from)..t.wrote.partition_point(|w| w.at <= to)].iter().filter(|w| path.is_none_or(|p| w.path == p));
    let (added, removed) = wrote.fold((0, 0), |(a, r), w| (a + u64::from(w.added), r + u64::from(w.removed)));
    Cost { tokens: used.iter().fold(Tokens::default(), |s, u| s.plus(u.tokens)), added, removed }
}

/// Spans sorted and joined where they overlap.
fn joined(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort();
    let mut out: Vec<Span> = Vec::new();
    for (from, to) in spans {
        match out.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// Whether `at` falls in one of `spans`, sorted and joined.
fn within(spans: &[Span], at: DateTime<Utc>) -> bool {
    let i = spans.partition_point(|(from, _)| *from <= at);
    i > 0 && at <= spans[i - 1].1
}

/// What went to waste in `tracks`, by the `alerts`, `found` and `tasks` of
/// the watcher, its days in `zone`.
#[must_use]
pub fn waste<'a>(tracks: impl Iterator<Item = &'a Track>, alerts: &[Alert], found: &[Found], tasks: &[Task], zone: FixedOffset) -> Waste {
    let tracks: HashMap<&str, &Track> = tracks.map(|t| (t.session.as_str(), t)).collect();
    let mut cases = HashMap::new();
    // Each session's spans of waste, by family; and the files edited over
    // and over, whose lines alone are waste.
    let mut spans: HashMap<&str, ByFamily> = HashMap::new();
    let mut files: HashMap<&str, Edited<'_>> = HashMap::new();
    for a in alerts {
        let Some(t) = tracks.get(a.session.as_str()) else { continue };
        let (from, to) = alert_span(a);
        let (family, waste) = (alert_family(a.kind), alert_wasted(a.kind));
        cases.insert(a.id.clone(), CaseCost { cost: cost(t, from, to, None), family, waste });
        if !waste {
            continue;
        }
        let mine = spans.entry(t.session.as_str()).or_default();
        mine.entry(family).or_default().push((from, to));
        // A task that never ends after a merge no spec gate proved is the
        // waste of that merge too.
        let unproven = a.kind == Kind::EndlessTask
            && tasks.iter().any(|task| {
                task.session == a.session
                    && a.id.starts_with(&format!("{}/endless_task/{}/", task.session, task.branch))
                    && task.merges.iter().any(|m| m.proven == Some(false))
            });
        if unproven {
            mine.entry("unproven").or_default().push((from, to));
        }
    }
    for f in found {
        let Some(t) = tracks.get(f.session.as_str()) else { continue };
        let (from, to) = found_span(f, t);
        let (family, waste) = (found_family(f.kind), found_wasted(f.kind));
        // A file edited over and over wasted the lines written to it.
        let path = (f.kind == Hidden::Thrash).then_some(f.target.as_deref()).flatten();
        cases.insert(f.id.clone(), CaseCost { cost: cost(t, from, to, path), family, waste });
        if waste && path.is_none() {
            spans.entry(t.session.as_str()).or_default().entry(family).or_default().push((from, to));
        }
        if let (true, Some(path)) = (waste, path) {
            files.entry(t.session.as_str()).or_default().push((from, to, path));
        }
    }
    let mut days: BTreeMap<(String, String, Option<String>), DayRow> = BTreeMap::new();
    for (session, t) in &tracks {
        let thrashed = files.get(session).cloned().unwrap_or_default();
        let families: Vec<(&'static str, Vec<Span>)> =
            spans.get(session).map(|by| by.iter().map(|(f, s)| (*f, joined(s.clone()))).collect()).unwrap_or_default();
        let all = joined(families.iter().flat_map(|(_, s)| s.iter().copied()).collect());
        let mut add = |at: DateTime<Utc>, model: &Option<String>, c: Cost, path: Option<&str>| {
            let day = at.with_timezone(&zone).format("%Y-%m-%d").to_string();
            let row = days.entry((day.clone(), (*session).to_string(), model.clone())).or_insert_with(|| DayRow {
                day,
                session: (*session).to_string(),
                model: model.clone(),
                all: Cost::default(),
                wasted: Cost::default(),
                families: BTreeMap::new(),
            });
            row.all.plus(&c);
            let thrash = path.is_some_and(|p| thrashed.iter().any(|(from, to, file)| *file == p && *from <= at && at <= *to));
            if thrash || within(&all, at) {
                row.wasted.plus(&c);
            }
            for (family, s) in &families {
                if within(s, at) || (thrash && *family == "waste") {
                    row.families.entry(family).or_default().plus(&c);
                }
            }
            if thrash && !families.iter().any(|(f, _)| *f == "waste") {
                row.families.entry("waste").or_default().plus(&c);
            }
        };
        for u in &t.used {
            add(u.at, &u.model, Cost { tokens: u.tokens, added: 0, removed: 0 }, None);
        }
        for w in &t.wrote {
            add(w.at, &w.model, Cost { tokens: Tokens::default(), added: u64::from(w.added), removed: u64::from(w.removed) }, Some(w.path.as_str()));
        }
    }
    Waste { cases, days: days.into_values().collect() }
}
