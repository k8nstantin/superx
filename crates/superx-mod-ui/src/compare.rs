//! Model comparison (#406, #414): which model to reach for, and why.
//!
//! [`crate::thrown`] answers "what did the work cost to keep". This
//! answers the operator's harder question: work is handed from one
//! model to another when a budget runs out, rework follows, and the
//! rework is currently blamed on whoever's lines died rather than on
//! whoever caused them to. So the switch itself has to be a measured
//! event, not a footnote.
//!
//! Everything here is read from git and from the same uncapped model
//! runs [`crate::thrown`] builds. Nothing samples a message window.
//!
//! # How git is read (#414)
//!
//! - **One walk per repository.** A repository is its common git dir;
//!   every worktree of it is the same repository. Walking each checkout
//!   credited every commit once per checkout.
//! - **Blame at the main line**, not the working tree: a checkout on
//!   another branch read every commit it did not contain as dead.
//! - **Landed is what reached the main line**: its commits, and the
//!   commits of any branch whose final version of a file the main line
//!   carries — a squash or a replay lands a branch's work without
//!   landing its commits. Those are credited by when the BRANCH work was
//!   done, not by when the squash was merged — and a branch deleted after
//!   it merged is read back from the reflogs of the checkouts that made it
//!   (#415 review).
//! - **Never landed** is the rest of the branch work — on real branches,
//!   by the identity this machine commits as. `refs/stash`, and another
//!   person's branches pulled in through `refs/remotes`, are not it.
//! - **A repository whose main line took no commit while its branches
//!   took many is not judged**: counting it called all of its work
//!   "never landed".

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::api::{Deviation, Handoff, ModelRun, RepoModel, UnjudgedRepo};
use crate::thrown::{credit, Claim};

/// Commit subjects that mean the work is being done again. Read from
/// git, so this is the agent's own account of its rework rather than
/// the operator's.
const REWORK_SUBJECTS: [&str; 8] = [
    "fix", "revert", "undo", "redo", "restore", "re-add", "readd", "rework",
];

/// A file touched at least this many times by one model is being
/// returned to rather than written.
const THRASH_TOUCHES: i64 = 3; // skill-allow: §9-const — read-path bound, not a policy tunable

/// How long after a switch still counts as "just took over".
const TAKEOVER_SECS: i64 = 3 * 3600; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A commit big enough to be a vendor drop or a generated purge rather
/// than authored work. Counting one of these as a model's own churn
/// moves every ratio by more than the entire rest of the history, and
/// it did exactly that before this bound existed.
const BULK_COMMIT: i64 = 5_000; // skill-allow: §9-const — read-path bound, not a policy tunable

fn is_rework(subject: &str) -> bool {
    REWORK_SUBJECTS.iter().any(|w| subject.starts_with(w))
        || REWORK_SUBJECTS
            .iter()
            .any(|w| subject.contains(&format!(" {w}")))
}

/// The top-level directory of a path — the unit of "was this the thing
/// I was asked to touch".
fn top_dir(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
}

/// Split `total` over `weights` in proportion, in whole units, so the
/// shares add up to `total` exactly — the remainder goes to the largest.
fn split(total: i64, weights: &[i64]) -> Vec<i64> {
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

/// One repository the runs worked in, and every run's claim on it.
struct RepoClaims {
    name: String,
    /// A checkout to read it from.
    dir: String,
    claims: Vec<Claim>,
    /// The start of the first run that worked here: nothing older can be
    /// credited, so nothing older is walked. The oldest run anywhere — a
    /// chat months back in no repository at all — dragged every walk back
    /// with it (#415 review).
    earliest: DateTime<Utc>,
}

/// Everything the comparison section draws.
///
/// Returned together because all of it comes from one walk of each
/// repository: walking twice would double the cost of the most
/// expensive read in the module.
pub struct Comparison {
    pub handoffs: Vec<Handoff>,
    pub deviations: Vec<Deviation>,
    pub repos: Vec<RepoModel>,
    pub unjudged: Vec<UnjudgedRepo>,
}

/// The comparison over every run. `mainlines` is the operator's
/// `attr_ui_mainline_refs`: repository name → the ref its work lands on.
#[must_use]
pub async fn compare(runs: &[ModelRun], mainlines: &HashMap<String, String>) -> Comparison {
    let cwds: HashSet<String> = runs.iter().flat_map(|r| r.work.iter().map(|w| w.cwd.clone())).collect();
    let checkouts = crate::checkout::Checkouts::resolve(cwds.iter()).await;
    let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));

    // Claims per repository — by its common git dir, so a worktree is its
    // repository (#414).
    let mut repos: HashMap<String, RepoClaims> = HashMap::new();
    for r in runs {
        let (Some(from), Some(to)) = (parse(&r.first), parse(&r.last)) else { continue };
        let mut seen: HashSet<&str> = HashSet::new();
        for w in &r.work {
            let Some(co) = checkouts.of(&w.cwd) else { continue };
            if !seen.insert(co.common.as_str()) {
                continue;
            }
            let rc = repos.entry(co.common.clone()).or_insert_with(|| RepoClaims {
                name: co.repo.clone(),
                dir: co.toplevel.clone(),
                claims: Vec::new(),
                earliest: from,
            });
            rc.earliest = rc.earliest.min(from);
            rc.claims.push(Claim { model: r.model.clone(), from, to });
        }
    }

    let mut dev: HashMap<String, DevAcc> = HashMap::new();
    let mut hand: HashMap<(String, String), HandAcc> = HashMap::new();
    let mut per_repo: HashMap<(String, String), DevAcc> = HashMap::new();
    let mut unjudged: Vec<UnjudgedRepo> = Vec::new();
    let mut judged: HashSet<String> = HashSet::new();

    let mut keys: Vec<&String> = repos.keys().collect();
    keys.sort_by_key(|k| &repos[*k].name);
    for key in keys {
        let rc = &repos[key];
        let dir = std::path::Path::new(&rc.dir);
        let mainline = crate::landed::mainline_of(dir, &rc.name, mainlines).await;
        let Some(work) = crate::landed::repo_work(dir, &mainline, Some(rc.earliest)).await else { continue };
        let ours = |author: &str| work.identity.as_deref() == Some(author);

        // A main line that took nothing while this machine's branches took
        // plenty is not where the work lands: say so, judge nothing.
        // Branch commits only: one known from a reflog alone may be an
        // amend's or a rebase's leftover.
        let off_ours = work.dormant_off.unwrap_or_else(|| {
            work.off_mainline
                .iter()
                .filter(|c| ours(&c.author) && !work.recovered.contains(&c.hash))
                .count() as i64
        });
        if work.landed.is_empty() && off_ours > 0 {
            unjudged.push(UnjudgedRepo {
                repo: rc.name.clone(),
                mainline,
                off_mainline_commits: off_ours,
            });
            continue;
        }
        judged.insert(key.clone());

        // Switch moments on this repository, in order.
        let mut spans: Vec<&Claim> = rc.claims.iter().collect();
        spans.sort_by_key(|c| c.from);
        let switches: Vec<(DateTime<Utc>, String, String)> = spans
            .windows(2)
            .filter(|w| w[0].model != w[1].model)
            .map(|w| (w[1].from, w[0].model.clone(), w[1].model.clone()))
            .collect();
        for (_, from, to) in &switches {
            hand.entry((from.clone(), to.clone())).or_default().switches += 1;
        }
        let takeover = |fam: &str, at: DateTime<Utc>| {
            switches
                .iter()
                .rev()
                .find(|(t, _, to)| *t <= at && to == fam)
                .filter(|(t, _, _)| (at - *t).num_seconds() <= TAKEOVER_SECS)
                .map(|(_, from, to)| (from.clone(), to.clone()))
        };

        // Branch work: per file, landed by a main-line commit or not.
        let mut shares: HashMap<&str, Vec<(DateTime<Utc>, i64)>> = HashMap::new();
        for c in work.off_mainline.iter().filter(|c| ours(&c.author)) {
            if c.added() + c.removed() > BULK_COMMIT {
                continue;
            }
            // A commit known only from a reflog says when landed work was
            // done, and nothing else (#415 review).
            let recovered = work.recovered.contains(&c.hash);
            let mut never = 0i64;
            for (path, added, removed) in &c.files {
                match work.landed_via.get(&(c.hash.clone(), path.clone())) {
                    Some(via) => shares.entry(via.as_str()).or_default().push((c.at, (added + removed).max(1))),
                    None if !recovered => never += added,
                    None => {}
                }
            }
            if never > 0 {
                if let Some(fam) = credit(&rc.claims, c.at) {
                    let e = dev.entry(fam.to_string()).or_default();
                    // Still checked out somewhere: in flight, not thrown
                    // away (#414).
                    if work.in_flight.contains(&c.hash) {
                        e.in_flight_lines += never;
                    } else {
                        e.abandoned_commits += 1;
                        e.abandoned_lines += never;
                    }
                }
            }
        }
        // The identities a squash of this machine's branches was made as —
        // the account that clicked merge — are this machine's too.
        let aliases: HashSet<&str> = work
            .landed
            .iter()
            .filter(|c| shares.contains_key(c.hash.as_str()))
            .map(|c| c.author.as_str())
            .collect();

        // Files a model has already touched here, for thrash.
        let mut touched: HashMap<(String, String), i64> = HashMap::new();
        for c in &work.landed {
            let (added, removed) = (c.added(), c.removed());
            // A vendor drop is not authored work.
            if added + removed > BULK_COMMIT {
                continue;
            }
            // When the work was done: the branch commits' times for a
            // squash of this machine's branches, else the commit's own —
            // and only for commits this machine made.
            let pieces: Vec<(DateTime<Utc>, i64)> = match shares.get(c.hash.as_str()) {
                Some(v) => v.clone(),
                None if ours(&c.author) || aliases.contains(c.author.as_str()) => vec![(c.at, 1)],
                None => continue,
            };
            let alive = work.alive.get(&c.hash).copied().unwrap_or(0);
            let weights: Vec<i64> = pieces.iter().map(|p| p.1).collect();
            let (sa, sr, sl) = (split(added, &weights), split(removed, &weights), split(alive, &weights));
            let mut dominant: Option<(i64, String)> = None;
            for (k, (at, w)) in pieces.iter().enumerate() {
                let Some(fam) = credit(&rc.claims, *at) else { continue };
                for acc in [dev.entry(fam.to_string()).or_default(),
                            per_repo.entry((rc.name.clone(), fam.to_string())).or_default()] {
                    acc.added += sa[k];
                    acc.removed += sr[k];
                    acc.alive += sl[k];
                }
                if let Some(pair) = takeover(fam, *at) {
                    let h = hand.entry(pair).or_default();
                    h.added += sa[k];
                    h.removed += sr[k];
                    h.alive += sl[k];
                    h.commits += 1;
                }
                if dominant.as_ref().is_none_or(|(dw, _)| w > dw) {
                    dominant = Some((*w, fam.to_string()));
                }
            }
            // A commit is one event: it counts once, for the model that
            // did most of it.
            let Some((_, fam)) = dominant else { continue };
            let rework = is_rework(&c.subject);
            {
                let e = dev.entry(fam.clone()).or_default();
                e.commits += 1;
                if rework {
                    e.rework_commits += 1;
                }
                let dirs: HashSet<&str> = c.files.iter().map(|f| top_dir(&f.0)).collect();
                if dirs.len() > 1 {
                    e.multi_dir_commits += 1;
                }
                e.dir_spread += dirs.len() as i64;
                e.ages.push((Utc::now() - c.at).num_days().max(0));
                for (path, _, _) in &c.files {
                    let n = touched.entry((fam.clone(), path.clone())).or_insert(0);
                    *n += 1;
                    if *n == THRASH_TOUCHES {
                        e.thrash_files += 1;
                    }
                }
            }
            let rk = per_repo.entry((rc.name.clone(), fam)).or_default();
            rk.commits += 1;
            if rework {
                rk.rework_commits += 1;
            }
        }
    }

    // Tokens, time and turns per model, over the work done in the
    // repositories judged here (#414) — the same work the lines above
    // come from. Spend outside any repository, or in one left unjudged,
    // bought nothing this section can see.
    let mut sp: HashMap<&str, SpendAcc> = HashMap::new();
    for r in runs {
        let e = sp.entry(r.model.as_str()).or_default();
        for w in &r.work {
            if checkouts.of(&w.cwd).is_some_and(|co| judged.contains(&co.common)) {
                e.out_tokens += w.out_tokens;
                e.messages += w.replies;
                e.minutes += w.minutes;
            }
        }
        e.runs += 1;
        e.operator_turns += r.operator_turns;
        e.corrections += r.redo_asks;
        e.context_weighted += r.context_avg.saturating_mul(r.messages);
        e.context_msgs += if r.context_avg > 0 { r.messages } else { 0 };
        e.context_peak = e.context_peak.max(r.context_peak);
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
            survived_pct: if a.added > 0 { (100 * a.alive) / a.added } else { 0 },
        })
        .collect();
    handoffs.sort_by_key(|h| std::cmp::Reverse(h.switches));

    let mut deviations: Vec<Deviation> = dev
        .into_iter()
        .map(|(model, a)| deviation(model, &a, &sp))
        .collect();
    deviations.sort_by_key(|d| std::cmp::Reverse(d.added));

    let mut repo_rows: Vec<RepoModel> = per_repo
        .into_iter()
        .map(|((repo, model), a)| RepoModel {
            repo,
            model,
            commits: a.commits,
            added: a.added,
            alive: a.alive,
            removed: a.removed,
            survived_pct: if a.added > 0 { (100 * a.alive) / a.added } else { 0 },
            rework_commits: a.rework_commits,
        })
        .collect();
    repo_rows.sort_by_key(|r| std::cmp::Reverse(r.added));
    unjudged.sort_by(|a, b| a.repo.cmp(&b.repo));

    Comparison { handoffs, deviations, repos: repo_rows, unjudged }
}

/// One model's row, every rate computed from its own summed numerator
/// and denominator.
fn deviation(model: String, a: &DevAcc, sp: &HashMap<&str, SpendAcc>) -> Deviation {
    let s = sp.get(model.as_str()).cloned().unwrap_or_default();
    let (op, redo) = (s.operator_turns, s.corrections);
    let thrown = (a.added - a.alive).max(0);
    let mut ages = a.ages.clone();
    ages.sort_unstable();
    let per = |num: i64, den: i64| if den > 0 { (100 * num) / den } else { 0 };
    Deviation {
        commits: a.commits,
        added: a.added,
        removed: a.removed,
        alive: a.alive,
        survived_pct: per(a.alive, a.added),
        removed_per_100_added: per(a.removed, a.added),
        rework_commits: a.rework_commits,
        rework_pct: per(a.rework_commits, a.commits),
        thrash_files: a.thrash_files,
        thrash_per_100_commits: per(a.thrash_files, a.commits),
        multi_dir_commits: a.multi_dir_commits,
        multi_dir_pct: per(a.multi_dir_commits, a.commits),
        dirs_per_commit_x10: if a.commits > 0 { (10 * a.dir_spread) / a.commits } else { 0 },
        operator_turns: op,
        corrections: redo,
        corrections_per_100: per(redo, op),
        out_tokens: s.out_tokens,
        messages: s.messages,
        runs: s.runs,
        thrown,
        tokens_thrown: if a.added > 0 { s.out_tokens.saturating_mul(thrown) / a.added } else { 0 },
        tokens_per_line_landed: if a.added > 0 { s.out_tokens / a.added } else { 0 },
        tokens_per_line_kept: if a.alive > 0 { s.out_tokens / a.alive } else { 0 },
        // -1 for none landed, as every median on the page (#415 review):
        // "0d" read as work written today.
        median_age_days: ages.get(ages.len() / 2).copied().unwrap_or(-1),
        context_avg: if s.context_msgs > 0 { s.context_weighted / s.context_msgs } else { 0 },
        context_peak: s.context_peak,
        minutes: s.minutes,
        // Time is charged the way tokens are: the model's own rate,
        // applied to the work that did not last.
        minutes_thrown: if a.added > 0 { s.minutes.saturating_mul(thrown) / a.added } else { 0 },
        alive_per_mtok: if s.out_tokens > 0 { a.alive.saturating_mul(1_000_000) / s.out_tokens } else { 0 },
        alive_per_hour: if s.minutes > 0 { (a.alive * 60) / s.minutes } else { 0 },
        abandoned_commits: a.abandoned_commits,
        abandoned_lines: a.abandoned_lines,
        abandoned_pct: per(a.abandoned_lines, a.added + a.abandoned_lines),
        in_flight_lines: a.in_flight_lines,
        model,
    }
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
    context_weighted: i64,
    context_msgs: i64,
    context_peak: i64,
    minutes: i64,
}

#[derive(Default)]
struct HandAcc {
    switches: i64,
    commits: i64,
    added: i64,
    removed: i64,
    alive: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fix_commit_is_rework_and_a_feature_is_not() {
        assert!(is_rework("fix(ui): the age was frozen"));
        assert!(is_rework("revert the scheduler"));
        assert!(is_rework("chore: undo the rename"));
        assert!(!is_rework("feat(ui): the real cost"));
        assert!(!is_rework("docs: describe the substrate"));
    }

    /// `prefix` must not match inside an unrelated word — "prefix" and
    /// "affix" both contain "fix" and neither is rework.
    #[test]
    fn rework_does_not_match_inside_a_word() {
        assert!(!is_rework("add a prefix to the key"));
        assert!(!is_rework("affix the label"));
    }

    #[test]
    fn a_split_adds_up_and_the_largest_takes_the_remainder() {
        // Ties go to the last of the largest, which takes the remainder.
        assert_eq!(split(10, &[1, 1, 1]), vec![3, 3, 4]);
        assert_eq!(split(100, &[3, 1]), vec![75, 25]);
        assert_eq!(split(7, &[0, 0]), vec![0, 0]);
        assert_eq!(split(5, &[2]).iter().sum::<i64>(), 5);
    }

    #[test]
    fn the_top_directory_is_the_scope_unit() {
        assert_eq!(top_dir("crates/superx-mod-ui/src/api.rs"), "crates");
        assert_eq!(top_dir("README.md"), "README.md");
    }
}
