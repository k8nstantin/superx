//! What a model's work threw away (#406).
//!
//! The question this answers is not "did the calls succeed" — tool
//! failure and test pass rates put the models within noise of each
//! other. It is "was it the right work": of the lines a model landed,
//! how many are still in the tree, and what did the rest cost.
//!
//! # Why this does not use the status walk
//!
//! [`crate::stats`] reads `message` newest-first under a row cap. That
//! is the right bound for a live dashboard and the wrong one for this
//! question: a model used earlier falls outside the window, and a
//! commit it authored is then credited to whichever model appears next
//! in the timeline. The first build of this section did exactly that,
//! handed one model's surviving lines to another, and argued the
//! reverse of the truth.
//!
//! So nothing here reads a message payload. Every per-session fact is
//! aggregated inside the engine and comes back as one row — a sum, a
//! min/max, a grouped count. The session list itself is uncapped, and
//! there are tens of sessions rather than tens of thousands of rows.
//! Cost scales with sessions, and the attribution covers all of them.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, Result};

use crate::api::{ModelRun, ThrownAway};

/// Working directories kept per session. A session is normally one
/// checkout; the cap catches the occasional wanderer without letting
/// one session claim every repository on the machine.
const CWDS_PER_SESSION: usize = 8; // skill-allow: §9-const — read-path bound, not a policy tunable

fn str_of(row: &Value, key: &str) -> Option<String> {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn int_of(row: &Value, key: &str) -> i64 {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int().unwrap_or(0),
            _ => 0,
        },
        _ => 0,
    }
}

fn time_of(row: &Value, key: &str) -> Option<DateTime<Utc>> {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::Datetime(d)) => Some(**d),
            _ => None,
        },
        _ => None,
    }
}

/// Every distinct model a session used, with the span it was in
/// charge and what it spent.
///
/// A session is NOT one model. The operator switches mid-session and
/// that is normal, so asking a session for "its" model returns only
/// the newest one and quietly credits the whole span to it. That is
/// the mistake this replaces: it folded every earlier model into
/// whichever one happened to finish the session.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_runs(kernel: &Kernel, session: RecordId, name: &str) -> Result<Vec<ModelRun>> {
    // Which checkouts this session worked in, by weight of activity.
    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT raw.cwd AS cwd, count() AS n FROM message \
             WHERE session = $sess AND raw.cwd != NONE \
             GROUP BY cwd ORDER BY n DESC LIMIT $cap",
        )
        .bind(("sess", session.clone()))
        .bind(("cap", CWDS_PER_SESSION as i64))
        .await?
        .take(0)?;
    let cwds: Vec<String> = rows.iter().filter_map(|r| str_of(r, "cwd")).collect();
    if cwds.is_empty() {
        return Ok(Vec::new());
    }

    // Every model that appears in the session, grouped by the engine.
    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT raw.message.model ?? raw.model AS model, count() AS n, \
                 math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) AS out \
             FROM message WHERE session = $sess \
                 AND (raw.message.model != NONE OR raw.model != NONE) \
             GROUP BY model",
        )
        .bind(("sess", session.clone()))
        .await?
        .take(0)?;

    let mut out = Vec::new();
    for row in &rows {
        // `<synthetic>` and friends are the runtime's own markers, not
        // models (#367/#388).
        let Some(model) = str_of(row, "model").filter(|m| !m.starts_with('<')) else {
            continue;
        };
        let messages = int_of(row, "n");
        let out_tokens = int_of(row, "out");
        if messages == 0 {
            continue;
        }
        // The span this model held, as two indexed lookups.
        // `math::min`/`math::max` aggregate numbers, not datetimes —
        // asking them for a timestamp returns nothing, which silently
        // emptied this whole table once.
        let edge = |dir: &str| {
            format!(
                "SELECT valid_from FROM message WHERE session = $sess \
                 AND (raw.message.model ?? raw.model) = $model \
                 ORDER BY valid_from {dir} LIMIT 1"
            )
        };
        let a: Vec<Value> = kernel
            .db()
            .query(edge("ASC"))
            .bind(("sess", session.clone()))
            .bind(("model", model.clone()))
            .await?
            .take(0)?;
        let b: Vec<Value> = kernel
            .db()
            .query(edge("DESC"))
            .bind(("sess", session.clone()))
            .bind(("model", model.clone()))
            .await?
            .take(0)?;
        let (Some(first), Some(last)) = (
            a.first().and_then(|r| time_of(r, "valid_from")),
            b.first().and_then(|r| time_of(r, "valid_from")),
        ) else {
            continue;
        };
        out.push(ModelRun {
            session: name.to_string(),
            model,
            cwds: cwds.clone(),
            first: first.to_rfc3339(),
            last: last.to_rfc3339(),
            messages,
            out_tokens,
        });
    }
    Ok(out)
}

/// Every model run across every session. Uncapped by design — see the
/// module note.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn model_runs(kernel: &Kernel) -> Result<Vec<ModelRun>> {
    let sessions = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await?;
    let mut out = Vec::new();
    for s in sessions {
        let name = match &s.payload {
            Value::Object(o) => match o.get("name") {
                Some(Value::String(v)) => v.clone(),
                _ => continue,
            },
            _ => continue,
        };
        if let Ok(runs) = session_runs(kernel, s.entity_id.clone(), &name).await {
            out.extend(runs);
        }
    }
    Ok(out)
}

/// A model's claim on a directory over a closed interval — the span
/// of one session that worked there.
#[derive(Debug, Clone)]
pub struct Claim {
    pub model: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// Credit one commit, or refuse to.
///
/// Exactly one claimant credits it. None means no session was working
/// that directory when it landed; several means the commit is
/// genuinely ambiguous. Both are left uncredited.
///
/// This is the whole correctness argument of the module, so it is a
/// pure function over its inputs and is tested directly. The version
/// this replaces took "the newest claim at or before the commit",
/// which credits a commit to a model that had not started yet as soon
/// as the real author falls out of the window.
#[must_use]
pub fn credit(claims: &[Claim], at: DateTime<Utc>) -> Option<&str> {
    let mut hit: Vec<&str> = claims
        .iter()
        .filter(|cl| cl.from <= at && at <= cl.to)
        .map(|cl| cl.model.as_str())
        .collect();
    hit.sort_unstable();
    hit.dedup();
    match hit.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// Roll credited commits and per-model token totals into the answer.
/// Pure, so the arithmetic is tested without touching git.
#[must_use]
pub fn tally(
    credited: &[(String, i64, i64, i64)],
    tokens: &HashMap<String, (i64, i64, i64)>,
) -> Vec<ThrownAway> {
    let mut per: HashMap<String, (i64, i64, i64, Vec<i64>)> = HashMap::new();
    for (model, added, alive, age) in credited {
        let e = per.entry(model.clone()).or_insert((0, 0, 0, Vec::new()));
        e.0 += 1;
        e.1 += added;
        e.2 += alive;
        e.3.push(*age);
    }
    let mut out: Vec<ThrownAway> = per
        .into_iter()
        .map(|(model, (commits, landed, alive, mut ages))| {
            ages.sort_unstable();
            let median_age_days = ages.get(ages.len() / 2).copied().unwrap_or(0);
            let (out_tokens, messages, sessions) = tokens.get(&model).copied().unwrap_or((0, 0, 0));
            let thrown = (landed - alive).max(0);
            // What the discarded lines cost: the model's own price per
            // landed line, charged on the lines that did not last.
            let tokens_thrown = if landed > 0 {
                out_tokens.saturating_mul(thrown) / landed
            } else {
                0
            };
            ThrownAway {
                model,
                sessions,
                messages,
                out_tokens,
                commits,
                landed,
                alive,
                thrown,
                survived_pct: if landed > 0 { (100 * alive) / landed } else { 0 },
                tokens_per_line_landed: if landed > 0 { out_tokens / landed } else { 0 },
                tokens_per_line_kept: if alive > 0 { out_tokens / alive } else { 0 },
                tokens_thrown,
                median_age_days,
            }
        })
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.tokens_thrown));
    out
}

/// Roll the model runs up into what each model threw away.
///
/// A commit is credited to the model that was in charge of **that
/// directory** when it landed, via [`credit`]. Anything it declines is
/// reported as uncredited rather than absorbed by a neighbour — most
/// of those are a repository's history from before capture began, or
/// a human's own commits.
#[must_use]
pub async fn thrown_away(runs: &[ModelRun]) -> (Vec<ThrownAway>, i64, i64) {
    // Claims per top-level repository, and the earliest moment any
    // model was at work — the git walk needs no commit older than that.
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
            let Some(top) = crate::landed::toplevel(std::path::Path::new(cwd)).await else {
                continue;
            };
            claims.entry(top).or_default().push(Claim {
                model: r.model.clone(),
                from,
                to,
            });
        }
    }

    // Tokens, messages and runs per model, over every run.
    let mut tokens: HashMap<String, (i64, i64, i64)> = HashMap::new();
    for r in runs {
        let e = tokens.entry(r.model.clone()).or_insert((0, 0, 0));
        e.0 += r.out_tokens;
        e.1 += r.messages;
        e.2 += 1;
    }

    let mut credited: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut uncredited_commits = 0i64;
    let mut uncredited_lines = 0i64;
    let now = Utc::now();
    let mut roots: Vec<&String> = claims.keys().collect();
    roots.sort();
    for top in roots {
        let list = &claims[top];
        for c in crate::landed::survival(std::path::Path::new(top), earliest).await {
            match credit(list, c.at) {
                Some(model) => credited.push((
                    model.to_string(),
                    c.added,
                    c.alive,
                    (now - c.at).num_days().max(0),
                )),
                None => {
                    uncredited_commits += 1;
                    uncredited_lines += c.added;
                }
            }
        }
    }

    (
        tally(&credited, &tokens),
        uncredited_commits,
        uncredited_lines,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn claim(model: &str, from: &str, to: &str) -> Claim {
        Claim {
            model: model.to_string(),
            from: t(from),
            to: t(to),
        }
    }

    #[test]
    fn a_commit_inside_one_session_is_credited_to_it() {
        let claims = vec![claim("fable", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")];
        assert_eq!(credit(&claims, t("2026-01-01T12:00:00Z")), Some("fable"));
    }

    /// The regression this module exists for. A commit older than every
    /// claim must NOT be handed to the model that came next — that is
    /// how one model's surviving lines were credited to another and the
    /// comparison inverted.
    #[test]
    fn a_commit_before_every_session_is_not_credited_to_the_next_model() {
        let claims = vec![claim("opus", "2026-06-01T00:00:00Z", "2026-06-02T00:00:00Z")];
        assert_eq!(credit(&claims, t("2026-01-01T00:00:00Z")), None);
    }

    #[test]
    fn a_commit_after_every_session_is_not_credited_either() {
        let claims = vec![claim("opus", "2026-06-01T00:00:00Z", "2026-06-02T00:00:00Z")];
        assert_eq!(credit(&claims, t("2026-09-01T00:00:00Z")), None);
    }

    #[test]
    fn two_models_working_at_once_leaves_the_commit_uncredited() {
        let claims = vec![
            claim("opus", "2026-06-01T00:00:00Z", "2026-06-03T00:00:00Z"),
            claim("fable", "2026-06-02T00:00:00Z", "2026-06-04T00:00:00Z"),
        ];
        assert_eq!(credit(&claims, t("2026-06-02T12:00:00Z")), None);
    }

    /// Two sessions of the SAME model overlapping is not ambiguity.
    #[test]
    fn the_same_model_twice_over_still_credits() {
        let claims = vec![
            claim("fable", "2026-06-01T00:00:00Z", "2026-06-03T00:00:00Z"),
            claim("fable", "2026-06-02T00:00:00Z", "2026-06-04T00:00:00Z"),
        ];
        assert_eq!(credit(&claims, t("2026-06-02T12:00:00Z")), Some("fable"));
    }

    #[test]
    fn the_boundaries_are_inclusive() {
        let claims = vec![claim("fable", "2026-06-01T00:00:00Z", "2026-06-02T00:00:00Z")];
        assert_eq!(credit(&claims, t("2026-06-01T00:00:00Z")), Some("fable"));
        assert_eq!(credit(&claims, t("2026-06-02T00:00:00Z")), Some("fable"));
    }

    #[test]
    fn thrown_tokens_are_the_models_own_price_on_the_lines_that_died() {
        // 1000 lines landed, 400 still there: 600 thrown of 1000, and
        // 1_000_000 tokens bought the lot.
        let credited = vec![("opus".to_string(), 1000, 400, 10)];
        let mut tokens = HashMap::new();
        tokens.insert("opus".to_string(), (1_000_000, 500, 3));
        let out = tally(&credited, &tokens);
        assert_eq!(out.len(), 1);
        let m = &out[0];
        assert_eq!(m.thrown, 600);
        assert_eq!(m.survived_pct, 40);
        assert_eq!(m.tokens_per_line_landed, 1000);
        assert_eq!(m.tokens_per_line_kept, 2500);
        assert_eq!(m.tokens_thrown, 600_000);
    }

    /// The headline ordering: worst waste first, so the page does not
    /// have to sort it again.
    #[test]
    fn models_come_back_worst_waste_first() {
        let credited = vec![
            ("fable".to_string(), 1000, 900, 30),
            ("opus".to_string(), 1000, 300, 20),
        ];
        let mut tokens = HashMap::new();
        tokens.insert("fable".to_string(), (1_000_000, 100, 1));
        tokens.insert("opus".to_string(), (1_000_000, 100, 1));
        let out = tally(&credited, &tokens);
        assert_eq!(out[0].model, "opus");
        assert_eq!(out[0].tokens_thrown, 700_000);
        assert_eq!(out[1].tokens_thrown, 100_000);
    }

    #[test]
    fn a_model_that_landed_nothing_reports_zero_not_a_division_by_zero() {
        let credited: Vec<(String, i64, i64, i64)> = Vec::new();
        let mut tokens = HashMap::new();
        tokens.insert("ghost".to_string(), (5_000, 10, 1));
        assert!(tally(&credited, &tokens).is_empty());
    }

    #[test]
    fn the_median_age_is_reported_so_the_confounder_is_checkable() {
        let credited = vec![
            ("opus".to_string(), 10, 5, 1),
            ("opus".to_string(), 10, 5, 30),
            ("opus".to_string(), 10, 5, 100),
        ];
        let mut tokens = HashMap::new();
        tokens.insert("opus".to_string(), (300, 10, 1));
        let out = tally(&credited, &tokens);
        assert_eq!(out[0].median_age_days, 30);
        assert_eq!(out[0].commits, 3);
    }
}
