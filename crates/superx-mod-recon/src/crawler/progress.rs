//! What the crawler is doing this second (#539, the tab's Now): kept in the
//! running process's memory, never written, read by the tab every two
//! seconds through recon's own URL. A pass starts it; its end keeps it as
//! the last pass, so the tab shows what ran while nothing runs.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// A repository crawl's steps, in the order `repos::crawl::run` takes them.
pub const STEPS: [&str; 11] =
    ["setup", "hosts", "refs", "commits", "reflogs", "placements", "landed", "survival", "host crawls", "sessions", "writing"];

/// The pass running, the last one that ended, and the last one that crawled
/// repositories: most passes only read the lines captured since the one
/// before, so the repositories are seen as the last crawl left them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Now {
    pub pass: Option<PassNow>,
    pub last: Option<PassNow>,
    pub last_crawl: Option<PassNow>,
}

/// One pass: its phase (lines, repositories, recording, done) and every
/// repository it crawls.
#[derive(Debug, Clone, Serialize)]
pub struct PassNow {
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub phase: &'static str,
    pub outcome: Option<&'static str>,
    pub rows: Option<i64>,
    pub lines_read: Option<i64>,
    pub lines_held: Option<i64>,
    pub repos: Vec<RepoNow>,
}

/// One repository in a pass: waiting, running or done; the step it is in of
/// [`STEPS`], since when, and what it has done so far.
#[derive(Debug, Clone, Serialize)]
pub struct RepoNow {
    pub name: String,
    pub common_dir: String,
    pub state: &'static str,
    pub step: &'static str,
    /// 1 for the first of [`STEPS`]; 0 while waiting.
    pub step_index: usize,
    pub steps: usize,
    pub step_since: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub git_calls: i64,
    pub git_failed: i64,
    pub blamed: i64,
    pub to_blame: i64,
    pub outcome: Option<String>,
    pub hosts: Vec<HostNow>,
}

/// One host repository crawled beside a repository.
#[derive(Debug, Clone, Serialize)]
pub struct HostNow {
    pub name: String,
    pub calls: i64,
    pub failed: i64,
    pub waits: i64,
    /// The path asked last.
    pub asking: Option<String>,
    pub done: bool,
}

/// The handle every part of the crawler reports to; a clone shares it.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<Mutex<Now>>);

impl Progress {
    fn now(&self) -> MutexGuard<'_, Now> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What it is doing, as it stands.
    #[must_use]
    pub fn snapshot(&self) -> Now {
        self.now().clone()
    }

    pub fn pass_started(&self, at: DateTime<Utc>) {
        self.now().pass = Some(PassNow {
            started_at: at,
            finished_at: None,
            phase: "lines",
            outcome: None,
            rows: None,
            lines_read: None,
            lines_held: None,
            repos: Vec::new(),
        });
    }

    /// The lines read; the repositories next.
    pub fn lines_read(&self, read: i64, held: i64) {
        if let Some(p) = self.now().pass.as_mut() {
            (p.lines_read, p.lines_held, p.phase) = (Some(read), Some(held), "repositories");
        }
    }

    /// The repositories the pass crawls, by name and common directory, each
    /// waiting its turn.
    pub fn queued(&self, repos: impl IntoIterator<Item = (String, String)>) {
        if let Some(p) = self.now().pass.as_mut() {
            p.repos = repos
                .into_iter()
                .map(|(name, common_dir)| RepoNow {
                    name,
                    common_dir,
                    state: "waiting",
                    step: "waiting",
                    step_index: 0,
                    steps: STEPS.len(),
                    step_since: None,
                    started_at: None,
                    finished_at: None,
                    git_calls: 0,
                    git_failed: 0,
                    blamed: 0,
                    to_blame: 0,
                    outcome: None,
                    hosts: Vec::new(),
                })
                .collect();
        }
    }

    fn repo(&self, common_dir: &str, change: impl FnOnce(&mut RepoNow)) {
        if let Some(r) = self.now().pass.as_mut().and_then(|p| p.repos.iter_mut().find(|r| r.common_dir == common_dir)) {
            change(r);
        }
    }

    /// A repository's crawl entered `step`, one of [`STEPS`].
    pub fn step(&self, common_dir: &str, step: &'static str) {
        let at = Utc::now();
        self.repo(common_dir, |r| {
            r.state = "running";
            r.started_at.get_or_insert(at);
            r.step = step;
            r.step_index = STEPS.iter().position(|s| *s == step).map_or(0, |i| i + 1);
            r.step_since = Some(at);
        });
    }

    pub fn git_call(&self, common_dir: &str, failed: bool) {
        self.repo(common_dir, |r| {
            r.git_calls += 1;
            r.git_failed += i64::from(failed);
        });
    }

    /// Files blamed so far of those to blame.
    pub fn blames(&self, common_dir: &str, blamed: i64, to_blame: i64) {
        self.repo(common_dir, |r| (r.blamed, r.to_blame) = (blamed, to_blame));
    }

    pub fn repo_done(&self, common_dir: &str, outcome: &str) {
        let at = Utc::now();
        self.repo(common_dir, |r| {
            (r.state, r.step, r.step_index, r.step_since, r.finished_at) = ("done", "done", STEPS.len(), Some(at), Some(at));
            r.outcome = Some(outcome.to_string());
        });
    }

    fn host(&self, common_dir: &str, host: &str, change: impl FnOnce(&mut HostNow)) {
        self.repo(common_dir, |r| {
            if let Some(h) = r.hosts.iter_mut().find(|h| h.name == host) {
                change(h);
            }
        });
    }

    pub fn host_started(&self, common_dir: &str, host: &str) {
        self.repo(common_dir, |r| {
            if !r.hosts.iter().any(|h| h.name == host) {
                r.hosts.push(HostNow { name: host.to_string(), calls: 0, failed: 0, waits: 0, asking: None, done: false });
            }
        });
    }

    pub fn host_call(&self, common_dir: &str, host: &str, path: &str, failed: bool) {
        self.host(common_dir, host, |h| {
            h.calls += 1;
            h.failed += i64::from(failed);
            h.asking = Some(path.to_string());
        });
    }

    pub fn host_wait(&self, common_dir: &str, host: &str) {
        self.host(common_dir, host, |h| h.waits += 1);
    }

    pub fn host_done(&self, common_dir: &str, host: &str) {
        self.host(common_dir, host, |h| (h.done, h.asking) = (true, None));
    }

    /// Every source done; the pass is being written.
    pub fn recording(&self) {
        if let Some(p) = self.now().pass.as_mut() {
            p.phase = "recording";
        }
    }

    /// The pass ended: kept as the last one, and as the last crawl when it
    /// crawled repositories.
    pub fn pass_ended(&self, outcome: &'static str, rows: i64, at: DateTime<Utc>) {
        let mut now = self.now();
        if let Some(mut p) = now.pass.take() {
            (p.finished_at, p.outcome, p.rows, p.phase) = (Some(at), Some(outcome), Some(rows), "done");
            if !p.repos.is_empty() {
                now.last_crawl = Some(p.clone());
            }
            now.last = Some(p);
        }
    }
}

/// What a part of the crawler reports as: the handle, the repository by its
/// common directory, and, for a host crawl, the host repository.
#[derive(Debug, Clone)]
pub struct Watch {
    pub progress: Progress,
    pub common_dir: String,
    pub host: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_shows_each_repository_waiting_running_in_its_step_and_done_and_is_kept_as_the_last_once_it_ends() {
        let p = Progress::default();
        let at = Utc::now();
        p.pass_started(at);
        p.lines_read(12, 12);
        p.queued([("a".to_string(), "/a/.git".to_string()), ("b".to_string(), "/b/.git".to_string())]);
        p.step("/a/.git", "commits");
        p.git_call("/a/.git", false);
        p.git_call("/a/.git", true);
        p.blames("/a/.git", 3, 10);
        p.host_started("/a/.git", "github.com/o/a");
        p.host_call("/a/.git", "github.com/o/a", "/repos/o/a/pulls", false);
        p.host_wait("/a/.git", "github.com/o/a");
        let now = p.snapshot().pass.expect("running");
        assert_eq!((now.phase, now.lines_read, now.lines_held), ("repositories", Some(12), Some(12)));
        let a = &now.repos[0];
        assert_eq!((a.state, a.step, a.step_index, a.steps, a.git_calls, a.git_failed, a.blamed, a.to_blame), ("running", "commits", 4, 11, 2, 1, 3, 10));
        assert_eq!((a.hosts[0].calls, a.hosts[0].waits, a.hosts[0].asking.as_deref()), (1, 1, Some("/repos/o/a/pulls")));
        assert_eq!((now.repos[1].state, now.repos[1].step_index), ("waiting", 0));

        p.host_done("/a/.git", "github.com/o/a");
        p.repo_done("/a/.git", "partial");
        p.recording();
        assert_eq!(p.snapshot().pass.expect("running").phase, "recording");
        p.pass_ended("partial", 40, at);
        let now = p.snapshot();
        assert!(now.pass.is_none(), "nothing runs");
        let last = now.last.expect("the last pass");
        assert_eq!((last.phase, last.outcome, last.rows), ("done", Some("partial"), Some(40)));
        assert_eq!((last.repos[0].state, last.repos[0].outcome.as_deref(), last.repos[0].hosts[0].done), ("done", Some("partial"), true));
        assert_eq!(now.last_crawl.as_ref().map(|c| c.started_at), Some(at), "it crawled repositories");

        // A pass that only read lines is the last pass; the last crawl stays.
        let later = Utc::now();
        p.pass_started(later);
        p.lines_read(3, 3);
        p.queued(Vec::<(String, String)>::new());
        p.pass_ended("complete", 3, later);
        let now = p.snapshot();
        assert_eq!(now.last.map(|l| l.repos.len()), Some(0));
        assert_eq!(now.last_crawl.map(|c| (c.started_at, c.repos.len())), Some((at, 2)), "the repositories as the last crawl left them");
    }

    #[test]
    fn a_report_for_a_repository_or_host_the_pass_does_not_hold_changes_nothing() {
        let p = Progress::default();
        p.step("/none/.git", "refs");
        p.git_call("/none/.git", false);
        assert!(p.snapshot().pass.is_none());
        p.pass_started(Utc::now());
        p.queued([("a".to_string(), "/a/.git".to_string())]);
        p.host_call("/a/.git", "github.com/o/none", "/x", false);
        assert!(p.snapshot().pass.expect("running").repos[0].hosts.is_empty());
    }
}
