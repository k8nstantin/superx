//! The `repos` source (#501, moved in #510): the git repositories the
//! sessions worked in, read with git, read-only.
//!
//! It builds its list from the logs. Every working directory the `lines`
//! source wrote (`session_place`), and every root the operator names
//! (`attr_performance_include`), is followed to the checkout and
//! repository holding it, and what is new is added. The logs are the first
//! to report a change, so each pass follows them: a directory new in the
//! lines is followed in the same pass, and the checkouts the lines just
//! read were written in are read again (whether the folder is still there,
//! and the branch it is on). Every `attr_performance_crawl_secs` it also
//! follows every directory on the list and reads every checkout, for what
//! changed with no agent there. Nothing is deleted: a checkout whose folder
//! is gone is recorded `present = false`, keeps its history, and is
//! recorded present again when the folder is back.
//!
//! Every git call is counted in the source's own record. Nothing a
//! repository names is ever run ([`git`]).

pub mod git;
pub mod list;
pub mod store;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::{Db, Result};

use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::repos::git::Git;
use crate::crawler::repos::store::{Checkout, CheckoutState, DirectoryRow, Held};
use crate::crawler::store::{self as rows, Batch};
use crate::crawler::{Outcome, SourceRun, Stop};
use crate::params::Params;

/// The source's name, as `pass_source` records it.
pub const SOURCE: &str = "repos";

/// What the source keeps from one pass to the next: the list as it stands,
/// and when it last read everything on it.
#[derive(Debug, Default)]
pub struct Memory {
    held: Option<Held>,
    last_full: Option<DateTime<Utc>>,
}

/// Follow the sessions' directories to their repositories. `active` are
/// the directories the lines of this pass were written in.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the module's database cannot answer
/// or refuses a row.
pub async fn crawl(
    db: &Db,
    params: &Params,
    memory: &mut Memory,
    active: &BTreeSet<String>,
    stop: &mut Stop,
) -> Result<SourceRun> {
    let mut run = SourceRun::start(SOURCE);
    if params.crawl_secs == 0 {
        run.note = Some("paused: attr_performance_crawl_secs is 0".to_string());
        return Ok(run);
    }
    if memory.held.is_none() {
        memory.held = Some(store::held(db).await?);
    }
    let Some(held) = memory.held.as_mut() else { return Ok(run) };
    let started = Utc::now();
    let every = TimeDelta::seconds(i64::try_from(params.crawl_secs).unwrap_or(i64::MAX));
    let full = memory.last_full.is_none_or(|t| started - t >= every);

    // The directories to follow: on a full pass every one on the list and
    // every one there is; otherwise those not on the list yet. A directory a
    // session worked in is a session's, whatever else names it.
    let worked_in = store::worked_in(db).await?;
    let include: BTreeSet<String> = params.include.iter().filter(|r| !r.is_empty()).cloned().collect();
    let mut directories: BTreeMap<String, &'static str> = BTreeMap::new();
    if full {
        directories.extend(held.directories.iter().map(|(path, row)| (path.clone(), row.source)));
    }
    for path in &include {
        if full || !held.directories.contains_key(path) {
            directories.entry(path.clone()).or_insert("include");
        }
    }
    for path in &worked_in {
        if full || !held.directories.contains_key(path) {
            directories.insert(path.clone(), "session");
        }
    }

    let mut git = Git::new(params.git_timeout_ms);
    let mut batch = Batch::default();
    let mut counts: BTreeMap<&'static str, i64> = BTreeMap::new();
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    let mut new_checkouts: Vec<Checkout> = Vec::new();
    let mut stopped = false;
    for (path, source) in &directories {
        let Some(place) = stop.unless(list::locate(&mut git, path)).await else {
            stopped = true;
            break;
        };
        run.items += 1;
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
                        let id = batch.create("repository", store::repository(&place.common, &name, Utc::now()));
                        bump(&mut counts, "repositories_new");
                        held.repositories.insert(place.common.clone(), id.clone());
                        id
                    }
                };
                let checkout = match held.checkouts.get(&place.toplevel) {
                    Some(c) => c.id.clone(),
                    None => {
                        let id = batch.create("checkout", store::checkout(&repository, &place.toplevel, Utc::now()));
                        bump(&mut counts, "checkouts_new");
                        let c = Checkout {
                            id: id.clone(),
                            repository,
                            common: place.common.clone(),
                            toplevel: place.toplevel.clone(),
                        };
                        held.checkouts.insert(place.toplevel.clone(), c.clone());
                        new_checkouts.push(c);
                        id
                    }
                };
                DirectoryRow { source, checkout: Some(checkout), exact: place.exact }
            }
            None => DirectoryRow { source, checkout: None, exact: Path::new(path).is_dir() },
        };
        let had = held.directories.get(path);
        if had != Some(&want) {
            bump(&mut counts, if had.is_some() { "directories_moved" } else { "directories_new" });
            batch.create("directory", store::directory(path, &want));
            held.directories.insert(path.clone(), want);
        }
    }
    counts.insert("directories_read", count(directories.len()));

    // Every checkout on the list on a full pass; otherwise the new ones and
    // those the agents are working in now: still there, and on which branch.
    let mut checkouts: Vec<Checkout> = if full {
        held.checkouts.values().cloned().collect()
    } else {
        let working: Vec<&superx_kernel::types::RecordId> =
            active.iter().filter_map(|d| held.directories.get(d)).filter_map(|row| row.checkout.as_ref()).collect();
        let mut chosen = new_checkouts;
        for c in held.checkouts.values() {
            if working.contains(&&c.id) && !chosen.iter().any(|n| n.id == c.id) {
                chosen.push(c.clone());
            }
        }
        chosen
    };
    counts.insert("checkouts_read", count(checkouts.len()));
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
        run.items += 1;
        let Some(key) = record_uuid_of(&co.id) else { continue };
        let had = held.states.get(&key);
        if had != Some(&state) {
            match had {
                Some(h) if h.present && !state.present => bump(&mut counts, "checkouts_gone"),
                Some(h) if !h.present && state.present => bump(&mut counts, "checkouts_back"),
                _ => {}
            }
            batch.create("checkout_state", store::checkout_state(&co.id, &state));
            held.states.insert(key, state);
        }
    }

    counts.insert("repositories_known", count(held.repositories.len()));
    counts.insert("checkouts_known", count(held.checkouts.len()));
    let present = |repository: &superx_kernel::types::RecordId| {
        held.checkouts.values().any(|c| {
            c.repository == *repository
                && record_uuid_of(&c.id).and_then(|k| held.states.get(&k)).is_some_and(|s| s.present)
        })
    };
    counts.insert("repositories_stale", count(held.repositories.values().filter(|id| !present(id)).count()));
    counts.insert("excluded", count(excluded.len()));
    let tally = git.tally();
    run.calls = count_u64(tally.calls);
    run.failed = count_u64(tally.failed);
    run.timed_out = count_u64(tally.timeouts);
    rows::flush(db, &mut batch).await?;
    run.rows = batch.added.clone();
    run.counts = counts;
    if stopped {
        run.outcome = Outcome::Stopped;
        run.note = Some("stopped before it finished".to_string());
    } else {
        if full {
            memory.last_full = Some(started);
        }
        if tally.timeouts > 0 {
            run.outcome = Outcome::Partial;
            run.note = Some(format!(
                "{} git calls cut off at attr_performance_git_timeout_ms ({} ms)",
                tally.timeouts, params.git_timeout_ms
            ));
        }
    }
    run.finished_at = Utc::now();
    Ok(run)
}

fn bump(counts: &mut BTreeMap<&'static str, i64>, name: &'static str) {
    *counts.entry(name).or_default() += 1;
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn count_u64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}
