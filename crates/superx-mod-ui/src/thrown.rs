//! The uncapped per-model reading layer (#406).
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

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, Result};

use crate::api::ModelRun;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("test fixtures are literal RFC3339 timestamps written in this file")
            .with_timezone(&Utc)
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

}
