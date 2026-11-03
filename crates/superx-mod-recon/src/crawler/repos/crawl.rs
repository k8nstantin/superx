//! One repository's crawl in depth (#538, steps 2 to 14), run by a worker
//! of its own: steps 3 to 11 in their order, step 4 starting the host crawl
//! of each host repository it takes; then those host crawls finished
//! (steps 15 to 21); then step 13. It holds every row they make and writes
//! them once done, the crawl row first and each row after the rows it
//! links; a crawl stopped writes nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};

use crate::crawler::hosts::crawl::{Beside, Done, HostCrawl, Shared, Standing, Summary};
use crate::crawler::hosts::held::{row_key, HostMemory};
use crate::crawler::hosts::{Endpoints, Host, HostPath};
use crate::crawler::progress::{Progress, Watch};
use crate::crawler::repos::commits::{self, FileChange};
use crate::crawler::repos::git::With;
use crate::crawler::repos::known::{self, GitEvent, Known};
use crate::crawler::repos::record::{Recorder, OK, YES_OR_NO};
use crate::crawler::repos::refs::{self, Held, Mainline, Ref};
use crate::crawler::repos::remotes::{self, Config};
use crate::crawler::repos::{mirror, reflogs};
use crate::crawler::store::{self, reserve, Batch, Row};
use crate::crawler::Stop;
use crate::params::Params;

/// Commits read at once: a page of `cat-file`, `log` and `patch-id` input.
pub(super) const COMMITS_PER_READ: usize = 500; // skill-allow: §9-const — read-path bound, not a policy tunable

/// What every worker of a pass shares.
pub struct Context {
    pub db: Db,
    pub params: Params,
    pub mirrors: mirror::Places,
    pub endpoints: Endpoints,
    pub hosts: Arc<Shared>,
    /// Every git event of the actions a crawl reads.
    pub events: Vec<GitEvent>,
    /// Every checkout on the list: its root and its repository's row.
    pub checkouts: Vec<(String, RecordId)>,
    /// Where each worker reports what it is doing (#539).
    pub progress: Progress,
}

/// A checkout of the repository a worker crawls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCheckout {
    pub id: RecordId,
    pub toplevel: String,
    pub present: bool,
    pub branch: Option<String>,
}

/// One repository to crawl.
#[derive(Debug, Clone)]
pub struct Job {
    pub repository: RecordId,
    pub common: PathBuf,
    pub name: String,
    pub checkouts: Vec<JobCheckout>,
    /// The refs its previous crawl since superx-recon started read.
    pub refs_before: Option<refs::Read>,
    /// The tip that crawl last blamed whole.
    pub blamed: Option<String>,
}

/// A crawl, done.
#[derive(Debug, Default)]
pub struct Finished {
    pub stopped: bool,
    pub crawl: Option<RecordId>,
    pub name: String,
    /// The repository's common git directory.
    pub common: PathBuf,
    pub outcome: &'static str,
    pub note: Option<String>,
    pub commits_held: i64,
    pub commits_read: i64,
    pub calls: i64,
    pub failed: i64,
    pub timed_out: i64,
    /// The rows it wrote of the repositories' tables, by table.
    pub rows: BTreeMap<&'static str, i64>,
    /// The rows it wrote of the hosts' tables, by table.
    pub host_rows: BTreeMap<&'static str, i64>,
    pub hosts: Vec<Summary>,
    pub refs: Option<refs::Read>,
    pub blamed: Option<String>,
    pub memories: Vec<(HostPath, HostMemory)>,
}

/// What a crawl counts, as its row records it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub refs_read: i64,
    pub files_read: i64,
    pub reflog_read: i64,
    pub unreachable_read: i64,
    pub blame_lines_read: i64,
    pub commits_held: i64,
    pub commits_read: i64,
    pub commits_new: i64,
    pub commit_files_new: i64,
    pub lines_added_new: i64,
    pub lines_removed_new: i64,
    pub refs_new: i64,
    pub refs_moved: i64,
    pub refs_gone: i64,
    pub refs_rewritten: i64,
    pub head_moves_new: i64,
    pub placements_changed: i64,
    pub landed_new: i64,
    pub survival_changed: i64,
    pub survival_delta: i64,
    pub files_blamed: i64,
    pub files_left: i64,
}

/// A commit the crawl holds a row for: held, or read in this crawl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRow {
    pub row: RecordId,
    pub committed_at: DateTime<Utc>,
    pub added: i64,
    pub merge: bool,
}

/// A mirror of one of the repository's remotes.
#[derive(Debug, Clone)]
pub struct Mirror {
    pub hp: HostPath,
    pub folder: PathBuf,
    /// Its `mirror` row, when the database holds one or this crawl made it.
    pub row: Option<RecordId>,
    /// This worker took its host repository in the pass.
    pub taken: bool,
}

/// One repository's crawl, as it goes.
pub struct Crawl<'a> {
    pub(super) ctx: &'a Context,
    pub(super) job: &'a Job,
    pub(super) id: RecordId,
    pub(super) started_at: DateTime<Utc>,
    pub(super) since: Option<DateTime<Utc>>,
    pub(super) rec: Recorder,
    /// The host repositories it made and the mirrors, written right after
    /// the crawl row.
    pub(super) head: Batch,
    /// Its own rows after those.
    pub(super) rows: Batch,
    /// Its ties to sessions, written after the host crawls' rows.
    pub(super) tail: Batch,
    pub(super) known: Known,
    pub(super) notes: Vec<String>,
    pub(super) counts: Counts,
    pub(super) config: Config,
    pub(super) shallow: bool,
    pub(super) named: Vec<(String, HostPath)>,
    pub(super) mirrors: Vec<Mirror>,
    pub(super) host_crawls: Vec<HostCrawl>,
    pub(super) repo_refs: Vec<Ref>,
    pub(super) mirror_refs: Vec<(PathBuf, Vec<Ref>)>,
    pub(super) refs_now: refs::Read,
    pub(super) mainline: Mainline,
    /// Every commit the crawl holds a row for, by hash.
    pub(super) commits: HashMap<String, CommitRow>,
    /// The commits of the crawl's since its refs, reflogs and unreachable
    /// objects reach, and which git directory holds each.
    pub(super) held: BTreeSet<String>,
    pub(super) in_repository: HashSet<String>,
    /// The commits only a mirror holds, by the mirror's folder.
    pub(super) mirror_only: HashMap<String, PathBuf>,
    /// The file changes read in this crawl, by commit.
    pub(super) changes: HashMap<String, Vec<FileChange>>,
    /// Where each commit of the crawl's since was placed.
    pub(super) places: HashMap<String, &'static str>,
    pub(super) first_parent: Vec<String>,
    pub(super) blamed: Option<String>,
}

impl<'a> Crawl<'a> {
    /// The repository's own git directory, and what its calls run with.
    pub(super) fn repository(&self) -> (PathBuf, With) {
        (self.job.common.clone(), With::default())
    }

    /// What a mirror's calls run with.
    pub(super) fn mirror_with(&self, host: Host) -> With {
        mirror::with(host, &self.ctx.mirrors)
    }

    /// The git directory that holds the main line, and what its calls run
    /// with.
    pub(super) fn tip_dir(&self) -> (PathBuf, With) {
        match &self.mainline.mirror {
            Some(folder) => {
                let host = self.mirrors.iter().find(|m| m.folder == *folder).map_or(Host::GitHub, |m| m.hp.host);
                (folder.clone(), self.mirror_with(host))
            }
            None => self.repository(),
        }
    }
}

/// Crawl one repository in depth and write what it read.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer for what
/// it holds of the repository or refuses a row.
pub async fn run(ctx: &Context, job: &Job, stop: &mut Stop) -> Result<Finished> {
    let started_at = Utc::now();
    let since = (ctx.params.since_days > 0)
        .then(|| started_at - TimeDelta::days(i64::try_from(ctx.params.since_days).unwrap_or(i64::MAX / 86_400 / 1_000)));
    let id = reserve("crawl");
    let key = job.common.display().to_string();
    let step = |s: &'static str| ctx.progress.step(&key, s);
    step("setup");
    let mut c = Crawl {
        ctx,
        job,
        rec: Recorder::new(id.clone(), ctx.params.git_timeout_ms).watched(Watch {
            progress: ctx.progress.clone(),
            common_dir: key.clone(),
            host: None,
        }),
        id,
        started_at,
        since,
        head: Batch::default(),
        rows: Batch::default(),
        tail: Batch::default(),
        known: known::load(&ctx.db, &job.repository).await?,
        notes: Vec::new(),
        counts: Counts::default(),
        config: Config::default(),
        shallow: false,
        named: Vec::new(),
        mirrors: Vec::new(),
        host_crawls: Vec::new(),
        repo_refs: Vec::new(),
        mirror_refs: Vec::new(),
        refs_now: refs::Read::new(),
        mainline: Mainline { name: "HEAD".to_string(), from: refs::From::Head, tip: None, mirror: None },
        commits: HashMap::new(),
        held: BTreeSet::new(),
        in_repository: HashSet::new(),
        mirror_only: HashMap::new(),
        changes: HashMap::new(),
        places: HashMap::new(),
        first_parent: Vec::new(),
        blamed: None,
    };
    for (hash, h) in &c.known.commits {
        c.commits.insert(hash.clone(), CommitRow { row: h.row.clone(), committed_at: h.committed_at, added: h.added, merge: false });
    }
    let readable = match c.setup(stop).await {
        Some(readable) => readable,
        None => return Ok(Finished { stopped: true, name: job.name.clone(), common: job.common.clone(), ..Finished::default() }),
    };
    if !readable {
        step("writing");
        return c.finish(Vec::new(), "unreadable").await;
    }
    let went = async {
        step("hosts");
        c.hosts_start(stop).await?;
        step("refs");
        c.refs(stop).await?;
        step("commits");
        c.commits(stop).await?;
        step("reflogs");
        c.reflogs(stop).await?;
        step("placements");
        c.placements(stop).await?;
        step("landed");
        c.landed(stop).await?;
        step("survival");
        c.survival(stop).await?;
        Some(())
    }
    .await;
    if went.is_none() || stop.requested() {
        return Ok(Finished { stopped: true, name: job.name.clone(), common: job.common.clone(), ..Finished::default() });
    }
    // The host crawls, once the repository's crawl is done.
    step("host crawls");
    let beside = Beside { tip: c.mainline.tip.clone(), first_parent: c.first_parent_held() };
    let mut done: Vec<Done> = Vec::new();
    for mut hc in std::mem::take(&mut c.host_crawls) {
        hc.finish(&beside, stop).await;
        ctx.progress.host_done(&key, &hc.hp.to_string());
        if hc.stopped || stop.requested() {
            return Ok(Finished { stopped: true, name: job.name.clone(), common: job.common.clone(), ..Finished::default() });
        }
        done.push(hc.done());
    }
    step("sessions");
    c.sessions(&done).await?;
    let outcome = if c.rec.failed > 0 || c.rec.timeouts > 0 || c.counts.commits_read != c.counts.commits_held { "partial" } else { "complete" };
    step("writing");
    c.finish(done, outcome).await
}

impl Crawl<'_> {
    /// Step 3, and whether git can read the repository: whether it is
    /// shallow, then its configuration. `None` once stopped.
    async fn setup(&mut self, stop: &mut Stop) -> Option<bool> {
        let (dir, with) = self.repository();
        let shallow = self.rec.run("setup", &dir, &["rev-parse", "--is-shallow-repository"], &with, OK, stop).await?;
        let Some(out) = shallow.ok() else {
            self.notes.extend(self.rec.failures.iter().cloned());
            return Some(false);
        };
        self.shallow = out.trim() == "true";
        if self.shallow {
            self.notes.push("shallow: its commits are those git holds".to_string());
        }
        let listing = self.rec.run("setup", &dir, &["config", "--null", "--list"], &with, OK, stop).await?;
        if let Some(out) = listing.ok() {
            self.config = remotes::config(out);
            for (name, remote) in remotes::changes(&self.known.remotes, &self.config) {
                self.rows.create(
                    "remote_state",
                    Row::new()
                        .link("repository", &self.job.repository)
                        .link("crawl", &self.id)
                        .text("name", name)
                        .some_text("url", remote.url)
                        .some_text("push_url", remote.push_url)
                        .some_text("host", remote.host)
                        .some_text("path", remote.path),
                );
            }
        }
        Some(true)
    }

    /// Step 4: each remote's host repository, in the order of their names;
    /// the host crawl of each this worker takes, and its mirror. `None`
    /// once stopped.
    async fn hosts_start(&mut self, stop: &mut Stop) -> Option<()> {
        let mut seen: BTreeSet<HostPath> = BTreeSet::new();
        for (name, remote) in &self.config.remotes {
            let (Some(host), Some(path)) = (remote.host.as_deref().and_then(Host::named), remote.path.clone()) else { continue };
            let hp = HostPath { host, path };
            if seen.insert(hp.clone()) {
                self.named.push((name.clone(), hp));
            }
        }
        for (_, hp) in self.named.clone() {
            let taken = self.ctx.hosts.take(&hp);
            let held = self.ctx.hosts.known.repositories.get(&(hp.host, hp.path.clone()));
            let mut mirror_row = held.and_then(|r| self.ctx.hosts.known.mirrors.get(&row_key(r)).cloned());
            if taken {
                let key = self.job.common.display().to_string();
                self.ctx.progress.host_started(&key, &hp.to_string());
                let watch = Watch { progress: self.ctx.progress.clone(), common_dir: key, host: Some(hp.to_string()) };
                let Ok(hc) = HostCrawl::start(&self.ctx.hosts, hp.clone(), self.id.clone(), Some(watch), stop).await else {
                    self.notes.push(format!("{hp}: what the database holds of it could not be read"));
                    continue;
                };
                if hc.stopped {
                    return None;
                }
                if hc.new_repository {
                    self.head.create_first(hc.repository.clone(), crate::crawler::hosts::crawl::repository_row(&hp, hc.host_id()));
                }
                if hc.answered && hc.standing == Standing::Asking {
                    if let Some(repos) = self.ctx.mirrors.repos.clone() {
                        let folder = mirror::folder(&repos, &hp);
                        let with = self.mirror_with(hp.host);
                        if !folder.exists() {
                            let target = folder.to_string_lossy().into_owned();
                            if std::fs::create_dir_all(&repos).is_err() {
                                self.notes.push(format!("{}: the repos folder cannot be made", repos.display()));
                            }
                            self.rec.run("fetch", &repos, &["init", "--bare", "--template=", &target], &with, OK, stop).await?;
                        }
                        if folder.exists() {
                            if mirror_row.is_none() {
                                mirror_row = Some(self.head.create(
                                    "mirror",
                                    Row::new()
                                        .link("host_repository", &hc.repository)
                                        .text("url", format!("https://{}/{}.git", hp.host.name(), hp.path))
                                        .text("folder", folder.to_string_lossy().into_owned())
                                        .time("created_at", Utc::now()),
                                ));
                            }
                            let url = self.ctx.endpoints.git_url(hp.host, &hp.path);
                            let args = mirror::fetch_args(&url);
                            let args: Vec<&str> = args.iter().map(String::as_str).collect();
                            let fetch = With { unbounded: true, ..with };
                            self.rec.run("fetch", &folder, &args, &fetch, OK, stop).await?;
                        }
                    }
                }
                self.host_crawls.push(hc);
            }
            if let Some(repos) = &self.ctx.mirrors.repos {
                let folder = mirror::folder(repos, &hp);
                if folder.is_dir() {
                    self.mirrors.push(Mirror { hp: hp.clone(), folder, row: mirror_row, taken });
                }
            }
        }
        Some(())
    }

    /// Step 5: every ref of the repository and of its remotes' mirrors,
    /// how they moved since the previous crawl, and the main line. `None`
    /// once stopped.
    async fn refs(&mut self, stop: &mut Stop) -> Option<()> {
        let (dir, with) = self.repository();
        let listed = self.rec.run("refs", &dir, &["for-each-ref", refs::FORMAT], &with, OK, stop).await?;
        self.repo_refs = listed.ok().map(refs::parse).unwrap_or_default();
        for r in &self.repo_refs {
            self.refs_now.insert((Held::Repository, r.name.clone()), r.commit.clone());
        }
        for m in self.mirrors.clone() {
            let with = self.mirror_with(m.hp.host);
            let listed = self.rec.run("refs", &m.folder, &["for-each-ref", refs::FORMAT], &with, OK, stop).await?;
            let read = listed.ok().map(refs::parse).unwrap_or_default();
            for r in &read {
                self.refs_now.insert((Held::Mirror(m.folder.clone()), r.name.clone()), r.commit.clone());
            }
            self.mirror_refs.push((m.folder.clone(), read));
        }
        self.counts.refs_read = i64::try_from(self.refs_now.len()).unwrap_or(i64::MAX);
        match &self.job.refs_before {
            None => self.notes.push("refs compared with none: the first crawl since superx-recon started".to_string()),
            Some(before) => {
                let moves = refs::compare(before, &self.refs_now);
                self.counts.refs_new = moves.new;
                self.counts.refs_gone = moves.gone;
                self.counts.refs_moved = i64::try_from(moves.moved.len()).unwrap_or(i64::MAX);
                for (held, _, old, new) in moves.moved {
                    let (Some(old), Some(new)) = (old, new) else { continue };
                    let (dir, with) = match &held {
                        Held::Repository => self.repository(),
                        Held::Mirror(folder) => {
                            let host = self.mirrors.iter().find(|m| m.folder == *folder).map_or(Host::GitHub, |m| m.hp.host);
                            (folder.clone(), self.mirror_with(host))
                        }
                    };
                    let ancestor = self.rec.run("refs", &dir, &["merge-base", "--is-ancestor", &old, &new], &with, YES_OR_NO, stop).await?;
                    if ancestor.exit == Some(1) {
                        self.counts.refs_rewritten += 1;
                    }
                }
            }
        }
        // The main line.
        let override_ref = self.ctx.params.mainline_refs.get(&self.job.name).map(String::as_str);
        let origin = self.named.iter().find(|(name, _)| name == "origin").map(|(_, hp)| hp.clone());
        let host_default = origin.as_ref().and_then(|hp| match self.host_crawls.iter().find(|hc| hc.hp == *hp) {
            Some(hc) => hc.default_branch(),
            None => self
                .ctx
                .hosts
                .known
                .repositories
                .get(&(hp.host, hp.path.clone()))
                .and_then(|r| self.ctx.hosts.known.default_branches.get(&row_key(r)).cloned()),
        });
        let origin_mirror = origin.as_ref().and_then(|hp| {
            let folder = self.mirrors.iter().find(|m| m.hp == *hp).map(|m| m.folder.clone())?;
            self.mirror_refs.iter().find(|(f, _)| *f == folder).map(|(f, r)| (f.clone(), r.clone()))
        });
        let chosen = refs::mainline(&self.repo_refs, override_ref, host_default.as_deref(), origin_mirror.as_ref().map(|(f, r)| (f, r.as_slice())));
        self.mainline = match chosen {
            Some(m) => m,
            None => {
                let head = self.rec.run("refs", &dir, &["rev-parse", "--verify", "--quiet", "--end-of-options", "HEAD^{commit}"], &with, YES_OR_NO, stop).await?;
                let tip = head.ok().map(|s| s.trim().to_string()).filter(|s| commits::is_hash(s));
                Mainline { name: "HEAD".to_string(), from: refs::From::Head, tip, mirror: None }
            }
        };
        if self.mainline.tip.is_none() {
            self.notes.push("no main line tip: nothing placed on the main line, nothing blamed".to_string());
        }
        Some(())
    }

    /// Steps 6 and 7: the unreachable commits, then every commit of the
    /// crawl's since, each one not held read once, whole. `None` once
    /// stopped.
    async fn commits(&mut self, stop: &mut Stop) -> Option<()> {
        let (dir, with) = self.repository();
        let fsck = self.rec.run("unreachable", &dir, &["fsck", "--unreachable", "--connectivity-only", "--no-progress"], &with, OK, stop).await?;
        let unreachable = fsck.ok().map(commits::unreachable).unwrap_or_default();
        self.counts.unreachable_read = i64::try_from(unreachable.len()).unwrap_or(i64::MAX);
        let cap = usize::try_from(self.ctx.params.unreachable_commits).unwrap_or(usize::MAX);
        let unreachable: Vec<String> = unreachable.into_iter().take(cap).collect();

        // Every commit the refs and reflogs reach, the unreachable ones and
        // those of the mirrors, each once, with when it was committed.
        let format = "--format=%H%x09%ct";
        let mut when: HashMap<String, i64> = HashMap::new();
        let mut sources: BTreeMap<String, (PathBuf, With)> = BTreeMap::new();
        let listing = self.rec.run("log", &dir, &["log", "--all", "--reflog", format], &with, OK, stop).await?;
        for (hash, at) in listing.ok().map(commits::listed).unwrap_or_default() {
            self.in_repository.insert(hash.clone());
            when.insert(hash, at);
        }
        if !unreachable.is_empty() {
            let input = With { input: Some(lines(&unreachable)), ..with.clone() };
            let listing = self.rec.run("log", &dir, &["log", "--no-walk=unsorted", "--stdin", format], &input, OK, stop).await?;
            for (hash, at) in listing.ok().map(commits::listed).unwrap_or_default() {
                self.in_repository.insert(hash.clone());
                when.insert(hash, at);
            }
        }
        for m in self.mirrors.clone() {
            let mwith = self.mirror_with(m.hp.host);
            let listing = self.rec.run("log", &m.folder, &["log", "--all", "--reflog", format], &mwith, OK, stop).await?;
            for (hash, at) in listing.ok().map(commits::listed).unwrap_or_default() {
                if !self.in_repository.contains(&hash) {
                    sources.entry(hash.clone()).or_insert_with(|| (m.folder.clone(), mwith.clone()));
                }
                when.insert(hash, at);
            }
        }
        self.mirror_only = sources.iter().map(|(hash, (folder, _))| (hash.clone(), folder.clone())).collect();
        let since = self.since.map(|s| s.timestamp());
        self.held = when.iter().filter(|(_, at)| since.is_none_or(|s| **at >= s)).map(|(h, _)| h.clone()).collect();
        self.counts.commits_held = i64::try_from(self.held.len()).unwrap_or(i64::MAX);

        // Each commit held for the first time, read from the repository
        // when it holds it, else from the mirror that does.
        let mut by_source: BTreeMap<PathBuf, (With, Vec<String>)> = BTreeMap::new();
        for hash in &self.held {
            if self.commits.contains_key(hash) {
                continue;
            }
            let (from, w) = if self.in_repository.contains(hash) {
                (dir.clone(), with.clone())
            } else if let Some((folder, w)) = sources.get(hash) {
                (folder.clone(), w.clone())
            } else {
                continue;
            };
            by_source.entry(from).or_insert_with(|| (w, Vec::new())).1.push(hash.clone());
        }
        for (from, (w, hashes)) in by_source {
            for page in hashes.chunks(COMMITS_PER_READ) {
                self.read_page(&from, &w, page, stop).await?;
            }
        }
        self.counts.commits_read = i64::try_from(self.held.iter().filter(|h| self.commits.contains_key(*h)).count()).unwrap_or(i64::MAX);
        if self.counts.commits_read != self.counts.commits_held {
            self.notes.push(format!("{} commits read of {} held", self.counts.commits_read, self.counts.commits_held));
        }
        Some(())
    }

    /// One page of commits read whole from `from`: their objects, their
    /// files counted as they are and with whitespace ignored, and their
    /// patch ids. `None` once stopped.
    async fn read_page(&mut self, from: &Path, with: &With, page: &[String], stop: &mut Stop) -> Option<()> {
        let input = lines(page);
        let raw = With { input: Some(input.clone()), raw: true, ..with.clone() };
        let objects = self.rec.run("log", from, &["cat-file", "--batch"], &raw, OK, stop).await?;
        let read: Vec<commits::Commit> = match (objects.exit, &objects.raw) {
            (Some(0), Some(out)) => commits::batch(out).into_iter().filter_map(|(_, c)| c).collect(),
            _ => Vec::new(),
        };
        let diff = ["--no-walk=unsorted", "--stdin", "-z", "--format=%x01%H", "--no-renames", "--diff-merges=first-parent", "--root", "--no-abbrev", "--no-color"];
        let mut args = vec!["log"];
        args.extend(diff);
        args.extend(["--raw", "--numstat"]);
        let files = self.rec.run("log", from, &args, &raw, OK, stop).await?;
        let mut changes = match (files.exit, &files.raw) {
            (Some(0), Some(out)) => commits::changes(out),
            _ => BTreeMap::new(),
        };
        let mut args = vec!["log"];
        args.extend(diff);
        args.extend(["--numstat", "--ignore-all-space"]);
        let spaced = self.rec.run("log", from, &args, &raw, OK, stop).await?;
        let ignoring = match (spaced.exit, &spaced.raw) {
            (Some(0), Some(out)) => commits::numstat(out),
            _ => BTreeMap::new(),
        };
        // Patch ids: of the commits that are no merge.
        let singles: Vec<String> = read.iter().filter(|c| c.parent_hashes.len() <= 1).map(|c| c.hash.clone()).collect();
        let mut patch_ids = HashMap::new();
        if !singles.is_empty() {
            let input = With { input: Some(lines(&singles)), raw: true, ..with.clone() };
            let patches = self.rec.run("patches", from, &["diff-tree", "--stdin", "-p", "--root", "--no-renames", "--no-color"], &input, OK, stop).await?;
            if let (Some(0), Some(out)) = (patches.exit, patches.raw) {
                let ids = With { input: Some(out), ..with.clone() };
                let named = self.rec.run("patches", from, &["patch-id", "--stable"], &ids, OK, stop).await?;
                patch_ids = named.ok().map(commits::patch_ids).unwrap_or_default();
            }
        }
        // A commit is read whole: its object and its files.
        for commit in read {
            let Some(files) = changes.remove(&commit.hash) else { continue };
            let spaced = ignoring.get(&commit.hash);
            let added: i64 = files.iter().map(|f| f.added).sum();
            let removed: i64 = files.iter().map(|f| f.removed).sum();
            let row = self.rows.create(
                "commit",
                Row::new()
                    .link("repository", &self.job.repository)
                    .text("hash", commit.hash.clone())
                    .time("authored_at", commit.author.at)
                    .time("committed_at", commit.committer.at)
                    .text("author", commit.author.email.clone())
                    .text("committer", commit.committer.email.clone())
                    .text("subject", commit.subject.clone())
                    .some_text("body", commit.body.clone())
                    .int("parents", i64::try_from(commit.parent_hashes.len()).unwrap_or(i64::MAX))
                    .int("added", added)
                    .int("removed", removed)
                    .text("author_name", commit.author.name.clone())
                    .text("committer_name", commit.committer.name.clone())
                    .texts("parent_hashes", &commit.parent_hashes)
                    .text("tree", commit.tree.clone())
                    .int("author_offset", commit.author.offset)
                    .int("committer_offset", commit.committer.offset)
                    .some_text("patch_id", patch_ids.get(&commit.hash).filter(|p| commits::is_hash(p)).cloned())
                    .some_text("signature", commit.signature.clone()),
            );
            for f in &files {
                let (a, r) = spaced.and_then(|s| s.get(&f.path)).copied().unwrap_or((0, 0));
                self.rows.create(
                    "commit_file",
                    Row::new()
                        .link("commit", &row)
                        .text("path", f.path.clone())
                        .text("change", f.change)
                        .int("added", f.added)
                        .int("removed", f.removed)
                        .int("added_ignoring_space", if f.binary { 0 } else { a })
                        .int("removed_ignoring_space", if f.binary { 0 } else { r })
                        .flag("binary", f.binary),
                );
                self.known.files.push((row_key(&row), f.path.clone(), f.binary));
            }
            self.counts.commits_new += 1;
            self.counts.commit_files_new += i64::try_from(files.len()).unwrap_or(i64::MAX);
            self.counts.files_read += i64::try_from(files.len()).unwrap_or(i64::MAX);
            self.counts.lines_added_new += added;
            self.counts.lines_removed_new += removed;
            self.commits.insert(
                commit.hash.clone(),
                CommitRow { row, committed_at: commit.committer.at, added, merge: commit.parent_hashes.len() > 1 },
            );
            self.changes.insert(commit.hash.clone(), files);
        }
        Some(())
    }

    /// Step 8: every reflog entry git keeps, of the repository, its
    /// checkouts and the mirrors this worker took, each written once.
    /// `None` once stopped.
    async fn reflogs(&mut self, stop: &mut Stop) -> Option<()> {
        let (dir, with) = self.repository();
        let format = self.rec.run("reflog", &dir, &["rev-parse", "--show-ref-format"], &with, OK, stop).await?;
        let reftable = format.ok().is_some_and(|f| f.trim() == "reftable");
        let job = self.job;
        let checkout_of = |root: Option<&PathBuf>| -> Option<RecordId> {
            let root = root?;
            job.checkouts.iter().find(|c| Path::new(&c.toplevel) == root.as_path()).map(|c| c.id.clone())
        };
        // Each of the repository's reflogs with its checkout, when it is a
        // checkout's HEAD.
        let mut found: Vec<(Option<RecordId>, Option<RecordId>, reflogs::Entry)> = Vec::new();
        let mut unread = 0;
        let worktrees = reflogs::worktrees(&self.job.common);
        if reftable {
            let list = self.rec.run("reflog", &dir, &["reflog", "list"], &with, OK, stop).await?;
            let mut names: Vec<(String, Option<RecordId>)> = list.ok().unwrap_or("").lines().map(|r| {
                let checkout = if r == "HEAD" { worktrees.first().and_then(|(_, root)| checkout_of(root.as_ref())) } else { None };
                (r.to_string(), checkout)
            }).collect();
            for (wt, root) in worktrees.iter().skip(1) {
                if let Some(name) = wt.file_name() {
                    names.push((format!("worktrees/{}/HEAD", name.to_string_lossy()), checkout_of(root.as_ref())));
                }
            }
            for (name, checkout) in names {
                let shown = self.rec.run("reflog", &dir, &["reflog", "show", "--date=unix", reflogs::SHOW_FORMAT, "--end-of-options", &name], &with, OK, stop).await?;
                let as_named = if name.ends_with("/HEAD") { "HEAD" } else { name.as_str() };
                let (entries, bad) = reflogs::shown(as_named, shown.ok().unwrap_or(""));
                unread += bad;
                found.extend(entries.into_iter().map(|e| (checkout.clone(), None, e)));
            }
        } else {
            for (i, (git_dir, root)) in worktrees.iter().enumerate() {
                let checkout = checkout_of(root.as_ref());
                for (name, file) in reflogs::files(git_dir) {
                    // A linked worktree's own logs hold its HEAD alone.
                    if i > 0 && name != "HEAD" {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&file) else { continue };
                    let (entries, bad) = reflogs::parse(&name, &text);
                    unread += bad;
                    let checkout = if name == "HEAD" { checkout.clone() } else { None };
                    found.extend(entries.into_iter().map(|e| (checkout.clone(), None, e)));
                }
            }
        }
        // The mirrors this worker took.
        let taken: Vec<Mirror> = self.mirrors.iter().filter(|m| m.taken && m.row.is_some()).cloned().collect();
        let rows: Vec<RecordId> = taken.iter().filter_map(|m| m.row.clone()).collect();
        let held_mirror = known::mirror_moves(&self.ctx.db, &rows).await.ok().unwrap_or_default();
        for m in &taken {
            for (name, file) in reflogs::files(&m.folder) {
                let Ok(text) = std::fs::read_to_string(&file) else { continue };
                let (entries, bad) = reflogs::parse(&name, &text);
                unread += bad;
                found.extend(entries.into_iter().map(|e| (None, m.row.clone(), e)));
            }
        }
        if unread > 0 {
            self.notes.push(format!("{unread} reflog lines in no shape git writes"));
        }
        self.counts.reflog_read = i64::try_from(found.len()).unwrap_or(i64::MAX);
        for (checkout, mirror, e) in found {
            let place = known::move_place(checkout.as_ref(), mirror.as_ref());
            let key = known::move_key(&place, &e.name, e.at, e.old.as_deref(), e.new.as_deref(), &e.action);
            if self.known.moves.contains(&key) || held_mirror.contains(&key) {
                continue;
            }
            self.known.moves.insert(key);
            if e.name == "HEAD" && mirror.is_none() && checkout.is_some() {
                self.counts.head_moves_new += 1;
            }
            self.rows.create(
                "ref_move",
                Row::new()
                    .link("repository", &self.job.repository)
                    .some_link("checkout", checkout.as_ref())
                    .some_link("mirror", mirror.as_ref())
                    .link("crawl", &self.id)
                    .text("ref", e.name)
                    .time("at", e.at)
                    .some_text("old", e.old)
                    .some_text("new", e.new)
                    .some_text("identity", e.identity)
                    .text("action", e.action),
            );
        }
        Some(())
    }

    /// The main line's first-parent history the crawl holds rows for,
    /// newest first.
    fn first_parent_held(&self) -> Vec<String> {
        self.first_parent.iter().filter(|h| self.commits.contains_key(*h) && self.held.contains(*h)).cloned().collect()
    }

    /// The crawl row, its rows and the host crawls', written in their
    /// order; what the pass records of it.
    async fn finish(mut self, done: Vec<Done>, outcome: &'static str) -> Result<Finished> {
        if outcome == "unreadable" {
            self.counts = Counts::default();
            self.mainline = Mainline { name: "HEAD".to_string(), from: refs::From::Head, tip: None, mirror: None };
        }
        let failures = std::mem::take(&mut self.rec.failures);
        if outcome != "unreadable" {
            self.notes.extend(failures);
        }
        let note = (!self.notes.is_empty()).then(|| self.notes.join("; "));
        let mut batch = Batch::default();
        let mut host_rows: BTreeMap<&'static str, i64> = BTreeMap::new();
        let mut memories = Vec::new();
        let mut hosts = Vec::new();
        let mut host_batches = Vec::new();
        for d in done {
            hosts.push(d.summary());
            for (table, n) in &d.rows.added {
                *host_rows.entry(table).or_default() += n;
            }
            memories.push((d.hp.clone(), d.memory));
            host_batches.push(d.rows);
        }
        let own = std::mem::take(&mut self.head);
        for (table, n) in &own.added {
            if *table == "host_repository" {
                *host_rows.entry(table).or_default() += n;
            }
        }
        let rows_written = i64::try_from(own.waiting() + self.rec.rows.waiting() + self.rows.waiting() + self.tail.waiting()).unwrap_or(i64::MAX) + 1
            - own.added.get("host_repository").copied().unwrap_or(0);
        let c = &self.counts;
        batch.create_first(
            self.id.clone(),
            Row::new()
                .link("repository", &self.job.repository)
                .time("started_at", self.started_at)
                .time("finished_at", Utc::now())
                .text("mainline", self.mainline.name.clone())
                .text("mainline_from", self.mainline.from.word())
                .some_text("identity", if outcome == "unreadable" { None } else { self.config.email.clone() })
                .some_text("tip", self.mainline.tip.clone())
                .some_time("since", self.since)
                .text("outcome", outcome)
                .int("commits_read", c.commits_read)
                .int("files_blamed", c.files_blamed)
                .int("files_left", c.files_left)
                .int("git_calls", self.rec.calls)
                .int("git_failed", self.rec.failed)
                .some_text("note", note.clone())
                .int("checkouts_read", if outcome == "unreadable" { 0 } else { i64::try_from(self.job.checkouts.len()).unwrap_or(i64::MAX) })
                .int("refs_read", c.refs_read)
                .int("files_read", c.files_read)
                .int("reflog_read", c.reflog_read)
                .int("unreachable_read", c.unreachable_read)
                .int("blame_lines_read", c.blame_lines_read)
                .int("bytes_read", self.rec.bytes)
                .int("commits_new", c.commits_new)
                .int("commit_files_new", c.commit_files_new)
                .int("lines_added_new", c.lines_added_new)
                .int("lines_removed_new", c.lines_removed_new)
                .int("refs_new", c.refs_new)
                .int("refs_moved", c.refs_moved)
                .int("refs_gone", c.refs_gone)
                .int("refs_rewritten", c.refs_rewritten)
                .int("head_moves_new", c.head_moves_new)
                .int("placements_changed", c.placements_changed)
                .int("landed_new", c.landed_new)
                .int("survival_changed", c.survival_changed)
                .int("survival_delta", c.survival_delta)
                .int("git_timeouts", self.rec.timeouts)
                .int("git_ms", self.rec.ms)
                .int("rows_written", rows_written),
        );
        batch.append(own);
        batch.append(std::mem::take(&mut self.rec.rows));
        batch.append(std::mem::take(&mut self.rows));
        for b in host_batches {
            batch.append(b);
        }
        batch.append(std::mem::take(&mut self.tail));
        // The accounts first: every worker's rows may link them, and they
        // are written once, by whichever worker writes first. Every row
        // becomes valid when it is written.
        let written_at = Utc::now();
        {
            let shared = Arc::clone(&self.ctx.hosts.accounts);
            let mut accounts = shared.lock().await;
            let mut waiting = std::mem::take(&mut accounts.waiting);
            for (table, n) in &waiting.added {
                *host_rows.entry(table).or_default() += n;
            }
            waiting.stamp(written_at);
            store::flush(&self.ctx.db, &mut waiting).await?;
        }
        batch.stamp(written_at);
        let mut rows: BTreeMap<&'static str, i64> = BTreeMap::new();
        for (table, n) in &batch.added {
            if !host_rows.contains_key(table) {
                *rows.entry(table).or_default() += n;
            }
        }
        store::flush(&self.ctx.db, &mut batch).await?;
        Ok(Finished {
            stopped: false,
            crawl: Some(self.id.clone()),
            name: self.job.name.clone(),
            common: self.job.common.clone(),
            outcome,
            note,
            commits_held: self.counts.commits_held,
            commits_read: self.counts.commits_read,
            calls: self.rec.calls,
            failed: self.rec.failed,
            timed_out: self.rec.timeouts,
            rows,
            host_rows,
            hosts,
            refs: (outcome != "unreadable").then(|| self.refs_now.clone()),
            blamed: self.blamed.clone(),
            memories,
        })
    }
}

/// Hashes as `git … --stdin` reads them, one a line.
#[must_use]
pub fn lines(hashes: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for h in hashes {
        out.extend_from_slice(h.as_bytes());
        out.push(b'\n');
    }
    out
}
