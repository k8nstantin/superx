//! One pass over the list (#501), every `attr_repos_crawl_secs`.
//!
//! A pass reads the working directories the sessions' lines carried since
//! the last pass (all of history the first time) and the roots the operator
//! names (`attr_repos_include`). It follows every directory on the list, old
//! and new, to the checkout and repository holding it, and adds what is
//! new. It reads every checkout on the list: whether its folder is still
//! there, and the branch it is on. Nothing is deleted: a checkout whose
//! folder is gone is recorded `present = false`, keeps its history, and is
//! recorded present again when the folder is back. Then the pass records
//! itself: what it read, what it found and wrote, and every git call it
//! made, counted.
//!
//! The kernel is read a page of lines at a time, by the lines' own ids, so
//! no read is unbounded and capture, which shares the substrate, is never
//! kept waiting long (#368). A pass reads from a minute before the last one
//! started, so a line captured as it began is read twice rather than never;
//! directories are a set.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::path::Path;

use chrono::{TimeDelta, Utc};
use superx_kernel::types::{RecordId, Uuid};
use superx_kernel::{Db, Kernel};
use tokio::sync::watch;

use crate::git::Git;
use crate::params::{self, Params};
use crate::store::{self, CheckoutState, DirectoryRow, Sweep};
use crate::{list, MODULE_NAME};

/// Lines one read of the kernel takes: on a copy of the live database the
/// slowest such page took 21 ms (#501).
const PAGE_LINES: u32 = 500; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How far before the last pass's start a pass reads again.
const OVERLAP_SECS: i64 = 60; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How soon a module with no pass to run looks again: crawling off
/// (`attr_repos_crawl_secs` = 0), or its database not provisioned yet.
const RECHECK_SECS: u64 = 60; // skill-allow: §9-const — idle re-check cadence, not a policy tunable

/// Whether the module was asked to stop, shared by everything it runs.
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

/// Pass after pass, until asked to stop: one right away, then one every
/// `attr_repos_crawl_secs`, read afresh before each. Nothing is crawled
/// while it is 0, or while the module's database is not provisioned; both
/// are looked at again a minute later.
pub async fn run(kernel: Kernel, mut stop: Stop) {
    loop {
        let wait_secs = match params::resolve(&kernel).await {
            Err(e) => {
                tracing::warn!(target: "repos", error = %e, "no pass: its parameters cannot be read");
                RECHECK_SECS
            }
            Ok(resolved) if resolved.params.crawl_secs == 0 => RECHECK_SECS,
            Ok(resolved) => match kernel.module_db(MODULE_NAME).await {
                Err(e) => {
                    tracing::warn!(target: "repos", error = %e, "no pass: its database is not available");
                    RECHECK_SECS
                }
                Ok(db) => {
                    match pass(&kernel, &db, &resolved.params, &mut stop).await {
                        Ok(s) => tracing::info!(
                            target: "repos",
                            outcome = s.outcome,
                            directories_read = s.directories_read,
                            repositories = s.repositories_known,
                            repositories_new = s.repositories_new,
                            repositories_stale = s.repositories_stale,
                            checkouts = s.checkouts_known,
                            checkouts_gone = s.checkouts_gone,
                            rows_written = s.rows_written,
                            git_calls = s.git_calls,
                            "pass"
                        ),
                        Err(e) => tracing::warn!(target: "repos", error = %e, "the pass failed and was not recorded"),
                    }
                    resolved.params.crawl_secs
                }
            },
        };
        if stop.unless(tokio::time::sleep(std::time::Duration::from_secs(wait_secs))).await.is_none() {
            return;
        }
    }
}

/// One pass, recorded when it ends. A stop ends it where it is, and it is
/// recorded as `stopped`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when a database cannot answer or refuses
/// a row. The pass is not recorded then; the next one reads again from the
/// last one recorded.
pub async fn pass(kernel: &Kernel, db: &Db, params: &Params, stop: &mut Stop) -> superx_kernel::Result<Sweep> {
    let started = Utc::now();
    let mut held = store::held(db).await?;
    let mut sweep =
        Sweep { started_at: Some(started), interval_secs: params.crawl_secs, outcome: "complete", ..Sweep::default() };
    let mut git = Git::new(params.git_timeout_ms);
    let mut stopped = false;
    let mut unread: Option<String> = None;

    // What the sessions' lines carried since the last pass began, a minute
    // before it; all of them the first time.
    let mut cursor = store::Cursor::At(
        held.last_pass.map_or_else(store::first_id, |t| store::first_id_at(t - TimeDelta::seconds(OVERLAP_SECS))),
    );
    // Sessions are counted by their UUID: a set keyed by a record id is one
    // clippy refuses (`mutable_key_type`).
    let mut sessions: HashSet<Uuid> = HashSet::new();
    let mut read: BTreeSet<String> = BTreeSet::new();
    loop {
        let Some(page) = stop.unless(store::lines(kernel, cursor, PAGE_LINES)).await else {
            stopped = true;
            break;
        };
        let page = page?;
        for line in &page {
            if let (Some(session), Some(cwd)) = (line.session.as_ref().and_then(store::uuid_of), &line.cwd) {
                sessions.insert(session);
                read.insert(cwd.clone());
            }
        }
        if page.len() < PAGE_LINES as usize {
            break;
        }
        match page.iter().rev().find_map(|l| store::uuid_of(&l.id)) {
            Some(last) => cursor = store::Cursor::After(last),
            None => {
                unread = Some("a page of captured lines held no UUID ids; reading stopped there".to_string());
                break;
            }
        }
    }
    sweep.sessions_read = sessions.len() as u64;
    let include: BTreeSet<String> = params.include.iter().filter(|r| !r.is_empty()).cloned().collect();
    sweep.directories_read = read.union(&include).count() as u64;

    // Every directory on the list or just read, followed to its checkout.
    // A directory a session worked in is a session's, whatever else names it.
    let mut directories: BTreeMap<String, &'static str> =
        held.directories.iter().map(|(path, row)| (path.clone(), row.source)).collect();
    for path in &include {
        directories.entry(path.clone()).or_insert("include");
    }
    for path in &read {
        directories.insert(path.clone(), "session");
    }
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    for (path, source) in &directories {
        if stopped {
            break;
        }
        let Some(place) = stop.unless(list::locate(&mut git, path)).await else {
            stopped = true;
            break;
        };
        let want = match place {
            Some(place) => {
                let name = list::repository_name(&place.common, &place.toplevel);
                if params.excludes(&name, &[&place.toplevel, &place.common]) {
                    excluded.insert(place.common);
                    continue;
                }
                let repository = match held.repositories.get(&place.common) {
                    Some(id) => id.clone(),
                    None => {
                        let id = store::create_repository(db, &place.common, &name, Utc::now()).await?;
                        sweep.repositories_new += 1;
                        sweep.rows_written += 1;
                        held.repositories.insert(place.common.clone(), id.clone());
                        id
                    }
                };
                let checkout = match held.checkouts.get(&place.toplevel) {
                    Some(c) => c.id.clone(),
                    None => {
                        let id = store::create_checkout(db, &repository, &place.toplevel, Utc::now()).await?;
                        sweep.checkouts_new += 1;
                        sweep.rows_written += 1;
                        held.checkouts.insert(
                            place.toplevel.clone(),
                            store::Checkout {
                                id: id.clone(),
                                repository,
                                common: place.common.clone(),
                                toplevel: place.toplevel.clone(),
                            },
                        );
                        id
                    }
                };
                DirectoryRow { source, checkout: Some(checkout), exact: place.exact }
            }
            None => DirectoryRow { source, checkout: None, exact: Path::new(path).is_dir() },
        };
        let had = held.directories.get(path);
        if had != Some(&want) {
            if had.is_some() {
                sweep.directories_moved += 1;
            } else {
                sweep.directories_new += 1;
            }
            store::create_directory(db, path, &want, Utc::now()).await?;
            sweep.rows_written += 1;
            held.directories.insert(path.clone(), want);
        }
    }
    sweep.excluded = excluded.len() as u64;

    // Every checkout on the list: still there, and on which branch.
    let mut checkouts: Vec<store::Checkout> = held.checkouts.values().cloned().collect();
    checkouts.sort_by(|a, b| a.toplevel.cmp(&b.toplevel));
    for co in &checkouts {
        if stopped {
            break;
        }
        if params.excludes(&list::repository_name(&co.common, &co.toplevel), &[&co.toplevel, &co.common]) {
            continue;
        }
        let state = if Path::new(&co.toplevel).is_dir() {
            let Some(b) = stop.unless(list::branches(&mut git, &co.toplevel)).await else {
                stopped = true;
                break;
            };
            CheckoutState { branch: b.branch, first_branch: b.first_branch, present: true }
        } else {
            CheckoutState { branch: None, first_branch: None, present: false }
        };
        let had = held.states.get(&co.id);
        if had != Some(&state) {
            match had {
                Some(h) if h.present && !state.present => sweep.checkouts_gone += 1,
                Some(h) if !h.present && state.present => sweep.checkouts_back += 1,
                _ => {}
            }
            store::create_checkout_state(db, &co.id, &state, Utc::now()).await?;
            sweep.rows_written += 1;
            held.states.insert(co.id.clone(), state);
        }
    }

    sweep.repositories_known = held.repositories.len() as u64;
    sweep.checkouts_known = held.checkouts.len() as u64;
    let present = |repository: &RecordId| {
        held.checkouts
            .values()
            .any(|c| c.repository == *repository && held.states.get(&c.id).is_some_and(|s| s.present))
    };
    sweep.repositories_stale = held.repositories.values().filter(|id| !present(id)).count() as u64;
    let tally = git.tally();
    sweep.git_calls = tally.calls;
    sweep.git_failed = tally.failed;
    sweep.git_timeouts = tally.timeouts;
    if stopped {
        sweep.outcome = "stopped";
        sweep.note = Some("stopped before it finished".to_string());
    } else if let Some(why) = unread {
        sweep.outcome = "partial";
        sweep.note = Some(why);
    } else if tally.timeouts > 0 {
        sweep.outcome = "partial";
        sweep.note =
            Some(format!("{} git calls cut off at attr_repos_git_timeout_ms ({} ms)", tally.timeouts, params.git_timeout_ms));
    }
    store::create_sweep(db, &sweep, Utc::now()).await?;
    Ok(sweep)
}
