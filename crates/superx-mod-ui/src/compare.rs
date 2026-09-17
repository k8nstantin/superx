//! Model comparison (#406): which model to reach for, and why.
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

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::api::{Deviation, Handoff, ModelRun, RepoModel};
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

/// Build the model timeline per repository, newest last.
fn claims_by_repo(runs: &[ModelRun]) -> (HashMap<String, Vec<Claim>>, Option<DateTime<Utc>>) {
    let mut claims: HashMap<String, Vec<Claim>> = HashMap::new();
    let mut earliest: Option<DateTime<Utc>> = None;
    for r in runs {
        let (Ok(from), Ok(to)) = (
            DateTime::parse_from_rfc3339(&r.first),
            DateTime::parse_from_rfc3339(&r.last),
        ) else {
            continue;
        };
        let (from, to) = (from.with_timezone(&Utc), to.with_timezone(&Utc));
        earliest = Some(earliest.map_or(from, |e: DateTime<Utc>| e.min(from)));
        for cwd in &r.cwds {
            claims.entry(cwd.clone()).or_default().push(Claim {
                model: r.model.clone(),
                from,
                to,
            });
        }
    }
    (claims, earliest)
}

/// Everything the comparison section draws.
///
/// Returned together because all of it comes from one walk of each
/// repository: walking twice would double the cost of the most
/// expensive read in the module.
#[must_use]
pub async fn compare(runs: &[ModelRun]) -> (Vec<Handoff>, Vec<Deviation>, Vec<RepoModel>) {
    let (by_cwd, earliest) = claims_by_repo(runs);

    // Fold the per-cwd claims onto repository roots.
    let mut claims: HashMap<String, Vec<Claim>> = HashMap::new();
    for (cwd, list) in &by_cwd {
        let Some(top) = crate::landed::toplevel(std::path::Path::new(cwd)).await else {
            continue;
        };
        claims.entry(top).or_default().extend(list.iter().cloned());
    }

    // Per model: the deviation counters.
    let mut dev: HashMap<String, DevAcc> = HashMap::new();
    // Per (from, to): what the incoming model did in its first hours.
    let mut hand: HashMap<(String, String), HandAcc> = HashMap::new();
    // Per repository and model, so one checkout that went badly after a
    // switch is visible instead of averaged into every other.
    let mut per_repo: HashMap<(String, String), DevAcc> = HashMap::new();

    let mut roots: Vec<&String> = claims.keys().collect();
    roots.sort();
    for top in roots {
        let list = &claims[top];
        // Switch moments on this repository, in order.
        let mut spans: Vec<&Claim> = list.iter().collect();
        spans.sort_by_key(|c| c.from);
        let mut switches: Vec<(DateTime<Utc>, String, String)> = Vec::new();
        for w in spans.windows(2) {
            if w[0].model != w[1].model {
                switches.push((w[1].from, w[0].model.clone(), w[1].model.clone()));
            }
        }

        let commits = crate::landed::survival(std::path::Path::new(top), earliest).await;
        // Files a model has already touched here, for thrash.
        let mut touched: HashMap<(String, String), i64> = HashMap::new();
        for c in &commits {
            let Some(model) = credit(list, c.at) else { continue };
            // A vendor drop is not authored work.
            if c.added + c.removed > BULK_COMMIT {
                continue;
            }
            let e = dev.entry(model.to_string()).or_default();
            e.commits += 1;
            e.added += c.added;
            e.removed += c.removed;
            e.alive += c.alive;
            if is_rework(&c.subject) {
                e.rework_commits += 1;
            }
            let short = std::path::Path::new(top)
                .file_name()
                .map_or_else(|| top.clone(), |n| n.to_string_lossy().into_owned());
            let rk = per_repo
                .entry((short, model.to_string()))
                .or_default();
            rk.commits += 1;
            rk.added += c.added;
            rk.removed += c.removed;
            rk.alive += c.alive;
            if is_rework(&c.subject) {
                rk.rework_commits += 1;
            }

            let dirs: HashSet<&str> = c.files.iter().map(|f| top_dir(f)).collect();
            if dirs.len() > 1 {
                e.multi_dir_commits += 1;
            }
            e.dir_spread += dirs.len() as i64;
            e.ages.push((Utc::now() - c.at).num_days().max(0));
            for f in &c.files {
                let k = (model.to_string(), f.clone());
                let n = touched.entry(k).or_insert(0);
                *n += 1;
                if *n == THRASH_TOUCHES {
                    e.thrash_files += 1;
                }
            }

            // Did this land in the first hours after a switch?
            if let Some((at, from, to)) = switches
                .iter()
                .rev()
                .find(|(at, _, to)| *at <= c.at && to == model)
            {
                if (c.at - *at).num_seconds() <= TAKEOVER_SECS {
                    let h = hand
                        .entry((from.clone(), to.clone()))
                        .or_insert_with(|| HandAcc {
                            switches: 0,
                            ..HandAcc::default()
                        });
                    h.added += c.added;
                    h.removed += c.removed;
                    h.alive += c.alive;
                    h.commits += 1;
                }
            }
        }
        for (at, from, to) in &switches {
            let _ = at;
            hand.entry((from.clone(), to.clone())).or_default().switches += 1;
        }
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
            survived_pct: if a.added > 0 {
                (100 * a.alive) / a.added
            } else {
                0
            },
        })
        .collect();
    handoffs.sort_by_key(|h| std::cmp::Reverse(h.switches));

    // Tokens, turns and context per model — the same rollup the pricing
    // view used, folded in here so there is ONE number per quantity.
    // Two walks computing "survived %" separately is how two sections
    // came to disagree with each other on screen.
    let mut sp: HashMap<&str, SpendAcc> = HashMap::new();
    for r in runs {
        let e = sp.entry(r.model.as_str()).or_default();
        e.out_tokens += r.out_tokens;
        e.messages += r.messages;
        e.runs += 1;
        e.operator_turns += r.operator_turns;
        e.corrections += r.redo_asks;
        e.context_weighted += r.context_avg.saturating_mul(r.messages);
        e.context_msgs += if r.context_avg > 0 { r.messages } else { 0 };
        e.context_peak = e.context_peak.max(r.context_peak);
        e.minutes += r.minutes;
    }

    let mut deviations: Vec<Deviation> = dev
        .into_iter()
        .map(|(model, a)| {
            let s = sp.get(model.as_str()).cloned().unwrap_or_default();
            let (op, redo) = (s.operator_turns, s.corrections);
            let thrown = (a.added - a.alive).max(0);
            let mut ages = a.ages.clone();
            ages.sort_unstable();
            Deviation {
                model,
                commits: a.commits,
                added: a.added,
                removed: a.removed,
                alive: a.alive,
                survived_pct: if a.added > 0 {
                    (100 * a.alive) / a.added
                } else {
                    0
                },
                removed_per_100_added: if a.added > 0 {
                    (100 * a.removed) / a.added
                } else {
                    0
                },
                rework_commits: a.rework_commits,
                rework_pct: if a.commits > 0 {
                    (100 * a.rework_commits) / a.commits
                } else {
                    0
                },
                thrash_files: a.thrash_files,
                thrash_per_100_commits: if a.commits > 0 {
                    (100 * a.thrash_files) / a.commits
                } else {
                    0
                },
                multi_dir_commits: a.multi_dir_commits,
                multi_dir_pct: if a.commits > 0 {
                    (100 * a.multi_dir_commits) / a.commits
                } else {
                    0
                },
                dirs_per_commit_x10: if a.commits > 0 {
                    (10 * a.dir_spread) / a.commits
                } else {
                    0
                },
                operator_turns: op,
                corrections: redo,
                corrections_per_100: if op > 0 { (100 * redo) / op } else { 0 },
                out_tokens: s.out_tokens,
                messages: s.messages,
                runs: s.runs,
                thrown,
                tokens_thrown: if a.added > 0 {
                    s.out_tokens.saturating_mul(thrown) / a.added
                } else {
                    0
                },
                tokens_per_line_landed: if a.added > 0 { s.out_tokens / a.added } else { 0 },
                tokens_per_line_kept: if a.alive > 0 { s.out_tokens / a.alive } else { 0 },
                median_age_days: ages.get(ages.len() / 2).copied().unwrap_or(0),
                context_avg: if s.context_msgs > 0 {
                    s.context_weighted / s.context_msgs
                } else {
                    0
                },
                context_peak: s.context_peak,
                minutes: s.minutes,
                // Time is charged the way tokens are: the model's own
                // rate, applied to the work that did not last.
                minutes_thrown: if a.added > 0 {
                    s.minutes.saturating_mul(thrown) / a.added
                } else {
                    0
                },
                alive_per_mtok: if s.out_tokens > 0 {
                    (a.alive.saturating_mul(1_000_000)) / s.out_tokens
                } else {
                    0
                },
                alive_per_hour: if s.minutes > 0 {
                    (a.alive * 60) / s.minutes
                } else {
                    0
                },
            }
        })
        .collect();
    deviations.sort_by_key(|d| std::cmp::Reverse(d.added));

    let mut repos: Vec<RepoModel> = per_repo
        .into_iter()
        .map(|((repo, model), a)| RepoModel {
            repo,
            model,
            commits: a.commits,
            added: a.added,
            alive: a.alive,
            removed: a.removed,
            survived_pct: if a.added > 0 {
                (100 * a.alive) / a.added
            } else {
                0
            },
            rework_commits: a.rework_commits,
        })
        .collect();
    repos.sort_by_key(|r| std::cmp::Reverse(r.added));

    (handoffs, deviations, repos)
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
    fn the_top_directory_is_the_scope_unit() {
        assert_eq!(top_dir("crates/superx-mod-ui/src/api.rs"), "crates");
        assert_eq!(top_dir("README.md"), "README.md");
    }
}
