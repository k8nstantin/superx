//! A crawl's git calls (#538, step 2): each one a `git_call` row of its
//! crawl, with the word of the step that made it, and counted. A call
//! failed when it exited otherwise than its step reads an answer from, was
//! cut off, or could not start; what git said of it goes to the crawl's
//! note, in git's words.

use std::path::Path;

use superx_kernel::types::RecordId;

use crate::crawler::repos::git::{self, Call, With};
use crate::crawler::store::{Batch, Row};
use crate::crawler::Stop;

/// The exits a call answers with: 0 for most.
pub const OK: &[i32] = &[0];

/// The exits of a call whose exit is its answer: `merge-base
/// --is-ancestor`, `rev-parse --verify --quiet`.
pub const YES_OR_NO: &[i32] = &[0, 1];

/// A crawl's git calls, as rows and counts.
#[derive(Debug)]
pub struct Recorder {
    crawl: RecordId,
    timeout: std::time::Duration,
    /// The `git_call` rows.
    pub rows: Batch,
    pub calls: i64,
    pub failed: i64,
    pub timeouts: i64,
    pub ms: i64,
    pub bytes: i64,
    /// What git said of each call that failed, for the crawl's note.
    pub failures: Vec<String>,
}

impl Recorder {
    #[must_use]
    pub fn new(crawl: RecordId, timeout_ms: u64) -> Self {
        Self {
            crawl,
            timeout: std::time::Duration::from_millis(timeout_ms),
            rows: Batch::default(),
            calls: 0,
            failed: 0,
            timeouts: 0,
            ms: 0,
            bytes: 0,
            failures: Vec::new(),
        }
    }

    /// The bound a call is cut off at.
    #[must_use]
    pub fn timeout(&self) -> std::time::Duration {
        self.timeout
    }

    /// Run `git -C dir args…` with `with` for `step`, unless a stop comes
    /// first: then `None`, and the call is killed.
    pub async fn run(&mut self, step: &'static str, dir: &Path, args: &[&str], with: &With, answers: &[i32], stop: &mut Stop) -> Option<Call> {
        let call = stop.unless(git::call(dir, args, with, self.timeout)).await?;
        self.record(step, &call, answers);
        Some(call)
    }

    /// Record a call made apart from [`Recorder::run`], one of several at
    /// once; whether it answered.
    pub fn record(&mut self, step: &'static str, call: &Call, answers: &[i32]) -> bool {
        let answered = !call.timed_out && call.exit.is_some_and(|e| answers.contains(&e));
        self.calls += 1;
        self.failed += i64::from(!answered);
        self.timeouts += i64::from(call.timed_out);
        self.ms = self.ms.saturating_add(i64::try_from(call.ms).unwrap_or(i64::MAX));
        self.bytes = self.bytes.saturating_add(i64::try_from(call.bytes_out).unwrap_or(i64::MAX));
        if !answered {
            // The subcommand, and what it was asked of: its last argument.
            let mut named = call.args.iter().skip_while(|a| *a == "-c" || a.contains('='));
            let subcommand = named.next().map_or("git", String::as_str);
            let of = call.args.last().filter(|a| !a.starts_with('-') && *a != subcommand).map_or(String::new(), |a| format!(" {a}"));
            let said = if call.timed_out {
                format!("cut off at {} ms", self.timeout.as_millis())
            } else {
                call.stderr.trim().to_string()
            };
            self.failures.push(format!("git {subcommand}{of} in {}: {said}", call.dir.display()));
        }
        self.rows.create(
            "git_call",
            Row::new()
                .link("crawl", &self.crawl)
                .text("step", step)
                .text("dir", call.dir.to_string_lossy().into_owned())
                .texts("args", &call.args)
                .time("started_at", call.started_at)
                .int("ms", i64::try_from(call.ms).unwrap_or(i64::MAX))
                .some_int("exit", call.exit.map(i64::from))
                .flag("timed_out", call.timed_out)
                .int("bytes_out", i64::try_from(call.bytes_out).unwrap_or(i64::MAX))
                .int("lines_out", i64::try_from(call.lines_out).unwrap_or(i64::MAX))
                .some_text("stderr", (!call.stderr.is_empty()).then(|| call.stderr.clone())),
        );
        answered
    }
}
