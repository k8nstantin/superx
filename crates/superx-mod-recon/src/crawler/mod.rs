//! The crawler (#510): one crawler fills the module's one database, pass
//! after pass while `superx-recon` runs, through its sources.
//!
//! A source reads one kind of thing: [`lines`], the lines the kernel
//! captured; [`repos`], the git repositories those lines lead to. Sources
//! share facts, not messages: a source reads what the others wrote to the
//! database. A pass runs them in the order their facts need: `lines`
//! first, then `repos`, which follows the working directories `lines` has
//! just written. Each source spreads its own work as far as its parameters
//! allow. A source added later (the GitHub and Bitbucket hosts, and
//! others) is one more module here, with its own tables of facts.
//!
//! A pass ends with its own row and one row per source (`pass`,
//! `pass_source`): what each source worked through, what it could not
//! read, its outside calls, the rows it added. A source that fails is
//! recorded `failed`, its memory dropped, and read again next pass; when
//! even that record cannot be written, the pass is not recorded and the
//! crawler starts over from its database.
//!
//! The crawler only reads the kernel (SELECT) and the repositories (git,
//! read-only), and only writes its own database (CREATE).

pub mod lines;
pub mod repos;
pub mod store;
pub mod value;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Uuid};
use superx_kernel::{Db, Kernel};
use tokio::sync::watch;

use crate::crawler::store::{Batch, Row};
use crate::params::{self, Params};
use crate::{status, MODULE_NAME};

/// While paused or without its database, how often the crawler looks again.
const RECHECK_SECS: u64 = 60; // skill-allow: §9-const — idle re-check cadence, not a policy tunable

/// A request to stop, as the crawler sees it.
#[derive(Debug, Clone)]
pub struct Stop(watch::Receiver<bool>);

impl Stop {
    #[must_use]
    pub fn new(asked: watch::Receiver<bool>) -> Self {
        Self(asked)
    }

    /// A stop was asked for.
    #[must_use]
    pub fn requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once a stop is asked for, or once nothing can ask for one
    /// any more: either way, it is time to stop.
    pub async fn wait(&mut self) {
        let _asked_or_gone = self.0.wait_for(|stopped| *stopped).await;
    }

    /// `step`, unless a stop comes first: then `None`, and `step` is
    /// dropped, which kills a git call it was waiting on.
    pub async fn unless<T>(&mut self, step: impl Future<Output = T>) -> Option<T> {
        if self.requested() {
            return None;
        }
        tokio::select! {
            out = step => Some(out),
            () = self.wait() => None,
        }
    }
}

/// How a source's part of a pass ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    /// It could not read everything.
    Partial,
    /// It stopped on an error.
    Failed,
    /// The module was stopped.
    Stopped,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// What one source did in a pass, as its `pass_source` row records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRun {
    pub source: &'static str,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    /// What it worked through: lines, directories and checkouts.
    pub items: i64,
    /// Of those, the ones in a shape it does not know.
    pub unread: i64,
    /// Calls it made outside the database, and those that did not end well.
    pub calls: i64,
    pub failed: i64,
    pub timed_out: i64,
    /// Rows it added, by table.
    pub rows: BTreeMap<&'static str, i64>,
    /// What else it counts, by name.
    pub counts: BTreeMap<&'static str, i64>,
    /// Where a source that reads in order began and stopped.
    pub from_id: Option<Uuid>,
    pub to_id: Option<Uuid>,
    /// The repository crawls it ran.
    pub crawls: Vec<RecordId>,
    pub outcome: Outcome,
    pub note: Option<String>,
}

impl SourceRun {
    /// A source's run that has done nothing yet.
    #[must_use]
    pub fn start(source: &'static str) -> Self {
        let now = Utc::now();
        Self {
            source,
            started_at: now,
            finished_at: now,
            items: 0,
            unread: 0,
            calls: 0,
            failed: 0,
            timed_out: 0,
            rows: BTreeMap::new(),
            counts: BTreeMap::new(),
            from_id: None,
            to_id: None,
            crawls: Vec::new(),
            outcome: Outcome::Complete,
            note: None,
        }
    }

    /// The rows it added in all.
    #[must_use]
    pub fn rows_written(&self) -> i64 {
        self.rows.values().sum()
    }
}

/// What one pass did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pass {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub sources: Vec<SourceRun>,
    pub rows_written: i64,
    pub outcome: &'static str,
    pub note: Option<String>,
}

impl Pass {
    /// What `source` did in this pass.
    #[must_use]
    pub fn source(&self, source: &str) -> Option<&SourceRun> {
        self.sources.iter().find(|s| s.source == source)
    }
}

/// What the crawler keeps from one pass to the next: its connection to its
/// own database, and each source's memory.
#[derive(Default)]
pub struct Crawler {
    db: Option<Db>,
    lines: lines::Memory,
    repos: repos::Memory,
}

/// Pass after pass, until asked to stop: one right away, then one every
/// `attr_recon_read_secs`, the parameters read afresh before each.
/// Nothing is read while it is 0, or while the module's database is not
/// provisioned; both are looked at again a minute later.
pub async fn run(kernel: Kernel, mut stop: Stop) {
    let mut crawler = Crawler::default();
    loop {
        let wait_secs = match params::resolve(&kernel).await {
            Err(e) => {
                tracing::warn!(target: "recon", error = %e, "no pass: its parameters cannot be read");
                RECHECK_SECS
            }
            Ok(resolved) if resolved.params.read_secs == 0 => RECHECK_SECS,
            Ok(resolved) => {
                if crawler.db.is_none() {
                    match kernel.module_db(MODULE_NAME).await {
                        Ok(db) => crawler.db = Some(db),
                        Err(e) => tracing::warn!(
                            target: "recon",
                            error = %status::own_hint(&e.to_string()),
                            "no pass: its database is not available"
                        ),
                    }
                }
                if crawler.db.is_some() {
                    log(pass(&kernel, &resolved.params, &mut crawler, &mut stop).await);
                    resolved.params.read_secs
                } else {
                    RECHECK_SECS
                }
            }
        };
        if stop.unless(tokio::time::sleep(std::time::Duration::from_secs(wait_secs))).await.is_none() {
            break;
        }
    }
}

fn log(pass: superx_kernel::Result<Pass>) {
    match pass {
        Ok(p) if p.rows_written > 0 || p.outcome != "complete" => {
            let lines = p.source(lines::SOURCE);
            let repos = p.source(repos::SOURCE);
            tracing::info!(
                target: "recon",
                outcome = p.outcome,
                lines_read = lines.map_or(0, |s| s.items),
                lines_unread = lines.map_or(0, |s| s.unread),
                repos_items = repos.map_or(0, |s| s.items),
                git_calls = repos.map_or(0, |s| s.calls),
                rows_written = p.rows_written,
                "pass"
            );
        }
        Ok(p) => tracing::debug!(target: "recon", sources = p.sources.len(), "pass, nothing new"),
        Err(e) => tracing::warn!(target: "recon", error = %e, "the pass failed and was not recorded"),
    }
}

/// One pass, recorded when it ends. A stop ends it where it is, and it is
/// recorded as `stopped`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot record the
/// pass. Nothing the crawler kept survives that: the next pass starts over
/// from its database.
pub async fn pass(kernel: &Kernel, params: &Params, crawler: &mut Crawler, stop: &mut Stop) -> superx_kernel::Result<Pass> {
    let Some(db) = crawler.db.clone() else {
        return Err(superx_kernel::KernelError::Config("the crawler has no database".to_string()));
    };
    let result = passes(kernel, &db, params, crawler, stop).await;
    if result.is_err() {
        *crawler = Crawler::default();
    }
    result
}

/// [`pass`] on a database the caller holds: for the tests, which hold the
/// module's database without an instance behind it.
///
/// # Errors
///
/// As [`pass`].
pub async fn pass_on(
    kernel: &Kernel,
    db: &Db,
    params: &Params,
    crawler: &mut Crawler,
    stop: &mut Stop,
) -> superx_kernel::Result<Pass> {
    crawler.db = Some(db.clone());
    pass(kernel, params, crawler, stop).await
}

async fn passes(
    kernel: &Kernel,
    db: &Db,
    params: &Params,
    crawler: &mut Crawler,
    stop: &mut Stop,
) -> superx_kernel::Result<Pass> {
    let started_at = Utc::now();
    let mut sources = Vec::new();
    // `lines` first: the logs report a change first, and `repos` follows
    // them to the repositories they were written in.
    let (lines, worked_in) = match lines::read(kernel, db, &mut crawler.lines, stop).await {
        Ok(reading) => (reading.run, reading.worked_in),
        Err(e) => {
            crawler.lines = lines::Memory::default();
            (failed(lines::SOURCE, &e), BTreeSet::new())
        }
    };
    let stopped = lines.outcome == Outcome::Stopped;
    sources.push(lines);
    if !stopped {
        let repos = match repos::crawl(db, params, &mut crawler.repos, &worked_in, stop).await {
            Ok(run) => run,
            Err(e) => {
                crawler.repos = repos::Memory::default();
                failed(repos::SOURCE, &e)
            }
        };
        sources.push(repos);
    }
    let outcome = if sources.iter().any(|s| s.outcome == Outcome::Stopped) {
        "stopped"
    } else if sources.iter().any(|s| matches!(s.outcome, Outcome::Partial | Outcome::Failed)) {
        "partial"
    } else {
        "complete"
    };
    let note = sources
        .iter()
        .filter_map(|s| s.note.as_ref().filter(|_| s.outcome != Outcome::Complete).map(|n| format!("{}: {n}", s.source)))
        .collect::<Vec<_>>();
    let pass = Pass {
        started_at,
        finished_at: Utc::now(),
        rows_written: sources.iter().map(SourceRun::rows_written).sum(),
        sources,
        outcome,
        note: (!note.is_empty()).then(|| note.join("; ")),
    };
    record(db, &pass).await?;
    Ok(pass)
}

/// A source that stopped on an error: recorded with what the error said.
fn failed(source: &'static str, e: &superx_kernel::KernelError) -> SourceRun {
    SourceRun { outcome: Outcome::Failed, note: Some(e.to_string()), ..SourceRun::start(source) }
}

/// The pass's row, then one row per source.
async fn record(db: &Db, pass: &Pass) -> superx_kernel::Result<()> {
    let mut batch = Batch::default();
    let row = batch.create(
        "pass",
        Row::new()
            .time("started_at", pass.started_at)
            .time("finished_at", pass.finished_at)
            .int("rows_written", pass.rows_written)
            .text("outcome", pass.outcome)
            .some_text("note", pass.note.clone()),
    );
    for s in &pass.sources {
        batch.create(
            "pass_source",
            Row::new()
                .link("pass", &row)
                .text("source", s.source)
                .time("started_at", s.started_at)
                .time("finished_at", s.finished_at)
                .int("items", s.items)
                .int("unread", s.unread)
                .int("calls", s.calls)
                .int("failed", s.failed)
                .int("timed_out", s.timed_out)
                .counts("rows", &s.rows)
                .counts("counts", &s.counts)
                .some_uuid("from_id", s.from_id)
                .some_uuid("to_id", s.to_id)
                .links("crawls", &s.crawls)
                .text("outcome", s.outcome.as_str())
                .some_text("note", s.note.clone()),
        );
    }
    store::flush(db, &mut batch).await
}
