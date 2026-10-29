//! Git, read-only, the way the crawler runs it: one `git -C <dir>` process
//! per call, never a shell.
//!
//! Nothing a repository names is run (#447): signature checks and the
//! fsmonitor hook are overridden on every call, diff drivers are off where
//! a subcommand takes them, and no optional lock is taken, so no index
//! refresh is written back (git(1), `GIT_OPTIONAL_LOCKS`). A call that
//! outlasts `attr_recon_git_timeout_ms` is killed, never left behind.
//!
//! What each call did is kept: when it started, how long it took, its exit
//! code, what it printed and what it wrote to its error output. The crawler
//! counts every call, and records it where its schema has a place for it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::process::Command;

/// Configuration under which a read would run a program the repository
/// names, overridden on every call (#447): `log.showSignature` hands each
/// signed commit to the repository's `gpg.program`, and `core.fsmonitor`
/// names a hook.
const NO_REPOSITORY_PROGRAMS: [&str; 4] = ["-c", "log.showSignature=false", "-c", "core.fsmonitor=false"];

/// The flags that keep a subcommand off the repository's diff drivers
/// (#447): `blame` runs a textconv filter by default, `log` and `diff` run
/// one when they print a patch, and an external diff would run too. Only
/// these subcommands take the flags; the others refuse them.
fn no_diff_drivers(subcommand: &str) -> &'static [&'static str] {
    match subcommand {
        "blame" => &["--no-textconv"],
        "log" | "diff" => &["--no-textconv", "--no-ext-diff"],
        _ => &[],
    }
}

/// One git call, as it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The directory git ran in.
    pub dir: PathBuf,
    /// Every argument after `git -C <dir>`, exactly as run: the overrides,
    /// the subcommand and its flags.
    pub args: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ms: u64,
    /// Its exit code; `None` when it was cut off, could not start, or was
    /// ended by a signal.
    pub exit: Option<i32>,
    /// Cut off at its bound and killed.
    pub timed_out: bool,
    pub stdout: String,
    /// What git wrote to its error output, whole; when git could not
    /// start, why.
    pub stderr: String,
}

impl Call {
    /// What it printed, when it succeeded.
    #[must_use]
    pub fn ok(&self) -> Option<&str> {
        (self.exit == Some(0)).then_some(self.stdout.as_str())
    }

    /// It did not succeed: an exit code other than 0, a cut-off, or a git
    /// that could not start.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.exit != Some(0)
    }
}

/// Every call one runner made, counted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub calls: u64,
    pub failed: u64,
    pub timeouts: u64,
    pub ms: u64,
    pub bytes_out: u64,
}

/// Runs git under one bound, and counts every call.
#[derive(Debug)]
pub struct Git {
    timeout: Duration,
    tally: Tally,
}

impl Git {
    /// A runner whose calls are killed after `timeout_ms`.
    #[must_use]
    pub fn new(timeout_ms: u64) -> Self {
        Self { timeout: Duration::from_millis(timeout_ms), tally: Tally::default() }
    }

    /// The calls made so far.
    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Run `git -C dir args…`.
    pub async fn run(&mut self, dir: &Path, args: &[&str]) -> Call {
        let started_at = Utc::now();
        let clock = Instant::now();
        let mut argv: Vec<String> = NO_REPOSITORY_PROGRAMS.iter().map(|a| (*a).to_string()).collect();
        if let Some((subcommand, rest)) = args.split_first() {
            argv.push((*subcommand).to_string());
            argv.extend(no_diff_drivers(subcommand).iter().map(|a| (*a).to_string()));
            argv.extend(rest.iter().map(|a| (*a).to_string()));
        }
        let run = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(&argv)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let (exit, timed_out, stdout, stderr) = match tokio::time::timeout(self.timeout, run).await {
            Ok(Ok(out)) => (
                out.status.code(),
                false,
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ),
            Ok(Err(e)) => (None, false, String::new(), format!("git could not run: {e}")),
            Err(_) => (None, true, String::new(), String::new()),
        };
        let call = Call {
            dir: dir.to_path_buf(),
            args: argv,
            started_at,
            ms: u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX),
            exit,
            timed_out,
            stdout,
            stderr,
        };
        self.tally.calls += 1;
        self.tally.failed += u64::from(call.failed());
        self.tally.timeouts += u64::from(call.timed_out);
        self.tally.ms = self.tally.ms.saturating_add(call.ms);
        self.tally.bytes_out = self.tally.bytes_out.saturating_add(call.stdout.len() as u64);
        call
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How often the test looks for the killed git.
    const PACE: Duration = Duration::from_millis(50); // skill-allow: §9-duration — test fixture, not a policy

    #[tokio::test]
    async fn a_call_past_its_bound_is_cut_off_and_killed() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = std::env::temp_dir().join(format!("superx-recon-git-{}", superx_kernel::types::Uuid::new_v7()));
        std::fs::create_dir_all(&dir).expect("dir");
        let pipe = dir.join("pipe");
        assert!(std::process::Command::new("mkfifo").arg(&pipe).status().expect("mkfifo runs").success());
        let path = pipe.to_string_lossy().into_owned();
        let mut git = Git::new(100);
        // git waits on a pipe nothing writes to: it can never answer in time.
        let call = git.run(here, &["hash-object", &path]).await;
        assert!(call.timed_out && call.exit.is_none() && call.failed(), "{call:?}");
        assert_eq!(git.tally().timeouts, 1);
        // Killed, not left waiting on the pipe.
        let waiting = || {
            std::process::Command::new("ps").args(["-axww", "-o", "stat=,command="]).output().is_ok_and(|o| {
                String::from_utf8_lossy(&o.stdout).lines().any(|l| l.contains(path.as_str()) && !l.trim_start().starts_with('Z'))
            })
        };
        let mut gone = false;
        for _ in 0..100 {
            if !waiting() {
                gone = true;
                break;
            }
            tokio::time::sleep(PACE).await;
        }
        assert!(gone, "the cut-off git was killed");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[tokio::test]
    async fn a_call_says_what_it_ran_how_it_ended_and_what_git_said() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut git = Git::new(120_000);
        let ok = git.run(here, &["rev-parse", "--is-inside-work-tree"]).await;
        assert_eq!(ok.ok().map(str::trim), Some("true"));
        assert_eq!(
            ok.args,
            ["-c", "log.showSignature=false", "-c", "core.fsmonitor=false", "rev-parse", "--is-inside-work-tree"]
        );
        let log = git.run(here, &["log", "-1", "--format=%H"]).await;
        assert!(log.args.contains(&"--no-textconv".to_string()) && log.args.contains(&"--no-ext-diff".to_string()));
        let bad = git.run(here, &["rev-parse", "--verify", "refs/heads/no-such-branch-anywhere"]).await;
        assert!(bad.failed() && !bad.timed_out && bad.exit.is_some_and(|c| c != 0), "{bad:?}");
        assert!(bad.stderr.contains("fatal"), "what git said is kept: {bad:?}");
        let t = git.tally();
        assert_eq!((t.calls, t.failed, t.timeouts), (3, 1, 0));
    }
}
