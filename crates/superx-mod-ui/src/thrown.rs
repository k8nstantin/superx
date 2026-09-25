//! The uncapped per-model reading layer (#406, #414).
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
//! So nothing here reads a message payload beyond the handful of fields
//! a run needs — when, which model, which reply, where, and what it
//! spent — and the session list itself is uncapped.
//!
//! # A run (#414)
//!
//! A session is split into RUNS: consecutive replies by one model
//! family, on the agent's clock. The earlier version gave each model one
//! span per session — first reply to last, by capture time — so a
//! session that went fable → opus → fable had two overlapping spans and
//! every commit between them was ambiguous, and a session captured by
//! the first backfill had every span squeezed into the minutes the
//! backfill took. A run's time is its WORKING time: the gaps between its
//! replies, each capped at the live-session threshold.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Kernel, Result};

use crate::api::{ModelRun, RunWork};

/// Working directories kept per run. A run is normally one checkout;
/// the cap catches the occasional wanderer without letting one run claim
/// every repository on the machine.
const CWDS_PER_RUN: usize = 8; // skill-allow: §9-const — read-path bound, not a policy tunable

/// The model family: `claude-opus-5-5` is `opus`, `claude-fable-5-1` is
/// `fable`, `gemini-3.1-pro` is `gemini` (#408, #414). Point releases of
/// one model are one choice from the operator's side, and splitting them
/// lets a thin recent release distort a rate. The family is the first
/// word of the name that is not the vendor's and not a version number.
#[must_use]
pub fn family(model: &str) -> String {
    let rest = model.strip_prefix("claude-").unwrap_or(model);
    rest.split('-')
        .find(|seg| seg.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
        .unwrap_or(rest)
        .to_string()
}

fn obj(v: &Value) -> Option<&Object> {
    match v {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

fn str_of<'a>(o: &'a Object, key: &str) -> Option<&'a str> {
    match o.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.as_str()),
        _ => None,
    }
}

fn time_of(o: &Object, key: &str) -> Option<DateTime<Utc>> {
    match o.get(key) {
        Some(Value::Datetime(d)) => Some(**d),
        _ => None,
    }
}

/// The reply a row belongs to: Claude's `message.id` or Gemini's record
/// `id` (strings), or the row's own id when it carries neither.
fn key_of(o: &Object) -> Option<String> {
    match o.get("k") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::RecordId(r)) => Some(superx_ops::record_uuid(r)),
        _ => None,
    }
}

/// One reply of a session, folded from its rows.
struct Reply {
    at: DateTime<Utc>,
    model: String,
    cwd: Option<String>,
    usage: Option<crate::stats::ReplyUsage>,
}

/// Every run of one session (#414). Replies are counted once (#409), a
/// run is contiguous, and its time is the gaps between its replies, each
/// capped at `idle_secs`.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_runs(
    kernel: &Kernel,
    session: RecordId,
    name: &str,
    idle_secs: i64,
) -> Result<Vec<ModelRun>> {
    let rows: Vec<Value> = kernel
        .db()
        .query(format!(
            "SELECT (emitted_at ?? valid_from) AS at, (raw.message.model ?? raw.model) AS model, \
                 {} AS k, raw.cwd AS cwd, \
                 raw.message.usage AS cu, raw.tokens AS gu, valid_from \
             FROM message WHERE session = $sess \
                 AND (raw.message.model != NONE OR raw.model != NONE) \
             ORDER BY valid_from ASC",
            crate::stats::REPLY_KEY_SQL
        ))
        .bind(("sess", session.clone()))
        .await?
        .take(0)?;
    // Fold a reply's rows: they share a key, and the last of them is the
    // fullest (a Gemini record re-emitted as it streamed).
    let mut replies: Vec<Reply> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for o in rows.iter().filter_map(obj) {
        // `<synthetic>` and friends are the runtime's own markers, not
        // models (#367/#388).
        let (Some(model), Some(at), Some(key)) = (
            str_of(o, "model").filter(|m| !m.starts_with('<')),
            time_of(o, "at"),
            key_of(o),
        ) else {
            continue;
        };
        let usage = crate::stats::usage_of(o.get("cu").and_then(obj), o.get("gu").and_then(obj));
        let reply = Reply {
            at,
            model: model.to_string(),
            cwd: str_of(o, "cwd").map(str::to_string),
            usage,
        };
        match index.get(&key) {
            Some(&i) => {
                let prev = &mut replies[i];
                prev.at = prev.at.max(reply.at);
                prev.usage = reply.usage.or(prev.usage);
                prev.cwd = reply.cwd.or(prev.cwd.take());
            }
            None => {
                index.insert(key, replies.len());
                replies.push(reply);
            }
        }
    }
    replies.sort_by_key(|r| r.at);

    // The operator's turns, each answered by the run that was going when
    // it was written (#414): the one that started last before it.
    let turns: Vec<(DateTime<Utc>, String)> = kernel
        .db()
        .query(
            "SELECT (emitted_at ?? valid_from) AS at, content, valid_from FROM message \
             WHERE session = $sess AND role = 'user' ORDER BY valid_from ASC",
        )
        .bind(("sess", session))
        .await?
        .take::<Vec<Value>>(0)?
        .iter()
        .filter_map(obj)
        .filter_map(|o| Some((time_of(o, "at")?, str_of(o, "content").unwrap_or("").to_string())))
        .collect();

    let mut out: Vec<ModelRun> = Vec::new();
    let mut starts: Vec<DateTime<Utc>> = Vec::new();
    let mut start = 0;
    while start < replies.len() {
        let fam = family(&replies[start].model);
        let mut end = start + 1;
        while end < replies.len() && family(&replies[end].model) == fam {
            end += 1;
        }
        starts.push(replies[start].at);
        out.push(build_run(name, &fam, &replies[start..end], idle_secs));
        start = end;
    }
    for (at, content) in &turns {
        // A turn before the first reply opened the session: the first run
        // answered it.
        let Some(i) = starts.iter().rposition(|s| s <= at).or((!out.is_empty()).then_some(0)) else {
            continue;
        };
        let run = &mut out[i];
        run.operator_turns += 1;
        if crate::stats::says_word(content, &crate::stats::CORRECTIONS, false) {
            run.redo_asks += 1;
        }
        if crate::stats::says_word(content, &crate::stats::ESCALATIONS, true) {
            run.escalations += 1;
        }
    }
    Ok(out)
}

/// One run from its replies, oldest first.
fn build_run(session: &str, fam: &str, replies: &[Reply], idle_secs: i64) -> ModelRun {
    let mut versions: HashMap<&str, i64> = HashMap::new();
    let mut work: HashMap<String, RunWork> = HashMap::new();
    let (mut out_tokens, mut ctx_sum, mut ctx_n, mut ctx_peak, mut secs) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for (i, r) in replies.iter().enumerate() {
        *versions.entry(r.model.as_str()).or_insert(0) += 1;
        // The gap before a reply was spent on it — up to the threshold.
        let gap = if i == 0 {
            0
        } else {
            (r.at - replies[i - 1].at).num_seconds().clamp(0, idle_secs)
        };
        secs += gap;
        let out = r.usage.map_or(0, |u| u.out);
        out_tokens += out;
        if let Some(u) = r.usage.filter(|u| u.context > 0) {
            ctx_sum += u.context;
            ctx_n += 1;
            ctx_peak = ctx_peak.max(u.context);
        }
        if let Some(cwd) = &r.cwd {
            let w = work.entry(cwd.clone()).or_insert_with(|| RunWork {
                cwd: cwd.clone(),
                replies: 0,
                out_tokens: 0,
                minutes: 0,
            });
            w.replies += 1;
            w.out_tokens += out;
            // Minutes are whole at the end; seconds carry until then.
            w.minutes += gap;
        }
    }
    let mut work: Vec<RunWork> = work
        .into_values()
        .map(|mut w| {
            w.minutes /= 60;
            w
        })
        .collect();
    work.sort_by(|a, b| b.replies.cmp(&a.replies).then(a.cwd.cmp(&b.cwd)));
    let mut cwds: Vec<String> = work.iter().map(|w| w.cwd.clone()).collect();
    cwds.truncate(CWDS_PER_RUN);
    let version = versions
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map_or_else(|| fam.to_string(), |(v, _)| (*v).to_string());
    ModelRun {
        session: session.to_string(),
        model: fam.to_string(),
        version,
        cwds,
        first: replies.first().map(|r| r.at.to_rfc3339()).unwrap_or_default(),
        last: replies.last().map(|r| r.at.to_rfc3339()).unwrap_or_default(),
        minutes: secs / 60,
        messages: replies.len() as i64,
        out_tokens,
        operator_turns: 0,
        redo_asks: 0,
        escalations: 0,
        context_avg: if ctx_n > 0 { ctx_sum / ctx_n } else { 0 },
        context_peak: ctx_peak,
        work,
    }
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
    let idle = crate::stats::resolved_active_secs(kernel).await;
    let mut out = Vec::new();
    for s in sessions {
        let name = match &s.payload {
            Value::Object(o) => match o.get("name") {
                Some(Value::String(v)) => v.clone(),
                _ => continue,
            },
            _ => continue,
        };
        if let Ok(runs) = session_runs(kernel, s.entity_id.clone(), &name, idle).await {
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
    fn a_family_is_the_model_without_its_vendor_or_version() {
        assert_eq!(family("claude-opus-5-5"), "opus");
        assert_eq!(family("claude-fable-5-1"), "fable");
        assert_eq!(family("claude-fable-5"), "fable");
        assert_eq!(family("claude-3-5-sonnet-20241022"), "sonnet");
        assert_eq!(family("claude-haiku-4-5-20251001"), "haiku");
        assert_eq!(family("gemini-3.1-pro-preview"), "gemini");
    }

    #[test]
    fn the_boundaries_are_inclusive() {
        let claims = vec![claim("fable", "2026-06-01T00:00:00Z", "2026-06-02T00:00:00Z")];
        assert_eq!(credit(&claims, t("2026-06-01T00:00:00Z")), Some("fable"));
        assert_eq!(credit(&claims, t("2026-06-02T00:00:00Z")), Some("fable"));
    }

}
