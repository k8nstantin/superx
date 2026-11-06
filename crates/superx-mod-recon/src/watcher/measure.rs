//! The watcher's measures (#573), in the operator's own days: where every
//! second of every session went, by day, model, and what it was on (the
//! tool and its target); and each session's hours, with what it did in
//! each.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, Utc};
use serde::Serialize;

use super::track::{Counts, On, Spend, Split, Track};

/// A day of a session's time on one thing, under one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimeRow {
    pub day: String,
    pub session: String,
    pub model: Option<String>,
    pub tool: String,
    pub target: String,
    pub split: Split,
    pub calls: u32,
    pub failed: u32,
}

/// An hour of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HourRow {
    pub hour: DateTime<Utc>,
    pub session: String,
    pub split: Split,
    pub counts: Counts,
}

/// Every session's time in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Time {
    pub rows: Vec<TimeRow>,
    pub hours: Vec<HourRow>,
}

/// The time of `tracks`, its days in `zone`.
#[must_use]
pub fn time<'a>(tracks: impl Iterator<Item = &'a Track>, zone: FixedOffset) -> Time {
    let mut days: BTreeMap<(String, String, Option<String>, On), Spend> = BTreeMap::new();
    let mut hours: BTreeMap<(DateTime<Utc>, String), (Split, Counts)> = BTreeMap::new();
    for t in tracks {
        for (k, s) in &t.ledger {
            let day = k.hour.with_timezone(&zone).format("%Y-%m-%d").to_string();
            let e = days.entry((day, t.session.clone(), k.model.clone(), k.on.clone())).or_default();
            e.split.plus(&s.split);
            e.calls += s.calls;
            e.failed += s.failed;
            hours.entry((k.hour, t.session.clone())).or_default().0.plus(&s.split);
        }
        for (h, c) in &t.counts {
            let e = &mut hours.entry((*h, t.session.clone())).or_default().1;
            e.prompts += c.prompts;
            e.calls += c.calls;
            e.launches += c.launches;
        }
    }
    Time {
        rows: days
            .into_iter()
            .map(|((day, session, model, on), s)| TimeRow { day, session, model, tool: on.tool, target: on.target, split: s.split, calls: s.calls, failed: s.failed })
            .collect(),
        hours: hours.into_iter().map(|((hour, session), (split, counts))| HourRow { hour, session, split, counts }).collect(),
    }
}
