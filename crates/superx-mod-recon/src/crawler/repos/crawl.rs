//! One repository crawled in depth by its own worker (#538, steps 2 to 14):
//! its remotes, its hosts' repositories and mirrors, its refs and main
//! line, its commits whole with their files, its reflogs, where each commit
//! sits, what landed through another commit and what survives at the main
//! line's tip, and the commits tied to the sessions that made them. Every
//! git call is a `git_call` row. The worker holds every row until it is
//! done, then writes them, the crawl row first; a worker stopped writes
//! nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::types::{RecordId, Uuid, Value};
use superx_kernel::{Db, Result};

use crate::crawler::hosts::crawl::{Context, HostCrawl};
use crate::crawler::hosts::objects::{json_of, Known};
use crate::crawler::hosts::{self, Host, Mirror};
use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::repos::commits::{self, Change};
use crate::crawler::repos::git::{Call, Git, Tally, With};
use crate::crawler::repos::operations;
use crate::crawler::repos::refs::{self, Refs};
use crate::crawler::repos::{reflog, remotes};
use crate::crawler::store::{self, Place, Row, Rows};
use crate::crawler::value::{at, text};
use crate::crawler::{Places, Stop};
use crate::params::Params;

/// The git actions a session prints a commit by (step 13, printed).
const PRINTED: [&str; 4] = ["commit", "merge", "revert", "cherry_pick"];

/// The git actions that create or merge a pull request (step 13).
const PULL_ACTIONS: [&str; 2] = ["pr_created", "pr_merged"];

/// A repository to crawl, as the list holds it.
#[derive(Debug, Clone)]
pub struct Job {
    pub repository: RecordId,
    pub common: String,
    pub name: String,
    pub checkouts: Vec<JobCheckout>,
}

/// One of its checkouts, as the list read it.
#[derive(Debug, Clone)]
pub struct JobCheckout {
    pub id: RecordId,
    pub toplevel: String,
    pub present: bool,
    pub branch: Option<String>,
}

/// A git event, as the lines source wrote it, with the session of its
/// tool call's reply.
#[derive(Debug, Clone)]
pub struct Event {
    pub action: String,
    pub hash: Option<String>,
    pub dir: Option<String>,
    pub url: Option<String>,
    pub pr_number: Option<i64>,
    pub at: DateTime<Utc>,
    pub tool_call: RecordId,
    pub session: Option<RecordId>,
    pub valid_from: DateTime<Utc>,
}

/// What every worker of one pass shares.
pub struct Shared {
    pub db: Db,
    pub params: Params,
    pub places: Places,
    pub hosts: Arc<hosts::Pass>,
    /// Every git event the lines have shown.
    pub events: Vec<Event>,
    /// Every checkout's root, and its repository: which one holds a
    /// directory.
    pub holders: Vec<(String, RecordId)>,
}

impl Shared {
    /// The repository whose checkout holds `dir`: the one with the longest
    /// root that `dir` is or is under.
    #[must_use]
    pub fn holder(&self, dir: &str) -> Option<&RecordId> {
        let dir = dir.trim_end_matches('/');
        self.holders
            .iter()
            .filter(|(root, _)| {
                let root = root.trim_end_matches('/');
                dir == root || dir.starts_with(&format!("{root}/"))
            })
            .max_by_key(|(root, _)| root.len())
            .map(|(_, repository)| repository)
    }
}

/// What the crawler keeps of one repository from one pass to the next.
#[derive(Debug, Clone, Default)]
pub struct Remembered {
    /// The refs its previous crawl read since superx-recon started.
    pub refs: Option<Refs>,
    /// The tip it last blamed since superx-recon started.
    pub blamed_tip: Option<String>,
}

/// What one worker did.
#[derive(Debug)]
pub struct Done {
    pub name: String,
    /// The crawl row it wrote.
    pub crawl: Option<RecordId>,
    pub outcome: &'static str,
    pub note: Option<String>,
    pub commits_held: i64,
    pub commits_read: i64,
    pub tally: Tally,
    /// Rows it wrote, by table.
    pub rows: BTreeMap<&'static str, i64>,
    pub hosts: Vec<hosts::Crawled>,
    pub stopped: bool,
    pub memory: Remembered,
    /// What its host crawls hold, by the host repository's UUID.
    pub known: Vec<(Uuid, Known)>,
}

/// One git directory the crawl reads: the repository's, or a mirror's.
#[derive(Debug, Clone)]
struct Dir {
    path: PathBuf,
    mirror: Option<MirrorDir>,
}

#[derive(Debug, Clone)]
struct MirrorDir {
    host: Host,
    host_repository: RecordId,
    /// Its mirror row, when the database holds one or this crawl made it.
    row: Option<RecordId>,
    /// This worker took its host repository in the pass.
    claimed: bool,
}

/// A commit the repository holds a row for.
#[derive(Debug, Clone)]
struct Held {
    id: RecordId,
    committed_at: DateTime<Utc>,
    added: i64,
}

/// A reflog entry, as the crawler tells one from another: its checkout's
/// and its mirror's UUIDs, its ref, its time, its commits and its message.
type MoveKey = (Option<Uuid>, Option<Uuid>, String, i64, Option<String>, Option<String>, String);

/// A commit tied to a session (step 13).
struct Tie {
    commit: RecordId,
    session: RecordId,
    tool_call: RecordId,
    via: &'static str,
    pull_request: Option<RecordId>,
    at: DateTime<Utc>,
}

/// Where each commit sits (step 9), for the steps after it.
#[derive(Debug, Default)]
struct Placed {
    place: HashMap<String, &'static str>,
    first_parent: Vec<String>,
}

/// What a crawl came to before it is written.
struct Ran {
    outcome: &'static str,
    memory: Remembered,
    hosts: Vec<hosts::Crawled>,
    commits_held: i64,
    commits_read: i64,
    kept: Vec<(Uuid, Known, Vec<RecordId>)>,
}

/// The crawl of one repository by its worker.
struct Worker<'s> {
    shared: &'s Shared,
    job: &'s Job,
    rows: Rows,
    crawl: Place,
    git: Git,
    stop: Stop,
    notes: Vec<String>,
    counts: BTreeMap<&'static str, i64>,
    started_at: DateTime<Utc>,
    since: Option<DateTime<Utc>>,
    identity: Option<String>,
    stopped: bool,
}

/// Crawl one repository in depth.
pub async fn crawl(shared: Arc<Shared>, job: Job, memory: Remembered, stop: Stop) -> Done {
    let started_at = Utc::now();
    let since = (shared.params.since_days > 0)
        .then(|| started_at - TimeDelta::days(i64::try_from(shared.params.since_days).unwrap_or(i64::MAX)));
    let mut rows = Rows::default();
    let crawl = rows.hold("crawl");
    let mut w = Worker {
        shared: &shared,
        job: &job,
        rows,
        crawl,
        git: Git::new(shared.params.git_timeout_ms),
        stop,
        notes: Vec::new(),
        counts: BTreeMap::new(),
        started_at,
        since,
        identity: None,
        stopped: false,
    };
    let ran = w.run(memory.clone()).await;
    let tally = w.git.tally();
    let failed = |note: String| Done {
        name: job.name.clone(),
        crawl: None,
        outcome: "partial",
        note: Some(note),
        commits_held: 0,
        commits_read: 0,
        tally,
        rows: BTreeMap::new(),
        hosts: Vec::new(),
        stopped: false,
        memory: Remembered::default(),
        known: Vec::new(),
    };
    let ran = match ran {
        Ok(ran) => ran,
        Err(e) => return failed(format!("{}: the database did not answer: {e}", job.name)),
    };
    if w.stopped {
        return Done { outcome: "stopped", stopped: true, note: None, memory, ..failed(String::new()) };
    }
    // Every row, the crawl's first, written one worker at a time; an
    // account another worker wrote already is left out.
    let crawl = w.crawl.id.clone();
    let pass = Arc::clone(&shared.hosts);
    let _one_at_a_time = pass.write.lock().await;
    let mut batch = w.rows.into_batch(&|id: &RecordId| id.table.as_str() == "host_user" && pass.user_written(id));
    let added = batch.added.clone();
    if let Err(e) = store::flush(&shared.db, &mut batch).await {
        return failed(format!("{}: its rows were refused: {e}", job.name));
    }
    pass.users_written(ran.kept.iter().flat_map(|(_, _, users)| users.clone()));
    Done {
        name: job.name.clone(),
        crawl: Some(crawl),
        outcome: ran.outcome,
        note: (!w.notes.is_empty()).then(|| w.notes.join("; ")),
        commits_held: ran.commits_held,
        commits_read: ran.commits_read,
        tally,
        rows: added,
        hosts: ran.hosts,
        stopped: false,
        memory: ran.memory,
        known: ran.kept.into_iter().map(|(u, k, _)| (u, k)).collect(),
    }
}

impl Worker<'_> {
    fn count(&mut self, name: &'static str, n: i64) {
        *self.counts.entry(name).or_default() += n;
    }

    fn stopped(&mut self, memory: Remembered) -> Ran {
        self.stopped = true;
        Ran { outcome: "stopped", memory, hosts: Vec::new(), commits_held: 0, commits_read: 0, kept: Vec::new() }
    }

    /// What every git call on a mirror carries (step 4).
    fn mirror_with(&self, host: Host) -> With<'static> {
        let places = &self.shared.places;
        let mut env: Vec<(String, String)> = vec![
            ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ];
        if let Some(askpass) = &places.askpass {
            env.push(("GIT_ASKPASS".to_string(), askpass.to_string_lossy().into_owned()));
        }
        env.push((hosts::ASKPASS_ENV.to_string(), host.name().to_string()));
        if let Some(home) = &places.home {
            env.push((hosts::HOME_ENV.to_string(), home.to_string_lossy().into_owned()));
        }
        let config = [
            "fetch.prune=false",
            "fetch.pruneTags=false",
            "gc.auto=0",
            "maintenance.auto=false",
            "core.logAllRefUpdates=always",
            "gc.reflogExpire=never",
            "gc.reflogExpireUnreachable=never",
            "core.hooksPath=/dev/null",
            "http.followRedirects=false",
            "credential.helper=",
            "http.lowSpeedLimit=1",
            "http.lowSpeedTime=60",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .chain(std::iter::once(format!("credential.username={}", host.git_user())))
        .collect();
        With { config, env, input: None, unbounded: false }
    }

    fn with_of(&self, dir: &Dir) -> With<'static> {
        dir.mirror.as_ref().map_or_else(With::default, |m| self.mirror_with(m.host))
    }

    /// One git call of a step, a git_call row; `None` when a stop came
    /// first, which kills it.
    async fn git(&mut self, step: &'static str, dir: &Path, args: &[&str], with: &With<'_>) -> Option<Call> {
        if self.stopped {
            return None;
        }
        let Some(call) = self.stop.clone().unless(self.git.run_with(dir, args, with)).await else {
            self.stopped = true;
            return None;
        };
        self.record(step, &call);
        Some(call)
    }

    fn record(&mut self, step: &'static str, call: &Call) {
        if call.failed() {
            let subcommand = call.args.iter().find(|a| !a.starts_with('-') && !a.contains('=')).cloned().unwrap_or_default();
            let said = if call.timed_out { "cut off at attr_recon_git_timeout_ms".to_string() } else { call.stderr.trim().to_string() };
            self.notes.push(format!("git {subcommand} in {}: {said}", call.dir.display()));
        }
        self.rows.push(
            "git_call",
            Row::new()
                .link("crawl", &self.crawl.id)
                .text("step", step)
                .text("dir", call.dir.to_string_lossy().into_owned())
                .texts("args", &call.args)
                .time("started_at", call.started_at)
                .int("ms", i64::try_from(call.ms).unwrap_or(i64::MAX))
                .some_int("exit", call.exit.map(i64::from))
                .flag("timed_out", call.timed_out)
                .int("bytes_out", i64::try_from(call.raw.len()).unwrap_or(i64::MAX))
                .int("lines_out", i64::try_from(call.lines_out()).unwrap_or(i64::MAX))
                .some_text("stderr", (!call.stderr.is_empty()).then(|| call.stderr.clone())),
        );
    }

    /// Steps 3 to 13, then the crawl's row (step 14).
    async fn run(&mut self, memory: Remembered) -> Result<Ran> {
        let db = self.shared.db.clone();
        let common = PathBuf::from(&self.job.common);
        let plain = With::default();

        // Step 3: the remotes.
        let Some(config) = self.git("setup", &common, &["config", "--null", "--list"], &plain).await else {
            return Ok(self.stopped(memory));
        };
        if config.failed() {
            // Its git directory is there and git cannot read it.
            self.fill_crawl("unreadable", "HEAD", "head", None, 0, &BTreeMap::new());
            return Ok(Ran { outcome: "unreadable", memory, hosts: Vec::new(), commits_held: 0, commits_read: 0, kept: Vec::new() });
        }
        let config = remotes::config(&config.stdout);
        let Some(shallow) = self.git("setup", &common, &["rev-parse", "--is-shallow-repository"], &plain).await else {
            return Ok(self.stopped(memory));
        };
        if shallow.ok().map(str::trim) == Some("true") {
            self.notes.push("shallow: its commits are those git holds".to_string());
        }
        self.identity = config.email.clone();
        self.remotes(&db, &config).await?;

        // Step 4: the hosts' repositories and the mirrors.
        let pass = Arc::clone(&self.shared.hosts);
        let mut dirs = vec![Dir { path: common.clone(), mirror: None }];
        let mut host_crawls: Vec<HostCrawl<'_>> = Vec::new();
        let mut origin: Option<RecordId> = None;
        let mut named: Vec<RecordId> = Vec::new();
        for (name, remote) in &config.remotes {
            let Some((host_name, path)) = remote.url.as_deref().and_then(remotes::host_path) else { continue };
            let Some(host) = Host::of(&host_name) else { continue };
            let folder = format!("{}/{path}.git", host.name());
            let mirror_path = self.shared.places.repos.as_ref().map(|r| r.join(&folder));
            if pass.claim(host, &path) {
                let held = pass.repository(host, &path).and_then(|h| pass.take_known(&h.id));
                let mut stop = self.stop.clone();
                let Some(hc) = HostCrawl::begin(&pass, &mut self.rows, &db, &self.crawl.id, host, &path, held, &mut stop).await? else {
                    return Ok(self.stopped(memory));
                };
                let hr = hc.host_repository().clone();
                let mut row = pass.mirror(&hr).map(|m| m.id);
                if let (true, Some(mirror_path)) = (hc.answered(), mirror_path.as_ref()) {
                    row = self.mirror(host, &path, &folder, mirror_path, &hr, row).await;
                    if self.stopped {
                        return Ok(self.stopped(memory));
                    }
                }
                if let Some(mirror_path) = mirror_path.filter(|p| p.is_dir()) {
                    dirs.push(Dir { path: mirror_path, mirror: Some(MirrorDir { host, host_repository: hr.clone(), row, claimed: true }) });
                }
                if name == "origin" {
                    origin = Some(hr.clone());
                }
                named.push(hr);
                host_crawls.push(hc);
            } else if let Some(hr) = pass.repository(host, &path).map(|h| h.id) {
                if let Some(mirror_path) = mirror_path.filter(|p| p.is_dir()) {
                    let row = pass.mirror(&hr).map(|m| m.id);
                    dirs.push(Dir { path: mirror_path, mirror: Some(MirrorDir { host, host_repository: hr.clone(), row, claimed: false }) });
                }
                if name == "origin" {
                    origin = Some(hr.clone());
                }
                named.push(hr);
            }
        }

        // Step 5: the refs and the main line.
        let mut now: Refs = BTreeMap::new();
        let mut by_dir: Vec<(Dir, Vec<refs::Ref>)> = Vec::new();
        for dir in dirs.clone() {
            let with = self.with_of(&dir);
            let Some(call) = self
                .git("refs", &dir.path, &["for-each-ref", "--format=%(refname)%09%(objectname)%09%(*objectname)%09%(symref)"], &with)
                .await
            else {
                return Ok(self.stopped(memory));
            };
            let read = refs::parse(call.ok().unwrap_or_default());
            for r in &read {
                now.insert((place_of(&dir), r.name.clone()), r.commit.clone());
            }
            by_dir.push((dir, read));
        }
        self.count("refs_read", count(now.len()));
        match &memory.refs {
            None => {
                self.notes.push("the refs were compared with none: its first crawl since superx-recon started".to_string());
            }
            Some(before) => {
                let changes = refs::compare(before, &now);
                self.count("refs_new", changes.new);
                self.count("refs_gone", changes.gone);
                self.count("refs_moved", count(changes.moved.len()));
                for ((place, _), old, new) in &changes.moved {
                    let Some(dir) = dirs.iter().find(|d| place_of(d) == *place).cloned() else { continue };
                    let with = self.with_of(&dir);
                    let Some(call) = self.git("refs", &dir.path, &["rev-list", "--count", old, "--not", new], &with).await else {
                        return Ok(self.stopped(memory));
                    };
                    if call.ok().map(str::trim).is_some_and(|n| n != "0") {
                        self.count("refs_rewritten", 1);
                    }
                }
            }
        }
        let (mainline, from, tip, tip_dir) = self.mainline(&db, &by_dir, &host_crawls, origin.as_ref()).await?;
        if self.stopped {
            return Ok(self.stopped(memory));
        }
        let tip_dir = tip_dir.unwrap_or_else(|| dirs[0].clone());
        if tip.is_none() {
            self.notes.push("no main line resolves: nothing is placed on it or blamed".to_string());
        }

        // Step 6: the unreachable commits.
        let Some(fsck) = self.git("unreachable", &common, &["fsck", "--unreachable", "--connectivity-only", "--no-progress"], &plain).await
        else {
            return Ok(self.stopped(memory));
        };
        let unreachable: Vec<String> =
            fsck.stdout.lines().filter_map(|l| l.strip_prefix("unreachable commit ")).map(|h| h.trim().to_string()).collect();
        self.count("unreachable_read", count(unreachable.len()));
        let limit = usize::try_from(self.shared.params.unreachable_commits).unwrap_or(usize::MAX);
        let unreachable: Vec<String> = unreachable.into_iter().take(limit).collect();

        // Step 7: the commits.
        let mut held = self.held_commits(&db).await?;
        let mut reach: Vec<(Dir, HashMap<String, i64>)> = Vec::new();
        for dir in dirs.clone() {
            let with = self.with_of(&dir);
            let Some(call) = self.git("log", &dir.path, &["rev-list", "--all", "--reflog", "--timestamp"], &with).await else {
                return Ok(self.stopped(memory));
            };
            let found = call
                .ok()
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let (t, h) = l.split_once(' ')?;
                    Some((h.trim().to_string(), t.parse::<i64>().ok()?))
                })
                .collect();
            reach.push((dir, found));
        }
        let Some((commits_held, commits_read, in_common)) = self.commits(&mut held, &reach, &unreachable).await else {
            return Ok(self.stopped(memory));
        };

        // Step 8: the reflogs.
        if !self.reflogs(&db, &dirs).await? {
            return Ok(self.stopped(memory));
        }

        // Step 9: the placements.
        let Some(placed) = self.placements(&db, &held, &dirs, tip.as_deref(), &tip_dir).await? else {
            return Ok(self.stopped(memory));
        };

        // Step 10: what landed.
        if !self.landed(&db, &held, &placed, &reach, &in_common, tip.as_deref(), &tip_dir).await? {
            return Ok(self.stopped(memory));
        }

        // Step 11: what survives.
        let mut memory = Remembered { refs: Some(now), blamed_tip: memory.blamed_tip };
        if !self.survival(&db, &held, tip.as_deref(), &tip_dir, &mut memory).await? {
            return Ok(self.stopped(memory));
        }

        // Steps 15 to 21: the host crawls, finished.
        let context = Context { tip: tip.clone(), first_parent: placed.first_parent.clone() };
        let mut crawled = Vec::new();
        let mut kept = Vec::new();
        for hc in host_crawls {
            let mut stop = self.stop.clone();
            let Some((c, k, users)) = hc.finish(&mut self.rows, &db, &context, &mut stop).await? else {
                return Ok(self.stopped(memory));
            };
            crawled.push(c);
            if let Some(u) = record_uuid_of(&k.host_repository) {
                kept.push((u, k, users));
            }
        }

        // Step 13: the commits tied to sessions.
        self.ties(&db, &held, &named, &kept).await?;

        // Step 14: the crawl.
        if commits_read != commits_held {
            self.notes.push(format!("{commits_read} commits read of {commits_held} held"));
        }
        let outcome = if self.git.tally().failed > 0 || commits_read != commits_held { "partial" } else { "complete" };
        self.count("checkouts_read", count(self.job.checkouts.len()));
        let counts = self.counts.clone();
        self.fill_crawl(outcome, &mainline, from, tip, commits_read, &counts);
        Ok(Ran { outcome, memory, hosts: crawled, commits_held, commits_read, kept })
    }

    /// The crawl's row, in its place.
    fn fill_crawl(&mut self, outcome: &str, mainline: &str, from: &str, tip: Option<String>, commits_read: i64, counts: &BTreeMap<&'static str, i64>) {
        let tally = self.git.tally();
        let n = |name: &str| counts.get(name).copied().unwrap_or(0);
        let rows_written: i64 = ["commit", "commit_file", "git_call", "remote_state", "ref_move", "placement", "landed_via", "survival", "commit_session", "mirror"]
            .iter()
            .map(|t| self.rows.count(t))
            .sum::<i64>()
            + 1;
        let mut row = Row::new()
            .link("repository", &self.job.repository)
            .time("started_at", self.started_at)
            .time("finished_at", Utc::now())
            .text("mainline", mainline)
            .text("mainline_from", from)
            .some_text("identity", self.identity.clone())
            .some_text("tip", tip)
            .some_time("since", self.since)
            .text("outcome", outcome)
            .int("commits_read", commits_read)
            .int("git_calls", u64_count(tally.calls))
            .int("git_failed", u64_count(tally.failed))
            .int("git_timeouts", u64_count(tally.timeouts))
            .int("git_ms", u64_count(tally.ms))
            .int("bytes_read", u64_count(tally.bytes_out))
            .int("rows_written", rows_written)
            .some_text("note", (!self.notes.is_empty()).then(|| self.notes.join("; ")));
        for name in [
            "files_blamed",
            "files_left",
            "checkouts_read",
            "refs_read",
            "files_read",
            "reflog_read",
            "unreachable_read",
            "blame_lines_read",
            "commits_new",
            "commit_files_new",
            "lines_added_new",
            "lines_removed_new",
            "refs_new",
            "refs_moved",
            "refs_gone",
            "refs_rewritten",
            "head_moves_new",
            "placements_changed",
            "landed_new",
            "survival_changed",
            "survival_delta",
        ] {
            row = row.int(name, n(name));
        }
        let place = self.crawl.clone();
        self.rows.fill(&place, row);
    }

    // ── step 3 ──────────────────────────────────────────────────────────

    /// Each remote a remote_state row when it changed, and one with no url
    /// once it is gone.
    async fn remotes(&mut self, db: &Db, config: &remotes::Config) -> Result<()> {
        let rows: Vec<Value> = db
            .query("SELECT name, url, push_url, host, path, valid_from FROM remote_state WHERE repository = $r")
            .bind(("r", self.job.repository.clone()))
            .await?
            .take(0)?;
        let mut newest: HashMap<String, (DateTime<Utc>, [Option<String>; 4])> = HashMap::new();
        for r in &rows {
            let (Some(name), Some(t)) = (text(r, &["name"]), time(r, "valid_from")) else { continue };
            let fields = ["url", "push_url", "host", "path"].map(|f| text(r, &[f]).map(str::to_string));
            match newest.get(name) {
                Some((seen, _)) if *seen >= t => {}
                _ => {
                    newest.insert(name.to_string(), (t, fields));
                }
            }
        }
        for (name, remote) in &config.remotes {
            let (host, path) = remote.url.as_deref().and_then(remotes::host_path).map_or((None, None), |(h, p)| (Some(h), Some(p)));
            let fields = [remote.url.clone(), remote.push_url.clone(), host, path];
            if newest.get(name).is_some_and(|(_, held)| *held == fields) {
                continue;
            }
            self.remote_row(name, &fields);
        }
        let gone: Vec<String> = newest
            .iter()
            .filter(|(name, (_, fields))| fields[0].is_some() && !config.remotes.contains_key(*name))
            .map(|(name, _)| name.clone())
            .collect();
        for name in gone {
            self.remote_row(&name, &[None, None, None, None]);
        }
        Ok(())
    }

    fn remote_row(&mut self, name: &str, fields: &[Option<String>; 4]) {
        self.rows.push(
            "remote_state",
            Row::new()
                .link("repository", &self.job.repository)
                .link("crawl", &self.crawl.id)
                .text("name", name)
                .some_text("url", fields[0].clone())
                .some_text("push_url", fields[1].clone())
                .some_text("host", fields[2].clone())
                .some_text("path", fields[3].clone()),
        );
    }

    // ── step 4 ──────────────────────────────────────────────────────────

    /// A mirror: made bare with no template when its folder is absent, its
    /// row written when the database holds none, then fetched.
    async fn mirror(&mut self, host: Host, path: &str, folder: &str, at_path: &Path, hr: &RecordId, row: Option<RecordId>) -> Option<RecordId> {
        let with = self.mirror_with(host);
        let url = self.shared.hosts.endpoints.git_url(host, path);
        if !at_path.is_dir() {
            let parent = at_path.parent()?;
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.notes.push(format!("{}: {e}", parent.display()));
                return row;
            }
            let target = at_path.to_string_lossy().into_owned();
            self.git("fetch", parent, &["init", "--bare", "--template=", &target], &with).await?;
        }
        if !at_path.is_dir() {
            return row;
        }
        let row = match row {
            Some(row) => row,
            None => {
                let id = self.rows.push(
                    "mirror",
                    Row::new().link("host_repository", hr).text("url", url.clone()).text("folder", folder).time("created_at", Utc::now()),
                );
                self.shared.hosts.mirror_made(hr, Mirror { id: id.clone(), folder: folder.to_string() });
                id
            }
        };
        let fetch = With { unbounded: true, ..with };
        self.git("fetch", at_path, &["fetch", "--no-auto-maintenance", "--no-recurse-submodules", &url, "+refs/*:refs/*"], &fetch).await?;
        Some(row)
    }

    // ── step 5 ──────────────────────────────────────────────────────────

    /// The main line: the first of override, host_default, origin_head,
    /// local_branch and head that resolves; its ref, its word, its tip and
    /// the git directory that holds it.
    async fn mainline(
        &mut self,
        db: &Db,
        by_dir: &[(Dir, Vec<refs::Ref>)],
        host_crawls: &[HostCrawl<'_>],
        origin: Option<&RecordId>,
    ) -> Result<(String, &'static str, Option<String>, Option<Dir>)> {
        let common = by_dir[0].0.clone();
        let repo_refs = &by_dir[0].1;
        let commit_of = |refs: &[refs::Ref], name: &str| refs.iter().find(|r| r.name == name).map(|r| r.commit.clone());

        if let Some(wanted) = self.shared.params.mainline_refs.get(&self.job.name).cloned() {
            let target = format!("{wanted}^{{commit}}");
            let plain = With::default();
            let Some(resolved) = self.git("refs", &common.path, &["rev-parse", "--verify", "--quiet", &target], &plain).await else {
                return Ok(("HEAD".to_string(), "head", None, None));
            };
            if let Some(tip) = resolved.ok().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string) {
                let Some(full) = self.git("refs", &common.path, &["rev-parse", "--symbolic-full-name", &wanted], &plain).await else {
                    return Ok(("HEAD".to_string(), "head", None, None));
                };
                let name = full.ok().map(str::trim).filter(|n| !n.is_empty()).map_or(wanted.clone(), str::to_string);
                return Ok((name, "override", Some(tip), Some(common)));
            }
        }

        if let Some(hr) = origin {
            let object = match host_crawls.iter().find(|hc| hc.host_repository() == hr) {
                Some(hc) => hc.repository_object().cloned(),
                None => newest_repository(db, hr).await?,
            };
            let branch = object.as_ref().and_then(|o| {
                o.get("default_branch")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| o.get("mainbranch").and_then(|m| m.get("name")).and_then(serde_json::Value::as_str))
                    .map(str::to_string)
            });
            if let Some(branch) = branch {
                let head = format!("refs/heads/{branch}");
                let mirror = by_dir.iter().find(|(d, _)| d.mirror.as_ref().is_some_and(|m| m.host_repository == *hr));
                if let Some((dir, refs)) = mirror {
                    if let Some(tip) = commit_of(refs, &head) {
                        return Ok((head, "host_default", Some(tip), Some(dir.clone())));
                    }
                }
                let remote = format!("refs/remotes/origin/{branch}");
                if let Some(tip) = commit_of(repo_refs, &remote) {
                    return Ok((remote, "host_default", Some(tip), Some(common)));
                }
            }
        }

        if let Some(target) = repo_refs.iter().find(|r| r.name == "refs/remotes/origin/HEAD").and_then(|r| r.symref.clone()) {
            if let Some(tip) = commit_of(repo_refs, &target) {
                return Ok((target, "origin_head", Some(tip), Some(common)));
            }
        }

        for name in ["refs/heads/main", "refs/heads/master"] {
            if let Some(tip) = commit_of(repo_refs, name) {
                return Ok((name.to_string(), "local_branch", Some(tip), Some(common)));
            }
        }

        let head = std::fs::read_to_string(common.path.join("HEAD")).unwrap_or_default();
        let head = head.trim();
        let tip = match head.strip_prefix("ref: ") {
            Some(target) => commit_of(repo_refs, target.trim()),
            None => is_full_hash(head).then(|| head.to_string()),
        };
        Ok(("HEAD".to_string(), "head", tip, Some(common)))
    }

    // ── step 7 ──────────────────────────────────────────────────────────

    async fn held_commits(&self, db: &Db) -> Result<HashMap<String, Held>> {
        let rows: Vec<Value> = db
            .query("SELECT id, hash, committed_at, added FROM commit WHERE repository = $r")
            .bind(("r", self.job.repository.clone()))
            .await?
            .take(0)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some((
                    text(r, &["hash"])?.to_string(),
                    Held { id: record(r, "id")?, committed_at: time(r, "committed_at")?, added: int(r, "added").unwrap_or(0) },
                ))
            })
            .collect())
    }

    /// The commits git holds within the crawl's since, each new one read
    /// once whole. `held` gains the new rows. Returns commits_held,
    /// commits_read and the commits the repository's own git directory
    /// holds; `None` when a stop came first.
    async fn commits(
        &mut self,
        held: &mut HashMap<String, Held>,
        reach: &[(Dir, HashMap<String, i64>)],
        unreachable: &[String],
    ) -> Option<(i64, i64, HashSet<String>)> {
        let since = self.since.map(|s| s.timestamp());
        let within = |t: i64| since.is_none_or(|s| t >= s);
        // Every commit a ref or a reflog reaches, with its time.
        let mut times: HashMap<String, i64> = HashMap::new();
        for (_, found) in reach {
            for (h, t) in found {
                times.entry(h.clone()).or_insert(*t);
            }
        }
        let mut in_common: HashSet<String> = reach[0].1.keys().cloned().collect();
        in_common.extend(unreachable.iter().cloned());
        let mut wanted: Vec<String> = times.iter().filter(|(h, t)| within(**t) && !held.contains_key(*h)).map(|(h, _)| h.clone()).collect();
        for h in unreachable {
            if !times.contains_key(h) && !held.contains_key(h) {
                wanted.push(h.clone());
            }
        }
        wanted.sort();
        wanted.dedup();

        // Read them, from the repository's git directory when it holds
        // them, else from the mirror that does.
        let mut read: HashMap<String, (commits::Commit, usize)> = HashMap::new();
        for (i, (dir, found)) in reach.iter().enumerate() {
            let here: Vec<&String> = wanted.iter().filter(|h| !read.contains_key(*h) && (i == 0 || found.contains_key(*h))).collect();
            if here.is_empty() {
                continue;
            }
            let input: Vec<u8> = here.iter().flat_map(|h| format!("{h}\n").into_bytes()).collect();
            let with = With { input: Some(&input), ..self.with_of(dir) };
            let call = self.git("log", &dir.path, &["cat-file", "--batch"], &with).await?;
            for (hash, object) in commits::objects(call.ok_raw().unwrap_or_default()) {
                if let Some(c) = commits::parse(&hash, &object) {
                    read.insert(hash, (c, i));
                }
            }
        }
        // Unreachable commits not reached otherwise count by their own time.
        for h in unreachable {
            if let (false, Some((c, _))) = (times.contains_key(h), read.get(h)) {
                times.insert(h.clone(), c.committer.at.timestamp());
            }
            if let (false, Some(row)) = (times.contains_key(h), held.get(h)) {
                times.insert(h.clone(), row.committed_at.timestamp());
            }
        }
        read.retain(|_, (c, _)| within(c.committer.at.timestamp()));
        let in_since: HashSet<&String> = times.iter().filter(|(_, t)| within(**t)).map(|(h, _)| h).collect();

        // Their files, their counts blind to whitespace, their patch ids.
        for (i, (dir, _)) in reach.iter().enumerate() {
            let mut here: Vec<&String> = read.iter().filter(|(_, (_, at_dir))| *at_dir == i).map(|(h, _)| h).collect();
            if here.is_empty() {
                continue;
            }
            here.sort();
            let input: Vec<u8> = here.iter().flat_map(|h| format!("{h}\n").into_bytes()).collect();
            let base = self.with_of(dir);
            let with = With { input: Some(&input), ..base.clone() };
            let files = self
                .git(
                    "log",
                    &dir.path,
                    &["log", "--stdin", "--no-walk=unsorted", "--diff-merges=first-parent", "--no-renames", "--raw", "--numstat", "--no-abbrev", "-z", "--format=commit %H"],
                    &with,
                )
                .await?;
            let files = commits::files(files.ok_raw().unwrap_or_default());
            let blind = self
                .git(
                    "log",
                    &dir.path,
                    &["log", "--stdin", "--no-walk=unsorted", "--diff-merges=first-parent", "--no-renames", "--numstat", "--ignore-all-space", "-z", "--format=commit %H"],
                    &with,
                )
                .await?;
            let blind = commits::numstat(blind.ok_raw().unwrap_or_default());
            let patches = self.git("patches", &dir.path, &["log", "--stdin", "--no-walk=unsorted", "--no-renames", "-p", "--format=commit %H"], &with).await?;
            let patch_input = patches.ok_raw().unwrap_or_default().to_vec();
            let ids = self.git("patches", &dir.path, &["patch-id", "--stable"], &With { input: Some(&patch_input), ..base.clone() }).await?;
            let ids = commits::patch_ids(ids.ok().unwrap_or_default());
            for hash in here {
                let Some((c, _)) = read.get(hash) else { continue };
                let files = files.get(hash).cloned().unwrap_or_default();
                let none = HashMap::new();
                let blind = blind.get(hash).unwrap_or(&none);
                let id = self.commit_row(c, &files, blind, ids.get(hash).cloned());
                let added = files.iter().filter_map(|f| f.lines.map(|l| l.0)).sum();
                held.insert(hash.clone(), Held { id, committed_at: c.committer.at, added });
            }
        }
        let commits_held = count(in_since.len());
        let commits_read = count(in_since.iter().filter(|h| held.contains_key(**h)).count());
        Some((commits_held, commits_read, in_common))
    }

    fn commit_row(&mut self, c: &commits::Commit, files: &[commits::File], blind: &HashMap<String, Option<(i64, i64)>>, patch_id: Option<String>) -> RecordId {
        let added: i64 = files.iter().filter_map(|f| f.lines.map(|l| l.0)).sum();
        let removed: i64 = files.iter().filter_map(|f| f.lines.map(|l| l.1)).sum();
        let id = self.rows.push(
            "commit",
            Row::new()
                .link("repository", &self.job.repository)
                .text("hash", c.hash.clone())
                .time("authored_at", c.author.at)
                .time("committed_at", c.committer.at)
                .text("author", c.author.email.clone())
                .text("committer", c.committer.email.clone())
                .text("subject", c.subject.clone())
                .some_text("body", c.body.clone())
                .int("parents", count(c.parents.len()))
                .int("added", added)
                .int("removed", removed)
                .text("author_name", c.author.name.clone())
                .text("committer_name", c.committer.name.clone())
                .texts("parent_hashes", &c.parents)
                .text("tree", c.tree.clone())
                .int("author_offset", c.author.offset)
                .int("committer_offset", c.committer.offset)
                .some_text("patch_id", patch_id)
                .some_text("signature", c.signature.clone()),
        );
        self.count("commits_new", 1);
        self.count("lines_added_new", added);
        self.count("lines_removed_new", removed);
        for f in files {
            let (a, r) = f.lines.unwrap_or((0, 0));
            let (ba, br) = if f.lines.is_some() { blind.get(&f.path).copied().flatten().unwrap_or((0, 0)) } else { (0, 0) };
            self.rows.push(
                "commit_file",
                Row::new()
                    .link("commit", &id)
                    .text("path", f.path.clone())
                    .text("change", f.change.as_str())
                    .int("added", a)
                    .int("removed", r)
                    .int("added_ignoring_space", ba)
                    .int("removed_ignoring_space", br)
                    .flag("binary", f.lines.is_none()),
            );
            self.count("commit_files_new", 1);
            self.count("files_read", 1);
        }
        id
    }

    // ── step 8 ──────────────────────────────────────────────────────────

    /// Every reflog entry git keeps, each one ref_move row, written once.
    /// False when a stop came first.
    async fn reflogs(&mut self, db: &Db, dirs: &[Dir]) -> Result<bool> {
        let common = dirs[0].path.clone();
        let Some(format) = self.git("reflog", &common, &["rev-parse", "--show-ref-format"], &With::default()).await else {
            return Ok(false);
        };
        let mirrors: Vec<&Dir> = dirs.iter().filter(|d| d.mirror.as_ref().is_some_and(|m| m.claimed && m.row.is_some())).collect();
        let mut seen: HashSet<MoveKey> = HashSet::new();
        let mut sql = "SELECT checkout, mirror, ref, at, old, new, action FROM ref_move WHERE repository = $r AND mirror = NONE;".to_string();
        let mirror_rows: Vec<RecordId> = mirrors.iter().filter_map(|d| d.mirror.as_ref().and_then(|m| m.row.clone())).collect();
        if !mirror_rows.is_empty() {
            sql.push_str("SELECT checkout, mirror, ref, at, old, new, action FROM ref_move WHERE mirror INSIDE $mirrors;");
        }
        let mut res = db.query(sql).bind(("r", self.job.repository.clone())).bind(("mirrors", mirror_rows.clone())).await?;
        let mut held: Vec<Value> = res.take(0)?;
        if !mirror_rows.is_empty() {
            held.extend(res.take::<Vec<Value>>(1)?);
        }
        for r in &held {
            let (Some(name), Some(t), Some(action)) = (text(r, &["ref"]), time(r, "at"), text(r, &["action"])) else { continue };
            seen.insert((
                record(r, "checkout").as_ref().and_then(record_uuid_of),
                record(r, "mirror").as_ref().and_then(record_uuid_of),
                name.to_string(),
                t.timestamp(),
                text(r, &["old"]).map(str::to_string),
                text(r, &["new"]).map(str::to_string),
                action.to_string(),
            ));
        }

        // Which reflog files: the repository's, each worktree's own, each
        // mirror's this worker took.
        let main = self.job.checkouts.iter().find(|c| operations::git_dir(Path::new(&c.toplevel)).is_some_and(|g| same_dir(&g, &common)));
        let mut sources: Vec<(PathBuf, Option<RecordId>, Option<RecordId>)> = vec![(common.clone(), main.map(|c| c.id.clone()), None)];
        for (dir, root) in reflog::worktrees(&common) {
            let checkout = root.and_then(|r| self.job.checkouts.iter().find(|c| same_dir(Path::new(&c.toplevel), Path::new(&r))).map(|c| c.id.clone()));
            sources.push((dir, checkout, None));
        }
        for d in &mirrors {
            sources.push((d.path.clone(), None, d.mirror.as_ref().and_then(|m| m.row.clone())));
        }
        let reftable = format.ok().map(str::trim) == Some("reftable");
        for (git_dir, checkout, mirror) in sources {
            let logs: Vec<(String, Vec<reflog::Entry>)> = if reftable && mirror.is_none() {
                let mut names = vec!["HEAD".to_string()];
                if checkout.as_ref().is_none_or(|_| git_dir == common) {
                    let listed = self.git("reflog", &git_dir, &["for-each-ref", "--format=%(refname)"], &With::default()).await;
                    let Some(listed) = listed else { return Ok(false) };
                    names.extend(listed.ok().unwrap_or_default().lines().map(str::to_string));
                }
                let mut out = Vec::new();
                for name in names {
                    let Some(shown) = self
                        .git("reflog", &git_dir, &["reflog", "show", "--date=unix", "--format=%H%x09%ge%x09%gd%x09%gs", &name], &With::default())
                        .await
                    else {
                        return Ok(false);
                    };
                    out.push((name, reflog::parse_shown(shown.ok().unwrap_or_default())));
                }
                out
            } else {
                reflog::logs(&git_dir)
                    .into_iter()
                    .map(|log| {
                        let text = std::fs::read_to_string(&log.path).unwrap_or_default();
                        (log.name, reflog::parse(&text))
                    })
                    .collect()
            };
            for (name, entries) in logs {
                let ref_checkout = if name == "HEAD" || git_dir != common { checkout.clone() } else { None };
                for e in entries {
                    self.count("reflog_read", 1);
                    let key = (
                        ref_checkout.as_ref().and_then(record_uuid_of),
                        mirror.as_ref().and_then(record_uuid_of),
                        name.clone(),
                        e.at.timestamp(),
                        e.old.clone(),
                        e.new.clone(),
                        e.action.clone(),
                    );
                    if !seen.insert(key) {
                        continue;
                    }
                    self.rows.push(
                        "ref_move",
                        Row::new()
                            .link("repository", &self.job.repository)
                            .some_link("checkout", ref_checkout.as_ref())
                            .some_link("mirror", mirror.as_ref())
                            .link("crawl", &self.crawl.id)
                            .text("ref", name.clone())
                            .time("at", e.at)
                            .some_text("old", e.old)
                            .some_text("new", e.new)
                            .some_text("identity", e.identity)
                            .text("action", e.action),
                    );
                    if name == "HEAD" && mirror.is_none() {
                        self.count("head_moves_new", 1);
                    }
                }
            }
        }
        Ok(true)
    }

    // ── step 9 ──────────────────────────────────────────────────────────

    /// Where each commit sits. `None` when a stop came first.
    async fn placements(&mut self, db: &Db, held: &HashMap<String, Held>, dirs: &[Dir], tip: Option<&str>, tip_dir: &Dir) -> Result<Option<Placed>> {
        let mut mainline: HashSet<String> = HashSet::new();
        let mut first_parent: Vec<String> = Vec::new();
        if let Some(tip) = tip {
            let with = self.with_of(tip_dir);
            let Some(all) = self.git("placement", &tip_dir.path, &["rev-list", tip], &with).await else { return Ok(None) };
            mainline = lines_of(&all);
            let Some(fp) = self.git("placement", &tip_dir.path, &["rev-list", "--first-parent", tip], &with).await else { return Ok(None) };
            first_parent = fp.ok().unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        }
        let first: HashSet<&String> = first_parent.iter().collect();
        let mut in_flight: HashSet<String> = HashSet::new();
        let branches: Vec<String> = self.job.checkouts.iter().filter(|c| c.present).filter_map(|c| c.branch.clone()).collect();
        for branch in branches {
            let name = format!("refs/heads/{branch}");
            let Some(call) = self.git("placement", &dirs[0].path, &["rev-list", &name], &With::default()).await else { return Ok(None) };
            in_flight.extend(lines_of(&call));
        }
        let mut any: HashSet<String> = HashSet::new();
        for dir in dirs {
            let with = self.with_of(dir);
            let Some(call) = self.git("placement", &dir.path, &["rev-list", "--all"], &with).await else { return Ok(None) };
            any.extend(lines_of(&call));
        }
        let rows: Vec<Value> = db
            .query("SELECT commit, place, first_parent, valid_from FROM placement WHERE commit.repository = $r")
            .bind(("r", self.job.repository.clone()))
            .await?
            .take(0)?;
        let mut newest: HashMap<Uuid, (DateTime<Utc>, String, bool)> = HashMap::new();
        for r in &rows {
            let (Some(c), Some(place), Some(t)) = (record(r, "commit").as_ref().and_then(record_uuid_of), text(r, &["place"]), time(r, "valid_from")) else {
                continue;
            };
            let fp = matches!(at(r, &["first_parent"]), Some(Value::Bool(true)));
            if newest.get(&c).is_none_or(|(seen, _, _)| *seen < t) {
                newest.insert(c, (t, place.to_string(), fp));
            }
        }
        let mut placed = Placed { first_parent: first_parent.clone(), ..Placed::default() };
        let since = self.since;
        let mut hashes: Vec<&String> = held.iter().filter(|(_, h)| since.is_none_or(|s| h.committed_at >= s)).map(|(k, _)| k).collect();
        hashes.sort();
        for hash in hashes {
            let place = if mainline.contains(hash) {
                "mainline"
            } else if in_flight.contains(hash) {
                "in_flight"
            } else if any.contains(hash) {
                "branch"
            } else {
                "recovered"
            };
            let fp = first.contains(hash);
            placed.place.insert(hash.clone(), place);
            let Some(row) = held.get(hash) else { continue };
            let Some(key) = record_uuid_of(&row.id) else { continue };
            if newest.get(&key).is_some_and(|(_, p, f)| p == place && *f == fp) {
                continue;
            }
            self.rows.push("placement", Row::new().link("commit", &row.id).link("crawl", &self.crawl.id).text("place", place).flag("first_parent", fp));
            self.count("placements_changed", 1);
        }
        Ok(Some(placed))
    }

    // ── step 10 ─────────────────────────────────────────────────────────

    /// What of a commit off the main line landed through a commit of the
    /// main line's first-parent history. False when a stop came first.
    #[allow(clippy::too_many_arguments)] // skill-allow: allow — what the step reads, each its own
    async fn landed(
        &mut self,
        db: &Db,
        held: &HashMap<String, Held>,
        placed: &Placed,
        reach: &[(Dir, HashMap<String, i64>)],
        in_common: &HashSet<String>,
        tip: Option<&str>,
        tip_dir: &Dir,
    ) -> Result<bool> {
        let Some(tip) = tip else { return Ok(true) };
        let with = self.with_of(tip_dir);
        let args = ["log", "--first-parent", "--diff-merges=first-parent", "--no-renames", "--raw", "--no-abbrev", "-z", "--format=commit %H", tip];
        let Some(call) = self.git("landed", &tip_dir.path, &args, &with).await else { return Ok(false) };
        // Each (path, blob) the first-parent history wrote, by its oldest
        // commit: the log lists the newest first.
        let mut wrote: HashMap<(String, String), String> = HashMap::new();
        let files = commits::files(call.ok_raw().unwrap_or_default());
        let order: Vec<String> = placed.first_parent.clone();
        for hash in &order {
            if !held.contains_key(hash) {
                continue;
            }
            for f in files.get(hash).into_iter().flatten() {
                if let (Some(blob), Change::Added | Change::Modified) = (&f.blob, f.change) {
                    wrote.insert((f.path.clone(), blob.clone()), hash.clone());
                }
            }
        }
        let off: Vec<&String> = placed.place.iter().filter(|(_, p)| **p != "mainline").map(|(h, _)| h).collect();
        if off.is_empty() || wrote.is_empty() {
            return Ok(true);
        }
        let rows: Vec<Value> = db
            .query("SELECT commit, path, via FROM landed_via WHERE commit.repository = $r")
            .bind(("r", self.job.repository.clone()))
            .await?
            .take(0)?;
        let mut seen: HashSet<(Uuid, String, Uuid)> = rows
            .iter()
            .filter_map(|r| {
                Some((record(r, "commit").as_ref().and_then(record_uuid_of)?, text(r, &["path"])?.to_string(), record(r, "via").as_ref().and_then(record_uuid_of)?))
            })
            .collect();
        for (i, (dir, found)) in reach.iter().enumerate() {
            let mut here: Vec<&&String> = off.iter().filter(|h| if i == 0 { in_common.contains(**h) } else { found.contains_key(**h) && !in_common.contains(**h) }).collect();
            if here.is_empty() {
                continue;
            }
            here.sort();
            let input: Vec<u8> = here.iter().flat_map(|h| format!("{h}\n").into_bytes()).collect();
            let with = With { input: Some(&input), ..self.with_of(dir) };
            let args = ["log", "--stdin", "--no-walk=unsorted", "--diff-merges=first-parent", "--no-renames", "--raw", "--no-abbrev", "-z", "--format=commit %H"];
            let Some(call) = self.git("landed", &dir.path, &args, &with).await else { return Ok(false) };
            let files = commits::files(call.ok_raw().unwrap_or_default());
            for hash in here {
                let Some(commit) = held.get(*hash) else { continue };
                for f in files.get(*hash).into_iter().flatten() {
                    let (Some(blob), Change::Added | Change::Modified) = (&f.blob, f.change) else { continue };
                    let Some(via) = wrote.get(&(f.path.clone(), blob.clone())).and_then(|v| held.get(v)) else { continue };
                    let (Some(c), Some(v)) = (record_uuid_of(&commit.id), record_uuid_of(&via.id)) else { continue };
                    if c == v || !seen.insert((c, f.path.clone(), v)) {
                        continue;
                    }
                    self.rows.push(
                        "landed_via",
                        Row::new().link("commit", &commit.id).text("path", f.path.clone()).link("via", &via.id).link("crawl", &self.crawl.id),
                    );
                    self.count("landed_new", 1);
                }
            }
        }
        Ok(true)
    }

    // ── step 11 ─────────────────────────────────────────────────────────

    /// What of each commit blame still credits at the main line's tip, when
    /// the tip moved since it was last blamed. False when a stop came first.
    async fn survival(&mut self, db: &Db, held: &HashMap<String, Held>, tip: Option<&str>, tip_dir: &Dir, memory: &mut Remembered) -> Result<bool> {
        let Some(tip) = tip else { return Ok(true) };
        if memory.blamed_tip.as_deref() == Some(tip) {
            return Ok(true);
        }
        let with = self.with_of(tip_dir);
        let Some(tree) = self.git("tip", &tip_dir.path, &["ls-tree", "-r", "-z", tip], &with).await else { return Ok(false) };
        let files: Vec<String> = tree
            .ok_raw()
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter_map(|entry| {
                let entry = String::from_utf8_lossy(entry);
                let (meta, path) = entry.split_once('\t')?;
                (meta.split(' ').nth(1) == Some("blob")).then(|| path.to_string())
            })
            .collect();
        let Some(history) = self.git("tip", &tip_dir.path, &["log", "--diff-merges=first-parent", "--no-renames", "--numstat", "-z", "--format=commit %H", tip], &with).await
        else {
            return Ok(false);
        };
        // A file's newest change on the main line: the first the log lists.
        let mut binary: HashMap<String, bool> = HashMap::new();
        let numstat = commits::numstat(history.ok_raw().unwrap_or_default());
        let order: Vec<String> = history
            .ok_raw()
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter_map(|t| String::from_utf8_lossy(t).trim_start_matches('\n').strip_prefix("commit ").map(|h| h.trim().to_string()))
            .collect();
        for hash in &order {
            for (path, lines) in numstat.get(hash).into_iter().flatten() {
                binary.entry(path.clone()).or_insert(lines.is_none());
            }
        }
        let candidates: Vec<String> =
            files.into_iter().filter(|f| !binary.get(f).copied().unwrap_or(false) && !self.shared.params.skips_survival(f)).collect();
        let budget = usize::try_from(self.shared.params.blame_files).unwrap_or(usize::MAX);
        let blamed: Vec<String> = candidates.iter().take(budget).cloned().collect();
        self.count("files_left", count(candidates.len().saturating_sub(blamed.len())));
        self.count("files_blamed", count(blamed.len()));

        // Blame them, attr_recon_blames_at_once at once.
        let at_once = usize::try_from(self.shared.params.blames_at_once).unwrap_or(usize::MAX).max(1);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(at_once));
        let mut set = tokio::task::JoinSet::new();
        for path in blamed {
            let (semaphore, dir, tip, with) = (Arc::clone(&semaphore), tip_dir.path.clone(), tip.to_string(), with.clone());
            let timeout = self.shared.params.git_timeout_ms;
            set.spawn(async move {
                let _permit = semaphore.acquire_owned().await;
                let mut git = Git::new(timeout);
                let call = git.run_with(&dir, &["blame", "--porcelain", &tip, "--", &path], &with).await;
                (git.tally(), call)
            });
        }
        let Some(results) = self.stop.clone().unless(async {
            let mut out = Vec::new();
            while let Some(done) = set.join_next().await {
                if let Ok(done) = done {
                    out.push(done);
                }
            }
            out
        })
        .await
        else {
            self.stopped = true;
            return Ok(false);
        };
        let mut credited: HashMap<String, i64> = HashMap::new();
        for (tally, call) in results {
            self.git.add(tally);
            self.record("blame", &call);
            for line in call.ok().unwrap_or_default().lines() {
                let mut words = line.split(' ');
                let (Some(hash), Some(orig)) = (words.next(), words.next()) else { continue };
                if is_full_hash(hash) && orig.parse::<u64>().is_ok() {
                    *credited.entry(hash.to_string()).or_default() += 1;
                    self.count("blame_lines_read", 1);
                }
            }
        }

        let rows: Vec<Value> = db
            .query("SELECT commit, alive, valid_from FROM survival WHERE commit.repository = $r")
            .bind(("r", self.job.repository.clone()))
            .await?
            .take(0)?;
        let mut newest: HashMap<Uuid, (DateTime<Utc>, i64)> = HashMap::new();
        for r in &rows {
            let (Some(c), Some(t), Some(alive)) = (record(r, "commit").as_ref().and_then(record_uuid_of), time(r, "valid_from"), int(r, "alive")) else {
                continue;
            };
            if newest.get(&c).is_none_or(|(seen, _)| *seen < t) {
                newest.insert(c, (t, alive));
            }
        }
        let mut hashes: Vec<&String> = held.keys().collect();
        hashes.sort();
        for hash in hashes {
            let Some(commit) = held.get(hash) else { continue };
            let Some(key) = record_uuid_of(&commit.id) else { continue };
            let before = newest.get(&key).map(|(_, a)| *a);
            let alive = match credited.get(hash) {
                Some(n) => (*n).min(commit.added),
                None if before.is_some_and(|a| a > 0) => 0,
                None => continue,
            };
            if before == Some(alive) {
                continue;
            }
            self.rows.push("survival", Row::new().link("commit", &commit.id).link("crawl", &self.crawl.id).int("alive", alive));
            self.count("survival_changed", 1);
            self.count("survival_delta", alive - before.unwrap_or(0));
        }
        memory.blamed_tip = Some(tip.to_string());
        Ok(true)
    }

    // ── step 13 ─────────────────────────────────────────────────────────

    /// Commits tied to the sessions that printed them or put them through a
    /// pull request.
    async fn ties(&mut self, db: &Db, held: &HashMap<String, Held>, named: &[RecordId], kept: &[(Uuid, Known, Vec<RecordId>)]) -> Result<()> {
        let rows: Vec<Value> = db
            .query("SELECT commit, session, via FROM commit_session WHERE commit.repository = $r")
            .bind(("r", self.job.repository.clone()))
            .await?
            .take(0)?;
        let mut seen: HashSet<(Uuid, Uuid, String)> = rows
            .iter()
            .filter_map(|r| {
                Some((record(r, "commit").as_ref().and_then(record_uuid_of)?, record(r, "session").as_ref().and_then(record_uuid_of)?, text(r, &["via"])?.to_string()))
            })
            .collect();
        let ours = |dir: Option<&String>| dir.and_then(|d| self.shared.holder(d)).is_some_and(|r| *r == self.job.repository);
        let mut ties: Vec<Tie> = Vec::new();

        // printed: the one commit a printed hash begins.
        for e in &self.shared.events {
            let (true, Some(hash), Some(session)) = (PRINTED.contains(&e.action.as_str()), e.hash.as_ref(), e.session.as_ref()) else { continue };
            if !ours(e.dir.as_ref()) {
                continue;
            }
            let mut matching = held.iter().filter(|(h, _)| h.starts_with(hash.as_str()));
            let (Some((_, commit)), None) = (matching.next(), matching.next()) else { continue };
            ties.push(Tie { commit: commit.id.clone(), session: session.clone(), tool_call: e.tool_call.clone(), via: "printed", pull_request: None, at: e.at });
        }

        // pull_request: the commits a session's pull request listed, and its
        // merge commit.
        let mut pulls: Vec<(RecordId, i64, String, Vec<String>)> = Vec::new();
        for hr in named {
            let known = match kept.iter().find(|(_, k, _)| k.host_repository == *hr) {
                Some((_, k, _)) => k.clone(),
                None => Known::load(db, hr).await?,
            };
            for (number, (id, url)) in &known.pulls {
                let Some(key) = record_uuid_of(id) else { continue };
                let mut hashes: Vec<String> = known.pull_commits.get(&key).map(|c| c.keys().cloned().collect()).unwrap_or_default();
                if let Some(Value::String(merge)) = known.pull_states.get(&key).and_then(|s| s.fields.iter().find(|(n, _)| *n == "merge_sha").map(|(_, v)| v)) {
                    hashes.push(merge.clone());
                }
                pulls.push((id.clone(), *number, url.clone(), hashes));
            }
        }
        for e in &self.shared.events {
            let (true, Some(session)) = (PULL_ACTIONS.contains(&e.action.as_str()), e.session.as_ref()) else { continue };
            let pull = match &e.url {
                Some(url) => pulls.iter().find(|(_, _, u, _)| u == url),
                None if ours(e.dir.as_ref()) => e.pr_number.and_then(|n| pulls.iter().find(|(_, number, _, _)| *number == n)),
                None => None,
            };
            let Some((pr, _, _, hashes)) = pull else { continue };
            for (hash, commit) in held {
                if hashes.iter().any(|h| hash.starts_with(h.as_str())) {
                    ties.push(Tie {
                        commit: commit.id.clone(),
                        session: session.clone(),
                        tool_call: e.tool_call.clone(),
                        via: "pull_request",
                        pull_request: Some(pr.clone()),
                        at: e.at,
                    });
                }
            }
        }
        for tie in ties {
            let (Some(c), Some(s)) = (record_uuid_of(&tie.commit), record_uuid_of(&tie.session)) else { continue };
            if !seen.insert((c, s, tie.via.to_string())) {
                continue;
            }
            self.rows.push(
                "commit_session",
                Row::new()
                    .link("commit", &tie.commit)
                    .link("session", &tie.session)
                    .link("tool_call", &tie.tool_call)
                    .text("via", tie.via)
                    .some_link("pull_request", tie.pull_request.as_ref())
                    .time("at", tie.at),
            );
        }
        Ok(())
    }
}

/// The newest repository object the database keeps of a host repository.
async fn newest_repository(db: &Db, hr: &RecordId) -> Result<Option<serde_json::Value>> {
    let rows: Vec<Value> = db
        .query("SELECT object, valid_from FROM host_object WHERE host_repository = $hr AND kind = 'repository'")
        .bind(("hr", hr.clone()))
        .await?
        .take(0)?;
    Ok(rows.iter().filter_map(|r| Some((time(r, "valid_from")?, at(r, &["object"])?))).max_by_key(|(t, _)| *t).map(|(_, o)| json_of(o)))
}

/// Where a git directory's refs are told apart from another's.
fn place_of(dir: &Dir) -> String {
    dir.path.to_string_lossy().into_owned()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

fn lines_of(call: &Call) -> HashSet<String> {
    call.ok().unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// A commit id as git gives it whole: 40 or 64 hex digits.
fn is_full_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn u64_count(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn record(v: &Value, key: &str) -> Option<RecordId> {
    match at(v, &[key])? {
        Value::RecordId(id) => Some(id.clone()),
        _ => None,
    }
}

fn time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match at(v, &[key])? {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}

fn int(v: &Value, key: &str) -> Option<i64> {
    match at(v, &[key])? {
        Value::Number(n) => n.to_int(),
        _ => None,
    }
}

