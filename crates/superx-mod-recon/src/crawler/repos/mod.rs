//! The `repos` source (#501, moved in #510): the git repositories the
//! sessions worked in, read with git, read-only.
//!
//! It builds its list from the logs. Every working directory the `lines`
//! source wrote (`session_place`), and every root the operator names
//! (`attr_recon_include`), is followed to the checkout and
//! repository holding it, and what is new is added. The logs are the first
//! to report a change, so each pass follows them: a directory new in the
//! lines is followed in the same pass, and the checkouts the lines just
//! read were written in are read again (whether the folder is still there,
//! and the branch it is on). Every `attr_recon_crawl_secs` it also
//! follows every directory on the list and reads every checkout, for what
//! changed with no agent there. Nothing is deleted: a checkout whose folder
//! is gone is recorded `present = false`, keeps its history, and is
//! recorded present again when the folder is back.
//!
//! Every git call is counted in the source's own record. Nothing a
//! repository names is ever run ([`git`]).
//!
//! Then it crawls repositories in depth (#538): at a full pass every one on
//! the list, at any other the new ones and those a git event of the pass's
//! lines was printed in, each by a worker of its own ([`crawl`]), as many
//! at once as `attr_recon_repos_at_once` allows; each worker crawls beside
//! its repository the host repositories it takes ([`crate::crawler::hosts`]).
//! The repos source and the hosts source each record what they did.

pub mod commits;
pub mod crawl;
pub mod git;
pub mod history;
pub mod known;
pub mod list;
pub mod mirror;
pub mod operations;
pub mod record;
pub mod reflogs;
pub mod refs;
pub mod remotes;
pub mod sessions;
pub mod store;
pub mod survival;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::crawler::hosts::call::Client;
use crate::crawler::hosts::crawl::{Shared, Summary};
use crate::crawler::hosts::held::{row_key, HostMemory};
use crate::crawler::hosts::objects::Accounts;
use crate::crawler::hosts::{self, HostPath};
use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::repos::crawl::{Context, Finished, Job, JobCheckout};
use crate::crawler::repos::git::Git;
use crate::crawler::repos::store::{Checkout, CheckoutState, DirectoryRow, Held};
use crate::crawler::store::{self as rows, Batch};
use crate::crawler::{Outcome, Places, SourceRun, Stop};
use crate::credentials::{self, Credentials};
use crate::params::Params;

/// The source's name, as `pass_source` records it.
pub const SOURCE: &str = "repos";

/// The actions of a git event that make a pass crawl the repository its
/// directory is in (step 1).
const MOVES: [&str; 10] =
    ["commit", "push", "merge", "rebase", "revert", "cherry_pick", "reset", "branch_deleted", "pr_created", "pr_merged"];

/// What the source keeps from one pass to the next: the list as it stands,
/// and when it last read everything on it; each repository's refs at its
/// previous crawl and the tip it was last blamed whole at, by its common
/// git directory; what the hosts part holds of each host repository, and
/// the accounts.
#[derive(Debug, Default)]
pub struct Memory {
    held: Option<Held>,
    last_full: Option<DateTime<Utc>>,
    refs: HashMap<String, refs::Read>,
    blamed: HashMap<String, String>,
    hosts: Arc<std::sync::Mutex<HashMap<HostPath, HostMemory>>>,
    accounts: Arc<tokio::sync::Mutex<Accounts>>,
}

/// What the repos source and the hosts source did in a pass.
#[derive(Debug)]
pub struct Crawled {
    pub repos: SourceRun,
    pub hosts: SourceRun,
}

/// Follow the sessions' directories to their repositories, then crawl
/// them in depth. `active` are the directories the lines of this pass were
/// written in; `began` is when the pass began.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the module's database cannot answer
/// or refuses a row.
pub async fn crawl(
    db: &Db,
    params: &Params,
    places: &Places,
    memory: &mut Memory,
    active: &BTreeSet<String>,
    began: DateTime<Utc>,
    stop: &mut Stop,
) -> Result<Crawled> {
    let mut run = SourceRun::start(SOURCE);
    let mut host_run = SourceRun::start(hosts::SOURCE);
    if params.crawl_secs == 0 {
        run.note = Some("paused: attr_recon_crawl_secs is 0".to_string());
        host_run.note = run.note.clone();
        return Ok(Crawled { repos: run, hosts: host_run });
    }
    if memory.held.is_none() {
        memory.held = Some(store::held(db).await?);
    }
    let Some(held) = memory.held.as_mut() else { return Ok(Crawled { repos: run, hosts: host_run }) };
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
                        held.names.insert(place.common.clone(), name.clone());
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
            let ops = operations::git_dir(Path::new(&co.toplevel)).map(|dir| operations::read(&dir)).unwrap_or_default();
            CheckoutState { branch: b.branch, first_branch: b.first_branch, present: true, operations: Some(ops) }
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
    let tally = git.tally();
    run.calls = count_u64(tally.calls);
    run.failed = count_u64(tally.failed);
    run.timed_out = count_u64(tally.timeouts);
    rows::flush(db, &mut batch).await?;
    run.rows = batch.added.clone();
    let mut notes: Vec<String> = Vec::new();
    if tally.timeouts > 0 {
        run.outcome = Outcome::Partial;
        notes.push(format!("{} git calls cut off at attr_recon_git_timeout_ms ({} ms)", tally.timeouts, params.git_timeout_ms));
    }

    // Step 1: the repositories crawled in depth.
    let mut jobs: Vec<Job> = Vec::new();
    if !stopped {
        let events = known::git_events(db).await?;
        let checkouts: Vec<(String, RecordId)> = held.checkouts.values().map(|c| (c.toplevel.clone(), c.repository.clone())).collect();
        let moved: BTreeSet<String> = events
            .iter()
            .filter(|e| e.valid_from >= began && MOVES.contains(&e.action.as_str()))
            .filter_map(|e| e.dir.as_deref())
            .filter_map(|dir| sessions::holder(&checkouts, dir).map(|(_, r)| row_key(r)))
            .collect();
        let mut chosen: Vec<(&String, &RecordId)> = held
            .repositories
            .iter()
            .filter(|(common, id)| full || new_repositories.contains(*common) || moved.contains(&row_key(id)))
            .collect();
        chosen.sort();
        for (common, id) in chosen {
            let name = held.names.get(common).cloned().unwrap_or_else(|| list::repository_name(common, common));
            let mine: Vec<JobCheckout> = held
                .checkouts
                .values()
                .filter(|c| c.repository == *id)
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
            let mut paths: Vec<&str> = mine.iter().map(|c| c.toplevel.as_str()).collect();
            paths.push(common);
            // An excluded repository is never crawled; one whose common
            // git directory is gone is not, and keeps what is held of it.
            if params.excludes(&name, &paths) || !Path::new(common).is_dir() {
                continue;
            }
            let mut checkouts_sorted = mine;
            checkouts_sorted.sort_by(|a, b| a.toplevel.cmp(&b.toplevel));
            jobs.push(Job {
                repository: id.clone(),
                common: Path::new(common).to_path_buf(),
                name,
                checkouts: checkouts_sorted,
                refs_before: memory.refs.get(common).cloned(),
                blamed: memory.blamed.get(common).cloned(),
            });
        }
        let ctx = Context {
            db: db.clone(),
            params: params.clone(),
            mirrors: mirror::Places { repos: places.repos.clone(), home: places.home.clone(), askpass: places.askpass.clone() },
            endpoints: places.endpoints.clone(),
            hosts: Arc::new(shared(db, params, places, memory).await?),
            events,
            checkouts,
        };
        let (finished, error) = workers(ctx, jobs, params, stop).await;
        stopped = stopped || finished.iter().any(|f| f.stopped) || stop.requested();
        let mut summaries: Vec<Summary> = Vec::new();
        let mut host_rows: BTreeMap<&'static str, i64> = BTreeMap::new();
        for f in finished {
            if f.stopped {
                continue;
            }
            if let Some(crawl) = &f.crawl {
                run.crawls.push(crawl.clone());
            }
            run.calls += f.calls;
            run.failed += f.failed;
            run.timed_out += f.timed_out;
            for (table, n) in &f.rows {
                *run.rows.entry(table).or_default() += n;
            }
            for (table, n) in &f.host_rows {
                *host_rows.entry(table).or_default() += n;
            }
            *counts.entry("commits_held").or_default() += f.commits_held;
            *counts.entry("commits_read").or_default() += f.commits_read;
            match f.outcome {
                "partial" => {
                    run.outcome = Outcome::Partial;
                    let differs = if f.commits_read == f.commits_held {
                        String::new()
                    } else {
                        format!(", {} commits read of {} held", f.commits_read, f.commits_held)
                    };
                    notes.push(format!("{} partial{differs}", f.name));
                }
                "unreadable" => {
                    run.outcome = Outcome::Partial;
                    notes.push(format!("{} unreadable", f.name));
                }
                _ => {}
            }
            if f.crawl.is_some() {
                let common = f.common.to_string_lossy().into_owned();
                if let Some(read) = f.refs.clone() {
                    memory.refs.insert(common.clone(), read);
                }
                match &f.blamed {
                    Some(tip) => {
                        memory.blamed.insert(common, tip.clone());
                    }
                    None => {
                        memory.blamed.remove(&common);
                    }
                }
            }
            if let Ok(mut hosts) = memory.hosts.lock() {
                for (hp, m) in f.memories {
                    hosts.insert(hp, m);
                }
            }
            summaries.extend(f.hosts);
        }
        // Accounts made by a worker that never wrote are known nowhere:
        // the next pass reads them afresh.
        {
            let mut accounts = memory.accounts.lock().await;
            if accounts.waiting.waiting() > 0 {
                accounts.forget_waiting();
            }
        }
        host_run = hosts::crawl::record(&summaries, host_rows, started);
        if let Some(e) = error {
            return Err(e);
        }
    }
    counts.entry("commits_held").or_default();
    counts.entry("commits_read").or_default();
    run.counts = counts;
    if stopped {
        run.outcome = Outcome::Stopped;
        host_run.outcome = Outcome::Stopped;
        notes.push("stopped before it finished".to_string());
    } else if full {
        memory.last_full = Some(started);
    }
    run.note = (!notes.is_empty()).then(|| notes.join("; "));
    run.finished_at = Utc::now();
    Ok(Crawled { repos: run, hosts: host_run })
}

/// What the hosts part shares across a pass's workers: the credentials
/// file read afresh, the client, what the database holds of the hosts, the
/// accounts and each host repository's memory.
async fn shared(db: &Db, params: &Params, places: &Places, memory: &Memory) -> Result<Shared> {
    let credentials = places.credentials.as_deref().map_or(Credentials::None, credentials::read);
    let client = Client::new(places.endpoints.clone(), params.host_timeout_ms);
    let (known, accounts) = hosts::held::known(db).await?;
    {
        let mut registry = memory.accounts.lock().await;
        if !registry.loaded() {
            registry.load(accounts);
        }
    }
    Ok(Shared {
        db: db.clone(),
        client,
        credentials,
        known,
        taken: std::sync::Mutex::new(BTreeSet::new()),
        accounts: Arc::clone(&memory.accounts),
        memories: Arc::clone(&memory.hosts),
    })
}

/// Each repository crawled by a worker of its own, as many at once as
/// `attr_recon_repos_at_once` allows, started in the order of their common
/// directories; the pass waits for every one. What they did, and the
/// first error one stopped on.
async fn workers(ctx: Context, jobs: Vec<Job>, params: &Params, stop: &mut Stop) -> (Vec<Finished>, Option<superx_kernel::KernelError>) {
    let at_once = usize::try_from(params.repos_at_once).unwrap_or(usize::MAX).clamp(1, Semaphore::MAX_PERMITS);
    let gate = Arc::new(Semaphore::new(at_once));
    let ctx = Arc::new(ctx);
    let mut running: JoinSet<(usize, Result<Finished>)> = JoinSet::new();
    for (i, job) in jobs.into_iter().enumerate() {
        let Some(Ok(turn)) = stop.unless(Arc::clone(&gate).acquire_owned()).await else { break };
        let (ctx, mut own) = (Arc::clone(&ctx), stop.clone());
        running.spawn(async move {
            let _turn = turn;
            (i, crawl::run(&ctx, &job, &mut own).await)
        });
    }
    let mut finished: Vec<(usize, Finished)> = Vec::new();
    let mut error = None;
    while let Some(joined) = running.join_next().await {
        match joined {
            Ok((i, Ok(f))) => finished.push((i, f)),
            Ok((_, Err(e))) => {
                error.get_or_insert(e);
            }
            Err(e) => {
                error.get_or_insert(superx_kernel::KernelError::Config(format!("a repository's worker ended: {e}")));
            }
        }
    }
    finished.sort_by_key(|(i, _)| *i);
    (finished.into_iter().map(|(_, f)| f).collect(), error)
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
