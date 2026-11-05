//! The watcher (#573): recon's second part, beside the crawler and sharing
//! nothing that runs with it. The crawler gathers the history into recon's
//! database; the watcher follows every session as it works and every job it
//! launches ([`follow`], [`line`]), measures where every second of each
//! session's time goes, on what, and how long each run takes ([`track`]),
//! raises the critical alerts alone ([`alerts`]): finished work run again,
//! a run going long, a stop not followed, a plan that repeats full runs;
//! and digs out what shows nowhere while it happens ([`found`]). When it
//! starts it reads the window back, so its measures, alerts and findings
//! cover the week before it, and then it follows. It reads the kernel's
//! database with SELECT alone and the plans as files, and writes nothing
//! anywhere.

pub mod alerts;
pub mod command;
pub mod contract;
pub mod follow;
pub mod found;
pub mod line;
pub mod measure;
pub mod plan;
pub mod routes;
pub mod task;
pub mod track;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use superx_kernel::Kernel;
use tokio::sync::watch as signal;

use crate::params::{self, Params};
use alerts::{Alert, Alerts};
use follow::{Captured, Follow, Unread};
use found::{Finds, Found};
use task::{Task, Tasks};
use track::{Fed, Step, StepKind, Track, FOREGROUND_LIMIT_SECS, UNSEEN_END_SECS};

/// While paused, how often the watcher looks at its interval again.
const PAUSE_RECHECK_SECS: u64 = 60; // skill-allow: §9-const — idle re-check cadence, not a policy tunable

/// A session with a line this recent, or a run still open, is shown as it
/// works.
const LIVE_SECS: i64 = 21_600; // skill-allow: §9-const — what "now" covers on the page, not a policy tunable

/// The most lines of a session one ask for its lines answers: the newest of
/// the span asked; the page asks again from the oldest for the ones before.
pub const LINES_PAGE: usize = 400; // skill-allow: §9-const — page size of a drill, not a policy tunable

/// The most calls one ask for calls answers, the newest first.
pub const CALLS_PAGE: usize = 1_000; // skill-allow: §9-const — page size of a drill, not a policy tunable

/// What the watcher holds, as the page reads it at each look.
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
    pub now: Vec<Now>,
    /// Every session in the window.
    pub sessions: Vec<SessionRow>,
    /// Every job launched, every run of `repeat_secs` or more, and every
    /// job still running, newest first.
    pub runs: Vec<RunRow>,
    /// Every task of the window, the newest worked on first.
    pub tasks: Vec<Task>,
    /// Every build of a task file read in the window, the newest first.
    pub builds: Vec<contract::BuildRow>,
}

/// A session as it works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Now {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub folder: Option<String>,
    pub newest: DateTime<Utc>,
    /// `working`, `waiting on the API`, `job running`, `waiting on you`,
    /// `waiting on your approval` or `idle`.
    pub state: &'static str,
    /// The request the API has not answered, once it waited
    /// `attr_recon_watch_api_wait_secs` or was sent again.
    pub api: Option<track::Unanswered>,
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

/// A session in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionRow {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub folder: Option<String>,
    pub first: DateTime<Utc>,
    pub newest: DateTime<Utc>,
}

/// One run, as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRow {
    pub session: String,
    pub name: String,
    pub model: Option<String>,
    pub call: String,
    pub tool: String,
    /// What it worked on, as its time is counted.
    pub target: String,
    pub label: String,
    pub what: String,
    pub launch: Option<String>,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub secs: i64,
    pub background: bool,
    pub failed: bool,
    pub status: Option<String>,
}

/// One line of a session as a drill shows it; a call with how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineRow {
    #[serde(flatten)]
    pub step: Step,
    pub target: Option<String>,
    pub end: Option<DateTime<Utc>>,
    pub secs: Option<i64>,
    pub failed: Option<bool>,
    pub status: Option<String>,
    pub background: Option<bool>,
}

/// A session's lines in a span: the newest [`LINES_PAGE`] of them, and
/// whether there are older ones in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lines {
    pub lines: Vec<LineRow>,
    pub earlier: bool,
}

/// The calls asked for: the newest [`CALLS_PAGE`] of them, and how many
/// there are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Calls {
    pub calls: Vec<RunRow>,
    pub total: usize,
}

/// Which calls to answer: of a session, of a tool, on a target or under a
/// folder, in a span; each left out asks for any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallsAsked {
    pub session: Option<String>,
    pub tool: Option<String>,
    pub target: Option<String>,
    pub under: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

/// The watcher, shared by its loop and the page's routes: the loop holds
/// it only while it feeds a look's lines, never while it reads them.
#[derive(Debug, Clone, Default)]
pub struct Watched(Arc<Mutex<Watcher>>);

impl Watched {
    #[must_use]
    pub fn new(w: Watcher) -> Self {
        Self(Arc::new(Mutex::new(w)))
    }

    /// What `f` reads of the watcher as it stands.
    pub fn with<R>(&self, f: impl FnOnce(&mut Watcher) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// One look at `now`, as [`Watcher::look`], holding the watcher only to
    /// take what was read.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel's database does not answer.
    pub async fn look(&self, kernel: &Kernel, p: &Params, now: DateTime<Utc>) -> superx_kernel::Result<()> {
        let (mut follow, window_from) = self.with(|w| w.to_follow(p, now));
        let lines = follow.next(kernel).await?;
        self.with(|w| w.took(follow, lines, now, window_from, p));
        Ok(())
    }
}

/// The watcher: where its following stands, every session in the window,
/// its alerts and its findings.
#[derive(Debug, Default)]
pub struct Watcher {
    follow: Option<Follow>,
    window_from: Option<DateTime<Utc>>,
    tracks: HashMap<String, Track>,
    alerts: Alerts,
    finds: Finds,
    tasks: Tasks,
    contracts: contract::Contracts,
    looked_at: Option<DateTime<Utc>>,
    /// The parameters of the newest look, and whether it is paused.
    params: Option<Params>,
    paused: bool,
    /// The lines fed with no kernel to follow: a replay's.
    fed: u64,
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
        let (mut follow, window_from) = self.to_follow(p, now);
        let lines = follow.next(kernel).await?;
        self.took(follow, lines, now, window_from, p);
        Ok(())
    }

    /// Where a look at `now` reads from, and the window it keeps.
    fn to_follow(&self, p: &Params, now: DateTime<Utc>) -> (Follow, DateTime<Utc>) {
        let window_from = now - secs(p.watch_window_secs);
        (self.follow.clone().unwrap_or_else(|| Follow::from(window_from)), window_from)
    }

    /// What a look read, taken: every line fed, then the look settled.
    fn took(&mut self, follow: Follow, lines: Vec<Captured>, now: DateTime<Utc>, window_from: DateTime<Utc>, p: &Params) {
        self.follow = Some(follow);
        for c in lines {
            self.feed(&c.session, &c.name, &c.key, c.at, &c.line, p);
        }
        self.settle(now, window_from, p);
    }

    /// The watcher paused: nothing is read until its interval is set again.
    pub fn pause(&mut self, p: &Params) {
        self.params = Some(p.clone());
        self.paused = true;
    }

    /// Feed one captured line of a session, written at `at`, and judge what
    /// it says: what a look does with each line it reads.
    pub fn feed(&mut self, session: &str, name: &str, key: &str, at: DateTime<Utc>, line: &serde_json::Value, p: &Params) {
        self.fed += 1;
        let reading = line::read(line);
        let track = self.tracks.entry(session.to_string()).or_insert_with(|| Track::new(session, name, key, at));
        let fed = track.feed(at, reading, p);
        for f in fed {
            self.judge(session, at, f, p);
        }
    }

    /// The end of a look at `now`: the runs gone long, and what fell out of
    /// the window from `window_from` forgotten.
    pub fn settle(&mut self, now: DateTime<Utc>, window_from: DateTime<Utc>, p: &Params) {
        self.alerts.long_runs(self.tracks.values(), now, p);
        self.alerts.api_waits(self.tracks.values().filter(|t| live(t, now)), now, p);
        self.alerts.api_alongside();
        for t in self.tracks.values_mut() {
            t.trim(window_from);
        }
        self.tracks.retain(|_, t| t.newest >= window_from || t.running().next().is_some());
        self.alerts.trim(window_from);
        let sessions: HashSet<&String> = self.tracks.keys().collect();
        self.finds.trim(window_from, &sessions);
        self.tasks.trim(window_from, &sessions);
        self.contracts.trim(&sessions);
        self.window_from = Some(window_from);
        self.looked_at = Some(now);
        self.params = Some(p.clone());
        self.paused = false;
    }

    fn judge(&mut self, session: &str, at: DateTime<Utc>, fed: Fed, p: &Params) {
        let (tracks, alerts, finds, tasks, contracts) = (&self.tracks, &mut self.alerts, &mut self.finds, &mut self.tasks, &mut self.contracts);
        let Some(track) = tracks.get(session) else { return };
        match fed {
            Fed::Started(i) => {
                let run = &track.runs[i];
                // A door gate run again is one alert, by the task file it breaks.
                if !contracts.started(track, run, alerts, p) {
                    alerts.run_again(track, run, tracks.values(), p);
                }
                alerts.stop_not_followed(track, run);
            }
            Fed::Wrote(i, content) => {
                let run = &track.runs[i];
                if is_plan(run, p) {
                    alerts.plan(track, run, &content, p);
                }
            }
            Fed::Ended(i, text) => {
                let run = &track.runs[i];
                if is_plan(run, p) {
                    let plan = match (&text, run.tool.as_str()) {
                        (Some(t), "Read") => Some(numbers_set_aside(t)),
                        (_, tool) if line::FILE_TOOLS.contains(&tool) => run.path.as_deref().and_then(|path| file(path, track.folder.as_deref())),
                        _ => None,
                    };
                    if let Some(plan) = plan {
                        alerts.plan(track, run, &plan, p);
                    }
                }
                if let Some(text) = text {
                    finds.ended(track, run, &text, p);
                    contracts.ended(track, run, &text, alerts, p);
                    if !run.failed {
                        tasks.ran(track, run, &text, alerts, p);
                    }
                }
            }
            Fed::Prompt { came, text, previous, .. } => {
                finds.prompt(track, at, came, &text, previous, p);
                tasks.prompt(track, &text, p);
                contracts.prompt(track, at, &text, previous.is_none(), p);
            }
            Fed::Said(text) => tasks.said(track, at, &text, p),
            Fed::Compacted => finds.compacted(track, at),
            Fed::Unanswered(u) | Fed::Answered(u) => alerts.api(track, &u, at, p),
            Fed::Running(i) => contracts.running(track, &track.runs[i], alerts),
            Fed::TurnEnded => {
                if let Some(stopped) = contracts.turn_ended(track, at, alerts) {
                    tasks.build_stopped(session, &stopped.ticket, stopped.at, stopped.how, alerts);
                }
            }
        }
    }

    /// The parameters of the newest look; the fallbacks before the first.
    fn params(&self) -> Params {
        self.params.clone().unwrap_or_else(|| params::from_values(&BTreeMap::new()).params)
    }

    /// What the page reads at each look.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let p = self.params();
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
        let mut now_rows: Vec<Now> = self
            .tracks
            .values()
            .filter(|t| live(t, now) || t.running().any(|r| r.background && job_seen(r, now)))
            .map(|t| {
                let fore = t.running().find(|r| !r.background).filter(|_| live(t, now));
                let jobs: Vec<Doing> = t.running().filter(|r| r.background && job_seen(r, now)).map(doing).collect();
                let api = t.unanswered.as_ref().filter(|u| u.waits(now) && (u.resent > 0 || u.secs(now) >= secs(p.watch_api_wait_secs).num_seconds()));
                let state = match fore {
                    Some(r) if r.secs(now) > FOREGROUND_LIMIT_SECS => "waiting on your approval",
                    Some(_) => "working",
                    None if api.is_some() => "waiting on the API",
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
                    api: api.cloned(),
                    doing: fore.map(doing),
                    jobs,
                }
            })
            .collect();
        now_rows.sort_by_key(|s| std::cmp::Reverse(s.newest));
        let mut sessions: Vec<SessionRow> = self
            .tracks
            .values()
            .map(|t| SessionRow { id: t.session.clone(), name: t.name.clone(), model: t.model.clone(), folder: t.folder.clone(), first: t.first, newest: t.newest })
            .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.newest));
        let repeat = secs(p.watch_repeat_secs).num_seconds();
        let mut runs: Vec<RunRow> = self
            .tracks
            .values()
            .flat_map(|t| t.runs.iter().map(move |r| (t, r)))
            .filter(|(_, r)| if r.end.is_none() { r.background && job_seen(r, now) } else { r.launch.is_some() || (r.measured(now) && r.secs(now) >= repeat) })
            .map(|(t, r)| run_row(t, r, now, &p))
            .collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.start));
        Snapshot {
            looked_at: self.looked_at,
            every_secs: p.watch_secs,
            paused: self.paused,
            window_from: self.window_from,
            lines_read: self.follow.as_ref().map_or(self.fed, Follow::lines),
            unread: self.follow.as_ref().map(Follow::unread).unwrap_or_default(),
            long_secs: p.watch_long_secs,
            repeat_secs: p.watch_repeat_secs,
            alerts: self.alerts.newest_first(&p.watch_jobs),
            now: now_rows,
            sessions,
            runs,
            tasks: {
                let mut tasks = self.tasks.list.clone();
                for task in &mut tasks {
                    let (Some(first), Some(t)) = (task.merges.first().map(|m| &m.at), self.tracks.get(&task.session)) else { continue };
                    let (from, to) = (track::hour_of(*first), track::hour_of(task.last));
                    task.after_merge_secs =
                        t.ledger.iter().filter(|(k, _)| k.hour >= from && k.hour <= to).map(|(_, s)| s.split.thinking + s.split.writing + s.split.tools + s.split.jobs).sum();
                }
                tasks.sort_by_key(|t| std::cmp::Reverse(t.last));
                tasks
            },
            builds: self.contracts.rows(),
        }
    }

    /// Every session followed in the window.
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.values()
    }

    /// Everything found in the window, newest first.
    #[must_use]
    pub fn found(&self) -> Vec<Found> {
        self.finds.newest_first()
    }

    /// A session's lines from `from` to `to`: the newest [`LINES_PAGE`],
    /// oldest first, each call with how it ended. `None` for a session not
    /// followed.
    #[must_use]
    pub fn lines(&self, session: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<Lines> {
        let t = self.tracks.get(session)?;
        let now = self.looked_at.unwrap_or_else(Utc::now);
        let span: Vec<&Step> = t.steps.iter().filter(|s| s.at >= from && s.at <= to).collect();
        let earlier = span.len() > LINES_PAGE;
        let lines = span[span.len().saturating_sub(LINES_PAGE)..]
            .iter()
            .map(|s| {
                let run = (s.kind == StepKind::Call).then(|| s.call.as_deref().and_then(|c| t.run_of(c))).flatten();
                LineRow {
                    step: (*s).clone(),
                    target: run.map(|r| r.target.clone()),
                    end: run.and_then(|r| r.end),
                    secs: run.map(|r| r.secs(now)),
                    failed: run.map(|r| r.failed),
                    status: run.and_then(|r| r.status.clone()),
                    background: run.map(|r| r.background),
                }
            })
            .collect();
        Some(Lines { lines, earlier })
    }

    /// The calls asked for, newest first.
    #[must_use]
    pub fn calls(&self, asked: &CallsAsked) -> Calls {
        let p = self.params();
        let now = self.looked_at.unwrap_or_else(Utc::now);
        let mut all: Vec<RunRow> = self
            .tracks
            .values()
            .filter(|t| asked.session.as_deref().is_none_or(|s| s == t.session))
            .flat_map(|t| t.runs.iter().map(move |r| (t, r)))
            .filter(|(_, r)| {
                asked.tool.as_deref().is_none_or(|x| x == r.tool)
                    && asked.target.as_deref().is_none_or(|x| x == r.target)
                    && asked.under.as_deref().is_none_or(|x| r.target.starts_with(x))
                    && asked.from.is_none_or(|x| r.start >= x)
                    && asked.to.is_none_or(|x| r.start < x)
            })
            .map(|(t, r)| run_row(t, r, now, &p))
            .collect();
        all.sort_by_key(|r| std::cmp::Reverse(r.start));
        let total = all.len();
        all.truncate(CALLS_PAGE);
        Calls { calls: all, total }
    }
}

fn run_row(t: &Track, r: &track::Run, now: DateTime<Utc>, p: &Params) -> RunRow {
    RunRow {
        session: t.session.clone(),
        name: t.name.clone(),
        model: t.model.clone(),
        call: r.call.clone(),
        tool: r.tool.clone(),
        target: r.target.clone(),
        label: r.label(&p.watch_jobs),
        what: r.what.clone(),
        launch: r.launch.clone(),
        start: r.start,
        end: r.end,
        secs: r.secs(now),
        background: r.background,
        failed: r.failed,
        status: r.status.clone(),
    }
}

/// Whether a run read or wrote one of the operator's plans.
fn is_plan(run: &track::Run, p: &Params) -> bool {
    run.path.as_deref().is_some_and(|path| p.watch_plans.iter().any(|m| !m.is_empty() && path.contains(m.as_str())))
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
            watched.with(|w| w.pause(&params));
            if waited(&mut stop, PAUSE_RECHECK_SECS).await {
                break;
            }
            continue;
        }
        let looked = tokio::select! {
            r = watched.look(&kernel, &params, Utc::now()) => r,
            _ = stop.changed() => break,
        };
        match looked {
            Ok(()) => failing = false,
            Err(e) => failing = told(failing, &e),
        }
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
