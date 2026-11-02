//! One host crawl (#538, steps 17 to 21): a host repository asked for
//! everything its host's read APIs answer, in the order the steps give,
//! each answer kept whole; its pull requests and CI runs typed beside the
//! objects they were read from; and a `host_crawl` row that says what it
//! did.
//!
//! It starts with the repository itself ([`HostCrawl::begin`]), the call
//! the mirror waits on, and finishes once the repository's own crawl has
//! read its main line ([`HostCrawl::finish`]), whose tip and first-parent
//! history say which commits' statuses to ask.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use superx_kernel::types::{Datetime, Number, RecordId, Value};
use superx_kernel::{Db, Result};

use crate::crawler::hosts::call::{Caller, Got, Shape, BITBUCKET_PAGE, GITHUB_PAGE};
use crate::crawler::hosts::objects::{self, Kept, Known, PullState, RunState};
use crate::crawler::hosts::{no_token_why, Crawled, Host, Key, Pass};
use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::store::{Place, Row, Rows};
use crate::crawler::Stop;

/// The columns of a pull request's state, in the structure's order.
pub const PULL_STATE_FIELDS: &[&str] = &[
    "state",
    "merged",
    "draft",
    "title",
    "head_sha",
    "base_sha",
    "merge_sha",
    "merged_at",
    "merged_by",
    "closed_at",
    "closed_by",
    "reason",
    "additions",
    "deletions",
    "changed_files",
    "commits",
    "comments",
    "review_comments",
    "updated_at",
    "object",
];

/// The columns of a CI run's state, in the structure's order.
pub const RUN_STATE_FIELDS: &[&str] =
    &["status", "conclusion", "started_at", "completed_at", "attempt", "number", "actor", "runner", "object"];

/// GitHub keeps no more of a pull request's commits in its list.
const GITHUB_PULL_COMMITS: i64 = 250; // skill-allow: §9-const — the host's own limit, not a policy tunable

/// What the repository's crawl tells its host crawl: the main line's tip
/// and its first-parent history.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub tip: Option<String>,
    pub first_parent: Vec<String>,
}

/// How a host crawl is ending.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Asking,
    NoCredentials(String),
    Unreadable(String),
}

/// Every object a list held, with the call that held it.
#[derive(Debug, Default)]
struct List {
    items: Vec<(serde_json::Value, RecordId)>,
    /// Every page was answered HTTP 200.
    whole: bool,
    /// The total the host stated beside the list.
    total: Option<i64>,
    /// Some page was answered unchanged.
    unchanged: bool,
}

/// One host repository crawled in one pass.
pub struct HostCrawl<'p> {
    pass: &'p Pass,
    host: Host,
    path: String,
    host_repository: RecordId,
    /// Its row, when this crawl is the first to reach it.
    made: Option<Place>,
    row: Place,
    crawl: RecordId,
    started_at: DateTime<Utc>,
    caller: Option<Caller<'p>>,
    pub known: Known,
    state: State,
    answered: bool,
    notes: Vec<String>,
    totals_stated: i64,
    totals_read: i64,
    objects_new: i64,
    objects_changed: i64,
    /// The accounts this crawl made rows for, and those it found named.
    users_made: HashSet<RecordId>,
    /// Rows this crawl made but its own.
    made_rows: i64,
    /// Pull requests asked in this crawl, by number.
    pulls_asked: BTreeSet<i64>,
}

impl<'p> HostCrawl<'p> {
    /// Start the host crawl of `path` on `host` beside the repository's
    /// crawl `crawl`: its rows' places, and, when the credentials file
    /// gives the host a token, the repository itself, the call the mirror
    /// waits on. `None` when a stop came first.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer.
    #[allow(clippy::too_many_arguments)] // skill-allow: allow — what a host crawl starts from, each its own
    pub async fn begin(
        pass: &'p Pass,
        rows: &mut Rows,
        db: &Db,
        crawl: &RecordId,
        host: Host,
        path: &str,
        known: Option<Known>,
        stop: &mut Stop,
    ) -> Result<Option<Self>> {
        let (host_repository, made) = match pass.repository(host, path) {
            Some(held) => (held.id, None),
            None => {
                let place = rows.hold("host_repository");
                pass.made(host, path, place.id.clone());
                (place.id.clone(), Some(place))
            }
        };
        let row = rows.hold("host_crawl");
        let known = match known {
            Some(k) => k,
            None if made.is_some() => Known::empty(&host_repository),
            None => Known::load(db, &host_repository).await?,
        };
        let token = host.token(&pass.credentials);
        let state = if token.is_none() { State::NoCredentials(no_token_why(&pass.credentials)) } else { State::Asking };
        let mut me = Self {
            pass,
            host,
            path: path.to_string(),
            host_repository,
            made,
            caller: token.map(|t| Caller::new(pass, host, t, row.id.clone())),
            row,
            crawl: crawl.clone(),
            started_at: Utc::now(),
            known,
            state,
            answered: false,
            notes: Vec::new(),
            totals_stated: 0,
            totals_read: 0,
            objects_new: 0,
            objects_changed: 0,
            users_made: HashSet::new(),
            made_rows: 0,
            pulls_asked: BTreeSet::new(),
        };
        if me.state != State::Asking {
            return Ok(Some(me));
        }
        // The repository itself: an answer HTTP 401, 403 or 404 makes the
        // host crawl unreadable, with nothing more asked.
        let (url, key) = match host {
            Host::GitHub => (format!("/repos/{path}"), "id"),
            Host::Bitbucket => (format!("/repositories/{path}"), "uuid"),
        };
        let Some(got) = me.get(rows, db, &url, Shape::Object, stop).await? else { return Ok(None) };
        if matches!(got.status, Some(401 | 403 | 404)) {
            me.state = State::Unreadable(format!("{url}: HTTP {}", got.status.unwrap_or_default()));
            return Ok(Some(me));
        }
        if got.ok {
            me.answered = true;
            me.keep_all(rows, "repository", &[(got.objects.first().cloned().unwrap_or_default(), got.call.clone())], &|o| o.get(key).and_then(Key::of));
        } else if got.not_modified {
            // Answered, and unchanged: what is kept of it stands.
            me.answered = true;
        } else {
            me.failed_path(&url, &got);
        }
        Ok(Some(me))
    }

    /// The repository on the host: whether it answered, so the mirror is
    /// made and fetched.
    #[must_use]
    pub fn answered(&self) -> bool {
        self.answered
    }

    #[must_use]
    pub fn host_repository(&self) -> &RecordId {
        &self.host_repository
    }

    /// The newest repository object kept: this crawl's, or the database's.
    #[must_use]
    pub fn repository_object(&self) -> Option<&serde_json::Value> {
        self.known.repository.as_ref().map(|(_, o)| o)
    }

    /// Ask the rest, in the order of step 17 or step 18, and write the
    /// host crawl's row. `None` when a stop came first.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer.
    pub async fn finish(mut self, rows: &mut Rows, db: &Db, context: &Context, stop: &mut Stop) -> Result<Option<(Crawled, Known, Vec<RecordId>)>> {
        if self.state == State::Asking {
            let went = match self.host {
                Host::GitHub => self.github(rows, db, context, stop).await?,
                Host::Bitbucket => self.bitbucket(rows, db, context, stop).await?,
            };
            if !went {
                return Ok(None);
            }
        }
        Ok(Some(self.end(rows)))
    }

    /// The host crawl's row, and what the source's record takes of it.
    fn end(mut self, rows: &mut Rows) -> (Crawled, Known, Vec<RecordId>) {
        let counts = self.caller.as_ref().map(|c| c.counts).unwrap_or_default();
        let short = self.totals_stated != self.totals_read;
        let (outcome, note) = match &self.state {
            State::NoCredentials(why) => ("no_credentials", Some(why.clone())),
            State::Unreadable(why) => ("unreadable", Some(why.clone())),
            State::Asking => {
                if short {
                    self.notes.push(format!("{} objects read of {} stated", self.totals_read, self.totals_stated));
                }
                let partial = counts.failed > 0 || counts.timed_out > 0 || short;
                (if partial { "partial" } else { "complete" }, (!self.notes.is_empty()).then(|| self.notes.join("; ")))
            }
        };
        let host_id = self.known.repository.as_ref().and_then(|(_, o)| match self.host {
            Host::GitHub => o.get("id").and_then(Key::of),
            Host::Bitbucket => o.get("uuid").and_then(Key::of),
        });
        if let Some(place) = &self.made {
            rows.fill(
                place,
                Row::new()
                    .text("host", self.host.name())
                    .text("path", self.path.clone())
                    .some_value("host_id", host_id.map(|k| k.value()))
                    .time("found_at", self.started_at),
            );
        }
        rows.fill(
            &self.row,
            Row::new()
                .link("host_repository", &self.host_repository)
                .link("crawl", &self.crawl)
                .time("started_at", self.started_at)
                .time("finished_at", Utc::now())
                .int("calls", counts.calls)
                .int("not_modified", counts.not_modified)
                .int("failed", counts.failed)
                .int("rate_waits", counts.rate_waits)
                .int("waited_ms", counts.waited_ms)
                .int("objects_new", self.objects_new)
                .int("objects_changed", self.objects_changed)
                .int("rows_written", self.made_rows + counts.calls + i64::from(self.made.is_some()))
                .text("outcome", outcome)
                .some_text("note", note.clone()),
        );
        let crawled = Crawled {
            host: Some(self.host),
            path: self.path.clone(),
            outcome,
            note,
            calls: counts.calls,
            failed: counts.failed,
            timed_out: counts.timed_out,
            unread: counts.unread,
            not_modified: counts.not_modified,
            rate_waits: counts.rate_waits,
            waited_ms: counts.waited_ms,
            objects_new: self.objects_new,
            objects_changed: self.objects_changed,
            totals_stated: self.totals_stated,
            totals_read: self.totals_read,
        };
        (crawled, self.known, self.users_made.into_iter().collect())
    }

    // ── calls ───────────────────────────────────────────────────────────

    async fn get(&mut self, rows: &mut Rows, db: &Db, path: &str, shape: Shape, stop: &mut Stop) -> Result<Option<Got>> {
        let Some(caller) = self.caller.as_mut() else { return Ok(None) };
        let url = caller.url(path);
        caller.get(db, rows, &url, shape, stop).await
    }

    /// Every page of a list, its first asked as the host takes most.
    async fn list(&mut self, rows: &mut Rows, db: &Db, path: &str, shape: Shape, stop: &mut Stop) -> Result<Option<List>> {
        let Some(caller) = self.caller.as_mut() else { return Ok(None) };
        let sep = if path.contains('?') { '&' } else { '?' };
        let first = match self.host {
            Host::GitHub => format!("{path}{sep}per_page={GITHUB_PAGE}"),
            Host::Bitbucket => format!("{path}{sep}pagelen={BITBUCKET_PAGE}"),
        };
        let mut url = caller.url(&first);
        let mut list = List { whole: true, ..List::default() };
        loop {
            let Some(caller) = self.caller.as_mut() else { return Ok(None) };
            let Some(got) = caller.get(db, rows, &url, shape, stop).await? else { return Ok(None) };
            if got.not_modified {
                list.unchanged = true;
                list.whole = false;
            } else if !got.ok {
                list.whole = false;
                self.failed_path(&url, &got);
                return Ok(Some(list));
            } else {
                list.total = got.total.or(list.total);
                list.items.extend(got.objects.iter().cloned().map(|o| (o, got.call.clone())));
            }
            match got.next {
                Some(next) => url = next,
                None => break,
            }
        }
        if let (true, Some(total)) = (list.whole, list.total) {
            self.totals_stated += total;
            self.totals_read += i64::try_from(list.items.len()).unwrap_or(i64::MAX);
        }
        Ok(Some(list))
    }

    fn failed_path(&mut self, url: &str, got: &Got) {
        let path = reqwest::Url::parse(url).map(|u| crate::crawler::hosts::call::path_of(&u)).unwrap_or_else(|_| url.to_string());
        let status = got.status.map_or_else(|| "no answer".to_string(), |s| format!("HTTP {s}"));
        self.notes.push(format!("{path}: {status}"));
    }

    // ── objects ─────────────────────────────────────────────────────────

    /// Keep each object under the key `key` gives it, and every account it
    /// holds; an object with no key is unread.
    fn keep_all(
        &mut self,
        rows: &mut Rows,
        kind: &str,
        items: &[(serde_json::Value, RecordId)],
        key: &dyn Fn(&serde_json::Value) -> Option<Key>,
    ) -> Vec<(Kept, serde_json::Value)> {
        let mut out = Vec::new();
        for (object, call) in items {
            let Some(k) = key(object) else {
                if let Some(c) = self.caller.as_mut() {
                    c.counts.unread += 1;
                }
                continue;
            };
            let kept = self.known.keep(rows, kind, &k, object, call);
            self.count(&kept);
            if kept.moved() {
                self.accounts(rows, object, call);
            }
            out.push((kept, object.clone()));
        }
        out
    }

    fn count(&mut self, kept: &Kept) {
        match kept {
            Kept::New(_) => {
                self.objects_new += 1;
                self.made_rows += 1;
            }
            Kept::Changed(_) => {
                self.objects_changed += 1;
                self.made_rows += 1;
            }
            Kept::Same(_) => {}
        }
    }

    /// Every account `object` holds: an object of the kind user, and the
    /// first time the host names it, a host_user row.
    fn accounts(&mut self, rows: &mut Rows, object: &serde_json::Value, call: &RecordId) {
        for account in objects::accounts(self.host, object) {
            let Some(key) = objects::account_key(self.host, account) else { continue };
            let kept = self.known.keep(rows, "user", &key, account, call);
            self.count(&kept);
            let id = self.pass.user(self.host, &key);
            if !self.pass.user_written(&id) && self.users_made.insert(id.clone()) {
                rows.push_as("host_user", id, objects::user_row(self.host, &key, account, Utc::now()));
                self.made_rows += 1;
            }
        }
    }

    /// The host_user row of an account an object names; its object was kept
    /// with the object that holds it.
    fn user_of(&mut self, rows: &mut Rows, account: Option<&serde_json::Value>) -> Option<RecordId> {
        let account = account.filter(|a| objects::is_account(self.host, a))?;
        let key = objects::account_key(self.host, account)?;
        let id = self.pass.user(self.host, &key);
        if !self.pass.user_written(&id) && self.users_made.insert(id.clone()) {
            rows.push_as("host_user", id.clone(), objects::user_row(self.host, &key, account, Utc::now()));
            self.made_rows += 1;
        }
        Some(id)
    }

    // ── GitHub, step 17 ─────────────────────────────────────────────────

    async fn github(&mut self, rows: &mut Rows, db: &Db, context: &Context, stop: &mut Stop) -> Result<bool> {
        let base = format!("/repos/{}", self.path);
        let id = |o: &serde_json::Value| o.get("id").and_then(Key::of);

        // Pull requests: the list says which to ask.
        let Some(pulls) = self.list(rows, db, &format!("{base}/pulls?state=all&sort=created&direction=asc"), Shape::Array, stop).await? else {
            return Ok(false);
        };
        for (pull, _) in &pulls.items {
            let (Some(number), Some(key)) = (pull.get("number").and_then(serde_json::Value::as_i64), id(pull)) else { continue };
            let listed = pull.get("updated_at").and_then(serde_json::Value::as_str).map(str::to_string);
            let kept = self.known.newest.get(&("pull_request".to_string(), key.clone())).and_then(|n| n.updated.clone());
            let held = self.known.newest.contains_key(&("pull_request".to_string(), key));
            if held && kept == listed {
                continue;
            }
            if !self.github_pull(rows, db, &base, number, stop).await? {
                return Ok(false);
            }
        }

        // Issues, and the timelines of those new or changed.
        let Some(issues) = self.list(rows, db, &format!("{base}/issues?state=all&sort=created&direction=asc"), Shape::Array, stop).await? else {
            return Ok(false);
        };
        let kept = self.keep_all(rows, "issue", &issues.items, &id);
        let moved: Vec<i64> = kept
            .iter()
            .filter(|(k, _)| k.moved())
            .filter_map(|(_, o)| o.get("number").and_then(serde_json::Value::as_i64))
            .filter(|n| !self.pulls_asked.contains(n))
            .collect();
        for number in moved {
            if self.timeline(rows, db, &base, number, stop).await?.is_none() {
                return Ok(false);
            }
        }

        for (path, kind) in [
            (format!("{base}/issues/comments?sort=created&direction=asc"), "issue_comment"),
            (format!("{base}/pulls/comments?sort=created&direction=asc"), "review_comment"),
            (format!("{base}/issues/events"), "issue_event"),
        ] {
            let Some(list) = self.list(rows, db, &path, Shape::Array, stop).await? else { return Ok(false) };
            self.keep_all(rows, kind, &list.items, &id);
        }

        // Actions: the runs, and the jobs of those new or changed.
        let Some(runs) = self.list(rows, db, &format!("{base}/actions/runs"), Shape::Wrapped("workflow_runs"), stop).await? else {
            return Ok(false);
        };
        let kept = self.keep_all(rows, "workflow_run", &runs.items, &id);
        for (k, run) in &kept {
            self.ci_run(rows, "workflow_run", run, k.id(), None);
        }
        for (k, run) in kept {
            if !k.moved() {
                continue;
            }
            let Some(run_id) = run.get("id").and_then(serde_json::Value::as_i64) else { continue };
            let Some(jobs) = self.list(rows, db, &format!("{base}/actions/runs/{run_id}/jobs?filter=all"), Shape::Wrapped("jobs"), stop).await? else {
                return Ok(false);
            };
            for (jk, job) in self.keep_all(rows, "workflow_job", &jobs.items, &id) {
                self.ci_run(rows, "workflow_job", &job, jk.id(), None);
            }
        }

        // Statuses, commit by commit.
        for sha in self.status_commits(db, context, "/commits/", &base).await? {
            let path = format!("{base}/commits/{sha}/statuses");
            let Some(list) = self.list(rows, db, &path, Shape::Array, stop).await? else { return Ok(false) };
            if list.whole || list.unchanged {
                self.known.statuses_asked.insert(sha.clone(), true);
            }
            for (k, status) in self.keep_all(rows, "commit_status", &list.items, &id) {
                self.ci_run(rows, "commit_status", &status, k.id(), Some(&sha));
            }
        }

        // Deployments and their statuses.
        let Some(deployments) = self.list(rows, db, &format!("{base}/deployments"), Shape::Array, stop).await? else {
            return Ok(false);
        };
        let kept = self.keep_all(rows, "deployment", &deployments.items, &id);
        for (_, deployment) in kept {
            let Some(d) = deployment.get("id").and_then(serde_json::Value::as_i64) else { continue };
            let Some(statuses) = self.list(rows, db, &format!("{base}/deployments/{d}/statuses"), Shape::Array, stop).await? else {
                return Ok(false);
            };
            self.keep_all(rows, "deployment_status", &statuses.items, &id);
        }

        let Some(environments) = self.list(rows, db, &format!("{base}/environments"), Shape::Wrapped("environments"), stop).await? else {
            return Ok(false);
        };
        self.keep_all(rows, "environment", &environments.items, &id);
        let Some(releases) = self.list(rows, db, &format!("{base}/releases"), Shape::Array, stop).await? else { return Ok(false) };
        self.keep_all(rows, "release", &releases.items, &id);

        // Rulesets: the list says which to ask.
        let Some(rulesets) = self.list(rows, db, &format!("{base}/rulesets"), Shape::Array, stop).await? else { return Ok(false) };
        for (ruleset, _) in &rulesets.items {
            let (Some(r), Some(key)) = (ruleset.get("id").and_then(serde_json::Value::as_i64), id(ruleset)) else { continue };
            let listed = ruleset.get("updated_at").and_then(serde_json::Value::as_str).map(str::to_string);
            let newest = self.known.newest.get(&("ruleset".to_string(), key));
            if newest.is_some_and(|n| n.updated == listed) {
                continue;
            }
            let path = format!("{base}/rulesets/{r}");
            let Some(got) = self.get(rows, db, &path, Shape::Object, stop).await? else { return Ok(false) };
            if got.ok {
                self.keep_all(rows, "ruleset", &[(got.objects.first().cloned().unwrap_or_default(), got.call.clone())], &id);
            } else if !got.not_modified {
                self.failed_path(&path, &got);
            }
        }

        // The default branch's protection: unprotected is no failure.
        let default = self.repository_object().and_then(|r| r.get("default_branch")).and_then(serde_json::Value::as_str).map(str::to_string);
        if let Some(branch) = default {
            let path = format!("{base}/branches/{branch}/protection");
            let Some(got) = self.get(rows, db, &path, Shape::Object, stop).await? else { return Ok(false) };
            if got.ok {
                let name = Key::Text(branch.clone());
                self.keep_all(rows, "branch_protection", &[(got.objects.first().cloned().unwrap_or_default(), got.call.clone())], &|_| Some(name.clone()));
            } else if got.status == Some(404) && got.body.contains("Branch not protected") {
                if let Some(c) = self.caller.as_mut() {
                    c.forgive();
                }
            } else if !got.not_modified {
                self.failed_path(&path, &got);
            }
        }
        Ok(true)
    }

    /// One GitHub pull request: itself, its commits, files, reviews and
    /// timeline; then its rows. False when a stop came first.
    async fn github_pull(&mut self, rows: &mut Rows, db: &Db, base: &str, number: i64, stop: &mut Stop) -> Result<bool> {
        self.pulls_asked.insert(number);
        let id = |o: &serde_json::Value| o.get("id").and_then(Key::of);
        let path = format!("{base}/pulls/{number}");
        let Some(got) = self.get(rows, db, &path, Shape::Object, stop).await? else { return Ok(false) };
        if !got.ok {
            if !got.not_modified {
                self.failed_path(&path, &got);
            }
            return Ok(true);
        }
        let Some(pull) = got.objects.first().cloned() else { return Ok(true) };
        let kept = self.keep_all(rows, "pull_request", &[(pull.clone(), got.call.clone())], &id);
        let Some((pull_kept, _)) = kept.into_iter().next() else { return Ok(true) };

        let Some(commits) = self.list(rows, db, &format!("{path}/commits"), Shape::Array, stop).await? else { return Ok(false) };
        let commit_kept = self.keep_all(rows, "pull_request_commit", &commits.items, &|o| o.get("sha").and_then(Key::of));
        let Some(files) = self.list(rows, db, &format!("{path}/files"), Shape::Array, stop).await? else { return Ok(false) };
        self.keep_all(rows, "pull_request_file", &files.items, &|o| {
            o.get("filename").and_then(serde_json::Value::as_str).map(|f| Key::Text(format!("{number}/{f}")))
        });
        let Some(reviews) = self.list(rows, db, &format!("{path}/reviews"), Shape::Array, stop).await? else { return Ok(false) };
        self.keep_all(rows, "review", &reviews.items, &id);
        let Some(timeline) = self.timeline(rows, db, base, number, stop).await? else { return Ok(false) };

        // Closed by: the actor of the newest closed event of the timeline as
        // kept; an unchanged page keeps what the newest state says.
        let closed = timeline
            .iter()
            .filter(|(_, e)| e.get("event").and_then(serde_json::Value::as_str) == Some("closed"))
            .max_by_key(|(_, e)| e.get("created_at").and_then(serde_json::Value::as_str).map(str::to_string))
            .map(|(call, e)| (e.get("actor").cloned(), call.clone()));
        let pr = self.pull_row(rows, &pull, pull_kept.id());
        let closed_by = match closed {
            Some((actor, _)) => self.user_of(rows, actor.as_ref()),
            None => None,
        };
        let state = self.github_state(rows, &pull, pull_kept.id(), closed_by, &pr);
        self.pull_state(rows, &pr, state);
        if commits.whole {
            let listed: Vec<(String, RecordId)> = commit_kept
                .iter()
                .filter_map(|(k, o)| Some((o.get("sha")?.as_str()?.to_string(), k.id().clone())))
                .collect();
            let count = pull.get("commits").and_then(serde_json::Value::as_i64);
            let capped = count.is_some_and(|c| c > i64::try_from(listed.len()).unwrap_or(i64::MAX) || c > GITHUB_PULL_COMMITS);
            self.pull_commits(rows, &pr, &listed, !capped);
        }
        Ok(true)
    }

    /// A GitHub issue's timeline, kept; the events read, each with its call.
    /// `None` when a stop came first.
    async fn timeline(&mut self, rows: &mut Rows, db: &Db, base: &str, number: i64, stop: &mut Stop) -> Result<Option<Vec<(RecordId, serde_json::Value)>>> {
        let Some(list) = self.list(rows, db, &format!("{base}/issues/{number}/timeline"), Shape::Array, stop).await? else {
            return Ok(None);
        };
        let kept = self.keep_all(rows, "timeline_event", &list.items, &|o| {
            o.get("id").and_then(Key::of).or_else(|| o.get("sha").and_then(Key::of)).or_else(|| Some(Key::Text(objects::hash(o))))
        });
        Ok(Some(kept.into_iter().zip(list.items.iter()).map(|((_, o), (_, call))| (call.clone(), o)).collect()))
    }

    fn github_state(
        &mut self,
        rows: &mut Rows,
        pull: &serde_json::Value,
        object: &RecordId,
        closed_by: Option<RecordId>,
        pr: &Option<RecordId>,
    ) -> Vec<(&'static str, Value)> {
        let merged = json_bool(pull, &["merged"]).unwrap_or(false);
        let merged_by = self.user_of(rows, pull.get("merged_by"));
        // With nothing read of the timeline, what the newest state says.
        let closed_by = closed_by.or_else(|| {
            let pr = pr.as_ref().and_then(record_uuid_of)?;
            self.known.pull_states.get(&pr).and_then(|s| field(&s.fields, "closed_by")).and_then(|v| match v {
                Value::RecordId(id) => Some(id.clone()),
                _ => None,
            })
        });
        vec![
            ("state", text_value(pull, &["state"])),
            ("merged", Value::Bool(merged)),
            ("draft", bool_value(pull, &["draft"])),
            ("title", text_value(pull, &["title"])),
            ("head_sha", text_value(pull, &["head", "sha"])),
            ("base_sha", text_value(pull, &["base", "sha"])),
            ("merge_sha", if merged { text_value(pull, &["merge_commit_sha"]) } else { Value::None }),
            ("merged_at", time_value(pull, &["merged_at"])),
            ("merged_by", link_value(merged_by.as_ref())),
            ("closed_at", time_value(pull, &["closed_at"])),
            ("closed_by", link_value(closed_by.as_ref())),
            ("reason", Value::None),
            ("additions", int_value(pull, &["additions"])),
            ("deletions", int_value(pull, &["deletions"])),
            ("changed_files", int_value(pull, &["changed_files"])),
            ("commits", int_value(pull, &["commits"])),
            ("comments", int_value(pull, &["comments"])),
            ("review_comments", int_value(pull, &["review_comments"])),
            ("updated_at", time_value(pull, &["updated_at"])),
            ("object", Value::RecordId(object.clone())),
        ]
    }

    // ── Bitbucket, step 18 ──────────────────────────────────────────────

    async fn bitbucket(&mut self, rows: &mut Rows, db: &Db, context: &Context, stop: &mut Stop) -> Result<bool> {
        let base = format!("/repositories/{}", self.path);
        let id = |o: &serde_json::Value| o.get("id").and_then(Key::of);
        let uuid = |o: &serde_json::Value| o.get("uuid").and_then(Key::of);

        let Some(pulls) = self
            .list(rows, db, &format!("{base}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on"), Shape::Page, stop)
            .await?
        else {
            return Ok(false);
        };
        for (pull, _) in &pulls.items {
            let (Some(number), Some(key)) = (pull.get("id").and_then(serde_json::Value::as_i64), id(pull)) else { continue };
            let listed = pull.get("updated_on").and_then(serde_json::Value::as_str).map(str::to_string);
            let kept = self.known.newest.get(&("pull_request".to_string(), key.clone())).and_then(|n| n.updated.clone());
            let held = self.known.newest.contains_key(&("pull_request".to_string(), key));
            if held && kept == listed {
                continue;
            }
            if !self.bitbucket_pull(rows, db, &base, number, stop).await? {
                return Ok(false);
            }
        }

        for hash in self.status_commits(db, context, "/commit/", &base).await? {
            let path = format!("{base}/commit/{hash}/statuses");
            let Some(list) = self.list(rows, db, &path, Shape::Page, stop).await? else { return Ok(false) };
            if list.whole || list.unchanged {
                self.known.statuses_asked.insert(hash.clone(), true);
            }
            let keyed = |o: &serde_json::Value| o.get("key").and_then(serde_json::Value::as_str).map(|k| Key::Text(format!("{hash}/{k}")));
            for (k, status) in self.keep_all(rows, "commit_status", &list.items, &keyed) {
                self.ci_run(rows, "commit_status", &status, k.id(), Some(&hash));
            }
        }

        let Some(pipelines) = self.list(rows, db, &format!("{base}/pipelines?sort=-created_on"), Shape::Page, stop).await? else {
            return Ok(false);
        };
        let kept = self.keep_all(rows, "pipeline", &pipelines.items, &uuid);
        for (k, pipeline) in &kept {
            self.ci_run(rows, "pipeline", pipeline, k.id(), None);
        }
        for (k, pipeline) in kept {
            if !k.moved() {
                continue;
            }
            let Some(u) = pipeline.get("uuid").and_then(serde_json::Value::as_str) else { continue };
            let encoded = u.replace('{', "%7B").replace('}', "%7D");
            let Some(steps) = self.list(rows, db, &format!("{base}/pipelines/{encoded}/steps"), Shape::Page, stop).await? else {
                return Ok(false);
            };
            for (sk, step) in self.keep_all(rows, "pipeline_step", &steps.items, &uuid) {
                self.ci_step(rows, &step, sk.id(), &pipeline);
            }
        }

        for (path, kind, key) in [
            (format!("{base}/deployments"), "deployment", "uuid"),
            (format!("{base}/environments"), "environment", "uuid"),
            (format!("{base}/branch-restrictions"), "branch_restriction", "id"),
            (format!("{base}/default-reviewers"), "default_reviewer", "uuid"),
        ] {
            let Some(list) = self.list(rows, db, &path, Shape::Page, stop).await? else { return Ok(false) };
            self.keep_all(rows, kind, &list.items, &|o| o.get(key).and_then(Key::of));
        }
        Ok(true)
    }

    /// One Bitbucket pull request: itself, its commits, diffstat, comments,
    /// activity and tasks; then its rows. False when a stop came first.
    async fn bitbucket_pull(&mut self, rows: &mut Rows, db: &Db, base: &str, number: i64, stop: &mut Stop) -> Result<bool> {
        self.pulls_asked.insert(number);
        let id = |o: &serde_json::Value| o.get("id").and_then(Key::of);
        let path = format!("{base}/pullrequests/{number}");
        let Some(got) = self.get(rows, db, &path, Shape::Object, stop).await? else { return Ok(false) };
        if !got.ok {
            if !got.not_modified {
                self.failed_path(&path, &got);
            }
            return Ok(true);
        }
        let Some(pull) = got.objects.first().cloned() else { return Ok(true) };
        let kept = self.keep_all(rows, "pull_request", &[(pull.clone(), got.call.clone())], &id);
        let Some((pull_kept, _)) = kept.into_iter().next() else { return Ok(true) };

        let Some(commits) = self.list(rows, db, &format!("{path}/commits"), Shape::Page, stop).await? else { return Ok(false) };
        let commit_kept = self.keep_all(rows, "pull_request_commit", &commits.items, &|o| o.get("hash").and_then(Key::of));
        let Some(diffstat) = self.list(rows, db, &format!("{path}/diffstat"), Shape::Page, stop).await? else { return Ok(false) };
        self.keep_all(rows, "pull_request_file", &diffstat.items, &|o| {
            let new = o.get("new").and_then(|n| n.get("path")).and_then(serde_json::Value::as_str);
            let old = o.get("old").and_then(|n| n.get("path")).and_then(serde_json::Value::as_str);
            new.or(old).map(|p| Key::Text(format!("{number}/{p}")))
        });
        let Some(comments) = self.list(rows, db, &format!("{path}/comments"), Shape::Page, stop).await? else { return Ok(false) };
        self.keep_all(rows, "pull_request_comment", &comments.items, &id);
        let Some(activity) = self.list(rows, db, &format!("{path}/activity"), Shape::Page, stop).await? else { return Ok(false) };
        self.keep_all(rows, "pull_request_activity", &activity.items, &|o| Some(Key::Text(objects::hash(o))));
        let Some(tasks) = self.list(rows, db, &format!("{path}/tasks"), Shape::Page, stop).await? else { return Ok(false) };
        self.keep_all(rows, "pull_request_task", &tasks.items, &id);

        let pr = self.pull_row(rows, &pull, pull_kept.id());
        let state_name = json_str(&pull, &["state"]);
        let merged = state_name.as_deref() == Some("MERGED");
        let closed_by = self.user_of(rows, pull.get("closed_by"));
        let state = vec![
            ("state", text_value(&pull, &["state"])),
            ("merged", Value::Bool(merged)),
            ("draft", bool_value(&pull, &["draft"])),
            ("title", text_value(&pull, &["title"])),
            ("head_sha", text_value(&pull, &["source", "commit", "hash"])),
            ("base_sha", text_value(&pull, &["destination", "commit", "hash"])),
            ("merge_sha", text_value(&pull, &["merge_commit", "hash"])),
            ("merged_at", Value::None),
            ("merged_by", if merged { link_value(closed_by.as_ref()) } else { Value::None }),
            ("closed_at", Value::None),
            ("closed_by", link_value(closed_by.as_ref())),
            ("reason", text_value(&pull, &["reason"])),
            ("additions", Value::None),
            ("deletions", Value::None),
            ("changed_files", Value::None),
            ("commits", Value::None),
            ("comments", int_value(&pull, &["comment_count"])),
            ("review_comments", Value::None),
            ("updated_at", time_value(&pull, &["updated_on"])),
            ("object", Value::RecordId(pull_kept.id().clone())),
        ];
        self.pull_state(rows, &pr, state);
        if commits.whole {
            let listed: Vec<(String, RecordId)> = commit_kept
                .iter()
                .filter_map(|(k, o)| Some((o.get("hash")?.as_str()?.to_string(), k.id().clone())))
                .collect();
            self.pull_commits(rows, &pr, &listed, true);
        }
        Ok(true)
    }

    // ── statuses ────────────────────────────────────────────────────────

    /// The commits whose statuses this crawl asks (steps 17 and 18): the
    /// head of every open pull request and the main line's tip, at every
    /// host crawl; every other commit a pull request has listed and every
    /// commit of the main line's first-parent history, at the first host
    /// crawl that knows it; and again while a status it holds is pending.
    async fn status_commits(&mut self, db: &Db, context: &Context, segment: &str, base: &str) -> Result<Vec<String>> {
        let mut every: BTreeSet<String> = BTreeSet::new();
        for state in self.known.pull_states.values() {
            let open = matches!(field(&state.fields, "state"), Some(Value::String(s)) if s == "open" || s == "OPEN");
            if let (true, Some(Value::String(head))) = (open, field(&state.fields, "head_sha")) {
                every.insert(head.clone());
            }
        }
        if let Some(tip) = &context.tip {
            every.insert(tip.clone());
        }
        let mut once: BTreeSet<String> = self.known.pull_commits.values().flat_map(|c| c.keys().cloned()).collect();
        once.extend(context.first_parent.iter().cloned());
        let pending: BTreeSet<String> = self
            .known
            .newest
            .iter()
            .filter(|((kind, _), n)| kind == "commit_status" && matches!(n.state.as_deref(), Some("pending" | "INPROGRESS")))
            .filter_map(|((_, key), n)| status_commit(key, n.commit.as_deref()))
            .collect();
        let mut out: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for sha in every.iter().chain(pending.iter()) {
            if seen.insert(sha.clone()) {
                out.push(sha.clone());
            }
        }
        for sha in once {
            if seen.contains(&sha) {
                continue;
            }
            if !self.asked_before(db, segment, base, &sha).await? {
                seen.insert(sha.clone());
                out.push(sha);
            }
        }
        Ok(out)
    }

    /// Whether an earlier call asked `sha`'s statuses and was answered.
    async fn asked_before(&mut self, db: &Db, segment: &str, base: &str, sha: &str) -> Result<bool> {
        if let Some(asked) = self.known.statuses_asked.get(sha) {
            return Ok(*asked);
        }
        let page = match self.host {
            Host::GitHub => format!("per_page={GITHUB_PAGE}"),
            Host::Bitbucket => format!("pagelen={BITBUCKET_PAGE}"),
        };
        let path = match self.host {
            Host::GitHub => format!("{base}{segment}{sha}/statuses?{page}"),
            Host::Bitbucket => {
                let api = reqwest::Url::parse(self.pass.endpoints.api(self.host)).map(|u| u.path().trim_end_matches('/').to_string()).unwrap_or_default();
                format!("{api}{base}{segment}{sha}/statuses?{page}")
            }
        };
        let rows: Vec<Value> = db.query("SELECT VALUE status FROM host_call WHERE path = $path").bind(("path", path)).await?.take(0)?;
        let asked = rows.iter().any(|s| matches!(s, Value::Number(n) if matches!(n.to_int(), Some(200 | 304))));
        self.known.statuses_asked.insert(sha.to_string(), asked);
        Ok(asked)
    }

    // ── step 19 ─────────────────────────────────────────────────────────

    /// A pull request's row, the first time: none when its object lacks
    /// what the row must hold, which is then unread.
    fn pull_row(&mut self, rows: &mut Rows, pull: &serde_json::Value, object: &RecordId) -> Option<RecordId> {
        let number = match self.host {
            Host::GitHub => pull.get("number"),
            Host::Bitbucket => pull.get("id"),
        }
        .and_then(serde_json::Value::as_i64)?;
        if let Some((id, _)) = self.known.pulls.get(&number) {
            return Some(id.clone());
        }
        let (author, created, source, target, source_repo, url) = match self.host {
            Host::GitHub => (
                pull.get("user"),
                json_time(pull, &["created_at"]),
                json_str(pull, &["head", "ref"]),
                json_str(pull, &["base", "ref"]),
                json_str(pull, &["head", "repo", "full_name"]),
                json_str(pull, &["html_url"]),
            ),
            Host::Bitbucket => (
                pull.get("author"),
                json_time(pull, &["created_on"]),
                json_str(pull, &["source", "branch", "name"]),
                json_str(pull, &["destination", "branch", "name"]),
                json_str(pull, &["source", "repository", "full_name"]),
                json_str(pull, &["links", "html", "href"]),
            ),
        };
        let host_id = pull.get("id").and_then(Key::of);
        let (Some(created), Some(source), Some(target), Some(url), Some(host_id)) = (created, source, target, url, host_id) else {
            if let Some(c) = self.caller.as_mut() {
                c.counts.unread += 1;
            }
            return None;
        };
        let author = author.cloned();
        let author = self.user_of(rows, author.as_ref());
        let source_path = source_repo.map(|p| p.to_lowercase()).filter(|p| *p != self.path);
        let id = rows.push(
            "pull_request",
            Row::new()
                .link("host_repository", &self.host_repository)
                .int("number", number)
                .value("host_id", host_id.value())
                .some_link("author", author.as_ref())
                .time("created_at", created)
                .text("source_branch", source)
                .text("target_branch", target)
                .some_text("source_path", source_path)
                .text("url", url.clone())
                .link("object", object),
        );
        self.made_rows += 1;
        self.known.pulls.insert(number, (id.clone(), url));
        Some(id)
    }

    /// A pull request's state row, when one of its columns differs from the
    /// newest.
    fn pull_state(&mut self, rows: &mut Rows, pr: &Option<RecordId>, fields: Vec<(&'static str, Value)>) {
        let Some(pr) = pr else { return };
        let Some(key) = record_uuid_of(pr) else { return };
        if self.known.pull_states.get(&key).is_some_and(|s| s.fields == fields) {
            return;
        }
        if !matches!(field(&fields, "title"), Some(Value::String(_))) || !matches!(field(&fields, "updated_at"), Some(Value::Datetime(_))) {
            if let Some(c) = self.caller.as_mut() {
                c.counts.unread += 1;
            }
            return;
        }
        let mut row = Row::new().link("pull_request", pr);
        for (name, value) in &fields {
            if !matches!(value, Value::None) {
                row = row.value(name, value.clone());
            }
        }
        rows.push("pull_request_state", row);
        self.made_rows += 1;
        self.known.pull_states.insert(key, PullState { fields });
    }

    /// The commits a pull request lists, against those it listed before.
    fn pull_commits(&mut self, rows: &mut Rows, pr: &Option<RecordId>, listed: &[(String, RecordId)], mark_absent: bool) {
        let Some(pr) = pr else { return };
        let Some(key) = record_uuid_of(pr) else { return };
        let mut held = self.known.pull_commits.remove(&key).unwrap_or_default();
        let now: HashSet<&str> = listed.iter().map(|(h, _)| h.as_str()).collect();
        for (position, (hash, object)) in listed.iter().enumerate() {
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            let present = held.get(hash).map(|(p, _)| *p);
            if present != Some(true) {
                rows.push(
                    "pull_request_commit",
                    Row::new()
                        .link("pull_request", pr)
                        .text("hash", hash.clone())
                        .int("position", position)
                        .flag("present", true)
                        .link("object", object),
                );
                self.made_rows += 1;
                held.insert(hash.clone(), (true, Some(position)));
            }
        }
        if mark_absent {
            let gone: Vec<String> = held.iter().filter(|(h, (p, _))| *p && !now.contains(h.as_str())).map(|(h, _)| h.clone()).collect();
            for hash in gone {
                rows.push("pull_request_commit", Row::new().link("pull_request", pr).text("hash", hash.clone()).flag("present", false));
                self.made_rows += 1;
                held.insert(hash, (false, None));
            }
        }
        self.known.pull_commits.insert(key, held);
    }

    // ── step 20 ─────────────────────────────────────────────────────────

    /// The ci_run of a workflow run, a workflow job or a commit status, and
    /// its state.
    fn ci_run(&mut self, rows: &mut Rows, kind: &str, o: &serde_json::Value, object: &RecordId, asked: Option<&str>) {
        let (host_id, columns, state) = match (self.host, kind) {
            (Host::GitHub, "workflow_run") => {
                let pull = o
                    .get("pull_requests")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.get("number").and_then(serde_json::Value::as_i64))
                    .find_map(|n| self.known.pulls.get(&n).map(|(id, _)| id.clone()));
                let actor = self.user_of(rows, o.get("triggering_actor"));
                (
                    o.get("id").and_then(Key::of),
                    RunColumns {
                        name: json_str(o, &["name"]),
                        commit_sha: json_str(o, &["head_sha"]),
                        reference: json_str(o, &["head_branch"]),
                        parent: None,
                        pull_request: pull,
                        url: json_str(o, &["html_url"]),
                        created_at: json_time(o, &["created_at"]),
                    },
                    vec![
                        ("status", text_value(o, &["status"])),
                        ("conclusion", text_value(o, &["conclusion"])),
                        ("started_at", time_value(o, &["run_started_at"])),
                        ("completed_at", Value::None),
                        ("attempt", int_value(o, &["run_attempt"])),
                        ("number", int_value(o, &["run_number"])),
                        ("actor", link_value(actor.as_ref())),
                        ("runner", Value::None),
                    ],
                )
            }
            (Host::GitHub, "workflow_job") => {
                let parent = o.get("run_id").and_then(Key::of).and_then(|k| self.known.runs.get(&("workflow_run".to_string(), k)).cloned());
                (
                    o.get("id").and_then(Key::of),
                    RunColumns {
                        name: json_str(o, &["name"]),
                        commit_sha: json_str(o, &["head_sha"]),
                        reference: json_str(o, &["head_branch"]),
                        parent,
                        pull_request: None,
                        url: json_str(o, &["html_url"]),
                        created_at: json_time(o, &["created_at"]),
                    },
                    vec![
                        ("status", text_value(o, &["status"])),
                        ("conclusion", text_value(o, &["conclusion"])),
                        ("started_at", time_value(o, &["started_at"])),
                        ("completed_at", time_value(o, &["completed_at"])),
                        ("attempt", int_value(o, &["run_attempt"])),
                        ("number", Value::None),
                        ("actor", Value::None),
                        ("runner", text_value(o, &["runner_name"])),
                    ],
                )
            }
            (Host::GitHub, _) => (
                o.get("id").and_then(Key::of),
                RunColumns {
                    name: json_str(o, &["context"]),
                    commit_sha: asked.map(str::to_string),
                    reference: None,
                    parent: None,
                    pull_request: None,
                    url: json_str(o, &["target_url"]),
                    created_at: json_time(o, &["created_at"]),
                },
                status_state(o),
            ),
            (Host::Bitbucket, "pipeline") => {
                let pull = o
                    .get("target")
                    .and_then(|t| t.get("pullrequest"))
                    .and_then(|p| p.get("id"))
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|n| self.known.pulls.get(&n).map(|(id, _)| id.clone()));
                let actor = self.user_of(rows, o.get("creator"));
                (
                    o.get("uuid").and_then(Key::of),
                    RunColumns {
                        name: json_str(o, &["target", "selector", "pattern"]),
                        commit_sha: json_str(o, &["target", "commit", "hash"]),
                        reference: json_str(o, &["target", "ref_name"]),
                        parent: None,
                        pull_request: pull,
                        url: None,
                        created_at: json_time(o, &["created_on"]),
                    },
                    vec![
                        ("status", text_value(o, &["state", "name"])),
                        ("conclusion", text_value(o, &["state", "result", "name"])),
                        ("started_at", Value::None),
                        ("completed_at", time_value(o, &["completed_on"])),
                        ("attempt", Value::None),
                        ("number", int_value(o, &["build_number"])),
                        ("actor", link_value(actor.as_ref())),
                        ("runner", Value::None),
                    ],
                )
            }
            (Host::Bitbucket, _) => (
                asked.and_then(|h| o.get("key").and_then(serde_json::Value::as_str).map(|k| Key::Text(format!("{h}/{k}")))),
                RunColumns {
                    name: json_str(o, &["name"]),
                    commit_sha: asked.map(str::to_string),
                    reference: json_str(o, &["refname"]),
                    parent: None,
                    pull_request: None,
                    url: json_str(o, &["url"]),
                    created_at: json_time(o, &["created_on"]),
                },
                status_state(o),
            ),
        };
        self.run_rows(rows, kind, host_id, columns, state, object);
    }

    /// A Bitbucket pipeline step's ci_run and state.
    fn ci_step(&mut self, rows: &mut Rows, o: &serde_json::Value, object: &RecordId, pipeline: &serde_json::Value) {
        let parent = pipeline.get("uuid").and_then(Key::of).and_then(|k| self.known.runs.get(&("pipeline".to_string(), k)).cloned());
        let columns = RunColumns {
            name: json_str(o, &["name"]),
            commit_sha: json_str(pipeline, &["target", "commit", "hash"]),
            reference: json_str(pipeline, &["target", "ref_name"]),
            parent,
            pull_request: None,
            url: None,
            created_at: None,
        };
        let state = vec![
            ("status", text_value(o, &["state", "name"])),
            ("conclusion", text_value(o, &["state", "result", "name"])),
            ("started_at", time_value(o, &["started_on"])),
            ("completed_at", time_value(o, &["completed_on"])),
            ("attempt", Value::None),
            ("number", Value::None),
            ("actor", Value::None),
            ("runner", Value::None),
        ];
        self.run_rows(rows, "pipeline_step", o.get("uuid").and_then(Key::of), columns, state, object);
    }

    fn run_rows(
        &mut self,
        rows: &mut Rows,
        kind: &str,
        host_id: Option<Key>,
        columns: RunColumns,
        mut state: Vec<(&'static str, Value)>,
        object: &RecordId,
    ) {
        let Some(host_id) = host_id else {
            if let Some(c) = self.caller.as_mut() {
                c.counts.unread += 1;
            }
            return;
        };
        let run = match self.known.runs.get(&(kind.to_string(), host_id.clone())) {
            Some(id) => id.clone(),
            None => {
                let id = rows.push(
                    "ci_run",
                    Row::new()
                        .link("host_repository", &self.host_repository)
                        .text("kind", kind.to_string())
                        .value("host_id", host_id.value())
                        .some_text("name", columns.name)
                        .some_text("commit_sha", columns.commit_sha)
                        .some_text("ref", columns.reference)
                        .some_link("parent", columns.parent.as_ref())
                        .some_link("pull_request", columns.pull_request.as_ref())
                        .some_text("url", columns.url)
                        .some_time("created_at", columns.created_at)
                        .link("object", object),
                );
                self.made_rows += 1;
                self.known.runs.insert((kind.to_string(), host_id), id.clone());
                id
            }
        };
        state.push(("object", Value::RecordId(object.clone())));
        let Some(key) = record_uuid_of(&run) else { return };
        if self.known.run_states.get(&key).is_some_and(|s| s.fields == state) {
            return;
        }
        let mut row = Row::new().link("ci_run", &run);
        for (name, value) in &state {
            if !matches!(value, Value::None) {
                row = row.value(name, value.clone());
            }
        }
        rows.push("ci_run_state", row);
        self.made_rows += 1;
        self.known.run_states.insert(key, RunState { fields: state });
    }
}

/// A ci_run's columns, those that differ by kind.
struct RunColumns {
    name: Option<String>,
    commit_sha: Option<String>,
    reference: Option<String>,
    parent: Option<RecordId>,
    pull_request: Option<RecordId>,
    url: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

/// A commit status's state: its state as the status, no conclusion.
fn status_state(o: &serde_json::Value) -> Vec<(&'static str, Value)> {
    vec![
        ("status", text_value(o, &["state"])),
        ("conclusion", Value::None),
        ("started_at", Value::None),
        ("completed_at", Value::None),
        ("attempt", Value::None),
        ("number", Value::None),
        ("actor", Value::None),
        ("runner", Value::None),
    ]
}

/// The commit a status is of: Bitbucket's key's first part, GitHub's url's
/// last segment.
fn status_commit(key: &Key, url: Option<&str>) -> Option<String> {
    match key {
        Key::Text(k) if k.contains('/') => k.split_once('/').map(|(h, _)| h.to_string()),
        _ => url.and_then(|u| u.rsplit('/').next()).map(str::to_string),
    }
}

fn field<'a>(fields: &'a [(&'static str, Value)], name: &str) -> Option<&'a Value> {
    fields.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
}

fn json_at<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut here = v;
    for key in path {
        here = here.get(key)?;
    }
    Some(here)
}

fn json_str(v: &serde_json::Value, path: &[&str]) -> Option<String> {
    json_at(v, path).and_then(serde_json::Value::as_str).map(str::to_string)
}

fn json_bool(v: &serde_json::Value, path: &[&str]) -> Option<bool> {
    json_at(v, path).and_then(serde_json::Value::as_bool)
}

fn json_time(v: &serde_json::Value, path: &[&str]) -> Option<DateTime<Utc>> {
    json_str(v, path).and_then(|t| DateTime::parse_from_rfc3339(&t).ok()).map(|t| t.with_timezone(&Utc))
}

fn text_value(v: &serde_json::Value, path: &[&str]) -> Value {
    json_str(v, path).map_or(Value::None, Value::String)
}

fn bool_value(v: &serde_json::Value, path: &[&str]) -> Value {
    json_bool(v, path).map_or(Value::None, Value::Bool)
}

fn int_value(v: &serde_json::Value, path: &[&str]) -> Value {
    json_at(v, path).and_then(serde_json::Value::as_i64).map_or(Value::None, |i| Value::Number(Number::Int(i)))
}

fn time_value(v: &serde_json::Value, path: &[&str]) -> Value {
    json_time(v, path).map_or(Value::None, |t| Value::Datetime(Datetime::from(t)))
}

fn link_value(id: Option<&RecordId>) -> Value {
    id.map_or(Value::None, |i| Value::RecordId(i.clone()))
}

/// The hosts source's own record (step 21): what its host crawls did.
#[must_use]
pub fn record(crawled: &[Crawled], rows: std::collections::BTreeMap<&'static str, i64>) -> crate::crawler::SourceRun {
    let mut run = crate::crawler::SourceRun::start(crate::crawler::hosts::SOURCE);
    run.items = i64::try_from(crawled.len()).unwrap_or(i64::MAX);
    let mut counts: std::collections::BTreeMap<&'static str, i64> = std::collections::BTreeMap::new();
    let mut partial = Vec::new();
    let mut named = Vec::new();
    for c in crawled {
        run.unread += c.unread;
        run.calls += c.calls;
        run.failed += c.failed;
        run.timed_out += c.timed_out;
        for (name, n) in [
            ("objects_new", c.objects_new),
            ("objects_changed", c.objects_changed),
            ("not_modified", c.not_modified),
            ("rate_waits", c.rate_waits),
            ("waited_ms", c.waited_ms),
            ("totals_stated", c.totals_stated),
            ("totals_read", c.totals_read),
        ] {
            *counts.entry(name).or_default() += n;
        }
        let host = c.host.map_or("", Host::name);
        let said = c.note.as_ref().map_or(String::new(), |n| format!(": {n}"));
        match c.outcome {
            "partial" => partial.push(format!("{host}/{} partial{said}", c.path)),
            "unreadable" | "no_credentials" => named.push(format!("{host}/{} {}{said}", c.path, c.outcome)),
            _ => {}
        }
    }
    for name in ["objects_new", "objects_changed", "not_modified", "rate_waits", "waited_ms", "totals_stated", "totals_read"] {
        counts.entry(name).or_default();
    }
    run.counts = counts;
    run.rows = rows;
    if !partial.is_empty() {
        run.outcome = crate::crawler::Outcome::Partial;
    }
    let notes: Vec<String> = partial.into_iter().chain(named).collect();
    run.note = (!notes.is_empty()).then(|| notes.join("; "));
    run.finished_at = Utc::now();
    run
}

/// Of a worker's rows by table, those of the hosts' tables.
#[must_use]
pub fn host_tables(table: &str) -> bool {
    matches!(
        table,
        "host_repository"
            | "host_object"
            | "host_user"
            | "host_crawl"
            | "host_call"
            | "mirror"
            | "pull_request"
            | "pull_request_state"
            | "pull_request_commit"
            | "ci_run"
            | "ci_run_state"
    )
}
