//! A window on the repositories (#539): what was done in each between two
//! times. Who worked there (agents, models, sessions, the operator's
//! prompts), what it cost, what changed file by file, the commits and what
//! became of them, the pull requests opened, merged or closed, CI, the
//! refs' moves, the tests and builds run, and the git commands the agents
//! ran; each count with the rows behind it, and a timeline in the steps the
//! page asks for. Work whose session was in no repository is its own row,
//! named empty, so every reply in the window is somewhere.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use super::compare::says_word;
use super::findings::host_run;
use super::trail::{rewrites, Trail};
use super::{int, text, DayFile, PAGE};
use crate::crawler::hosts::held::{record, row_key};
use crate::params::Params;

/// A repository's numbers in the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub sessions: i64,
    pub agents: i64,
    pub models: i64,
    pub replies: i64,
    pub subagent_replies: i64,
    pub out_tokens: i64,
    pub context_tokens: i64,
    pub thinking_ms: i64,
    pub tool_calls: i64,
    pub active_secs: i64,
    pub prompts: i64,
    pub corrections: i64,
    pub escalations: i64,
    pub interrupts: i64,
    pub edits: i64,
    pub edit_files: i64,
    pub edit_added: i64,
    pub edit_removed: i64,
    pub commits: i64,
    pub commit_added: i64,
    pub commit_removed: i64,
    pub commits_mainline: i64,
    pub commits_in_flight: i64,
    pub commits_branch: i64,
    pub commits_recovered: i64,
    pub landed: i64,
    pub alive: i64,
    pub reverted: i64,
    pub prs_opened: i64,
    pub prs_merged: i64,
    pub prs_closed: i64,
    pub ci_passed: i64,
    pub ci_failed: i64,
    pub moves: i64,
    pub rewrites: i64,
    pub pushes: i64,
    pub tests: i64,
    pub tests_failed: i64,
    pub builds: i64,
    pub builds_failed: i64,
}

/// One model in a repository in the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Model {
    pub model: String,
    pub sessions: i64,
    pub replies: i64,
    pub out_tokens: i64,
    pub context_tokens: i64,
    pub thinking_ms: i64,
    pub tool_calls: i64,
    pub active_secs: i64,
    pub edit_added: i64,
    pub edit_removed: i64,
    pub commits: i64,
    pub commit_added: i64,
    pub commit_removed: i64,
    pub landed: i64,
    pub alive: i64,
    pub prompts: i64,
    pub corrections: i64,
    pub escalations: i64,
    pub tests: i64,
    pub tests_failed: i64,
}

/// One step of the timeline, for one model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Step {
    pub start: DateTime<Utc>,
    pub model: String,
    pub replies: i64,
    pub out_tokens: i64,
    pub tool_calls: i64,
    pub edit_added: i64,
    pub edit_removed: i64,
    pub commits: i64,
    pub commit_added: i64,
    pub commit_removed: i64,
    pub prompts: i64,
}

/// A session that worked in the repository in the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Session {
    pub session: String,
    pub agent: String,
    pub models: Vec<String>,
    pub first: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
    pub replies: i64,
    pub tool_calls: i64,
    pub out_tokens: i64,
    pub active_secs: i64,
    pub edit_added: i64,
    pub edit_removed: i64,
    pub prompts: i64,
    pub commits: i64,
    pub tests: i64,
    pub builds: i64,
}

/// A file changed in the window, by the agents' edits and by commits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct File {
    pub path: String,
    pub models: Vec<String>,
    pub edits: i64,
    pub edit_added: i64,
    pub edit_removed: i64,
    pub commits: i64,
    pub commit_added: i64,
    pub commit_removed: i64,
}

/// A commit made in the window and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub at: DateTime<Utc>,
    pub author_name: String,
    pub ours: bool,
    pub squash: bool,
    pub model: String,
    pub by: Option<&'static str>,
    pub session: Option<String>,
    pub added: i64,
    pub removed: i64,
    pub place: Option<String>,
    pub landed: i64,
    pub alive: Option<i64>,
    pub never_landed: i64,
    pub reverted: bool,
    pub ci: Option<bool>,
    pub pulls: Vec<i64>,
    pub tested: Option<bool>,
    pub files: Vec<DayFile>,
}

/// An operator's message in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Prompt {
    pub at: DateTime<Utc>,
    pub session: Option<String>,
    pub model: String,
    pub kind: String,
    pub text: String,
    pub correction: bool,
    pub escalation: bool,
}

/// A pull request opened, merged or closed in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pull {
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub merged: bool,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub additions: i64,
    pub deletions: i64,
    pub model: Option<String>,
    pub ci_failed: i64,
    pub ci_passed: i64,
    pub ci_first: Option<bool>,
    /// What happened to it in the window: opened, merged, closed.
    pub happened: Vec<&'static str>,
}

/// A CI run started in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub name: String,
    pub kind: String,
    pub verdict: Option<bool>,
    pub failed_before: bool,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub sha: Option<String>,
    pub git_ref: Option<String>,
    pub pull: Option<i64>,
    pub url: Option<String>,
}

/// A ref's move in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Move {
    pub at: DateTime<Utc>,
    pub git_ref: String,
    pub kind: &'static str,
    pub action: String,
    pub identity: Option<String>,
    pub old: Option<String>,
    pub new: Option<String>,
    pub mirror: bool,
}

/// A test or build an agent ran in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub at: DateTime<Utc>,
    pub test: bool,
    pub what: String,
    pub command: String,
    pub passed: i64,
    pub failed: i64,
    pub ok: bool,
    pub session: Option<String>,
    pub model: String,
}

/// A git or host command an agent ran in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Git {
    pub at: DateTime<Utc>,
    pub action: String,
    pub hash: Option<String>,
    pub branch: Option<String>,
    pub subject: Option<String>,
    pub pr_number: Option<i64>,
    pub url: Option<String>,
    pub session: Option<String>,
    pub model: String,
}

/// One repository in the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Repo {
    /// Empty for work whose session was in no repository.
    pub repo: String,
    pub totals: Totals,
    pub agents: BTreeMap<String, i64>,
    pub models: Vec<Model>,
    pub timeline: Vec<Step>,
    pub sessions: Vec<Session>,
    pub files: Vec<File>,
    pub commits: Vec<Commit>,
    pub prompts: Vec<Prompt>,
    pub pulls: Vec<Pull>,
    pub ci: Vec<Run>,
    pub moves: Vec<Move>,
    pub checks: Vec<Check>,
    pub git: Vec<Git>,
}

/// The window answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub step_secs: i64,
    pub repos: Vec<Repo>,
    pub computed_at: String,
}

#[derive(Default)]
struct Acc {
    repo: Repo,
    agents: BTreeMap<String, BTreeSet<String>>,
    models: BTreeMap<String, Model>,
    model_sessions: BTreeMap<String, BTreeSet<String>>,
    sessions: BTreeMap<String, Session>,
    session_models: BTreeMap<String, BTreeSet<String>>,
    files: BTreeMap<String, File>,
    file_models: BTreeMap<String, BTreeSet<String>>,
    timeline: BTreeMap<(DateTime<Utc>, String), Step>,
    commits: Vec<usize>,
}

impl Acc {
    fn model(&mut self, model: &str) -> &mut Model {
        self.models.entry(model.to_string()).or_insert_with(|| Model { model: model.to_string(), ..Model::default() })
    }

    fn step(&mut self, start: DateTime<Utc>, model: &str) -> &mut Step {
        self.timeline
            .entry((start, model.to_string()))
            .or_insert_with(|| Step { start, model: model.to_string(), ..Step::default() })
    }

    fn file(&mut self, path: &str, model: &str) -> &mut File {
        if !model.is_empty() {
            self.file_models.entry(path.to_string()).or_default().insert(model.to_string());
        }
        self.files.entry(path.to_string()).or_insert_with(|| File { path: path.to_string(), ..File::default() })
    }
}

/// What was done in each repository from `from` to `to`, its timeline in
/// steps of `step_secs`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer the
/// window's commits' files.
#[allow(clippy::too_many_lines)] // skill-allow: allow — one walk over each kind of fact, each step named
pub async fn window(
    db: &Db,
    trail: &Trail,
    params: &Params,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    step_secs: i64,
) -> Result<Window> {
    let step_secs = step_secs.max(1);
    let inside = |at: DateTime<Utc>| from <= at && at < to;
    let start = |at: DateTime<Utc>| from + TimeDelta::seconds((at - from).num_seconds() / step_secs * step_secs);
    let name = |repo: Option<&String>| trail.map.name(repo);
    let kernel = |s: &String| trail.kernel_sessions.get(s).cloned();
    let mut accs: BTreeMap<String, Acc> = BTreeMap::new();

    for r in trail.replies.iter().filter(|r| inside(r.at)) {
        let a = accs.entry(name(r.repo.as_ref())).or_default();
        let t = &mut a.repo.totals;
        t.replies += 1;
        t.subagent_replies += i64::from(r.subagent);
        t.out_tokens += r.out_tokens;
        t.context_tokens += r.context;
        t.thinking_ms += r.thinking_ms;
        t.tool_calls += r.tool_calls;
        t.active_secs += r.active_secs;
        let m = a.model(&r.family);
        m.replies += 1;
        m.out_tokens += r.out_tokens;
        m.context_tokens += r.context;
        m.thinking_ms += r.thinking_ms;
        m.tool_calls += r.tool_calls;
        m.active_secs += r.active_secs;
        a.model_sessions.entry(r.family.clone()).or_default().insert(r.session.clone());
        a.session_models.entry(r.session.clone()).or_default().insert(r.family.clone());
        a.agents.entry(trail.agents.get(&r.session).cloned().unwrap_or_default()).or_default().insert(r.session.clone());
        let s = a.sessions.entry(r.session.clone()).or_insert_with(|| Session {
            session: kernel(&r.session).unwrap_or_default(),
            agent: trail.agents.get(&r.session).cloned().unwrap_or_default(),
            ..Session::default()
        });
        s.replies += 1;
        s.tool_calls += r.tool_calls;
        s.out_tokens += r.out_tokens;
        s.active_secs += r.active_secs;
        s.first = s.first.min(Some(r.at)).or(Some(r.at));
        s.last = s.last.max(Some(r.at));
        let p = a.step(start(r.at), &r.family);
        p.replies += 1;
        p.out_tokens += r.out_tokens;
        p.tool_calls += r.tool_calls;
    }

    for e in &trail.edits {
        let Some(r) = e.reply.and_then(|i| trail.replies.get(i)) else { continue };
        let at = e.at.unwrap_or(r.at);
        if !inside(at) {
            continue;
        }
        let a = accs.entry(name(r.repo.as_ref())).or_default();
        a.repo.totals.edits += 1;
        a.repo.totals.edit_added += e.added;
        a.repo.totals.edit_removed += e.removed;
        let m = a.model(&r.family);
        m.edit_added += e.added;
        m.edit_removed += e.removed;
        if let Some(s) = a.sessions.get_mut(&r.session) {
            s.edit_added += e.added;
            s.edit_removed += e.removed;
        }
        let f = a.file(&e.path, &r.family);
        f.edits += 1;
        f.edit_added += e.added;
        f.edit_removed += e.removed;
        let p = a.step(start(at), &r.family);
        p.edit_added += e.added;
        p.edit_removed += e.removed;
    }

    for p in trail.prompts.iter().filter(|p| inside(p.at)) {
        let a = accs.entry(name(p.repo.as_ref())).or_default();
        let model = p.family.clone().unwrap_or_default();
        let typed = matches!(p.kind.as_str(), "typed" | "queued");
        let correction = typed && says_word(&p.text, &params.correction_words, false);
        let escalation = typed && says_word(&p.text, &params.escalation_words, true);
        if typed {
            a.repo.totals.prompts += 1;
            a.repo.totals.corrections += i64::from(correction);
            a.repo.totals.escalations += i64::from(escalation);
            let m = a.model(&model);
            m.prompts += 1;
            m.corrections += i64::from(correction);
            m.escalations += i64::from(escalation);
            if let Some(s) = a.sessions.get_mut(&p.session) {
                s.prompts += 1;
            }
            a.step(start(p.at), &model).prompts += 1;
        } else if p.kind == "interrupt" {
            a.repo.totals.interrupts += 1;
        }
        a.repo.prompts.push(Prompt {
            at: p.at,
            session: kernel(&p.session),
            model,
            kind: p.kind.clone(),
            text: p.text.clone(),
            correction,
            escalation,
        });
    }

    for c in trail.checks.iter().filter(|c| inside(c.at)) {
        let r = c.reply.and_then(|i| trail.replies.get(i));
        let a = accs.entry(name(r.and_then(|r| r.repo.as_ref()))).or_default();
        let model = r.map(|r| r.family.clone()).unwrap_or_default();
        if c.test {
            a.repo.totals.tests += 1;
            a.repo.totals.tests_failed += i64::from(c.failed > 0);
            let m = a.model(&model);
            m.tests += 1;
            m.tests_failed += i64::from(c.failed > 0);
        } else {
            a.repo.totals.builds += 1;
            a.repo.totals.builds_failed += i64::from(!c.ok);
        }
        if let Some(s) = r.and_then(|r| a.sessions.get_mut(&r.session)) {
            if c.test {
                s.tests += 1;
            } else {
                s.builds += 1;
            }
        }
        a.repo.checks.push(Check {
            at: c.at,
            test: c.test,
            what: c.what.clone(),
            command: c.command.clone(),
            passed: c.passed,
            failed: c.failed,
            ok: c.ok,
            session: r.and_then(|r| kernel(&r.session)),
            model,
        });
    }

    for g in trail.git.iter().filter(|g| inside(g.at)) {
        let r = g.reply.and_then(|i| trail.replies.get(i));
        let a = accs.entry(name(r.and_then(|r| r.repo.as_ref()))).or_default();
        a.repo.git.push(Git {
            at: g.at,
            action: g.action.clone(),
            hash: g.hash.clone(),
            branch: g.branch.clone(),
            subject: g.subject.clone(),
            pr_number: g.pr_number,
            url: g.url.clone(),
            session: r.and_then(|r| kernel(&r.session)),
            model: r.map(|r| r.family.clone()).unwrap_or_default(),
        });
    }

    // The window's commits, and their files.
    let mut ids: Vec<RecordId> = Vec::new();
    for (i, c) in trail.commits.iter().enumerate().filter(|(_, c)| inside(c.at)) {
        let a = accs.entry(name(Some(&c.repo))).or_default();
        a.commits.push(i);
        ids.push(c.id.clone());
        let t = &mut a.repo.totals;
        t.commits += 1;
        t.commit_added += c.added;
        t.commit_removed += c.removed;
        match c.place.as_deref() {
            Some("mainline") => t.commits_mainline += 1,
            Some("in_flight") => t.commits_in_flight += 1,
            Some("branch") => t.commits_branch += 1,
            Some("recovered") => t.commits_recovered += 1,
            _ => {}
        }
        if !c.squash {
            t.landed += c.landed;
            t.alive += c.alive.unwrap_or(0);
        }
        t.reverted += i64::from(c.reverted);
        let model = c.family.clone().unwrap_or_default();
        if c.family.is_some() {
            let m = a.model(&model);
            m.commits += 1;
            m.commit_added += c.added;
            m.commit_removed += c.removed;
            m.landed += c.landed;
            m.alive += c.alive.unwrap_or(0);
        }
        if let Some(s) = c.session.as_ref().and_then(|s| a.sessions.get_mut(s)) {
            s.commits += 1;
        }
        let p = a.step(start(c.at), &model);
        p.commits += 1;
        p.commit_added += c.added;
        p.commit_removed += c.removed;
    }
    let mut files: BTreeMap<String, Vec<DayFile>> = BTreeMap::new();
    for chunk in ids.chunks(usize::try_from(PAGE).unwrap_or(usize::MAX)) {
        let rows: Vec<Value> = db
            .query("SELECT commit, path, change, added, removed FROM commit_file WHERE commit INSIDE $ids")
            .bind(("ids", chunk.to_vec()))
            .await?
            .take(0)?;
        for f in &rows {
            let Some(c) = record(f, "commit") else { continue };
            files.entry(row_key(&c)).or_default().push(DayFile {
                path: text(f, "path").unwrap_or_default(),
                change: text(f, "change").unwrap_or_default(),
                added: int(f, "added"),
                removed: int(f, "removed"),
            });
        }
    }
    for a in accs.values_mut() {
        let mine = std::mem::take(&mut a.commits);
        for i in mine {
            let c = &trail.commits[i];
            let model = c.family.clone().unwrap_or_default();
            let mut its = files.get(&c.key).cloned().unwrap_or_default();
            its.sort_by(|x, y| (y.added + y.removed).cmp(&(x.added + x.removed)).then_with(|| x.path.cmp(&y.path)));
            for f in &its {
                let file = a.file(&f.path, &model);
                file.commits += 1;
                file.commit_added += f.added;
                file.commit_removed += f.removed;
            }
            a.repo.commits.push(Commit {
                hash: c.hash.clone(),
                subject: c.subject.clone(),
                at: c.at,
                author_name: c.author_name.clone(),
                ours: c.ours,
                squash: c.squash,
                model,
                by: c.by,
                session: c.session.as_ref().and_then(kernel),
                added: c.added,
                removed: c.removed,
                place: c.place.clone(),
                landed: c.landed,
                alive: c.alive,
                never_landed: c.never_landed,
                reverted: c.reverted,
                ci: c.ci,
                pulls: c.pulls.iter().filter_map(|&p| trail.pulls.get(p)).map(|p| p.number).collect(),
                tested: c.tested,
                files: its,
            });
        }
    }

    for p in &trail.pulls {
        let mut happened: Vec<&'static str> = Vec::new();
        if inside(p.created_at) {
            happened.push("opened");
        }
        if p.merged_at.is_some_and(inside) {
            happened.push("merged");
        } else if !p.merged && p.closed_at.is_some_and(inside) {
            happened.push("closed");
        }
        if happened.is_empty() {
            continue;
        }
        let a = accs.entry(name(p.repo.as_ref())).or_default();
        a.repo.totals.prs_opened += i64::from(happened.contains(&"opened"));
        a.repo.totals.prs_merged += i64::from(happened.contains(&"merged"));
        a.repo.totals.prs_closed += i64::from(happened.contains(&"closed"));
        a.repo.pulls.push(Pull {
            number: p.number,
            title: p.title.clone(),
            url: p.url.clone(),
            state: p.state.clone(),
            merged: p.merged,
            created_at: p.created_at,
            merged_at: p.merged_at,
            closed_at: p.closed_at,
            additions: p.additions,
            deletions: p.deletions,
            model: p.family.clone(),
            ci_failed: p.ci_failed,
            ci_passed: p.ci_passed,
            ci_first: p.ci_first,
            happened,
        });
    }

    for run in &trail.ci {
        if !run.created_at.is_some_and(inside) {
            continue;
        }
        let a = accs.entry(name(run.repo.as_ref())).or_default();
        if host_run(&run.kind) {
            a.repo.totals.ci_passed += i64::from(run.verdict == Some(true));
            a.repo.totals.ci_failed += i64::from(run.verdict == Some(false) || run.failed_before);
        }
        a.repo.ci.push(Run {
            name: run.name.clone(),
            kind: run.kind.clone(),
            verdict: run.verdict,
            failed_before: run.failed_before,
            status: run.status.clone(),
            conclusion: run.conclusion.clone(),
            created_at: run.created_at,
            completed_at: run.completed_at,
            sha: run.sha.clone(),
            git_ref: run.git_ref.clone(),
            pull: run.pull.and_then(|p| trail.pulls.get(p)).map(|p| p.number),
            url: run.url.clone(),
        });
    }

    for m in trail.moves.iter().filter(|m| inside(m.at)) {
        let a = accs.entry(name(Some(&m.repo))).or_default();
        a.repo.totals.moves += 1;
        a.repo.totals.rewrites += i64::from(rewrites(m.kind));
        a.repo.totals.pushes += i64::from(m.kind == "push");
        a.repo.moves.push(Move {
            at: m.at,
            git_ref: m.git_ref.clone(),
            kind: m.kind,
            action: m.action.clone(),
            identity: m.identity.clone(),
            old: m.old.clone(),
            new: m.new.clone(),
            mirror: m.mirror,
        });
    }

    let mut repos: Vec<Repo> = accs
        .into_iter()
        .map(|(repo, mut a)| {
            for (model, sessions) in std::mem::take(&mut a.model_sessions) {
                a.model(&model).sessions = i64::try_from(sessions.len()).unwrap_or(i64::MAX);
            }
            for (session, s) in &mut a.sessions {
                s.models = a.session_models.get(session).map(|m| m.iter().cloned().collect()).unwrap_or_default();
            }
            for (path, f) in &mut a.files {
                f.models = a.file_models.get(path).map(|m| m.iter().cloned().collect()).unwrap_or_default();
            }
            let mut out = a.repo;
            out.repo = repo;
            out.totals.sessions = i64::try_from(a.sessions.len()).unwrap_or(i64::MAX);
            out.totals.agents = i64::try_from(a.agents.len()).unwrap_or(i64::MAX);
            out.totals.models = i64::try_from(a.models.len()).unwrap_or(i64::MAX);
            out.totals.edit_files = i64::try_from(a.files.values().filter(|f| f.edits > 0).count()).unwrap_or(i64::MAX);
            out.agents = a.agents.into_iter().map(|(agent, s)| (agent, i64::try_from(s.len()).unwrap_or(i64::MAX))).collect();
            out.models = a.models.into_values().collect();
            out.models.sort_by(|x, y| y.replies.cmp(&x.replies).then_with(|| x.model.cmp(&y.model)));
            out.timeline = a.timeline.into_values().collect();
            out.sessions = a.sessions.into_values().collect();
            out.sessions.sort_by_key(|s| s.first);
            out.files = a.files.into_values().collect();
            out.files.sort_by(|x, y| {
                (y.edit_added + y.edit_removed + y.commit_added + y.commit_removed)
                    .cmp(&(x.edit_added + x.edit_removed + x.commit_added + x.commit_removed))
                    .then_with(|| x.path.cmp(&y.path))
            });
            out
        })
        .collect();
    repos.sort_by(|x, y| {
        (y.totals.replies + y.totals.commits + y.totals.moves)
            .cmp(&(x.totals.replies + x.totals.commits + x.totals.moves))
            .then_with(|| x.repo.cmp(&y.repo))
    });
    Ok(Window { from, to, step_secs, repos, computed_at: Utc::now().to_rfc3339() })
}
