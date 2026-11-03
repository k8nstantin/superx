//! One commit, whole (#539): what is behind a row of the Results. Who made
//! it and with what effort and context, where it went and what of it lives,
//! its files, every CI run on it (its jobs and checks too), the pull
//! requests that carried it, and the tests and builds its session ran since
//! that session's previous commit.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::findings::in_flight;
use super::trail::Trail;
use super::DayFile;

/// A CI run on the commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetailRun {
    pub name: String,
    pub kind: String,
    pub verdict: Option<bool>,
    pub failed_before: bool,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub url: Option<String>,
}

/// A pull request that carried the commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetailPull {
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub merged: bool,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
}

/// A test or build its session ran before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetailCheck {
    pub at: DateTime<Utc>,
    pub test: bool,
    pub what: String,
    pub command: String,
    pub passed: i64,
    pub failed: i64,
    pub ok: bool,
}

/// One commit, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitDetail {
    pub repo: String,
    pub hash: String,
    pub subject: String,
    pub at: DateTime<Utc>,
    pub author_name: String,
    /// The model credited; empty when none is.
    pub model: String,
    pub by: Option<&'static str>,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub effort: Option<String>,
    pub context: Option<i64>,
    pub added: i64,
    pub removed: i64,
    pub place: Option<String>,
    pub in_flight: bool,
    pub squash: bool,
    pub landed: i64,
    pub alive: Option<i64>,
    pub never_landed: i64,
    pub reverted: bool,
    pub files: Vec<DayFile>,
    pub ci: Vec<DetailRun>,
    pub pulls: Vec<DetailPull>,
    pub checks: Vec<DetailCheck>,
}

/// The commit `hash` of the repository named `repo`; none when the trail
/// holds no such commit.
#[must_use]
pub fn commit(trail: &Trail, repo: &str, hash: &str) -> Option<CommitDetail> {
    let c = trail.commits.iter().find(|c| c.hash == hash && trail.map.names.get(&c.repo).is_some_and(|n| n == repo))?;
    let reply = c.reply.and_then(|i| trail.replies.get(i));
    let checks: Vec<DetailCheck> = match (&c.session, c.tied_at) {
        (Some(session), Some(to)) => {
            let since = trail.commits.iter().filter(|o| o.session.as_ref() == Some(session)).filter_map(|o| o.tied_at).filter(|t| *t < to).max();
            trail
                .checks
                .iter()
                .filter(|k| k.reply.and_then(|i| trail.replies.get(i)).is_some_and(|r| &r.session == session))
                .filter(|k| since.is_none_or(|s| k.at > s) && k.at <= to)
                .map(|k| DetailCheck {
                    at: k.at,
                    test: k.test,
                    what: k.what.clone(),
                    command: k.command.clone(),
                    passed: k.passed,
                    failed: k.failed,
                    ok: k.ok,
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let ci: Vec<DetailRun> = trail
        .ci
        .iter()
        .filter(|r| r.repo.as_ref() == Some(&c.repo) && r.sha.as_deref() == Some(hash))
        .map(|r| DetailRun {
            name: r.name.clone(),
            kind: r.kind.clone(),
            verdict: r.verdict,
            failed_before: r.failed_before,
            status: r.status.clone(),
            conclusion: r.conclusion.clone(),
            created_at: r.created_at,
            completed_at: r.completed_at,
            url: r.url.clone(),
        })
        .collect();
    let pulls: Vec<DetailPull> = c
        .pulls
        .iter()
        .filter_map(|&i| trail.pulls.get(i))
        .map(|p| DetailPull {
            number: p.number,
            title: p.title.clone(),
            url: p.url.clone(),
            state: p.state.clone(),
            merged: p.merged,
            created_at: p.created_at,
            merged_at: p.merged_at,
        })
        .collect();
    let mut files = trail.files.get(&c.key).cloned().unwrap_or_default();
    files.sort_by(|x, y| (y.added + y.removed).cmp(&(x.added + x.removed)).then_with(|| x.path.cmp(&y.path)));
    Some(CommitDetail {
        repo: repo.to_string(),
        hash: c.hash.clone(),
        subject: c.subject.clone(),
        at: c.at,
        author_name: c.author_name.clone(),
        model: c.family.clone().unwrap_or_default(),
        by: c.by,
        session: c.session.as_ref().and_then(|s| trail.kernel_sessions.get(s)).cloned(),
        agent: c.session.as_ref().and_then(|s| trail.agents.get(s)).cloned(),
        effort: reply.and_then(|r| r.effort.clone()),
        context: reply.map(|r| r.context),
        added: c.added,
        removed: c.removed,
        place: c.place.clone(),
        in_flight: in_flight(c),
        squash: c.squash,
        landed: c.landed,
        alive: c.alive,
        never_landed: c.never_landed,
        reverted: c.reverted,
        files,
        ci,
        pulls,
        checks,
    })
}
