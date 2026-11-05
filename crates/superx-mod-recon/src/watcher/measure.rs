//! The watcher's measures (#573), from its snapshot, in the operator's own
//! days: where the agents' time goes, by model and by day; and by day the
//! sessions, prompts, calls and launches, the long runs and the alerts.

use std::collections::{BTreeMap, BTreeSet};

use chrono::FixedOffset;
use serde::Serialize;

use super::alerts::Kind;
use super::track::{Counts, Split};
use super::Snapshot;

/// One model's time, in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelRow {
    pub model: String,
    pub split: Split,
    pub sessions: usize,
    pub counts: Counts,
}

/// One day, in the operator's zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayRow {
    pub day: String,
    pub split: Split,
    pub sessions: usize,
    pub counts: Counts,
    /// Runs that started that day and ran past the long bound.
    pub long_runs: usize,
    /// The alerts raised that day, by kind.
    pub alerts: BTreeMap<Kind, usize>,
}

/// Where each model's time went, the most worked first.
#[must_use]
pub fn models(s: &Snapshot) -> Vec<ModelRow> {
    let mut by: BTreeMap<String, (Split, BTreeSet<String>, Counts)> = BTreeMap::new();
    for h in &s.hours {
        let e = by.entry(h.model.clone().unwrap_or_else(|| "unknown".to_string())).or_default();
        e.0.plus(&h.at.split);
        e.1.insert(h.session.clone());
        add(&mut e.2, &h.at.counts);
    }
    let mut rows: Vec<ModelRow> =
        by.into_iter().map(|(model, (split, sessions, counts))| ModelRow { model, split, sessions: sessions.len(), counts }).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.split.thinking + r.split.writing + r.split.tools + r.split.jobs));
    rows
}

/// A day as it is summed.
#[derive(Default)]
struct Sum {
    split: Split,
    sessions: BTreeSet<String>,
    counts: Counts,
    long_runs: usize,
    alerts: BTreeMap<Kind, usize>,
}

/// Every day of the window, in `zone`, oldest first.
#[must_use]
pub fn days(s: &Snapshot, zone: FixedOffset) -> Vec<DayRow> {
    let day = |t: chrono::DateTime<chrono::Utc>| t.with_timezone(&zone).format("%Y-%m-%d").to_string();
    let mut by: BTreeMap<String, Sum> = BTreeMap::new();
    for h in &s.hours {
        let e = by.entry(day(h.hour)).or_default();
        e.split.plus(&h.at.split);
        e.sessions.insert(h.session.clone());
        add(&mut e.counts, &h.at.counts);
    }
    let long = i64::try_from(s.long_secs).unwrap_or(i64::MAX);
    for r in s.runs.iter().filter(|r| r.background && r.secs >= long) {
        by.entry(day(r.start)).or_default().long_runs += 1;
    }
    for a in &s.alerts {
        *by.entry(day(a.at)).or_default().alerts.entry(a.kind).or_insert(0) += 1;
    }
    by.into_iter()
        .map(|(day, e)| DayRow { day, split: e.split, sessions: e.sessions.len(), counts: e.counts, long_runs: e.long_runs, alerts: e.alerts })
        .collect()
}

fn add(to: &mut Counts, c: &Counts) {
    to.prompts += c.prompts;
    to.calls += c.calls;
    to.launches += c.launches;
}
