//! CI runs (#538, step 20): GitHub's check suites and check runs, workflow
//! runs and their jobs and commit statuses, Bitbucket's pipelines and their
//! steps and commit statuses. Each makes its `ci_run` row the first time,
//! and a `ci_run_state` row when one of its state's columns differs from
//! the newest; its object changing alone makes none.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;
use superx_kernel::types::{RecordId, Value};

use crate::crawler::hosts::objects::{key_text, key_value, Accounts};
use crate::crawler::hosts::pulls::{j, n, s, sha, t, Pull};
use crate::crawler::hosts::Host;
use crate::crawler::store::{Batch, Row};
use crate::crawler::value::{at, int, text};

/// A CI run the crawler holds.
#[derive(Debug, Clone)]
pub struct CiRun {
    pub row: RecordId,
    pub name: Option<String>,
    pub commit_sha: Option<String>,
    /// The ref it ran on.
    pub git_ref: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub state: Option<CiState>,
}

/// A CI run's state, as its newest row says it: every column but the
/// object it was read from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiState {
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub attempt: Option<i64>,
    pub number: Option<i64>,
    pub actor: Option<RecordId>,
    pub runner: Option<String>,
}

impl CiState {
    /// A `ci_run_state` row, as the database gives it back.
    #[must_use]
    pub fn from_row(v: &Value) -> Self {
        let time = |key: &str| match at(v, &[key]) {
            Some(Value::Datetime(d)) => Some(DateTime::<Utc>::from(*d)),
            _ => None,
        };
        Self {
            status: text(v, &["status"]).map(str::to_string),
            conclusion: text(v, &["conclusion"]).map(str::to_string),
            started_at: time("started_at"),
            completed_at: time("completed_at"),
            attempt: int(v, &["attempt"]),
            number: int(v, &["number"]),
            actor: match at(v, &["actor"]) {
                Some(Value::RecordId(id)) => Some(id.clone()),
                _ => None,
            },
            runner: text(v, &["runner"]).map(str::to_string),
        }
    }
}

/// The kinds of CI object, as the structure names them.
pub const CHECK_SUITE: &str = "check_suite";
pub const CHECK_RUN: &str = "check_run";
pub const WORKFLOW_RUN: &str = "workflow_run";
pub const WORKFLOW_JOB: &str = "workflow_job";
pub const COMMIT_STATUS: &str = "commit_status";
pub const PIPELINE: &str = "pipeline";
pub const PIPELINE_STEP: &str = "pipeline_step";

/// What the rows of one CI object need besides the object.
pub struct Read<'a> {
    pub host: Host,
    pub host_repository: &'a RecordId,
    pub kind: &'static str,
    /// Its own `host_object` row.
    pub object: &'a RecordId,
    /// The commit a status was asked for.
    pub commit: Option<&'a str>,
    /// The host repository's pull requests, by number.
    pub pulls: &'a HashMap<i64, Pull>,
}

/// The first of a list of pull requests whose number is one of the host
/// repository's.
fn first_pull(list: Option<&Json>, pulls: &HashMap<i64, Pull>) -> Option<RecordId> {
    list?.as_array()?.iter().filter_map(|p| p.get("number").and_then(Json::as_i64)).find_map(|n| pulls.get(&n).map(|p| p.row.clone()))
}

/// A run's host id, for its kind: GitHub's id, Bitbucket's uuid, a
/// Bitbucket status's commit and key.
#[must_use]
pub fn host_id(kind: &str, host: Host, object: &Json, commit: Option<&str>) -> Option<Value> {
    match (kind, host) {
        (COMMIT_STATUS, Host::Bitbucket) => Some(Value::String(format!("{}/{}", commit?, s(object, &["key"])?))),
        (PIPELINE | PIPELINE_STEP, _) => object.get("uuid").and_then(key_value),
        _ => object.get("id").and_then(key_value),
    }
}

/// One CI object read: its `ci_run` row the first time, its
/// `ci_run_state` row when its state changed; the run as held after it.
pub fn read(rows: &mut Batch, accounts: &mut Accounts, runs: &mut HashMap<(String, String), CiRun>, object: &Json, r: &Read<'_>) -> Option<RecordId> {
    let id = host_id(r.kind, r.host, object, r.commit)?;
    let key = (r.kind.to_string(), key_text(&id));
    let row = match runs.get(&key) {
        Some(run) => run.row.clone(),
        None => {
            let parent = |kind: &str, of: Option<Value>| of.and_then(|k| runs.get(&(kind.to_string(), key_text(&k))).map(|p| p.row.clone()));
            let (name, commit_sha, git_ref, parent, pull, url, created_at) = match (r.kind, r.host) {
                (CHECK_SUITE, _) => (
                    s(object, &["app", "name"]).map(str::to_string),
                    s(object, &["head_sha"]).map(str::to_string),
                    s(object, &["head_branch"]).map(str::to_string),
                    None,
                    first_pull(object.get("pull_requests"), r.pulls),
                    s(object, &["url"]).map(str::to_string),
                    t(object, &["created_at"]),
                ),
                (CHECK_RUN, _) => (
                    s(object, &["name"]).map(str::to_string),
                    s(object, &["head_sha"]).map(str::to_string),
                    None,
                    parent(CHECK_SUITE, j(object, &["check_suite", "id"]).and_then(key_value)),
                    first_pull(object.get("pull_requests"), r.pulls),
                    s(object, &["html_url"]).map(str::to_string),
                    None,
                ),
                (WORKFLOW_RUN, _) => (
                    s(object, &["name"]).map(str::to_string),
                    s(object, &["head_sha"]).map(str::to_string),
                    s(object, &["head_branch"]).map(str::to_string),
                    None,
                    first_pull(object.get("pull_requests"), r.pulls),
                    s(object, &["html_url"]).map(str::to_string),
                    t(object, &["created_at"]),
                ),
                (WORKFLOW_JOB, _) => (
                    s(object, &["name"]).map(str::to_string),
                    s(object, &["head_sha"]).map(str::to_string),
                    s(object, &["head_branch"]).map(str::to_string),
                    parent(WORKFLOW_RUN, object.get("run_id").and_then(key_value)),
                    None,
                    s(object, &["html_url"]).map(str::to_string),
                    t(object, &["created_at"]),
                ),
                (COMMIT_STATUS, Host::GitHub) => (
                    s(object, &["context"]).map(str::to_string),
                    r.commit.map(str::to_string),
                    None,
                    None,
                    None,
                    s(object, &["target_url"]).map(str::to_string),
                    t(object, &["created_at"]),
                ),
                (COMMIT_STATUS, Host::Bitbucket) => (
                    s(object, &["name"]).map(str::to_string),
                    r.commit.map(str::to_string),
                    s(object, &["refname"]).map(str::to_string),
                    None,
                    None,
                    s(object, &["url"]).map(str::to_string),
                    t(object, &["created_on"]),
                ),
                (PIPELINE, _) => (
                    s(object, &["target", "selector", "pattern"]).map(str::to_string),
                    s(object, &["target", "commit", "hash"]).map(str::to_string),
                    s(object, &["target", "ref_name"]).map(str::to_string),
                    None,
                    n(object, &["target", "pullrequest", "id"]).and_then(|id| r.pulls.get(&id).map(|p| p.row.clone())),
                    None,
                    t(object, &["created_on"]),
                ),
                (PIPELINE_STEP, _) => {
                    let pipeline = j(object, &["pipeline", "uuid"]).and_then(key_value);
                    let held = pipeline.as_ref().and_then(|k| runs.get(&(PIPELINE.to_string(), key_text(k))));
                    (
                        s(object, &["name"]).map(str::to_string),
                        held.and_then(|p| p.commit_sha.clone()),
                        held.and_then(|p| p.git_ref.clone()),
                        held.map(|p| p.row.clone()),
                        None,
                        None,
                        None,
                    )
                }
                _ => return None,
            };
            let commit_sha = commit_sha.filter(|h| sha(h));
            let row = rows.create(
                "ci_run",
                Row::new()
                    .link("host_repository", r.host_repository)
                    .text("kind", r.kind)
                    .value("host_id", id)
                    .some_text("name", name.clone())
                    .some_text("commit_sha", commit_sha.clone())
                    .some_text("ref", git_ref.clone())
                    .some_link("parent", parent.as_ref())
                    .some_link("pull_request", pull.as_ref())
                    .some_text("url", url)
                    .some_time("created_at", created_at)
                    .link("object", r.object),
            );
            runs.insert(key.clone(), CiRun { row: row.clone(), name, commit_sha, git_ref, created_at, state: None });
            row
        }
    };
    let state = state(r.kind, r.host, object, accounts);
    let run = runs.get_mut(&key)?;
    if run.state.as_ref() != Some(&state) {
        rows.create(
            "ci_run_state",
            Row::new()
                .link("ci_run", &row)
                .some_text("status", state.status.clone())
                .some_text("conclusion", state.conclusion.clone())
                .some_time("started_at", state.started_at)
                .some_time("completed_at", state.completed_at)
                .some_int("attempt", state.attempt)
                .some_int("number", state.number)
                .some_link("actor", state.actor.as_ref())
                .some_text("runner", state.runner.clone())
                .link("object", r.object),
        );
        run.state = Some(state);
    }
    Some(row)
}

/// A CI object's state, its columns as step 20 reads them for its kind.
fn state(kind: &str, host: Host, object: &Json, accounts: &mut Accounts) -> CiState {
    let text = |path: &[&str]| s(object, path).map(str::to_string);
    let mut actor = |path: &[&str]| j(object, path).and_then(|a| accounts.row(host, a));
    match (kind, host) {
        (CHECK_SUITE, _) => CiState { status: text(&["status"]), conclusion: text(&["conclusion"]), ..CiState::default() },
        (CHECK_RUN, _) => CiState {
            status: text(&["status"]),
            conclusion: text(&["conclusion"]),
            started_at: t(object, &["started_at"]),
            completed_at: t(object, &["completed_at"]),
            ..CiState::default()
        },
        (WORKFLOW_RUN, _) => CiState {
            status: text(&["status"]),
            conclusion: text(&["conclusion"]),
            started_at: t(object, &["run_started_at"]),
            attempt: n(object, &["run_attempt"]),
            number: n(object, &["run_number"]),
            actor: actor(&["triggering_actor"]),
            ..CiState::default()
        },
        (WORKFLOW_JOB, _) => CiState {
            status: text(&["status"]),
            conclusion: text(&["conclusion"]),
            started_at: t(object, &["started_at"]),
            completed_at: t(object, &["completed_at"]),
            attempt: n(object, &["run_attempt"]),
            runner: text(&["runner_name"]),
            ..CiState::default()
        },
        (COMMIT_STATUS, _) => CiState { status: text(&["state"]), ..CiState::default() },
        (PIPELINE, _) => CiState {
            status: text(&["state", "name"]),
            conclusion: text(&["state", "result", "name"]),
            completed_at: t(object, &["completed_on"]),
            number: n(object, &["build_number"]),
            actor: actor(&["creator"]),
            ..CiState::default()
        },
        (PIPELINE_STEP, _) => CiState {
            status: text(&["state", "name"]),
            conclusion: text(&["state", "result", "name"]),
            started_at: t(object, &["started_on"]),
            completed_at: t(object, &["completed_on"]),
            ..CiState::default()
        },
        _ => CiState::default(),
    }
}

/// Whether a run's newest state is one a host crawl asks again: a check
/// suite or run not completed, a Bitbucket status in progress.
#[must_use]
pub fn pending(kind: &str, state: Option<&CiState>) -> bool {
    let status = state.and_then(|s| s.status.as_deref());
    match kind {
        CHECK_SUITE | CHECK_RUN => status != Some("completed"),
        COMMIT_STATUS => matches!(status, Some("pending" | "INPROGRESS")),
        _ => false,
    }
}
