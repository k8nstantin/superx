//! One host repository's crawl (#538, steps 15 to 21): its token, its
//! first call and every other call its host's step names, the rows its
//! answers make, and its `host_crawl` row; and the hosts source's own
//! record over a pass's host crawls.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use crate::credentials::{Credentials, Token};
use crate::crawler::hosts::call::{path_of, Calls, Client, Counts, Ended, Got};
use crate::crawler::hosts::ci::{self, COMMIT_STATUS};
use crate::crawler::hosts::held::{self, Asked, HostMemory, Known};
use crate::crawler::hosts::objects::{accounts, account_id, held as held_of, key_value, Accounts, From, Held, Shape, Taken};
use crate::crawler::hosts::pulls::{self, Page};
use crate::crawler::hosts::{bitbucket, github, Host, HostPath, SOURCE};
use crate::crawler::progress::Watch;
use crate::crawler::store::{reserve, Batch, Row};
use crate::crawler::{Outcome, SourceRun, Stop};

/// What the hosts part shares across a pass's workers.
pub struct Shared {
    pub db: Db,
    pub client: std::result::Result<Client, String>,
    /// The credentials file, read afresh at each pass (step 4).
    pub credentials: Credentials,
    /// The host repositories and mirrors the database held when the pass
    /// began.
    pub known: Known,
    /// The host repositories taken in this pass, each by the first worker
    /// to reach it.
    pub taken: std::sync::Mutex<BTreeSet<HostPath>>,
    /// The accounts, shared by every worker.
    pub accounts: Arc<tokio::sync::Mutex<Accounts>>,
    /// What the crawler holds of each host repository, from pass to pass.
    pub memories: Arc<std::sync::Mutex<HashMap<HostPath, HostMemory>>>,
}

impl Shared {
    /// Take a host repository for this worker: true for the first worker
    /// to reach it in the pass.
    #[must_use]
    pub fn take(&self, hp: &HostPath) -> bool {
        self.taken.lock().map(|mut t| t.insert(hp.clone())).unwrap_or(false)
    }

    /// The token the credentials file gives a host, or why it gives none.
    fn token(&self, host: Host) -> std::result::Result<Token, String> {
        match &self.credentials {
            Credentials::None => Err("none".to_string()),
            Credentials::Refused { mode } => Err(format!("refused, mode {mode:03o}")),
            Credentials::Unreadable(why) => Err(format!("unreadable: {why}")),
            Credentials::Hosts(hosts) => {
                let token = match host {
                    Host::GitHub => hosts.github.clone(),
                    Host::Bitbucket => hosts.bitbucket.clone(),
                };
                token.ok_or_else(|| "no token".to_string())
            }
        }
    }
}

/// What a host crawl reads beside its repository's crawl: the main line's
/// tip and its first-parent history, newest first.
#[derive(Debug, Clone, Default)]
pub struct Beside {
    pub tip: Option<String>,
    pub first_parent: Vec<String>,
}

/// How a host crawl stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Asking.
    Asking,
    /// The credentials file gives the host no token: nothing asked.
    NoCredentials,
    /// The host refused the repository: nothing more asked.
    Unreadable,
}

/// One host repository's crawl.
pub struct HostCrawl {
    pub hp: HostPath,
    /// Its `host_repository` row: held, or made by this crawl.
    pub repository: RecordId,
    /// This crawl makes its `host_repository` row.
    pub new_repository: bool,
    /// Its own `host_crawl` row, written once its calls are done.
    pub id: RecordId,
    pub repo_crawl: RecordId,
    pub started_at: DateTime<Utc>,
    pub standing: Standing,
    /// Why nothing was asked, or what refused the repository.
    pub why: Option<String>,
    pub calls: Option<Calls>,
    /// Its rows after its own: calls, objects, pull requests, CI runs.
    pub rows: Batch,
    pub memory: HostMemory,
    /// The repository object this crawl read, when it read one.
    pub repository_object: Option<Json>,
    /// The host answered its repository: its mirror is made and fetched.
    pub answered: bool,
    pub objects_new: i64,
    pub objects_changed: i64,
    pub totals_stated: i64,
    pub totals_read: i64,
    pub differences: Vec<String>,
    /// The newest answer to each path this crawl asked.
    asked: Vec<(String, Asked)>,
    pub stopped: bool,
    /// The accounts, shared by every worker of the pass.
    pub accounts: Arc<tokio::sync::Mutex<Accounts>>,
}

/// One page of a list, or one object.
pub struct Paged {
    pub ended: Ended,
    /// Its answer's status, NONE when no answer came.
    pub status: Option<u16>,
    pub held: Option<Held>,
    pub call: Option<RecordId>,
    /// The URL of the page that follows.
    pub next: Option<String>,
}

/// A list, read to its end or to the page that stopped it.
#[derive(Default)]
pub struct Listed {
    /// Every object read on it, with its row when it is kept.
    pub objects: Vec<(Json, Option<Taken>, Option<RecordId>)>,
    /// Each page's own: its objects, or `None` for a page answered HTTP
    /// 304.
    pub pages: Vec<Option<Vec<Json>>>,
    /// Every page was answered HTTP 200.
    pub all_read: bool,
    /// Every page was answered HTTP 200 or HTTP 304.
    pub whole: bool,
    pub stopped: bool,
}

/// How objects of a list or a path are kept.
pub struct Keep<'k> {
    pub kind: &'static str,
    pub key: &'k (dyn Fn(&Json) -> Option<Value> + Sync),
}

impl HostCrawl {
    /// Start a host repository's crawl beside the crawl `repo_crawl`: its
    /// token, and, with one, its first call (steps 4, 17, 18); each call
    /// reported to `watch` as it happens.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer for
    /// what it holds of the host repository.
    pub async fn start(shared: &Shared, hp: HostPath, repo_crawl: RecordId, watch: Option<Watch>, stop: &mut Stop) -> Result<Self> {
        let (repository, new_repository) = match shared.known.repositories.get(&(hp.host, hp.path.clone())) {
            Some(id) => (id.clone(), false),
            None => (reserve("host_repository"), true),
        };
        let held_memory = shared.memories.lock().ok().and_then(|mut m| m.remove(&hp));
        let memory = match held_memory {
            Some(m) => m,
            None if new_repository => HostMemory::default(),
            None => held::load(&shared.db, &repository).await?,
        };
        let mut crawl = Self {
            repository,
            new_repository,
            id: reserve("host_crawl"),
            repo_crawl,
            started_at: Utc::now(),
            standing: Standing::Asking,
            why: None,
            calls: None,
            rows: Batch::default(),
            memory,
            repository_object: None,
            answered: false,
            objects_new: 0,
            objects_changed: 0,
            totals_stated: 0,
            totals_read: 0,
            differences: Vec::new(),
            asked: Vec::new(),
            stopped: false,
            accounts: Arc::clone(&shared.accounts),
            hp,
        };
        let token = match shared.token(crawl.hp.host) {
            Ok(token) => token,
            Err(why) => {
                crawl.standing = Standing::NoCredentials;
                crawl.why = Some(why);
                return Ok(crawl);
            }
        };
        let client = match &shared.client {
            Ok(client) => client.clone(),
            Err(why) => {
                crawl.differences.push(why.clone());
                return Ok(crawl);
            }
        };
        crawl.calls = Some(Calls { watch, ..Calls::new(client, crawl.hp.host, token, crawl.id.clone()) });
        let path = match crawl.hp.host {
            Host::GitHub => format!("/repos/{}", crawl.hp.path),
            Host::Bitbucket => format!("/repositories/{}", crawl.hp.path),
        };
        let host = crawl.hp.host;
        let keyed = move |o: &Json| -> Option<Value> {
            match host {
                Host::GitHub => o.get("id").and_then(key_value),
                Host::Bitbucket => o.get("uuid").and_then(key_value),
            }
        };
        let got = crawl.one(&path, &Keep { kind: "repository", key: &keyed }, true, stop).await;
        match (got.ended, got.status) {
            (Ended::Read, _) => {
                crawl.answered = true;
                crawl.repository_object = got.held.and_then(|h| h.objects.into_iter().next());
                crawl.memory.repository.clone_from(&crawl.repository_object);
            }
            (Ended::NotModified, _) => crawl.answered = true,
            (Ended::Failed, Some(status @ (401 | 403 | 404))) => {
                crawl.standing = Standing::Unreadable;
                crawl.why = Some(format!("{path}: HTTP {status}"));
            }
            (Ended::Stopped, _) => crawl.stopped = true,
            _ => {}
        }
        Ok(crawl)
    }

    /// The host repository's own id, from the repository object this crawl
    /// read: GitHub's id, Bitbucket's uuid.
    #[must_use]
    pub fn host_id(&self) -> Option<Value> {
        let o = self.repository_object.as_ref()?;
        match self.hp.host {
            Host::GitHub => o.get("id").and_then(key_value),
            Host::Bitbucket => o.get("uuid").and_then(key_value),
        }
    }

    /// The branch the newest repository object kept names as the
    /// repository's default: GitHub's `default_branch`, Bitbucket's
    /// `mainbranch.name`.
    #[must_use]
    pub fn default_branch(&self) -> Option<String> {
        let o = self.repository_object.as_ref().or(self.memory.repository.as_ref())?;
        let named = match self.hp.host {
            Host::GitHub => o.get("default_branch"),
            Host::Bitbucket => o.get("mainbranch").and_then(|m| m.get("name")),
        };
        named.and_then(Json::as_str).filter(|b| !b.is_empty()).map(str::to_string)
    }

    /// The rest of its calls, once its repository's crawl is done (steps 17
    /// to 20).
    pub async fn finish(&mut self, beside: &Beside, stop: &mut Stop) {
        if self.standing != Standing::Asking || self.calls.is_none() || self.stopped {
            return;
        }
        match self.hp.host {
            Host::GitHub => github::crawl(self, beside, stop).await,
            Host::Bitbucket => bitbucket::crawl(self, beside, stop).await,
        }
    }

    /// The API a path is asked of.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        self.calls.as_ref().map_or_else(String::new, |c| c.client.url(self.hp.host, path))
    }

    /// Ask one page at `url`: its etag sent back when the newest answer to
    /// it gave one in a host crawl that ended complete; its call recorded.
    pub async fn page(&mut self, url: &str, shape: Shape, first: bool, stop: &mut Stop) -> Paged {
        let Some(calls) = self.calls.as_mut() else {
            return Paged { ended: Ended::Failed, status: None, held: None, call: None, next: None };
        };
        let path = reqwest::Url::parse(url).map(|u| path_of(&u)).unwrap_or_else(|_| url.to_string());
        let before = self.memory.answers.get(&path).cloned();
        let etag = before.as_ref().filter(|a| a.complete).and_then(|a| a.etag.clone());
        let mut got: Got = calls.get(&mut self.rows, url, etag.as_deref(), first, stop).await;
        if got.ended == Ended::Stopped {
            self.stopped = true;
            return Paged { ended: Ended::Stopped, status: None, held: None, call: None, next: None };
        }
        let held = if got.ended == Ended::Read { held_of(&got.answer.body, shape) } else { None };
        let unread = got.ended == Ended::Read && held.is_none();
        let origin = reqwest::Url::parse(url).ok().map(|u| u.origin().ascii_serialization());
        let next = match (&got.ended, &held) {
            (Ended::Read, Some(h)) => match shape {
                Shape::Page => h.next.clone(),
                _ => got.answer.link_next.clone(),
            },
            // What the newest earlier answer named next.
            (Ended::NotModified, _) => before.as_ref().and_then(|a| a.next.clone()).and_then(|p| origin.as_ref().map(|o| format!("{o}{p}"))),
            _ => None,
        };
        let objects = held.as_ref().map_or(0, |h| h.objects.len());
        let call = calls.record(&mut self.rows, &mut got, objects, next.is_some(), unread);
        let next_path = next.as_deref().and_then(|n| reqwest::Url::parse(n).ok()).map(|u| path_of(&u));
        let asked = match got.ended {
            Ended::NotModified => Asked { etag: etag.clone(), complete: true, read: true, next: before.and_then(|a| a.next) },
            ended => Asked { etag: got.answer.etag.clone(), complete: true, read: ended == Ended::Read, next: next_path },
        };
        self.asked.push((path, asked));
        Paged { ended: got.ended, status: got.answer.status, held, call: Some(call), next }
    }

    /// Ask one path whose answer is one object, kept as `keep` says.
    pub async fn one(&mut self, path: &str, keep: &Keep<'_>, first: bool, stop: &mut Stop) -> One {
        let url = self.url(path);
        let paged = self.page(&url, Shape::Object, first, stop).await;
        let mut taken = None;
        if let (Some(h), Some(call)) = (&paged.held, &paged.call) {
            if let Some(object) = h.objects.first() {
                taken = self.keep(keep, object, call).await;
            }
        }
        One { ended: paged.ended, status: paged.status, held: paged.held, taken }
    }

    /// A list's first page as the crawler asks it: with the page size its
    /// host takes (step 15).
    #[must_use]
    pub fn first_page(&self, path: &str) -> String {
        let size = match self.hp.host {
            Host::GitHub => "per_page=100",
            Host::Bitbucket => "pagelen=50",
        };
        if path.contains('?') {
            format!("{path}&{size}")
        } else {
            format!("{path}?{size}")
        }
    }

    /// The key a path asked is kept under: its path and query on the host.
    fn key(&self, asked: &str) -> String {
        reqwest::Url::parse(&self.url(asked)).map(|u| path_of(&u)).unwrap_or_else(|_| asked.to_string())
    }

    /// Whether `asked`, a path and query as the crawler asks it, was read in
    /// a host crawl that ended complete: a path asked on a condition is
    /// asked again for itself when it was not, since a host's answer or a
    /// crawl not complete leaves it to the next host crawl (step 15).
    #[must_use]
    pub fn settled(&self, asked: &str) -> bool {
        self.memory.answers.get(&self.key(asked)).is_some_and(Asked::settled)
    }

    /// The segments `<s>` of every path `<prefix><s><suffix>`, query aside,
    /// asked at an earlier host crawl and not settled: what an earlier
    /// host crawl left to this one.
    #[must_use]
    pub fn unsettled(&self, prefix: &str, suffix: &str) -> BTreeSet<String> {
        let prefix = self.key(prefix);
        self.memory
            .answers
            .iter()
            .filter(|(_, asked)| !asked.settled())
            .filter_map(|(path, _)| {
                let bare = path.split_once('?').map_or(path.as_str(), |(p, _)| p);
                let segment = bare.strip_prefix(&prefix)?.strip_suffix(suffix)?;
                (!segment.is_empty() && !segment.contains('/')).then(|| segment.to_string())
            })
            .collect()
    }

    /// Ask a list from `path`, its first page with the page size its host
    /// takes, following each next page as given; its objects kept as `keep`
    /// says. A list's stated total is compared with what was read of it
    /// when every page was answered HTTP 200.
    pub async fn list(&mut self, path: &str, shape: Shape, keep: Option<&Keep<'_>>, stop: &mut Stop) -> Listed {
        let first = self.first_page(path);
        let mut url = Some(self.url(&first));
        let mut listed = Listed { all_read: true, whole: true, ..Listed::default() };
        let mut stated: Option<i64> = None;
        while let Some(u) = url.take() {
            let paged = self.page(&u, shape, false, stop).await;
            match paged.ended {
                Ended::Read => {
                    let Some(h) = paged.held else {
                        listed.all_read = false;
                        listed.whole = false;
                        break;
                    };
                    stated = stated.or(h.total);
                    for object in &h.objects {
                        let taken = match (keep, &paged.call) {
                            (Some(k), Some(call)) => self.keep(k, object, call).await,
                            _ => None,
                        };
                        listed.objects.push((object.clone(), taken, paged.call.clone()));
                    }
                    listed.pages.push(Some(h.objects));
                }
                Ended::NotModified => {
                    listed.all_read = false;
                    listed.pages.push(None);
                }
                Ended::Stopped => {
                    listed.stopped = true;
                    listed.all_read = false;
                    listed.whole = false;
                    break;
                }
                _ => {
                    listed.all_read = false;
                    listed.whole = false;
                    break;
                }
            }
            url = paged.next;
        }
        if let (true, Some(total)) = (listed.all_read, stated) {
            let read = i64::try_from(listed.objects.len()).unwrap_or(i64::MAX);
            self.totals_stated += total;
            self.totals_read += read;
            if read != total {
                self.differences.push(format!("{first}: {read} objects read of {total} stated"));
            }
        }
        listed
    }

    /// Keep one object, and every account it holds: the row that holds its
    /// content.
    pub async fn keep(&mut self, keep: &Keep<'_>, object: &Json, call: &RecordId) -> Option<Taken> {
        let key = (keep.key)(object)?;
        let repository = self.repository.clone();
        let from = From { host_repository: &repository, call, at: Utc::now() };
        let taken = self.memory.kept.take(&mut self.rows, &from, keep.kind, &key, object);
        self.count(&taken);
        let found = accounts(self.hp.host, object);
        if !found.is_empty() {
            let shared = Arc::clone(&self.accounts);
            let mut registry = shared.lock().await;
            for account in &found {
                if let Some(id) = account_id(self.hp.host, account) {
                    let user = self.memory.kept.take(&mut self.rows, &from, "user", &id, account);
                    self.count(&user);
                    registry.row(self.hp.host, account);
                }
            }
        }
        Some(taken)
    }

    fn count(&mut self, taken: &Taken) {
        if taken.written {
            if taken.changed {
                self.objects_changed += 1;
            } else {
                self.objects_new += 1;
            }
        }
    }

    /// The crawl's own row and every row after it, and what it adds to the
    /// hosts source's record. The memory goes back to the pass.
    #[must_use]
    pub fn done(mut self) -> Done {
        let counts = self.calls.as_ref().map(|c| c.counts).unwrap_or_default();
        let (failures, answers) = self.calls.as_ref().map(|c| (c.failures.clone(), c.answers.clone())).unwrap_or_default();
        let outcome = match self.standing {
            Standing::NoCredentials => "no_credentials",
            Standing::Unreadable => "unreadable",
            Standing::Asking if counts.failed > 0 || counts.timed_out > 0 || !self.differences.is_empty() => "partial",
            Standing::Asking => "complete",
        };
        let mut note: Vec<String> = Vec::new();
        if let Some(why) = &self.why {
            note.push(why.clone());
        }
        if self.standing == Standing::Asking {
            note.extend(failures);
            note.extend(answers);
            note.extend(self.differences.iter().cloned());
        }
        let complete = outcome == "complete";
        for (path, mut asked) in std::mem::take(&mut self.asked) {
            asked.complete = complete;
            self.memory.answers.insert(path, asked);
        }
        let finished_at = Utc::now();
        let mut rows = std::mem::take(&mut self.rows);
        let rows_written = i64::try_from(rows.waiting()).unwrap_or(i64::MAX) + 1 + i64::from(self.new_repository);
        rows.create_first(
            self.id.clone(),
            Row::new()
                .link("host_repository", &self.repository)
                .link("crawl", &self.repo_crawl)
                .time("started_at", self.started_at)
                .time("finished_at", finished_at)
                .int("calls", counts.calls)
                .int("not_modified", counts.not_modified)
                .int("failed", counts.failed)
                .int("rate_waits", counts.rate_waits)
                .int("waited_ms", counts.waited_ms)
                .int("objects_new", self.objects_new)
                .int("objects_changed", self.objects_changed)
                .int("rows_written", rows_written)
                .text("outcome", outcome)
                .some_text("note", (!note.is_empty()).then(|| note.join("; "))),
        );
        Done {
            hp: self.hp,
            rows,
            outcome,
            note: (!note.is_empty()).then(|| note.join("; ")),
            counts,
            objects_new: self.objects_new,
            objects_changed: self.objects_changed,
            totals_stated: self.totals_stated,
            totals_read: self.totals_read,
            memory: self.memory,
            stopped: self.stopped,
        }
    }
}

/// One reading of a pull request's commits whose every page was answered
/// HTTP 200 or HTTP 304: its pages, and the pull request's own count of its
/// commits where its host gives one (step 19).
pub struct Reading {
    pub number: i64,
    pub pages: Vec<Page>,
    pub stated: Option<i64>,
}

/// A list's pages, each the commits its objects name with their rows, or
/// unchanged.
#[must_use]
pub fn pages_of(listed: &Listed, hash_of: impl Fn(&Json) -> Option<String>) -> Vec<Page> {
    let mut objects = listed.objects.iter();
    listed
        .pages
        .iter()
        .map(|page| match page {
            None => Page::Unchanged,
            Some(on_page) => Page::Read(
                objects
                    .by_ref()
                    .take(on_page.len())
                    .filter_map(|(o, taken, _)| Some((hash_of(o)?, taken.as_ref()?.row.clone())))
                    .collect(),
            ),
        })
        .collect()
}

impl HostCrawl {
    /// The rows of the pull request objects read in this crawl (step 19),
    /// then those of each reading of their commits.
    pub async fn pull_rows(&mut self, read: &[(Json, RecordId)], readings: &[Reading]) {
        let host = self.hp.host;
        let accounts = Arc::clone(&self.accounts);
        let mut registry = accounts.lock().await;
        for (object, row) in read {
            let Some(number) = pulls::number(host, object) else { continue };
            let closed_actor = self.memory.closed.get(&number).map(|(_, actor)| actor.clone()).filter(|a| !a.is_null());
            let r = pulls::Read {
                host,
                path: &self.hp.path,
                host_repository: &self.repository,
                object: row,
                closed_actor: closed_actor.as_ref(),
            };
            let held = self.memory.pulls.get(&number).cloned();
            if let Some(pull) = pulls::read(&mut self.rows, &mut registry, held.as_ref(), object, &r) {
                self.memory.pulls.insert(number, pull);
            }
        }
        drop(registry);
        let size = match host {
            Host::GitHub => 100,
            Host::Bitbucket => 50,
        };
        for reading in readings {
            if let Some(pull) = self.memory.pulls.get_mut(&reading.number) {
                pulls::commits(&mut self.rows, pull, &reading.pages, size, reading.stated, host);
            }
        }
    }

    /// The CI rows of a list's objects of `kind` (step 20); `commit`, the
    /// commit a status was asked for.
    pub async fn ci_rows(&mut self, listed: &Listed, kind: &'static str, commit: Option<&str>) {
        let host = self.hp.host;
        let accounts = Arc::clone(&self.accounts);
        let mut registry = accounts.lock().await;
        for (object, taken, _) in &listed.objects {
            let Some(taken) = taken else { continue };
            let r = ci::Read { host, host_repository: &self.repository, kind, object: &taken.row, commit, pulls: &self.memory.pulls };
            ci::read(&mut self.rows, &mut registry, &mut self.memory.ci, object, &r);
        }
    }

    /// The commits whose statuses, or check suites and runs, a host crawl
    /// asks (steps 17, 18), each once, in this order: the main line's tip
    /// and the head of every open pull request, at every host crawl; every
    /// commit of the main line's first-parent history and every other
    /// commit a pull request has listed, while `asks` gives a path of it no
    /// host crawl settled; and the commits `pending` names.
    #[must_use]
    pub fn commits_to_ask(&self, beside: &Beside, asks: &dyn Fn(&str) -> Vec<String>, pending: &BTreeSet<String>) -> Vec<String> {
        let mut heads: Vec<(i64, String)> = self
            .memory
            .pulls
            .iter()
            .filter_map(|(number, p)| p.state.as_ref().filter(|s| s.open()).and_then(|s| s.head_sha.clone()).map(|h| (*number, h)))
            .collect();
        heads.sort();
        let mut listed: BTreeSet<String> = BTreeSet::new();
        for p in self.memory.pulls.values() {
            listed.extend(p.commits.keys().cloned());
        }
        let unsettled = |commit: &String| asks(commit).iter().any(|path| !self.settled(&self.first_page(path)));
        let every = beside.tip.iter().cloned().chain(heads.into_iter().map(|(_, h)| h));
        let first = beside.first_parent.iter().cloned().chain(listed).filter(unsettled);
        let mut seen = BTreeSet::new();
        every.chain(first).chain(pending.iter().cloned()).filter(|c| pulls::sha(c) && seen.insert(c.clone())).collect()
    }

    /// The commits a run this crawler holds is pending on (steps 17, 18):
    /// for a commit status, one whose context's newest status is pending
    /// (GitHub) or in progress (Bitbucket); for a check suite or run, one
    /// not completed.
    #[must_use]
    pub fn pending_commits(&self, kinds: &[&str]) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for kind in kinds {
            let of_kind = self.memory.ci.iter().filter(|((k, _), _)| k == kind);
            if *kind == COMMIT_STATUS && self.hp.host == Host::GitHub {
                // A context's statuses are objects of their own, each with a
                // new id: the newest is the context's state.
                // By commit and context: when, its id, and whether pending.
                type Newest = HashMap<(String, String), ((Option<DateTime<Utc>>, i64), bool)>;
                let mut newest: Newest = HashMap::new();
                for ((_, id), run) in of_kind {
                    let Some(commit) = run.commit_sha.clone() else { continue };
                    let order = (run.created_at, id.parse::<i64>().unwrap_or(0));
                    let k = (commit, run.name.clone().unwrap_or_default());
                    if newest.get(&k).is_none_or(|(seen, _)| *seen < order) {
                        newest.insert(k, (order, ci::pending(kind, run.state.as_ref())));
                    }
                }
                out.extend(newest.into_iter().filter(|(_, (_, pending))| *pending).map(|((commit, _), _)| commit));
            } else {
                out.extend(of_kind.filter(|(_, run)| ci::pending(kind, run.state.as_ref())).filter_map(|(_, run)| run.commit_sha.clone()));
            }
        }
        out
    }
}

/// One path's answer, its object kept.
pub struct One {
    pub ended: Ended,
    pub status: Option<u16>,
    pub held: Option<Held>,
    pub taken: Option<Taken>,
}

/// A host crawl, done.
pub struct Done {
    pub hp: HostPath,
    pub rows: Batch,
    pub outcome: &'static str,
    pub note: Option<String>,
    pub counts: Counts,
    pub objects_new: i64,
    pub objects_changed: i64,
    pub totals_stated: i64,
    pub totals_read: i64,
    pub memory: HostMemory,
    pub stopped: bool,
}

/// The hosts source's `pass_source` record over a pass's host crawls: its
/// items, the host repositories crawled; its calls; its rows; its counts;
/// partial when a host crawl is, an unreadable or uncredentialed host
/// named in its note and leaving it complete.
#[must_use]
pub fn record(done: &[Summary], rows: BTreeMap<&'static str, i64>, started: DateTime<Utc>) -> SourceRun {
    let mut run = SourceRun::start(SOURCE);
    run.started_at = started;
    run.items = i64::try_from(done.len()).unwrap_or(i64::MAX);
    let mut counts: BTreeMap<&'static str, i64> = BTreeMap::new();
    let mut partial = Vec::new();
    let mut named = Vec::new();
    for d in done {
        run.unread += d.counts.unread;
        run.calls += d.counts.calls;
        run.failed += d.counts.failed;
        run.timed_out += d.counts.timed_out;
        *counts.entry("objects_new").or_default() += d.objects_new;
        *counts.entry("objects_changed").or_default() += d.objects_changed;
        *counts.entry("not_modified").or_default() += d.counts.not_modified;
        *counts.entry("rate_waits").or_default() += d.counts.rate_waits;
        *counts.entry("waited_ms").or_default() += d.counts.waited_ms;
        *counts.entry("totals_stated").or_default() += d.totals_stated;
        *counts.entry("totals_read").or_default() += d.totals_read;
        let said = d.note.as_ref().map_or(String::new(), |n| format!(": {n}"));
        match d.outcome {
            "partial" => partial.push(format!("{} partial{said}", d.hp)),
            "unreadable" | "no_credentials" => named.push(format!("{} {}{said}", d.hp, d.outcome)),
            _ => {}
        }
    }
    for name in ["objects_new", "objects_changed", "not_modified", "rate_waits", "waited_ms", "totals_stated", "totals_read"] {
        counts.entry(name).or_default();
    }
    run.counts = counts;
    run.rows = rows;
    if !partial.is_empty() {
        run.outcome = Outcome::Partial;
    }
    let note: Vec<String> = partial.into_iter().chain(named).collect();
    run.note = (!note.is_empty()).then(|| note.join("; "));
    run.finished_at = Utc::now();
    run
}

/// What one host crawl adds to the hosts source's record.
#[derive(Debug, Clone)]
pub struct Summary {
    pub hp: HostPath,
    pub outcome: &'static str,
    pub note: Option<String>,
    pub counts: Counts,
    pub objects_new: i64,
    pub objects_changed: i64,
    pub totals_stated: i64,
    pub totals_read: i64,
}

impl Done {
    /// What it adds to the hosts source's record.
    #[must_use]
    pub fn summary(&self) -> Summary {
        Summary {
            hp: self.hp.clone(),
            outcome: self.outcome,
            note: self.note.clone(),
            counts: self.counts,
            objects_new: self.objects_new,
            objects_changed: self.objects_changed,
            totals_stated: self.totals_stated,
            totals_read: self.totals_read,
        }
    }
}

/// A `host_repository` row, made by the crawl that first reaches it.
#[must_use]
pub fn repository_row(hp: &HostPath, host_id: Option<Value>) -> Row {
    Row::new().text("host", hp.host.name()).text("path", hp.path.clone()).some_value("host_id", host_id).time("found_at", Utc::now())
}
