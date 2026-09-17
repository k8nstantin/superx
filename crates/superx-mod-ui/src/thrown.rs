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

use crate::api::{ModelRun, RunPoint, ThrownAway};

/// Working directories kept per session. A session is normally one
/// checkout; the cap catches the occasional wanderer without letting
/// one session claim every repository on the machine.
/// A run must have landed at least this much for its survival rate to
/// be a point rather than noise.
const RUN_MIN_LANDED: i64 = 200; // skill-allow: §9-const — render-layer floor, not a policy tunable

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


/// A SurrealQL disjunction matching any marker against `content`,
/// lower-cased on both sides. Markers are literals from
/// [`crate::stats`]; they are interpolated rather than bound because a
/// bind cannot stand where a list of alternatives goes. They contain
/// only letters, spaces and punctuation the parser treats as text —
/// asserted here so a future marker cannot smuggle syntax in.
fn any_marker(markers: &[&str]) -> String {
    let parts: Vec<String> = markers
        .iter()
        .filter(|m| !m.contains('\'') && !m.contains('\\'))
        .map(|m| format!("string::lowercase(content) CONTAINS '{}'", m.to_lowercase()))
        .collect();
    if parts.is_empty() {
        // Nothing safe to match: a predicate that is always false, so
        // the caller reads zero rather than every row.
        return "false".to_string();
    }
    format!("({})", parts.join(" OR "))
}

/// Count the operator's own turns in a span, optionally only those
/// carrying one of `markers`. `role = 'user'` is the operator: tool
/// results ride `role = 'tool'`, so they are not counted as a person
/// speaking.
async fn operator_turns(
    kernel: &Kernel,
    session: &RecordId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    markers: Option<&[&str]>,
) -> i64 {
    let extra = markers.map_or_else(String::new, |m| format!(" AND {}", any_marker(m)));
    let q = format!(
        "SELECT count() AS n FROM message \
         WHERE session = $sess AND role = 'user' \
             AND valid_from >= $from AND valid_from <= $to{extra} \
         GROUP ALL"
    );
    let rows: std::result::Result<Vec<Value>, _> = async {
        kernel
            .db()
            .query(q)
            .bind(("sess", session.clone()))
            .bind(("from", from))
            .bind(("to", to))
            .await?
            .take(0)
    }
    .await;
    rows.ok()
        .and_then(|r| r.first().map(|v| int_of(v, "n")))
        .unwrap_or(0)
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
        // The operator's half of the record, over this model's span.
        // One person writes every one of these turns, so their style is
        // a constant and a difference between models is the models.
        let asks = operator_turns(kernel, &session, first, last, None).await;
        let redo =
            operator_turns(kernel, &session, first, last, Some(&crate::stats::CORRECTIONS)).await;
        let cross =
            operator_turns(kernel, &session, first, last, Some(&crate::stats::ESCALATIONS)).await;

        // What it carried to do the work. Context is the whole prompt:
        // fresh input plus everything read back from cache.
        let rows: Vec<Value> = kernel
            .db()
            .query(
                // Every term is parenthesised: `??` binds tighter than
                // `+`, so the unbracketed sum parses as a chain of
                // coalesces and returns single digits for a prompt of
                // half a million tokens. It did exactly that once.
                "SELECT math::mean((raw.message.usage.input_tokens ?? 0) \
                     + (raw.message.usage.cache_read_input_tokens ?? 0) \
                     + (raw.message.usage.cache_creation_input_tokens ?? 0)) AS avg, \
                   math::max((raw.message.usage.input_tokens ?? 0) \
                     + (raw.message.usage.cache_read_input_tokens ?? 0) \
                     + (raw.message.usage.cache_creation_input_tokens ?? 0)) AS peak \
                 FROM message WHERE session = $sess \
                     AND (raw.message.model ?? raw.model) = $model \
                     AND raw.message.usage != NONE GROUP ALL",
            )
            .bind(("sess", session.clone()))
            .bind(("model", model.clone()))
            .await?
            .take(0)?;
        let context_avg = rows.first().map_or(0, |r| int_of(r, "avg"));
        let context_peak = rows.first().map_or(0, |r| int_of(r, "peak"));

        out.push(ModelRun {
            session: name.to_string(),
            model,
            cwds: cwds.clone(),
            first: first.to_rfc3339(),
            last: last.to_rfc3339(),
            minutes: (last - first).num_minutes().max(0),
            messages,
            out_tokens,
            operator_turns: asks,
            redo_asks: redo,
            escalations: cross,
            context_avg,
            context_peak,
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
#[derive(Debug, Default, Clone)]
pub struct Spend {
    pub out_tokens: i64,
    pub messages: i64,
    pub runs: i64,
    pub operator_turns: i64,
    pub redo_asks: i64,
    pub escalations: i64,
    pub minutes: i64,
    pub context_weighted: i64,
    pub context_msgs: i64,
    pub context_peak: i64,
}

#[must_use]
pub fn tally(
    credited: &[(String, i64, i64, i64)],
    spend: &HashMap<String, Spend>,
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
            let sp = spend.get(&model).cloned().unwrap_or_default();
            let (out_tokens, messages, sessions) = (sp.out_tokens, sp.messages, sp.runs);
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
                operator_turns: sp.operator_turns,
                redo_asks: sp.redo_asks,
                escalations: sp.escalations,
                redo_per_100: if sp.operator_turns > 0 {
                    (100 * sp.redo_asks) / sp.operator_turns
                } else {
                    0
                },
                minutes: sp.minutes,
                context_avg: if sp.context_msgs > 0 {
                    sp.context_weighted / sp.context_msgs
                } else {
                    0
                },
                context_peak: sp.context_peak,
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
pub async fn thrown_away(runs: &[ModelRun]) -> (Vec<ThrownAway>, Vec<RunPoint>, i64, i64) {
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

    // Everything that is summed per model rather than read from git.
    let mut spend: HashMap<String, Spend> = HashMap::new();
    for r in runs {
        let e = spend.entry(r.model.clone()).or_default();
        e.out_tokens += r.out_tokens;
        e.messages += r.messages;
        e.runs += 1;
        e.operator_turns += r.operator_turns;
        e.redo_asks += r.redo_asks;
        e.escalations += r.escalations;
        e.minutes += r.minutes;
        // Context is a per-turn figure, so it averages by message
        // count rather than by run: a two-message stint must not weigh
        // the same as a thousand-message one.
        e.context_weighted += r.context_avg.saturating_mul(r.messages);
        e.context_msgs += if r.context_avg > 0 { r.messages } else { 0 };
        e.context_peak = e.context_peak.max(r.context_peak);
    }

    let mut credited: Vec<(String, i64, i64, i64)> = Vec::new();
    // Per RUN as well as per model: three models make three points,
    // which is not a scatter. Each run is its own point, so the
    // relationship between going off course and work that lasts can be
    // seen rather than asserted.
    let mut per_run: HashMap<usize, (i64, i64)> = HashMap::new();
    let mut uncredited_commits = 0i64;
    let mut uncredited_lines = 0i64;
    let now = Utc::now();
    let mut roots: Vec<&String> = claims.keys().collect();
    roots.sort();
    for top in roots {
        let list = &claims[top];
        for c in crate::landed::survival(std::path::Path::new(top), earliest).await {
            match credit(list, c.at) {
                Some(model) => {
                    credited.push((
                        model.to_string(),
                        c.added,
                        c.alive,
                        (now - c.at).num_days().max(0),
                    ));
                    // The run that owned this checkout at that moment.
                    if let Some(i) = runs.iter().position(|r| {
                        r.model == model
                            && r.cwds.iter().any(|w| w.starts_with(top.as_str()))
                            && DateTime::parse_from_rfc3339(&r.first)
                                .map(|f| f.with_timezone(&Utc) <= c.at)
                                .unwrap_or(false)
                            && DateTime::parse_from_rfc3339(&r.last)
                                .map(|l| c.at <= l.with_timezone(&Utc))
                                .unwrap_or(false)
                    }) {
                        let e = per_run.entry(i).or_insert((0, 0));
                        e.0 += c.added;
                        e.1 += c.alive;
                    }
                }
                None => {
                    uncredited_commits += 1;
                    uncredited_lines += c.added;
                }
            }
        }
    }

    // Points for the correlation: one per run that both landed work
    // and saw the operator speak, so neither axis is invented.
    let mut points: Vec<RunPoint> = per_run
        .into_iter()
        .filter_map(|(i, (landed, alive))| {
            let r = runs.get(i)?;
            (landed >= RUN_MIN_LANDED && r.operator_turns > 0).then(|| RunPoint {
                model: r.model.clone(),
                session: r.session.clone(),
                landed,
                survived_pct: (100 * alive) / landed,
                minutes: r.minutes,
                messages: r.messages,
                operator_turns: r.operator_turns,
                corrections: r.redo_asks,
                corrections_per_100: (100 * r.redo_asks) / r.operator_turns,
            })
        })
        .collect();
    points.sort_by_key(|p| std::cmp::Reverse(p.landed));

    (
        tally(&credited, &spend),
        points,
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

    /// Tokens, messages and runs for one model — the fields the
    /// arithmetic tests care about; the rest default to zero.
    fn spend_of(out_tokens: i64, messages: i64, runs: i64) -> Spend {
        Spend {
            out_tokens,
            messages,
            runs,
            ..Spend::default()
        }
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
        tokens.insert("opus".to_string(), spend_of(1_000_000, 500, 3));
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
        tokens.insert("fable".to_string(), spend_of(1_000_000, 100, 1));
        tokens.insert("opus".to_string(), spend_of(1_000_000, 100, 1));
        let out = tally(&credited, &tokens);
        assert_eq!(out[0].model, "opus");
        assert_eq!(out[0].tokens_thrown, 700_000);
        assert_eq!(out[1].tokens_thrown, 100_000);
    }

    #[test]
    fn a_model_that_landed_nothing_reports_zero_not_a_division_by_zero() {
        let credited: Vec<(String, i64, i64, i64)> = Vec::new();
        let mut tokens = HashMap::new();
        tokens.insert("ghost".to_string(), spend_of(5_000, 10, 1));
        assert!(tally(&credited, &tokens).is_empty());
    }

    /// Going off course is counted as a RATE against the operator's own
    /// turns, not as a raw total. One model getting twice the work is
    /// otherwise indistinguishable from one going wrong twice as often.
    #[test]
    fn being_told_again_is_a_rate_not_a_count() {
        let credited = vec![
            ("busy".to_string(), 1000, 500, 10),
            ("rare".to_string(), 1000, 500, 10),
        ];
        let mut spend = HashMap::new();
        // Twice the turns, twice the redo asks: the SAME rate.
        spend.insert(
            "busy".to_string(),
            Spend { out_tokens: 10, messages: 400, runs: 2, operator_turns: 200, redo_asks: 20, ..Spend::default() },
        );
        spend.insert(
            "rare".to_string(),
            Spend { out_tokens: 10, messages: 200, runs: 1, operator_turns: 100, redo_asks: 10, ..Spend::default() },
        );
        let out = tally(&credited, &spend);
        let busy = out.iter().find(|m| m.model == "busy").unwrap();
        let rare = out.iter().find(|m| m.model == "rare").unwrap();
        assert_eq!(busy.redo_per_100, 10);
        assert_eq!(rare.redo_per_100, 10);
        assert_eq!(busy.redo_asks, 20);
    }

    /// A model nobody spoke to reports no rate rather than dividing by
    /// zero and claiming perfection.
    #[test]
    fn no_operator_turns_means_no_rate_not_a_perfect_score() {
        let credited = vec![("silent".to_string(), 100, 50, 5)];
        let mut spend = HashMap::new();
        spend.insert("silent".to_string(), Spend { out_tokens: 10, ..Spend::default() });
        let out = tally(&credited, &spend);
        assert_eq!(out[0].redo_per_100, 0);
        assert_eq!(out[0].operator_turns, 0);
    }

    #[test]
    fn the_median_age_is_reported_so_the_confounder_is_checkable() {
        let credited = vec![
            ("opus".to_string(), 10, 5, 1),
            ("opus".to_string(), 10, 5, 30),
            ("opus".to_string(), 10, 5, 100),
        ];
        let mut tokens = HashMap::new();
        tokens.insert("opus".to_string(), spend_of(300, 10, 1));
        let out = tally(&credited, &tokens);
        assert_eq!(out[0].median_age_days, 30);
        assert_eq!(out[0].commits, 3);
    }
}
