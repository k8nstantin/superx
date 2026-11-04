//! The watcher (#566): recon's second part, beside the crawler and sharing
//! nothing that runs with it (rules crawler-is-the-reader). The crawler
//! gathers the history into recon's database; the watcher watches every
//! session as it works, the coding agents' adversary. Its own loop follows
//! the newest captured lines ([`follow`]), holds one watch per session in
//! memory ([`watch`]), judges each line against the operator's orders and
//! the task file the session executes, finds each session's memory folder,
//! and reports what it finds ([`report`]) on the module's page as it
//! happens. It reads the kernel's database with SELECT alone, the task
//! files and the memory folders as files, and writes nothing anywhere.

pub mod follow;
pub mod line;
pub mod memory;
pub mod report;
pub mod routes;
pub mod spec;
pub mod watch;
pub mod words;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use superx_kernel::Kernel;
use tokio::sync::watch as signal;

use crate::params::{self, Params};
use follow::{Follow, Unread};
use report::Report;
use watch::{Context, SessionView, Watch};

/// While paused, how often the watcher looks at its interval again.
const PAUSE_RECHECK_SECS: u64 = 60; // skill-allow: §9-const — idle re-check cadence, not a policy tunable

/// What the watcher holds, as the page reads it (#566, step 14).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub looked_at: Option<DateTime<Utc>>,
    pub every_secs: u64,
    pub paused: bool,
    pub window_from: Option<DateTime<Utc>>,
    pub lines_read: u64,
    pub unread: Vec<Unread>,
    pub sessions: Vec<SessionView>,
    /// Every report of every watch, newest first.
    pub reports: Vec<Report>,
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

    /// Publish a snapshot, as the watcher's loop does after each look.
    pub fn publish(&self, snapshot: Snapshot) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = snapshot;
    }
}

/// The watcher's state: where its following stands, and its watches.
#[derive(Debug)]
pub struct Watcher {
    started: DateTime<Utc>,
    home: Option<PathBuf>,
    follow: Option<Follow>,
    watches: BTreeMap<String, Watch>,
    looked_at: Option<DateTime<Utc>>,
}

impl Watcher {
    /// A watcher started at `started`; `home` stands for a `~` in a path a
    /// command writes, and holds the adapter's fallback projects root.
    #[must_use]
    pub fn new(started: DateTime<Utc>, home: Option<PathBuf>) -> Self {
        Self { started, home, follow: None, watches: BTreeMap::new(), looked_at: None }
    }

    /// One look, judging at `now` (#566, step 2): the projects roots, the
    /// lines captured since the last look, each given to its session's
    /// watch, what time alone makes, the window, the task files that
    /// changed and the memory folders. A look that cannot read changes
    /// nothing the watcher holds.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel's database does not answer.
    pub async fn look(&mut self, kernel: &Kernel, params: &Params, now: DateTime<Utc>) -> superx_kernel::Result<()> {
        let roots = memory::roots(kernel, self.home.as_deref()).await?;
        let window = secs(params.watch_window_secs);
        let started = self.started;
        let follow = self.follow.get_or_insert_with(|| Follow::from(started - window));
        let lines = follow.next(kernel).await?;
        let ctx = Context { params, roots: &roots };
        for captured in &lines {
            let reading = line::read(&captured.line, self.home.as_deref());
            let watch = self.watches.entry(captured.session.clone()).or_insert_with(|| Watch::new(captured));
            watch.feed(captured, &reading, &ctx);
        }
        let from = now - window;
        for watch in self.watches.values_mut() {
            watch.judge(now, params);
            watch.reread(now);
        }
        self.watches.retain(|_, w| w.trim(from));
        let held = usize::try_from(params.watch_sessions).unwrap_or(usize::MAX);
        if self.watches.len() > held {
            let mut newest: Vec<(Option<DateTime<Utc>>, String)> =
                self.watches.iter().map(|(id, w)| (w.newest(), id.clone())).collect();
            newest.sort();
            for (_, id) in &newest[..self.watches.len() - held] {
                self.watches.remove(id);
            }
        }
        for watch in self.watches.values_mut() {
            watch.list_memory(&roots);
        }
        self.looked_at = Some(now);
        Ok(())
    }

    /// The watch of `session`, when one is held.
    #[must_use]
    pub fn watch(&self, session: &str) -> Option<&Watch> {
        self.watches.get(session)
    }

    /// What the watcher holds, at `now`.
    #[must_use]
    pub fn snapshot(&self, params: &Params, now: DateTime<Utc>, paused: bool) -> Snapshot {
        let mut reports: Vec<Report> = self.watches.values().flat_map(|w| w.reports().iter().cloned()).collect();
        reports.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        Snapshot {
            looked_at: self.looked_at,
            every_secs: params.watch_secs,
            paused,
            window_from: Some(now - secs(params.watch_window_secs)),
            lines_read: self.follow.as_ref().map_or(0, Follow::lines),
            unread: self.follow.as_ref().map(Follow::unread).unwrap_or_default(),
            sessions: self.watches.values().map(|w| w.view(now, params)).collect(),
            reports,
        }
    }
}

/// The watcher's loop (#566, step 2): a look every attr_recon_watch_secs
/// seconds, nothing read while it is 0, its snapshot published after each,
/// until `stop` asks it to end, in its wait or in a look.
pub async fn run(kernel: Kernel, mut stop: signal::Receiver<bool>, watched: Watched) {
    // skill-allow: §9-default — where a `~` in a written path points, and the adapter's own fallback root
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut watcher = Watcher::new(Utc::now(), home);
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
            watched.publish(watcher.snapshot(&params, Utc::now(), true));
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
        watched.publish(watcher.snapshot(&params, Utc::now(), false));
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
