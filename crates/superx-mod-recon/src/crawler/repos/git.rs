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
//! counts every call, and records it where its schema has a place for it:
//! a crawl's calls are `git_call` rows (#538, step 2).
//!
//! A call on one of the crawler's own mirrors runs with more than the
//! overrides ([`With`]): the configuration and environment step 4 names,
//! and no bound when it fetches.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// Configuration under which a read would run a program the repository
/// names, overridden on every call (#447): `log.showSignature` hands each
/// signed commit to the repository's `gpg.program`, and `core.fsmonitor`
/// names a hook.
const NO_REPOSITORY_PROGRAMS: [&str; 4] = ["-c", "log.showSignature=false", "-c", "core.fsmonitor=false"];

/// The flags that keep a subcommand off the repository's diff drivers
/// (#447): `blame` runs a textconv filter by default, `log` and `diff` run
/// one when they print a patch, and an external diff would run too;
/// `diff-tree` takes the same flags (#538, step 2). Only these subcommands
/// take the flags; the others refuse them.
fn no_diff_drivers(subcommand: &str) -> &'static [&'static str] {
    match subcommand {
        "blame" => &["--no-textconv"],
        "log" | "diff" | "diff-tree" => &["--no-textconv", "--no-ext-diff"],
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
    /// What it printed, byte for byte, when the call asked for its bytes
    /// ([`With::raw`]); `stdout` is then empty.
    pub raw: Option<Vec<u8>>,
    /// What git wrote to its error output, whole; when git could not
    /// start, why.
    pub stderr: String,
    /// The bytes and the lines it printed.
    pub bytes_out: u64,
    pub lines_out: u64,
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

/// What a call runs with beyond the overrides every call has.
#[derive(Debug, Clone, Default)]
pub struct With {
    /// `-c key=value` pairs, before the subcommand.
    pub config: Vec<String>,
    /// Variables set in its environment.
    pub env: Vec<(String, String)>,
    /// What it reads on its standard input.
    pub input: Option<Vec<u8>>,
    /// Keep what it prints byte for byte ([`Call::raw`]).
    pub raw: bool,
    /// Never cut off: a fetch takes what the repository's size makes it.
    pub unbounded: bool,
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

    /// The bound its calls are killed at.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Run `git -C dir args…`.
    pub async fn run(&mut self, dir: &Path, args: &[&str]) -> Call {
        self.run_with(dir, args, &With::default()).await
    }

    /// Run `git -C dir args…` with `with`.
    pub async fn run_with(&mut self, dir: &Path, args: &[&str], with: &With) -> Call {
        let call = call(dir, args, with, self.timeout).await;
        self.count(&call);
        call
    }

    /// Count a call made apart from this runner, one of several at once.
    pub fn count(&mut self, call: &Call) {
        self.tally.calls += 1;
        self.tally.failed += u64::from(call.failed());
        self.tally.timeouts += u64::from(call.timed_out);
        self.tally.ms = self.tally.ms.saturating_add(call.ms);
        self.tally.bytes_out = self.tally.bytes_out.saturating_add(call.bytes_out);
    }
}

/// The arguments a call runs with after `git -C <dir>`: the overrides, the
/// call's own configuration, the subcommand and its flags.
#[must_use]
pub fn argv(args: &[&str], with: &With) -> Vec<String> {
    let mut argv: Vec<String> = NO_REPOSITORY_PROGRAMS.iter().map(|a| (*a).to_string()).collect();
    for pair in &with.config {
        argv.push("-c".to_string());
        argv.push(pair.clone());
    }
    if let Some((subcommand, rest)) = args.split_first() {
        argv.push((*subcommand).to_string());
        argv.extend(no_diff_drivers(subcommand).iter().map(|a| (*a).to_string()));
        argv.extend(rest.iter().map(|a| (*a).to_string()));
    }
    argv
}

/// One git call: `git -C dir args…`, killed at `timeout` unless `with`
/// says it is unbounded, and killed when the call is dropped.
pub async fn call(dir: &Path, args: &[&str], with: &With, timeout: Duration) -> Call {
    let started_at = Utc::now();
    let clock = Instant::now();
    let argv = argv(args, with);
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(&argv)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .envs(with.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(if with.input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let run = async {
        let mut child = command.spawn()?;
        let feeding = match (child.stdin.take(), with.input.clone()) {
            (Some(mut stdin), Some(input)) => Some(tokio::spawn(async move {
                // A git that stops reading early closes the pipe: what it did
                // not read is no error of the call's.
                let _written = stdin.write_all(&input).await;
            })),
            _ => None,
        };
        let out = child.wait_with_output().await;
        if let Some(feeding) = feeding {
            let _fed = feeding.await;
        }
        out
    };
    let ended = if with.unbounded { Ok(run.await) } else { tokio::time::timeout(timeout, run).await };
    let (exit, timed_out, out, stderr) = match ended {
        Ok(Ok(out)) => (out.status.code(), false, out.stdout, String::from_utf8_lossy(&out.stderr).into_owned()),
        Ok(Err(e)) => (None, false, Vec::new(), format!("git could not run: {e}")),
        Err(_) => (None, true, Vec::new(), String::new()),
    };
    let bytes_out = out.len() as u64;
    let lines_out = out.iter().filter(|b| **b == b'\n').count() as u64 + u64::from(out.last().is_some_and(|b| *b != b'\n'));
    let (stdout, raw) = if with.raw { (String::new(), Some(out)) } else { (String::from_utf8_lossy(&out).into_owned(), None) };
    Call {
        dir: dir.to_path_buf(),
        args: argv,
        started_at,
        ms: u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX),
        exit,
        timed_out,
        stdout,
        raw,
        stderr,
        bytes_out,
        lines_out,
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
