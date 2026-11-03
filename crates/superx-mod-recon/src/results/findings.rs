//! The findings (#539): what recon can say that the captured lines alone
//! cannot. Each model's work followed past its session, into what landed,
//! what is alive, what CI said, which pull requests carried it and what was
//! thrown away, and how the operator directed it. Every commit of ours is
//! given whole with what became of it, every pull request with how it went,
//! and each model's work week by week in each repository, so the page
//! slices them any way (by effort, by context, tested or not) and every bar
//! opens on the commits behind it.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, TimeDelta, Utc};
use serde::Serialize;

use super::compare::says_word;
use super::trail::{rewrites, Trail};
use crate::crawler::hosts::ci::{PIPELINE, WORKFLOW_RUN};
use crate::params::Params;

/// One CI workflow's newest verdict on a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CiName {
    pub name: String,
    pub verdict: Option<bool>,
}

/// A commit of ours and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundCommit {
    pub repo: String,
    pub hash: String,
    pub subject: String,
    pub at: DateTime<Utc>,
    /// The model credited; empty when no session's work covers it.
    pub model: String,
    /// `printed` or `claim`.
    pub by: Option<&'static str>,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub added: i64,
    pub removed: i64,
    pub place: Option<String>,
    /// On a checked-out branch, or in an open pull request: it may still
    /// land, so its lines off the main line are not counted as never landed.
    pub in_flight: bool,
    /// The main-line commit this same change was rewritten as.
    pub landed_as: Option<String>,
    pub landed: i64,
    pub alive: Option<i64>,
    pub never_landed: i64,
    /// Its files.
    pub files: i64,
    pub bulk: bool,
    pub reverted: bool,
    pub ci: Option<bool>,
    pub ci_names: Vec<CiName>,
    /// The numbers of the pull requests that carried it.
    pub pulls: Vec<i64>,
    pub tested: Option<bool>,
    pub tests_green: Option<bool>,
    pub built: Option<bool>,
    /// The thinking effort of the reply that made it.
    pub effort: Option<String>,
    /// The tokens that reply was sent.
    pub context: Option<i64>,
    pub subagent: Option<bool>,
}

/// A pull request and how it went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundPull {
    pub repo: String,
    pub host: String,
    pub path: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub state: String,
    pub merged: bool,
    pub merged_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub minutes_to_merge: Option<i64>,
    pub additions: i64,
    pub deletions: i64,
    pub commits: i64,
    pub model: Option<String>,
    /// `opened` or `commits`.
    pub by: Option<&'static str>,
    pub session: Option<String>,
    pub ci_failed: i64,
    pub ci_passed: i64,
    pub ci_first: Option<bool>,
}

/// One model's work in one repository in one week (Monday, UTC).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Week {
    pub week: String,
    pub model: String,
    /// Empty where the session was in no repository.
    pub repo: String,
    pub replies: i64,
    pub subagent_replies: i64,
    pub out_tokens: i64,
    pub context_tokens: i64,
    pub thinking_ms: i64,
    pub tool_calls: i64,
    pub active_secs: i64,
    pub edits: i64,
    pub edit_added: i64,
    pub edit_removed: i64,
    pub operator_changed: i64,
    pub tests: i64,
    pub tests_failed: i64,
    pub builds: i64,
    pub builds_failed: i64,
    pub prompts: i64,
    pub corrections: i64,
    pub escalations: i64,
    pub interrupts: i64,
}

/// One repository's refs and CI in one week.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepoWeek {
    pub week: String,
    pub repo: String,
    /// Reflog entries by what they did.
    pub moves: BTreeMap<String, i64>,
    pub rewrites: i64,
    pub ci_passed: i64,
    pub ci_failed: i64,
    /// Runs that failed, then passed when run again.
    pub ci_turned: i64,
}

/// One CI workflow of one repository, over all time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Workflow {
    pub repo: String,
    pub name: String,
    pub kind: String,
    pub runs: i64,
    pub passed: i64,
    pub failed: i64,
    pub turned: i64,
}

/// A folder read as another repository: a clone or a build folder whose
/// remote names that repository's host repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Folder {
    pub folder: String,
    pub repo: String,
}

/// The findings answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Findings {
    pub commits: Vec<FoundCommit>,
    pub pulls: Vec<FoundPull>,
    pub weeks: Vec<Week>,
    pub repo_weeks: Vec<RepoWeek>,
    pub workflows: Vec<Workflow>,
    /// CI runs held, by kind.
    pub ci_kinds: BTreeMap<String, i64>,
    pub folders: Vec<Folder>,
    pub computed_at: String,
}

/// What a repository's week is told: a ref's move, or a host CI run's
/// verdict and whether a re-run turned it.
enum Seen {
    Move(&'static str),
    Run(Option<bool>, bool),
}

/// The Monday of `at`'s week, UTC.
pub(crate) fn week_of(at: DateTime<Utc>) -> String {
    let d = at.date_naive();
    (d - TimeDelta::days(i64::from(d.weekday().num_days_from_monday()))).format("%Y-%m-%d").to_string()
}

fn slot<'a>(weeks: &'a mut BTreeMap<(String, String, String), Week>, at: DateTime<Utc>, model: &str, repo: &str) -> &'a mut Week {
    let week = week_of(at);
    weeks.entry((week.clone(), model.to_string(), repo.to_string())).or_insert_with(|| Week {
        week,
        model: model.to_string(),
        repo: repo.to_string(),
        ..Week::default()
    })
}

/// Whether a commit off the main line may still land: it is on a checked-out
/// branch, or in an open pull request.
pub(crate) fn in_flight(c: &super::trail::Commit) -> bool {
    c.landed_as.is_none() && (c.place.as_deref() == Some("in_flight") || (c.open_pull && c.place.as_deref() != Some("mainline")))
}

/// Whether a run is a host's own CI run, not one of its parts or a status.
pub(crate) fn host_run(kind: &str) -> bool {
    kind == WORKFLOW_RUN || kind == PIPELINE
}

/// The findings, from the trail.
#[must_use]
pub fn findings(trail: &Trail, params: &Params) -> Findings {
    let name = |repo: Option<&String>| trail.map.name(repo);
    let mut weeks: BTreeMap<(String, String, String), Week> = BTreeMap::new();
    for r in &trail.replies {
        let w = slot(&mut weeks, r.at, &r.family, &name(r.repo.as_ref()));
        w.replies += 1;
        w.subagent_replies += i64::from(r.subagent);
        w.out_tokens += r.out_tokens;
        w.context_tokens += r.context;
        w.thinking_ms += r.thinking_ms;
        w.tool_calls += r.tool_calls;
        w.active_secs += r.active_secs;
    }
    for e in &trail.edits {
        let Some(r) = e.reply.and_then(|i| trail.replies.get(i)) else { continue };
        let w = slot(&mut weeks, e.at.unwrap_or(r.at), &r.family, &name(r.repo.as_ref()));
        w.edits += 1;
        w.edit_added += e.added;
        w.edit_removed += e.removed;
        w.operator_changed += i64::from(e.operator_changed);
    }
    for c in &trail.checks {
        let Some(r) = c.reply.and_then(|i| trail.replies.get(i)) else { continue };
        let w = slot(&mut weeks, c.at, &r.family, &name(r.repo.as_ref()));
        if c.test {
            w.tests += 1;
            w.tests_failed += i64::from(c.failed > 0);
        } else {
            w.builds += 1;
            w.builds_failed += i64::from(!c.ok);
        }
    }
    for p in &trail.prompts {
        let w = slot(&mut weeks, p.at, p.family.as_deref().unwrap_or_default(), &name(p.repo.as_ref()));
        match p.kind.as_str() {
            "typed" | "queued" => {
                w.prompts += 1;
                w.corrections += i64::from(says_word(&p.text, &params.correction_words, false));
                w.escalations += i64::from(says_word(&p.text, &params.escalation_words, true));
            }
            "interrupt" => w.interrupts += 1,
            _ => {}
        }
    }

    let commits: Vec<FoundCommit> = trail
        .commits
        .iter()
        .filter(|c| c.ours && !c.squash && !c.copy)
        .map(|c| {
            let reply = c.reply.and_then(|i| trail.replies.get(i));
            FoundCommit {
                repo: name(Some(&c.repo)),
                hash: c.hash.clone(),
                subject: c.subject.clone(),
                at: c.at,
                model: c.family.clone().unwrap_or_default(),
                by: c.by,
                session: c.session.as_ref().and_then(|s| trail.kernel_sessions.get(s)).cloned(),
                agent: c.session.as_ref().and_then(|s| trail.agents.get(s)).cloned(),
                added: c.added,
                removed: c.removed,
                place: c.place.clone(),
                in_flight: in_flight(c),
                landed_as: c.landed_as.clone(),
                landed: c.landed,
                alive: c.alive,
                never_landed: c.never_landed,
                files: trail.files.get(&c.key).map_or(0, |f| i64::try_from(f.len()).unwrap_or(i64::MAX)),
                bulk: c.bulk,
                reverted: c.reverted,
                ci: c.ci,
                ci_names: c.ci_names.iter().map(|(name, verdict)| CiName { name: name.clone(), verdict: *verdict }).collect(),
                pulls: c.pulls.iter().filter_map(|&i| trail.pulls.get(i)).map(|p| p.number).collect(),
                tested: c.tested,
                tests_green: c.tests_green,
                built: c.built,
                effort: reply.and_then(|r| r.effort.clone()),
                context: reply.map(|r| r.context),
                subagent: reply.map(|r| r.subagent),
            }
        })
        .collect();

    let pulls: Vec<FoundPull> = trail
        .pulls
        .iter()
        .map(|p| FoundPull {
            repo: name(p.repo.as_ref()),
            host: p.host.clone(),
            path: p.path.clone(),
            number: p.number,
            title: p.title.clone(),
            url: p.url.clone(),
            created_at: p.created_at,
            state: p.state.clone(),
            merged: p.merged,
            merged_at: p.merged_at,
            closed_at: p.closed_at,
            minutes_to_merge: p.merged_at.map(|m| (m - p.created_at).num_minutes()),
            additions: p.additions,
            deletions: p.deletions,
            commits: i64::try_from(p.hashes.len()).unwrap_or(i64::MAX),
            model: p.family.clone(),
            by: p.by,
            session: p.session.as_ref().and_then(|s| trail.kernel_sessions.get(s)).cloned(),
            ci_failed: p.ci_failed,
            ci_passed: p.ci_passed,
            ci_first: p.ci_first,
        })
        .collect();

    let mut repo_weeks: BTreeMap<(String, String), RepoWeek> = BTreeMap::new();
    let mut repo_week = |at: DateTime<Utc>, repo: String| -> (String, String) {
        let key = (week_of(at), repo);
        repo_weeks.entry(key.clone()).or_insert_with(|| RepoWeek { week: key.0.clone(), repo: key.1.clone(), ..RepoWeek::default() });
        key
    };
    let mut keyed: Vec<((String, String), Seen)> = Vec::new();
    for m in &trail.moves {
        keyed.push((repo_week(m.at, name(Some(&m.repo))), Seen::Move(m.kind)));
    }
    let mut ci_kinds: BTreeMap<String, i64> = BTreeMap::new();
    let mut workflows: BTreeMap<(String, String, String), Workflow> = BTreeMap::new();
    for run in &trail.ci {
        *ci_kinds.entry(run.kind.clone()).or_default() += 1;
        if !host_run(&run.kind) {
            continue;
        }
        let repo = name(run.repo.as_ref());
        let w = workflows.entry((repo.clone(), run.name.clone(), run.kind.clone())).or_insert_with(|| Workflow {
            repo: repo.clone(),
            name: run.name.clone(),
            kind: run.kind.clone(),
            ..Workflow::default()
        });
        w.runs += 1;
        w.passed += i64::from(run.verdict == Some(true));
        w.failed += i64::from(run.verdict == Some(false));
        w.turned += i64::from(run.failed_before);
        if let Some(at) = run.created_at {
            keyed.push((repo_week(at, repo), Seen::Run(run.verdict, run.failed_before)));
        }
    }
    for (key, seen) in keyed {
        let Some(w) = repo_weeks.get_mut(&key) else { continue };
        match seen {
            Seen::Move(kind) => {
                *w.moves.entry(kind.to_string()).or_default() += 1;
                w.rewrites += i64::from(rewrites(kind));
            }
            Seen::Run(verdict, turned) => {
                w.ci_passed += i64::from(verdict == Some(true));
                w.ci_failed += i64::from(verdict == Some(false) || turned);
                w.ci_turned += i64::from(turned);
            }
        }
    }

    Findings {
        commits,
        pulls,
        weeks: weeks.into_values().collect(),
        repo_weeks: repo_weeks.into_values().collect(),
        workflows: workflows.into_values().collect(),
        ci_kinds,
        folders: trail.map.folders.iter().map(|(folder, repo)| Folder { folder: folder.clone(), repo: repo.clone() }).collect(),
        computed_at: Utc::now().to_rfc3339(),
    }
}
