//! The `repos` source (#501, moved in #510, in depth #538): the git
//! repositories the sessions worked in, read with git, read-only.
//!
//! It builds its list from the logs. Every working directory the `lines`
//! source wrote (`session_place`), and every root the operator names
//! (`attr_recon_include`), is followed to the checkout and
//! repository holding it, and what is new is added. The logs are the first
//! to report a change, so each pass follows them: a directory new in the
//! lines is followed in the same pass, and the checkouts the lines just
//! read were written in are read again (whether the folder is still there,
//! the branch it is on, and what git is in the middle of there). Every
//! `attr_recon_crawl_secs` it also follows every directory on the list and
//! reads every checkout, for what changed with no agent there. Nothing is
//! deleted: a checkout whose folder is gone is recorded `present = false`,
//! keeps its history, and is recorded present again when the folder is
//! back.
//!
//! Then it crawls repositories in depth ([`crawl`]): every one at a full
//! pass; at another, those new to the list and those a git event of the
//! pass's lines was written in. Each has a worker of its own, as many at
//! once as `attr_recon_repos_at_once` allows, which crawls beside it the
//! host repositories its remotes name ([`crate::crawler::hosts`]).
//!
//! Every git call is counted in the source's own record. Nothing a
//! repository names is ever run ([`git`]).

pub mod commits;
pub mod crawl;
pub mod git;
pub mod list;
pub mod operations;
pub mod reflog;
pub mod refs;
pub mod remotes;
pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::types::Uuid;
use superx_kernel::{Db, Result};

use crate::crawler::hosts::{self, objects::Known};
use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::repos::crawl::{Job, JobCheckout, Remembered, Shared};
use crate::crawler::repos::git::Git;
use crate::crawler::repos::store::{Checkout, CheckoutState, DirectoryRow, Held};
use crate::crawler::store::{self as rows, Batch};
use crate::crawler::{Outcome, Places, SourceRun, Stop};
use crate::params::Params;

/// The source's name, as `pass_source` records it.
pub const SOURCE: &str = "repos";

/// What the source keeps from one pass to the next: the list as it stands,
/// when it last read everything on it, what each repository's crawls
/// remember, and what the host repositories hold.
#[derive(Debug, Default)]
pub struct Memory {
    held: Option<Held>,
    last_full: Option<DateTime<Utc>>,
    repositories: HashMap<Uuid, Remembered>,
    known: HashMap<Uuid, Known>,
}

/// What the source did in one pass, and what the hosts' did beside it.
#[derive(Debug)]
pub struct Crawled {
    pub repos: SourceRun,
    pub hosts: SourceRun,
}

/// Follow the sessions' directories to their repositories, and crawl them.
/// `active` are the directories the lines of this pass were written in;
/// `began` is when the pass began.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the module's database cannot answer
/// or refuses a row.
#[allow(clippy::too_many_arguments)] // skill-allow: allow — what a pass gives the source, each its own
pub async fn crawl(
    db: &Db,
    params: &Params,
    places: &Places,
    memory: &mut Memory,
    hosts_memory: &hosts::Memory,
    active: &BTreeSet<String>,
    began: DateTime<Utc>,
    stop: &mut Stop,
) -> Result<Crawled> {
    let mut run = SourceRun::start(SOURCE);
    let mut hosts_run = SourceRun::start(hosts::SOURCE);
    if params.crawl_secs == 0 {
        run.note = Some("paused: attr_recon_crawl_secs is 0".to_string());
        return Ok(Crawled { repos: run, hosts: hosts_run });
    }
    if memory.held.is_none() {
        memory.held = Some(store::held(db).await?);
    }
    let Some(held) = memory.held.as_mut() else { return Ok(Crawled { repos: run, hosts: hosts_run }) };
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
    let mut new_repositories: BTreeSet<String> = BTreeSet::new();
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
                        new_repositories.insert(place.common.clone());
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
    // those the agents are working in now: still there, on which branch,
    // and in the middle of what (step 12).
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
            let operations = operations::git_dir(Path::new(&co.toplevel)).map(|d| operations::read(&d));
            CheckoutState { branch: b.branch, first_branch: b.first_branch, present: true, operations }
        } else {
            CheckoutState { branch: None, first_branch: None, present: false, operations: None }
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
    let list_tally = git.tally();
    rows::flush(db, &mut batch).await?;
    let mut rows_by: BTreeMap<&'static str, i64> = batch.added.clone();
    let mut hosts_rows: BTreeMap<&'static str, i64> = BTreeMap::new();

    // Step 1: which repositories are crawled in depth, each by a worker of
    // its own.
    let mut crawls = Vec::new();
    let mut commits_held = 0;
    let mut commits_read = 0;
    let mut tally = list_tally;
    let mut notes: Vec<String> = Vec::new();
    let mut partial = false;
    let mut host_crawls = Vec::new();
    if !stopped {
        let events = store::events(db).await?;
        let holders: Vec<(String, superx_kernel::types::RecordId)> =
            held.checkouts.values().map(|c| (c.toplevel.clone(), c.repository.clone())).collect();
        let mut jobs: Vec<Job> = Vec::new();
        for (common, repository) in &held.repositories {
            let name = list::repository_name(common, common.trim_end_matches("/.git"));
            let mine: Vec<&Checkout> = held.checkouts.values().filter(|c| c.repository == *repository).collect();
            let paths: Vec<&str> = mine.iter().map(|c| c.toplevel.as_str()).chain(std::iter::once(common.as_str())).collect();
            let name = mine.first().map_or(name, |c| list::repository_name(common, &c.toplevel));
            if params.excludes(&name, &paths) || !Path::new(common).is_dir() {
                continue;
            }
            let worked = || {
                events.iter().any(|e| {
                    e.valid_from >= began
                        && e.dir.as_deref().and_then(|d| holder(&holders, d)).is_some_and(|r| r == repository)
                })
            };
            if !(full || new_repositories.contains(common) || worked()) {
                continue;
            }
            let checkouts = mine
                .iter()
                .map(|c| {
                    let state = record_uuid_of(&c.id).and_then(|k| held.states.get(&k));
                    JobCheckout {
                        id: c.id.clone(),
                        toplevel: c.toplevel.clone(),
                        present: state.is_some_and(|s| s.present),
                        branch: state.and_then(|s| s.branch.clone()),
                    }
                })
                .collect();
            jobs.push(Job { repository: repository.clone(), common: common.clone(), name, checkouts });
        }
        jobs.sort_by(|a, b| a.common.cmp(&b.common));

        let credentials = places.credentials.as_ref().map_or(crate::credentials::Credentials::None, |p| crate::credentials::read(p));
        let pass = Arc::new(
            hosts::Pass::begin(db, places.hosts.clone(), credentials, params.host_timeout_ms, hosts_memory.clone(), std::mem::take(&mut memory.known)).await?,
        );
        let shared = Arc::new(Shared { db: db.clone(), params: params.clone(), places: places.clone(), hosts: Arc::clone(&pass), events, holders });
        let at_once = usize::try_from(params.repos_at_once).unwrap_or(usize::MAX).clamp(1, tokio::sync::Semaphore::MAX_PERMITS);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(at_once));
        let mut set = tokio::task::JoinSet::new();
        for job in jobs {
            let Some(permit) = stop.unless(Arc::clone(&semaphore).acquire_owned()).await else {
                stopped = true;
                break;
            };
            let remembered = record_uuid_of(&job.repository).and_then(|k| memory.repositories.remove(&k)).unwrap_or_default();
            let (shared, worker_stop) = (Arc::clone(&shared), stop.clone());
            set.spawn(async move {
                let _permit = permit;
                let key = record_uuid_of(&job.repository);
                (key, crawl::crawl(shared, job, remembered, worker_stop).await)
            });
        }
        // The pass waits for every worker before it records itself.
        while let Some(done) = set.join_next().await {
            let Ok((key, done)) = done else { continue };
            stopped |= done.stopped;
            if let Some(key) = key {
                memory.repositories.insert(key, done.memory.clone());
            }
            tally.add(done.tally);
            commits_held += done.commits_held;
            commits_read += done.commits_read;
            for (table, n) in &done.rows {
                let into = if hosts::crawl::host_tables(table) { &mut hosts_rows } else { &mut rows_by };
                *into.entry(table).or_default() += n;
            }
            if let Some(c) = done.crawl {
                crawls.push(c);
            }
            if !done.stopped && matches!(done.outcome, "partial" | "unreadable") {
                partial = true;
                let said = done.note.as_ref().map_or(String::new(), |n| format!(": {n}"));
                notes.push(format!("{} {}{said}", done.name, done.outcome));
            }
            if !done.stopped && done.commits_read != done.commits_held {
                notes.push(format!("{}: {} commits read of {} held", done.name, done.commits_read, done.commits_held));
            }
            host_crawls.extend(done.hosts);
            memory.known.extend(done.known);
        }
        memory.known.extend(pass.leftover_known());
    }

    counts.insert("commits_held", commits_held);
    counts.insert("commits_read", commits_read);
    run.calls = count_u64(tally.calls);
    run.failed = count_u64(tally.failed);
    run.timed_out = count_u64(tally.timeouts);
    run.rows = rows_by;
    run.counts = counts;
    run.crawls = crawls;
    if stopped {
        run.outcome = Outcome::Stopped;
        run.note = Some("stopped before it finished".to_string());
        hosts_run.outcome = Outcome::Stopped;
        hosts_run.note = Some("stopped before it finished".to_string());
        hosts_run.finished_at = Utc::now();
    } else {
        if full {
            memory.last_full = Some(started);
        }
        if list_tally.timeouts > 0 {
            partial = true;
            notes.insert(
                0,
                format!("{} git calls cut off at attr_recon_git_timeout_ms ({} ms)", list_tally.timeouts, params.git_timeout_ms),
            );
        }
        if partial {
            run.outcome = Outcome::Partial;
        }
        run.note = (!notes.is_empty()).then(|| notes.join("; "));
        hosts_run = hosts::crawl::record(&host_crawls, hosts_rows);
    }
    run.finished_at = Utc::now();
    Ok(Crawled { repos: run, hosts: hosts_run })
}

/// The repository whose checkout holds `dir`: the one with the longest root
/// that `dir` is or is under.
fn holder<'a>(holders: &'a [(String, superx_kernel::types::RecordId)], dir: &str) -> Option<&'a superx_kernel::types::RecordId> {
    let dir = dir.trim_end_matches('/');
    holders
        .iter()
        .filter(|(root, _)| {
            let root = root.trim_end_matches('/');
            dir == root || dir.starts_with(&format!("{root}/"))
        })
        .max_by_key(|(root, _)| root.len())
        .map(|(_, repository)| repository)
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
