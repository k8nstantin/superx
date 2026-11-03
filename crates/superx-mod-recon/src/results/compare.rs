//! Model comparison (#539): which model to reach for, and what choosing the
//! other one costs. The measures and their arithmetic are mod-ui's model
//! comparison (#406, #414), so the two pages read against each other; the
//! facts are recon's own: the sessions' replies, tokens and prompts, and
//! where each session worked, correlated with the commits the repositories'
//! crawls read, placed, landed and blamed.
//!
//! - **A run** is a stretch of one session's replies by one model family;
//!   its time is the gaps between its replies, each capped at
//!   attr_recon_active_secs. Each reply worked where its session's place
//!   was at that moment.
//! - **A claim** is a run's span in a repository it worked in. A commit by
//!   the identity the repository's crawl commits as is credited to the one
//!   model whose claim covers it; none, or several, credits no one. A commit
//!   a session printed is that session's model's by evidence, before any
//!   claim.
//! - **Landed** is what reached the main line: its commits, and the branch
//!   commits whose files' versions the main line carries (a squash or a
//!   replay), credited by when the branch work was done. **Never landed** is
//!   the rest of the branch work, in flight while a checkout still has it.
//! - **A repository whose main line took none of this identity's work while
//!   its branches took some is not judged.**
//! - Tokens, minutes and messages count the work done in judged
//!   repositories; turns and corrections count every run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use superx_kernel::types::Value;
use superx_kernel::{Db, Result};

use super::{int, model_of, owned, text};
use crate::crawler::hosts::held::{record, row_key, time};
use crate::params::Params;

/// One model's row, every rate from its own summed numerator and
/// denominator (mod-ui's `Deviation`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Deviation {
    pub model: String,
    pub commits: i64,
    pub added: i64,
    pub removed: i64,
    pub alive: i64,
    pub survived_pct: i64,
    pub removed_per_100_added: i64,
    pub rework_commits: i64,
    pub rework_pct: i64,
    pub thrash_files: i64,
    pub thrash_per_100_commits: i64,
    pub multi_dir_commits: i64,
    pub multi_dir_pct: i64,
    pub dirs_per_commit_x10: i64,
    pub operator_turns: i64,
    pub corrections: i64,
    pub corrections_per_100: i64,
    pub escalations: i64,
    pub out_tokens: i64,
    pub messages: i64,
    pub runs: i64,
    pub thrown: i64,
    pub tokens_thrown: i64,
    pub tokens_per_line_landed: i64,
    pub tokens_per_line_kept: i64,
    pub median_age_days: i64,
    pub context_avg: i64,
    pub context_peak: i64,
    pub minutes: i64,
    pub minutes_thrown: i64,
    pub alive_per_mtok: i64,
    pub alive_per_hour: i64,
    pub abandoned_commits: i64,
    pub abandoned_lines: i64,
    pub abandoned_pct: i64,
    pub in_flight_lines: i64,
}

/// One model in one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepoModel {
    pub repo: String,
    pub model: String,
    pub commits: i64,
    pub added: i64,
    pub alive: i64,
    pub removed: i64,
    pub survived_pct: i64,
    pub rework_commits: i64,
}

/// What one direction of switch led to, in the takeover window after it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Handoff {
    pub from: String,
    pub to: String,
    pub switches: i64,
    pub commits: i64,
    pub added: i64,
    pub removed: i64,
    pub alive: i64,
    pub survived_pct: i64,
}

/// A repository the comparison could not judge, and why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UnjudgedRepo {
    pub repo: String,
    pub mainline: String,
    pub off_mainline_commits: i64,
    pub unresolved: bool,
}

/// The model-comparison answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Comparison {
    pub handoffs: Vec<Handoff>,
    pub deviations: Vec<Deviation>,
    pub repos: Vec<RepoModel>,
    pub unjudged: Vec<UnjudgedRepo>,
    pub computed_at: String,
}

/// A model's family, as mod-ui names it: `claude-opus-5-5` is `opus 5.5`,
/// a dated snapshot is its version, `gemini-3.1-pro-preview` is
/// `gemini 3.1 pro`.
#[must_use]
pub fn family(model: &str) -> String {
    let (gemini, rest) = match model.strip_prefix("gemini-") {
        Some(rest) => (true, rest),
        None => (false, model.strip_prefix("claude-").unwrap_or(model)),
    };
    let words: Vec<&str> = rest.split('-').collect();
    let name = words.iter().find(|w| w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())).copied();
    let version = words.iter().skip_while(|w| !version_word(w)).take_while(|w| version_word(w)).copied().collect::<Vec<_>>().join(".");
    let parts: Vec<&str> = if gemini {
        ["gemini", version.as_str(), name.unwrap_or_default()].into()
    } else {
        [name.unwrap_or(rest), version.as_str()].into()
    };
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// A word of a version — `5`, `3.1` — and not a date, a build or a
/// preview's day: digits and dots, no run of more than two digits.
fn version_word(w: &str) -> bool {
    w.chars().next().is_some_and(|c| c.is_ascii_digit())
        && w.chars().all(|c| c.is_ascii_digit() || c == '.')
        && w.split('.').all(|d| !d.is_empty() && d.len() <= 2)
}

/// Whether `text` says one of `markers`: as a whole word, or, with `stem`,
/// as the start of one.
#[must_use]
pub fn says_word(text: &str, markers: &[String], stem: bool) -> bool {
    let low = text.to_lowercase();
    let bounded = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    markers.iter().any(|m| {
        let m = m.to_lowercase();
        !m.is_empty()
            && low.match_indices(m.as_str()).any(|(i, _)| {
                bounded(low[..i].chars().next_back()) && (stem || bounded(low[i + m.len()..].chars().next()))
            })
    })
}

/// A subject that says the work is being done again.
#[must_use]
pub fn is_rework(subject: &str, words: &[String]) -> bool {
    words.iter().filter(|w| !w.is_empty()).any(|w| subject.starts_with(w.as_str()) || subject.contains(&format!(" {w}")))
}

/// The top-level directory of a path: the unit of "was this the thing I was
/// asked to touch".
fn top_dir(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
}

/// Split `total` over `weights` in proportion, in whole units, so the shares
/// add up to `total` exactly; the remainder goes to the largest.
#[must_use]
pub fn split(total: i64, weights: &[i64]) -> Vec<i64> {
    let sum: i64 = weights.iter().sum();
    if sum <= 0 {
        return vec![0; weights.len()];
    }
    let mut out: Vec<i64> = weights.iter().map(|w| total * w / sum).collect();
    let rest = total - out.iter().sum::<i64>();
    if let Some(i) = (0..weights.len()).max_by_key(|&i| weights[i]) {
        out[i] += rest;
    }
    out
}

/// `num` as a whole percentage of `den`, rounded to the nearest, as the page
/// rounds.
#[must_use]
pub fn pct(num: i64, den: i64) -> i64 {
    if den > 0 { (200 * num + den) / (2 * den) } else { 0 }
}

/// Each session's places, oldest first: when, and the repository there.
type Places = HashMap<String, Vec<(DateTime<Utc>, Option<String>)>>;

/// Each main-line commit's share of branch work: when it was done, its
/// weight, and the model a printed tie gives it.
type Shares = HashMap<String, Vec<(DateTime<Utc>, i64, Option<String>)>>;

/// A run's span in a repository.
#[derive(Debug, Clone)]
struct Claim {
    model: String,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

/// The one model whose claim covers `at`; none when none does or several do.
fn credit(claims: &[Claim], at: DateTime<Utc>) -> Option<&str> {
    let mut hit: Vec<&str> = claims.iter().filter(|c| c.from <= at && at <= c.to).map(|c| c.model.as_str()).collect();
    hit.sort_unstable();
    hit.dedup();
    match hit.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Reply {
    key: String,
    at: DateTime<Utc>,
    family: String,
}

#[derive(Debug, Clone)]
struct Run {
    family: String,
    first: DateTime<Utc>,
    last: DateTime<Utc>,
    /// Indices into the session's replies.
    replies: Vec<usize>,
    turns: i64,
    corrections: i64,
    escalations: i64,
}

#[derive(Debug, Clone)]
struct Commit {
    key: String,
    author: String,
    at: DateTime<Utc>,
    added: i64,
    removed: i64,
    subject: String,
    place: Option<String>,
    files: Vec<(String, i64, i64)>,
}

#[derive(Default)]
struct DevAcc {
    commits: i64,
    added: i64,
    removed: i64,
    alive: i64,
    rework_commits: i64,
    thrash_files: i64,
    multi_dir_commits: i64,
    dir_spread: i64,
    ages: Vec<i64>,
    abandoned_commits: i64,
    abandoned_lines: i64,
    in_flight_lines: i64,
}

#[derive(Default, Clone)]
struct SpendAcc {
    out_tokens: i64,
    messages: i64,
    runs: i64,
    operator_turns: i64,
    corrections: i64,
    escalations: i64,
    context_weighted: i64,
    context_msgs: i64,
    context_peak: i64,
    seconds: i64,
}

#[derive(Default)]
struct HandAcc {
    switches: i64,
    commits: i64,
    added: i64,
    removed: i64,
    alive: i64,
}

/// The comparison over every run and every repository recon holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
#[allow(clippy::too_many_lines)] // skill-allow: allow — one walk over every fact, as mod-ui's, so every measure comes from one set of numbers
pub async fn compare(db: &Db, params: &Params) -> Result<Comparison> {
    let mut answer = db
        .query(
            "SELECT id, session, at, model FROM reply;\n\
             SELECT reply, valid_from, output_tokens, input_tokens, cache_read_tokens, cache_write_tokens FROM reply_state;\n\
             SELECT session, at, text FROM prompt;\n\
             SELECT session, cwd, at FROM session_place;\n\
             SELECT path, checkout.repository AS repository FROM directory;\n\
             SELECT toplevel, repository FROM checkout;\n\
             SELECT id, name FROM repository;\n\
             SELECT repository, identity, mainline, started_at FROM crawl;\n\
             SELECT id, repository, author, committed_at, added, removed, subject FROM commit;\n\
             SELECT commit, place, valid_from FROM placement;\n\
             SELECT commit, path, via FROM landed_via;\n\
             SELECT commit, alive, valid_from FROM survival;\n\
             SELECT commit, path, added, removed FROM commit_file;\n\
             SELECT commit, commit.committed_at AS at, tool_call.reply.model AS model, at AS tied_at FROM commit_session;",
        )
        .await?;
    let replies: Vec<Value> = answer.take(0)?;
    let mut states: Vec<Value> = answer.take(1)?;
    let prompts: Vec<Value> = answer.take(2)?;
    let places: Vec<Value> = answer.take(3)?;
    let directories: Vec<Value> = answer.take(4)?;
    let checkouts: Vec<Value> = answer.take(5)?;
    let repositories: Vec<Value> = answer.take(6)?;
    let mut crawls: Vec<Value> = answer.take(7)?;
    let commit_rows: Vec<Value> = answer.take(8)?;
    let mut placements: Vec<Value> = answer.take(9)?;
    let landed_rows: Vec<Value> = answer.take(10)?;
    let mut survival: Vec<Value> = answer.take(11)?;
    let file_rows: Vec<Value> = answer.take(12)?;
    let ties: Vec<Value> = answer.take(13)?;

    // Each reply's newest usage: its output, and the context it carried.
    states.sort_by_key(|s| std::cmp::Reverse(time(s, "valid_from")));
    let mut usage: HashMap<String, (i64, i64)> = HashMap::new();
    for s in &states {
        let Some(reply) = record(s, "reply") else { continue };
        usage.entry(row_key(&reply)).or_insert_with(|| {
            (int(s, "output_tokens"), int(s, "input_tokens") + int(s, "cache_read_tokens") + int(s, "cache_write_tokens"))
        });
    }

    // Where a directory is: its own row's repository, else the checkout
    // whose top level holds it, the deepest.
    let names: HashMap<String, String> = repositories
        .iter()
        .filter_map(|r| Some((row_key(&record(r, "id")?), text(r, "name").unwrap_or_default())))
        .collect();
    let exact: HashMap<String, String> = directories
        .iter()
        .filter_map(|d| Some((text(d, "path")?, row_key(&record(d, "repository")?))))
        .collect();
    let mut tops: Vec<(String, String)> =
        checkouts.iter().filter_map(|c| Some((text(c, "toplevel")?, row_key(&record(c, "repository")?)))).collect();
    tops.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));
    let repo_of = |cwd: &str| -> Option<String> {
        exact.get(cwd).cloned().or_else(|| {
            tops.iter()
                .find(|(t, _)| cwd == t || cwd.strip_prefix(t.as_str()).is_some_and(|rest| rest.starts_with('/')))
                .map(|(_, r)| r.clone())
        })
    };

    // Replies by session, oldest first; each session's places likewise.
    let mut by_session: BTreeMap<String, Vec<Reply>> = BTreeMap::new();
    for r in &replies {
        let (Some(id), Some(session), Some(at)) = (record(r, "id"), record(r, "session"), time(r, "at")) else { continue };
        let session = row_key(&session);
        by_session.entry(session).or_default().push(Reply { key: row_key(&id), at, family: family(&model_of(r)) });
    }
    let mut place_of: Places = HashMap::new();
    for p in &places {
        let (Some(session), Some(at), Some(cwd)) = (record(p, "session"), time(p, "at"), text(p, "cwd")) else { continue };
        place_of.entry(row_key(&session)).or_default().push((at, repo_of(&cwd)));
    }
    for v in place_of.values_mut() {
        v.sort_by_key(|(at, _)| *at);
    }
    let mut prompts_of: HashMap<String, Vec<(DateTime<Utc>, String)>> = HashMap::new();
    for p in &prompts {
        let (Some(session), Some(at)) = (record(p, "session"), time(p, "at")) else { continue };
        prompts_of.entry(row_key(&session)).or_default().push((at, text(p, "text").unwrap_or_default()));
    }

    // Runs, each reply's repository, and the claims on each repository.
    let active = i64::try_from(params.active_secs).unwrap_or(i64::MAX);
    let mut runs: Vec<(String, Run)> = Vec::new();
    let mut reply_repo: HashMap<String, Option<String>> = HashMap::new();
    let mut claims: BTreeMap<String, Vec<Claim>> = BTreeMap::new();
    for (session, list) in &mut by_session {
        list.sort_by_key(|r| r.at);
        let here = place_of.get(session);
        let mut session_runs: Vec<Run> = Vec::new();
        for (i, r) in list.iter().enumerate() {
            let repo = here.and_then(|p| {
                p.iter().rev().find(|(at, _)| *at <= r.at).or_else(|| p.first()).and_then(|(_, repo)| repo.clone())
            });
            reply_repo.insert(r.key.clone(), repo);
            match session_runs.last_mut() {
                Some(run) if run.family == r.family => {
                    run.last = r.at;
                    run.replies.push(i);
                }
                _ => session_runs.push(Run {
                    family: r.family.clone(),
                    first: r.at,
                    last: r.at,
                    replies: vec![i],
                    turns: 0,
                    corrections: 0,
                    escalations: 0,
                }),
            }
        }
        // Each prompt to the run in force when it was typed, else the first.
        let starts: Vec<DateTime<Utc>> = session_runs.iter().map(|r| r.first).collect();
        for (at, said) in prompts_of.get(session).map(Vec::as_slice).unwrap_or_default() {
            let Some(i) = starts.iter().rposition(|s| s <= at).or((!session_runs.is_empty()).then_some(0)) else { continue };
            let run = &mut session_runs[i];
            run.turns += 1;
            run.corrections += i64::from(says_word(said, &params.correction_words, false));
            run.escalations += i64::from(says_word(said, &params.escalation_words, true));
        }
        for run in &session_runs {
            let worked: BTreeSet<String> =
                run.replies.iter().filter_map(|&i| reply_repo.get(&list[i].key).cloned().flatten()).collect();
            for repo in worked {
                claims.entry(repo).or_default().push(Claim { model: run.family.clone(), from: run.first, to: run.last });
            }
        }
        runs.extend(session_runs.into_iter().map(|r| (session.clone(), r)));
    }

    // The commits, their newest placement, their files, what landed them and
    // what of them is alive at the newest blame.
    placements.sort_by_key(|p| std::cmp::Reverse(time(p, "valid_from")));
    let mut place: HashMap<String, String> = HashMap::new();
    for p in &placements {
        if let (Some(c), Some(at)) = (record(p, "commit"), text(p, "place")) {
            place.entry(row_key(&c)).or_insert(at);
        }
    }
    survival.sort_by_key(|s| std::cmp::Reverse(time(s, "valid_from")));
    let mut alive_of: HashMap<String, i64> = HashMap::new();
    for s in &survival {
        if let Some(c) = record(s, "commit") {
            alive_of.entry(row_key(&c)).or_insert_with(|| int(s, "alive"));
        }
    }
    let mut files: HashMap<String, Vec<(String, i64, i64)>> = HashMap::new();
    for f in &file_rows {
        if let (Some(c), Some(path)) = (record(f, "commit"), text(f, "path")) {
            files.entry(row_key(&c)).or_default().push((path, int(f, "added"), int(f, "removed")));
        }
    }
    let mut landed_via: HashMap<(String, String), String> = HashMap::new();
    for l in &landed_rows {
        if let (Some(c), Some(path), Some(via)) = (record(l, "commit"), text(l, "path"), record(l, "via")) {
            landed_via.insert((row_key(&c), path), row_key(&via));
        }
    }
    crawls.sort_by_key(|c| std::cmp::Reverse(time(c, "started_at")));
    let mut identity: HashMap<String, String> = HashMap::new();
    let mut mainline_name: HashMap<String, String> = HashMap::new();
    for c in &crawls {
        let Some(repo) = record(c, "repository").map(|r| row_key(&r)) else { continue };
        if let Some(who) = text(c, "identity") {
            identity.entry(repo.clone()).or_insert(who);
        }
        if let Some(m) = text(c, "mainline") {
            mainline_name.entry(repo).or_insert(m);
        }
    }
    let tied: HashMap<String, String> =
        owned(ties).iter().filter_map(|t| Some((row_key(&record(t, "commit")?), family(&model_of(t))))).collect();
    let mut by_repo: BTreeMap<String, Vec<Commit>> = BTreeMap::new();
    for c in &commit_rows {
        let (Some(id), Some(repo), Some(at)) = (record(c, "id"), record(c, "repository"), time(c, "committed_at")) else { continue };
        let key = row_key(&id);
        by_repo.entry(row_key(&repo)).or_default().push(Commit {
            place: place.get(&key).cloned(),
            files: files.remove(&key).unwrap_or_default(),
            key,
            author: text(c, "author").unwrap_or_default(),
            at,
            added: int(c, "added"),
            removed: int(c, "removed"),
            subject: text(c, "subject").unwrap_or_default(),
        });
    }

    let bulk = |c: &Commit| c.added + c.removed > i64::try_from(params.bulk_lines).unwrap_or(i64::MAX);
    let takeover_secs = i64::try_from(params.takeover_secs).unwrap_or(i64::MAX);
    let thrash_at = i64::try_from(params.thrash_touches).unwrap_or(i64::MAX);
    let mut dev: HashMap<String, DevAcc> = HashMap::new();
    let mut hand: HashMap<(String, String), HandAcc> = HashMap::new();
    let mut per_repo: HashMap<(String, String), DevAcc> = HashMap::new();
    let mut unjudged: Vec<UnjudgedRepo> = Vec::new();
    let mut judged: HashSet<String> = HashSet::new();
    let empty: Vec<Commit> = Vec::new();
    let mut repos_claimed: Vec<&String> = claims.keys().collect();
    repos_claimed.sort_by_key(|k| names.get(*k).cloned().unwrap_or_default());
    for repo in repos_claimed {
        let rc = &claims[repo];
        let name = names.get(repo).cloned().unwrap_or_default();
        let commits = by_repo.get(repo).unwrap_or(&empty);
        let who = identity.get(repo);
        let ours = |c: &Commit| who.is_some_and(|w| *w == c.author) || tied.contains_key(&c.key);
        let off = |c: &Commit| matches!(c.place.as_deref(), Some("branch" | "in_flight" | "recovered"));
        // The model of a piece of work: the session that printed it, else
        // the one claim that covers its time.
        let model_at = |c: &Commit, at: DateTime<Utc>| tied.get(&c.key).cloned().or_else(|| credit(rc, at).map(str::to_string));

        // Branch work, per file: which main-line commit landed it. A bulk
        // drop's branch commit shares in nothing, and marks what it landed
        // through as no one's authored work.
        let mut shares: Shares = HashMap::new();
        let mut bulk_via: HashSet<String> = HashSet::new();
        for c in commits.iter().filter(|c| off(c) && ours(c)) {
            for (path, added, removed) in &c.files {
                if let Some(via) = landed_via.get(&(c.key.clone(), path.clone())) {
                    if bulk(c) {
                        bulk_via.insert(via.clone());
                    } else {
                        shares.entry(via.clone()).or_default().push((c.at, (added + removed).max(1), tied.get(&c.key).cloned()));
                    }
                }
            }
        }
        let off_ours = commits.iter().filter(|c| off(c) && ours(c) && c.place.as_deref() != Some("recovered")).count();
        let landed_ours =
            commits.iter().any(|c| c.place.as_deref() == Some("mainline") && (ours(c) || shares.contains_key(&c.key)));
        if !landed_ours && off_ours > 0 {
            unjudged.push(UnjudgedRepo {
                repo: name.clone(),
                mainline: mainline_name.get(repo).cloned().unwrap_or_default(),
                off_mainline_commits: i64::try_from(off_ours).unwrap_or(i64::MAX),
                unresolved: false,
            });
            continue;
        }
        judged.insert(repo.clone());

        // Switch moments in this repository, in order.
        let mut spans: Vec<&Claim> = rc.iter().collect();
        spans.sort_by_key(|c| c.from);
        let switches: Vec<(DateTime<Utc>, String, String)> =
            spans.windows(2).filter(|w| w[0].model != w[1].model).map(|w| (w[1].from, w[0].model.clone(), w[1].model.clone())).collect();
        for (_, from, to) in &switches {
            hand.entry((from.clone(), to.clone())).or_default().switches += 1;
        }
        let takeover = |fam: &str, at: DateTime<Utc>| {
            switches
                .iter()
                .rev()
                .find(|(t, _, to)| *t <= at && to == fam)
                .filter(|(t, _, _)| (at - *t).num_seconds() <= takeover_secs)
                .map(|(_, from, to)| (from.clone(), to.clone()))
        };

        // Branch work that did not land: in flight, or thrown away.
        for c in commits.iter().filter(|c| off(c) && ours(c) && !bulk(c) && c.place.as_deref() != Some("recovered")) {
            let never: i64 =
                c.files.iter().filter(|(path, _, _)| !landed_via.contains_key(&(c.key.clone(), path.clone()))).map(|(_, a, _)| a).sum();
            if never > 0 {
                if let Some(fam) = model_at(c, c.at) {
                    let e = dev.entry(fam).or_default();
                    if c.place.as_deref() == Some("in_flight") {
                        e.in_flight_lines += never;
                    } else {
                        e.abandoned_commits += 1;
                        e.abandoned_lines += never;
                    }
                }
            }
        }
        // What landed.
        let mut touched: HashMap<(String, String), i64> = HashMap::new();
        for c in commits.iter().filter(|c| c.place.as_deref() == Some("mainline")) {
            let authored_squash = shares.contains_key(&c.key) && !bulk_via.contains(&c.key);
            if bulk(c) && !authored_squash {
                continue;
            }
            let pieces: Vec<(DateTime<Utc>, i64, Option<String>)> = match shares.get(&c.key) {
                Some(v) => v.clone(),
                None if ours(c) => vec![(c.at, 1, None)],
                None => continue,
            };
            let alive = alive_of.get(&c.key).copied().unwrap_or(0);
            let weights: Vec<i64> = pieces.iter().map(|p| p.1).collect();
            let (sa, sr, sl) = (split(c.added, &weights), split(c.removed, &weights), split(alive, &weights));
            let mut dominant: Option<(i64, String)> = None;
            for (k, (at, w, tie)) in pieces.iter().enumerate() {
                let Some(fam) = tie.clone().or_else(|| model_at(c, *at)) else { continue };
                for acc in [dev.entry(fam.clone()).or_default(), per_repo.entry((name.clone(), fam.clone())).or_default()] {
                    acc.added += sa[k];
                    acc.removed += sr[k];
                    acc.alive += sl[k];
                }
                if let Some(pair) = takeover(&fam, *at) {
                    let h = hand.entry(pair).or_default();
                    h.added += sa[k];
                    h.removed += sr[k];
                    h.alive += sl[k];
                    h.commits += 1;
                }
                if dominant.as_ref().is_none_or(|(dw, _)| w > dw) {
                    dominant = Some((*w, fam));
                }
            }
            // A commit is one event: it counts once, for the model that did
            // most of it.
            let Some((_, fam)) = dominant else { continue };
            let rework = is_rework(&c.subject, &params.rework_words);
            let e = dev.entry(fam.clone()).or_default();
            e.commits += 1;
            e.rework_commits += i64::from(rework);
            let dirs: HashSet<&str> = c.files.iter().map(|f| top_dir(&f.0)).collect();
            e.multi_dir_commits += i64::from(dirs.len() > 1);
            e.dir_spread += i64::try_from(dirs.len()).unwrap_or(i64::MAX);
            e.ages.push((Utc::now() - c.at).num_days().max(0));
            for (path, _, _) in &c.files {
                let n = touched.entry((fam.clone(), path.clone())).or_insert(0);
                *n += 1;
                if *n == thrash_at {
                    e.thrash_files += 1;
                }
            }
            let rk = per_repo.entry((name.clone(), fam)).or_default();
            rk.commits += 1;
            rk.rework_commits += i64::from(rework);
        }
    }

    // Tokens, time and turns per model: tokens, minutes and messages over the
    // work done in judged repositories, turns and context over every run.
    let mut sp: HashMap<String, SpendAcc> = HashMap::new();
    for (session, run) in &runs {
        let list = &by_session[session];
        let e = sp.entry(run.family.clone()).or_default();
        let (mut ctx_sum, mut ctx_n, mut ctx_peak) = (0i64, 0i64, 0i64);
        for (k, &i) in run.replies.iter().enumerate() {
            let r = &list[i];
            let (out, ctx) = usage.get(&r.key).copied().unwrap_or((0, 0));
            if ctx > 0 {
                ctx_sum += ctx;
                ctx_n += 1;
                ctx_peak = ctx_peak.max(ctx);
            }
            if reply_repo.get(&r.key).cloned().flatten().is_some_and(|repo| judged.contains(&repo)) {
                e.out_tokens += out;
                e.messages += 1;
                if k > 0 {
                    e.seconds += (r.at - list[run.replies[k - 1]].at).num_seconds().clamp(0, active);
                }
            }
        }
        e.runs += 1;
        e.operator_turns += run.turns;
        e.corrections += run.corrections;
        e.escalations += run.escalations;
        let messages = i64::try_from(run.replies.len()).unwrap_or(i64::MAX);
        let avg = if ctx_n > 0 { ctx_sum / ctx_n } else { 0 };
        e.context_weighted += avg.saturating_mul(messages);
        e.context_msgs += if avg > 0 { messages } else { 0 };
        e.context_peak = e.context_peak.max(ctx_peak);
    }

    let mut handoffs: Vec<Handoff> = hand
        .into_iter()
        .map(|((from, to), a)| Handoff {
            from,
            to,
            switches: a.switches,
            commits: a.commits,
            added: a.added,
            removed: a.removed,
            alive: a.alive,
            survived_pct: pct(a.alive, a.added),
        })
        .collect();
    handoffs.sort_by(|a, b| b.switches.cmp(&a.switches).then_with(|| (&a.from, &a.to).cmp(&(&b.from, &b.to))));
    let mut deviations: Vec<Deviation> = dev.into_iter().map(|(model, a)| deviation(model, &a, &sp)).collect();
    deviations.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| a.model.cmp(&b.model)));
    let mut repos: Vec<RepoModel> = per_repo
        .into_iter()
        .map(|((repo, model), a)| RepoModel {
            repo,
            model,
            commits: a.commits,
            added: a.added,
            alive: a.alive,
            removed: a.removed,
            survived_pct: pct(a.alive, a.added),
            rework_commits: a.rework_commits,
        })
        .collect();
    repos.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| (&a.repo, &a.model).cmp(&(&b.repo, &b.model))));
    unjudged.sort_by(|a, b| a.repo.cmp(&b.repo));
    Ok(Comparison { handoffs, deviations, repos, unjudged, computed_at: Utc::now().to_rfc3339() })
}

/// One model's row, every rate from its own summed numerator and
/// denominator.
fn deviation(model: String, a: &DevAcc, sp: &HashMap<String, SpendAcc>) -> Deviation {
    let s = sp.get(&model).cloned().unwrap_or_default();
    let thrown = (a.added - a.alive).max(0);
    let minutes = s.seconds / 60;
    let mut ages = a.ages.clone();
    ages.sort_unstable();
    Deviation {
        commits: a.commits,
        added: a.added,
        removed: a.removed,
        alive: a.alive,
        survived_pct: pct(a.alive, a.added),
        removed_per_100_added: pct(a.removed, a.added),
        rework_commits: a.rework_commits,
        rework_pct: pct(a.rework_commits, a.commits),
        thrash_files: a.thrash_files,
        thrash_per_100_commits: pct(a.thrash_files, a.commits),
        multi_dir_commits: a.multi_dir_commits,
        multi_dir_pct: pct(a.multi_dir_commits, a.commits),
        dirs_per_commit_x10: if a.commits > 0 { (10 * a.dir_spread) / a.commits } else { 0 },
        operator_turns: s.operator_turns,
        corrections: s.corrections,
        corrections_per_100: pct(s.corrections, s.operator_turns),
        escalations: s.escalations,
        out_tokens: s.out_tokens,
        messages: s.messages,
        runs: s.runs,
        thrown,
        tokens_thrown: if a.added > 0 { s.out_tokens.saturating_mul(thrown) / a.added } else { 0 },
        tokens_per_line_landed: if a.added > 0 { s.out_tokens / a.added } else { 0 },
        tokens_per_line_kept: if a.alive > 0 { s.out_tokens / a.alive } else { 0 },
        // -1 for none landed: "0d" would read as work written today.
        median_age_days: ages.get(ages.len() / 2).copied().unwrap_or(-1),
        context_avg: if s.context_msgs > 0 { s.context_weighted / s.context_msgs } else { 0 },
        context_peak: s.context_peak,
        minutes,
        minutes_thrown: if a.added > 0 { minutes.saturating_mul(thrown) / a.added } else { 0 },
        alive_per_mtok: if s.out_tokens > 0 { a.alive.saturating_mul(1_000_000) / s.out_tokens } else { 0 },
        alive_per_hour: if minutes > 0 { (a.alive * 60) / minutes } else { 0 },
        abandoned_commits: a.abandoned_commits,
        abandoned_lines: a.abandoned_lines,
        abandoned_pct: pct(a.abandoned_lines, a.added + a.abandoned_lines),
        in_flight_lines: a.in_flight_lines,
        model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_family_is_its_name_and_version_as_mod_ui_names_it() {
        assert_eq!(family("claude-opus-5-5"), "opus 5.5");
        assert_eq!(family("claude-haiku-4-5-20251001"), "haiku 4.5", "a dated snapshot is its version");
        assert_eq!(family("gemini-3.1-pro-preview"), "gemini 3.1 pro");
        assert_eq!(family("claude-fable-5-1"), "fable 5.1");
        assert_eq!(family("unknown"), "unknown");
    }

    #[test]
    fn a_share_is_rounded_as_the_page_rounds() {
        assert_eq!(pct(811, 7411), 11);
        assert_eq!(pct(2, 3), 67);
        assert_eq!(pct(1, 200), 1);
        assert_eq!(pct(5, 0), 0);
    }

    #[test]
    fn a_fix_commit_is_rework_and_a_word_inside_another_is_not() {
        let w = words(&["fix", "revert", "undo"]);
        assert!(is_rework("fix(ui): the age was frozen", &w));
        assert!(is_rework("chore: undo the rename", &w));
        assert!(!is_rework("feat(ui): the real cost", &w));
        assert!(!is_rework("add a prefix to the key", &w));
    }

    #[test]
    fn a_correction_is_a_whole_word_and_an_escalation_a_words_start() {
        assert!(says_word("No, do it again", &words(&["again"]), false));
        assert!(!says_word("the deviation band", &words(&["deviate"]), false));
        assert!(says_word("this is fucking wrong", &words(&["fuck"]), true));
        assert!(!says_word("rashit", &words(&["shit"]), true));
    }

    #[test]
    fn a_split_adds_up_and_the_largest_takes_the_remainder() {
        assert_eq!(split(10, &[1, 1, 1]), vec![3, 3, 4]);
        assert_eq!(split(100, &[3, 1]), vec![75, 25]);
        assert_eq!(split(7, &[0, 0]), vec![0, 0]);
    }

    #[test]
    fn a_commit_is_credited_only_when_exactly_one_model_claims_its_time() {
        let t = |s: &str| DateTime::parse_from_rfc3339(s).expect("a time").with_timezone(&Utc);
        let claims = vec![
            Claim { model: "opus 5.5".to_string(), from: t("2026-11-01T10:00:00Z"), to: t("2026-11-01T11:00:00Z") },
            Claim { model: "sonnet 5.5".to_string(), from: t("2026-11-01T10:30:00Z"), to: t("2026-11-01T12:00:00Z") },
        ];
        assert_eq!(credit(&claims, t("2026-11-01T10:15:00Z")), Some("opus 5.5"));
        assert_eq!(credit(&claims, t("2026-11-01T10:45:00Z")), None, "two models: ambiguous");
        assert_eq!(credit(&claims, t("2026-11-01T11:30:00Z")), Some("sonnet 5.5"));
        assert_eq!(credit(&claims, t("2026-11-01T13:00:00Z")), None, "no one was working");
    }
}
