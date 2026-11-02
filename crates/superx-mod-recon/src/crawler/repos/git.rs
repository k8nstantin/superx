//! Git, read-only, the way the crawler runs it: one `git -C <dir>` process
//! per call, never a shell.
//!
//! Nothing a repository names is run (#447): signature checks and the
//! fsmonitor hook are overridden on every call, diff drivers are off where
//! a subcommand takes them, and no optional lock is taken, so no index
//! refresh is written back (git(1), `GIT_OPTIONAL_LOCKS`). A call that
//! outlasts `attr_recon_git_timeout_ms` is killed, never left behind.
//!
//! A call on one of the crawler's own mirrors (#538) carries more: the
//! mirror's settings on its command line and its environment, which
//! [`With`] gives. A fetch is not bound by the timeout: only a stop ends
//! it, and git ends one that stalls.
//!
//! What each call did is kept: when it started, how long it took, its exit
//! code, what it printed and what it wrote to its error output. The crawler
//! counts every call, and records it where its schema has a place for it.

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
/// one when they print a patch, and an external diff would run too. Only
/// these subcommands take the flags; the others refuse them.
fn no_diff_drivers(subcommand: &str) -> &'static [&'static str] {
    match subcommand {
        "blame" => &["--no-textconv"],
        "log" | "diff" => &["--no-textconv", "--no-ext-diff"],
        _ => &[],
    }
}

/// What a call takes beyond t01's overrides: settings on its command line
/// and variables in its environment (a mirror's, step 4), what it is given
/// to read, and whether the runner's bound applies.
#[derive(Debug, Default, Clone)]
pub struct With<'a> {
    /// `key=value` settings, each given to git as `-c key=value`, after
    /// the overrides and before the subcommand.
    pub config: Vec<String>,
    /// Variables set in the call's environment, beside `GIT_OPTIONAL_LOCKS`.
    pub env: Vec<(String, String)>,
    /// What git reads on its standard input; none when absent.
    pub input: Option<&'a [u8]>,
    /// Not cut off at the runner's bound: only a stop ends it (a fetch).
    pub unbounded: bool,
}

/// One git call, as it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The directory git ran in.
    pub dir: PathBuf,
    /// Every argument after `git -C <dir>`, exactly as run: the overrides,
    /// the subcommand and its flags.
    pub args: Vec<String>,
    /// Every variable the runner set in its environment.
    pub env: Vec<(String, String)>,
    pub started_at: DateTime<Utc>,
    pub ms: u64,
    /// Its exit code; `None` when it was cut off, could not start, or was
    /// ended by a signal.
    pub exit: Option<i32>,
    /// Cut off at its bound and killed.
    pub timed_out: bool,
    pub stdout: String,
    /// What it printed, byte for byte.
    pub raw: Vec<u8>,
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

    /// What it printed, byte for byte, when it succeeded.
    #[must_use]
    pub fn ok_raw(&self) -> Option<&[u8]> {
        (self.exit == Some(0)).then_some(self.raw.as_slice())
    }

    /// It did not succeed: an exit code other than 0, a cut-off, or a git
    /// that could not start.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.exit != Some(0)
    }

    /// The lines it printed.
    #[must_use]
    pub fn lines_out(&self) -> u64 {
        u64::try_from(self.raw.iter().filter(|b| **b == b'\n').count()).unwrap_or(u64::MAX)
            + u64::from(self.raw.last().is_some_and(|b| *b != b'\n'))
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

impl Tally {
    /// Another tally's calls added to this one's.
    pub fn add(&mut self, other: Tally) {
        self.calls += other.calls;
        self.failed += other.failed;
        self.timeouts += other.timeouts;
        self.ms = self.ms.saturating_add(other.ms);
        self.bytes_out = self.bytes_out.saturating_add(other.bytes_out);
    }
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

    /// Calls another runner made for this one's work: blames run at once,
    /// each on its own runner.
    pub fn add(&mut self, other: Tally) {
        self.tally.add(other);
    }

    /// Run `git -C dir args…`.
    pub async fn run(&mut self, dir: &Path, args: &[&str]) -> Call {
        self.run_with(dir, args, &With::default()).await
    }

    /// Run `git -C dir args…` with what `with` adds.
    pub async fn run_with(&mut self, dir: &Path, args: &[&str], with: &With<'_>) -> Call {
        let started_at = Utc::now();
        let clock = Instant::now();
        let mut argv: Vec<String> = NO_REPOSITORY_PROGRAMS.iter().map(|a| (*a).to_string()).collect();
        for setting in &with.config {
            argv.push("-c".to_string());
            argv.push(setting.clone());
        }
        if let Some((subcommand, rest)) = args.split_first() {
            argv.push((*subcommand).to_string());
            argv.extend(no_diff_drivers(subcommand).iter().map(|a| (*a).to_string()));
            argv.extend(rest.iter().map(|a| (*a).to_string()));
        }
        let mut env = vec![("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())];
        env.extend(with.env.iter().cloned());
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .args(&argv)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(if with.input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let input = with.input.map(<[u8]>::to_vec);
        let run = async move {
            let mut command = command;
            let mut child = command.spawn()?;
            let writer = match (child.stdin.take(), input) {
                (Some(mut stdin), Some(input)) => Some(tokio::spawn(async move {
                    // A git that stops reading early closes the pipe: what
                    // it printed says what it did with what it read.
                    let _written = stdin.write_all(&input).await;
                })),
                _ => None,
            };
            let out = child.wait_with_output().await;
            if let Some(writer) = writer {
                let _ended = writer.await;
            }
            out
        };
        let ran = if with.unbounded { Ok(run.await) } else { tokio::time::timeout(self.timeout, run).await };
        let (exit, timed_out, raw, stderr) = match ran {
            Ok(Ok(out)) => (out.status.code(), false, out.stdout, String::from_utf8_lossy(&out.stderr).into_owned()),
            Ok(Err(e)) => (None, false, Vec::new(), format!("git could not run: {e}")),
            Err(_) => (None, true, Vec::new(), String::new()),
        };
        let call = Call {
            dir: dir.to_path_buf(),
            args: argv,
            env,
            started_at,
            ms: u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX),
            exit,
            timed_out,
            stdout: String::from_utf8_lossy(&raw).into_owned(),
            raw,
            stderr,
        };
        self.tally.calls += 1;
        self.tally.failed += u64::from(call.failed());
        self.tally.timeouts += u64::from(call.timed_out);
        self.tally.ms = self.tally.ms.saturating_add(call.ms);
        self.tally.bytes_out = self.tally.bytes_out.saturating_add(call.raw.len() as u64);
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
