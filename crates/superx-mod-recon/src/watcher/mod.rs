//! The watcher (#573): recon's second part, beside the crawler and sharing
//! nothing that runs with it. The crawler gathers the history into recon's
//! database; the watcher follows every session as it works and every job it
//! launches ([`follow`], [`line`]), measures where each session's time goes
//! and how long each run takes ([`track`]), and raises the critical alerts
//! alone ([`alerts`]): finished work run again, a run going long, a stop not
//! followed, a plan that repeats full runs. When it starts it reads the
//! window back, so its measures and alerts cover the week before it, and
//! then it follows. It reads the kernel's database with SELECT alone and the
//! plans as files, and writes nothing anywhere.

pub mod alerts;
pub mod follow;
pub mod line;
pub mod measure;
pub mod plan;
pub mod routes;
pub mod track;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use superx_kernel::Kernel;
use tokio::sync::watch as signal;

use crate::params::{self, Params};
use alerts::{Alert, Alerts};
use follow::{Follow, Unread};
use track::{Fed, Hour, Track, FOREGROUND_LIMIT_SECS, UNSEEN_END_SECS};

/// While paused, how often the watcher looks at its interval again.
const PAUSE_RECHECK_SECS: u64 = 60; // skill-allow: §9-const — idle re-check cadence, not a policy tunable

/// A session with a line this recent, or a run still open, is shown as it
/// works.
const LIVE_SECS: i64 = 21_600; // skill-allow: §9-const — what "now" covers on the page, not a policy tunable

/// What the watcher holds, as the page reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub looked_at: Option<DateTime<Utc>>,
    pub every_secs: u64,
    pub paused: bool,
    pub window_from: Option<DateTime<Utc>>,
    pub lines_read: u64,
    pub unread: Vec<Unread>,
    /// The bounds in force: a run past `long_secs` is long; a run of
    /// `repeat_secs` or more is costly to run again.
    pub long_secs: u64,
    pub repeat_secs: u64,
    /// Every alert, newest first.
    pub alerts: Vec<Alert>,
    /// The sessions working now, newest first.
    pub sessions: Vec<Now>,
    /// Every session's hours in the window.
    pub hours: Vec<HourRow>,
    /// Every run of `repeat_secs` or more, and every run still open, newest
    /// first.
    pub runs: Vec<RunRow>,
}

/// A session as it works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Now {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub folder: Option<String>,
    pub newest: DateTime<Utc>,
    /// `working`, `job running`, `waiting on you`, `waiting on your
    /// approval` or `idle`.
    pub state: &'static str,
    /// The call it is in, when one runs in the foreground.
    pub doing: Option<Doing>,
    /// Its background jobs running.
    pub jobs: Vec<Doing>,
}

/// A call running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Doing {
    pub tool: String,
    pub what: String,
    pub since: DateTime<Utc>,
    /// When the same launch ended the last time it ran to its end, counted
    /// from this start.
    pub expected_end: Option<DateTime<Utc>>,
}

/// One hour of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HourRow {
    pub hour: DateTime<Utc>,
    pub session: String,
    pub model: Option<String>,
    #[serde(flatten)]
    pub at: Hour,
}

/// One run, as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRow {
    pub session: String,
    pub name: String,
    pub model: Option<String>,
    pub tool: String,
    pub label: String,
    pub what: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub secs: i64,
    pub background: bool,
    pub failed: bool,
    pub status: Option<String>,
}

/// The watcher's snapshot, shared with the routes: published at each look,
/// read without waiting on one.
#[derive(Debug, Clone, Default)]
pub struct Watched(Arc<Mutex<Snapshot>>);

impl Watched {
    /// The newest snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Publish a look's snapshot.
    pub fn publish(&self, s: Snapshot) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }
}

/// The watcher: where its following stands, every session in the window,
/// and its alerts.
#[derive(Debug, Default)]
pub struct Watcher {
    follow: Option<Follow>,
    window_from: Option<DateTime<Utc>>,
    tracks: HashMap<String, Track>,
    alerts: Alerts,
    looked_at: Option<DateTime<Utc>>,
}

impl Watcher {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One look at `now`: every line captured since the last, each fed to
    /// its session and judged, then the runs gone long. The first look reads
    /// the window back. A look that cannot read changes nothing.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel's database does not answer.
    pub async fn look(&mut self, kernel: &Kernel, p: &Params, now: DateTime<Utc>) -> superx_kernel::Result<()> {
        let window_from = now - secs(p.watch_window_secs);
        let mut follow = self.follow.clone().unwrap_or_else(|| Follow::from(window_from));
        let lines = follow.next(kernel).await?;
        self.follow = Some(follow);
        for c in lines {
            self.feed(&c.session, &c.name, &c.key, c.at, &c.line, p);
        }
        self.settle(now, window_from, p);
        Ok(())
    }

    /// Feed one captured line of a session, written at `at`, and judge what
    /// it says: what a look does with each line it reads.
    pub fn feed(&mut self, session: &str, name: &str, key: &str, at: DateTime<Utc>, line: &serde_json::Value, p: &Params) {
        let reading = line::read(line);
        let track = self.tracks.entry(session.to_string()).or_insert_with(|| Track::new(session, name, key, at));
        let fed = track.feed(at, reading, &p.watch_jobs, &p.watch_stop_words);
        for f in fed {
            self.judge(session, f, p);
        }
    }

    /// The end of a look at `now`: the runs gone long, and what fell out of
    /// the window from `window_from` forgotten.
    pub fn settle(&mut self, now: DateTime<Utc>, window_from: DateTime<Utc>, p: &Params) {
        self.alerts.long_runs(self.tracks.values(), now, p);
        for t in self.tracks.values_mut() {
            t.trim(window_from);
        }
        self.tracks.retain(|_, t| t.newest >= window_from || t.running().next().is_some());
        self.alerts.trim(window_from);
        self.window_from = Some(window_from);
        self.looked_at = Some(now);
    }

    fn judge(&mut self, session: &str, fed: Fed, p: &Params) {
        let (tracks, alerts) = (&self.tracks, &mut self.alerts);
        let Some(track) = tracks.get(session) else { return };
        match fed {
            Fed::Started(i) => {
                let run = &track.runs[i];
                alerts.run_again(track, run, tracks.values(), p);
                alerts.stop_not_followed(track, run);
            }
            Fed::Ended(i, text) => {
                let run = &track.runs[i];
                let Some(path) = run.path.as_deref() else { return };
                if !p.watch_plans.iter().any(|m| !m.is_empty() && path.contains(m.as_str())) {
                    return;
                }
                let text = match text {
                    Some(t) if run.tool == "Read" => Some(numbers_set_aside(&t)),
                    Some(t) => Some(t),
                    None => file(path, track.folder.as_deref()),
                };
                if let Some(text) = text {
                    alerts.plan(track, run, &text, p);
                }
            }
            Fed::Prompt { .. } | Fed::Said => {}
        }
    }

    /// What the page reads.
    #[must_use]
    pub fn snapshot(&self, p: &Params, paused: bool) -> Snapshot {
        let now = self.looked_at.unwrap_or_else(Utc::now);
        let latest_well = |launch: &str, before: DateTime<Utc>| {
            self.tracks
                .values()
                .flat_map(|t| t.runs.iter())
                .filter(|r| r.launch.as_deref() == Some(launch) && r.ended_well() && r.end.is_some_and(|e| e <= before))
                .max_by_key(|r| r.start)
                .map(|r| r.secs(before))
        };
        let doing = |r: &track::Run| Doing {
            tool: r.tool.clone(),
            what: r.label(&p.watch_jobs),
            since: r.start,
            expected_end: r.launch.as_deref().and_then(|l| latest_well(l, r.start)).map(|s| r.start + TimeDelta::seconds(s)),
        };
        let mut sessions: Vec<Now> = self
            .tracks
            .values()
            .filter(|t| live(t, now) || t.running().any(|r| r.background && job_seen(r, now)))
            .map(|t| {
                let fore = t.running().find(|r| !r.background).filter(|_| live(t, now));
                let jobs: Vec<Doing> = t.running().filter(|r| r.background && job_seen(r, now)).map(doing).collect();
                let state = match fore {
                    Some(r) if r.secs(now) > FOREGROUND_LIMIT_SECS => "waiting on your approval",
                    Some(_) => "working",
                    None if !jobs.is_empty() => "job running",
                    None if t.turn_over => "waiting on you",
                    None if now - t.newest > TimeDelta::minutes(30) => "idle",
                    None => "working",
                };
                Now {
                    id: t.session.clone(),
                    name: t.name.clone(),
                    model: t.model.clone(),
                    folder: t.folder.clone(),
                    newest: t.newest,
                    state,
                    doing: fore.map(doing),
                    jobs,
                }
            })
            .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.newest));
        let mut hours: Vec<HourRow> = self
            .tracks
            .values()
            .flat_map(|t| t.hours.iter().map(|(h, at)| HourRow { hour: *h, session: t.session.clone(), model: t.model.clone(), at: *at }))
            .collect();
        hours.sort_by(|a, b| a.hour.cmp(&b.hour).then_with(|| a.session.cmp(&b.session)));
        let mut runs: Vec<RunRow> = self
            .tracks
            .values()
            .flat_map(|t| t.runs.iter().map(move |r| (t, r)))
            .filter(|(_, r)| if r.end.is_none() { r.background && job_seen(r, now) } else { r.measured(now) && r.secs(now) >= secs(p.watch_repeat_secs).num_seconds() })
            .map(|(t, r)| RunRow {
                session: t.session.clone(),
                name: t.name.clone(),
                model: t.model.clone(),
                tool: r.tool.clone(),
                label: r.label(&p.watch_jobs),
                what: r.what.clone(),
                start: r.start,
                end: r.end,
                secs: r.secs(now),
                background: r.background,
                failed: r.failed,
                status: r.status.clone(),
            })
            .collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.start));
        Snapshot {
            looked_at: self.looked_at,
            every_secs: p.watch_secs,
            paused,
            window_from: self.window_from,
            lines_read: self.follow.as_ref().map_or(0, Follow::lines),
            unread: self.follow.as_ref().map(Follow::unread).unwrap_or_default(),
            long_secs: p.watch_long_secs,
            repeat_secs: p.watch_repeat_secs,
            alerts: self.alerts.newest_first(),
            sessions,
            hours,
            runs,
        }
    }
}

/// Whether a background job with no notice yet is still known to run at
/// `now`: within [`UNSEEN_END_SECS`] of its start.
fn job_seen(r: &track::Run, now: DateTime<Utc>) -> bool {
    now - r.start < TimeDelta::seconds(UNSEEN_END_SECS)
}

/// Whether a session wrote a line recently enough to be shown as it works:
/// a call of one that wrote nothing since is not known to wait.
fn live(t: &Track, now: DateTime<Utc>) -> bool {
    now - t.newest < TimeDelta::seconds(LIVE_SECS)
}

/// A Read's result without the line numbers Claude Code writes before each
/// line.
fn numbers_set_aside(text: &str) -> String {
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            match t.split_once('\t') {
                Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
                _ => l,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A plan's file as it stands, read where the session named it.
fn file(path: &str, folder: Option<&str>) -> Option<String> {
    let p = Path::new(path);
    let full = if p.is_absolute() { p.to_path_buf() } else { Path::new(folder?).join(p) };
    std::fs::read_to_string(full).ok()
}

/// The watcher's loop: a look every `attr_recon_watch_secs`, until the stop
/// that ends the module; 0 pauses it.
pub async fn run(kernel: Kernel, mut stop: signal::Receiver<bool>, watched: Watched) {
    let mut watcher = Watcher::new();
    let mut failing = false;
    let mut announced = false;
    loop {
        if *stop.borrow() {
            break;
        }
        let resolved = tokio::select! {
            r = params::resolve(&kernel) => r,
            _ = stop.changed() => break,
        };
        let params = match resolved {
            Ok(r) => r.params,
            Err(e) => {
                failing = told(failing, &e);
                if waited(&mut stop, PAUSE_RECHECK_SECS).await {
                    break;
                }
                continue;
            }
        };
        if !announced {
            tracing::info!(target: "recon", window_secs = params.watch_window_secs, "watcher started");
            announced = true;
        }
        if params.watch_secs == 0 {
            watched.publish(watcher.snapshot(&params, true));
            if waited(&mut stop, PAUSE_RECHECK_SECS).await {
                break;
            }
            continue;
        }
        let looked = tokio::select! {
            r = watcher.look(&kernel, &params, Utc::now()) => r,
            _ = stop.changed() => break,
        };
        match looked {
            Ok(()) => failing = false,
            Err(e) => failing = told(failing, &e),
        }
        watched.publish(watcher.snapshot(&params, false));
        if waited(&mut stop, params.watch_secs).await {
            break;
        }
    }
    tracing::info!(target: "recon", "watcher stopped");
}

/// A look that could not read, logged once until a look reads again.
fn told(failing: bool, e: &superx_kernel::KernelError) -> bool {
    if !failing {
        tracing::warn!(target: "recon", error = %e, "the watcher could not read; it looks again at its next interval");
    }
    true
}

/// Wait `secs` seconds, or until a stop: whether a stop came.
async fn waited(stop: &mut signal::Receiver<bool>, secs: u64) -> bool {
    tokio::select! {
        () = tokio::time::sleep(Duration::from_secs(secs)) => *stop.borrow(),
        r = stop.changed() => r.is_err() || *stop.borrow(),
    }
}

fn secs(n: u64) -> TimeDelta {
    TimeDelta::seconds(i64::try_from(n).unwrap_or(i64::MAX / 1_000))
}
