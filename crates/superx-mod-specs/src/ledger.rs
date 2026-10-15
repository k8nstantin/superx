//! The fault ledger: per task, every spec version and every build of it,
//! in order — who committed each version and why, what changed in it,
//! how each build did against its checks, and what became of its branch.
//!
//! Computed when it is asked for, from this module's own tables; nothing
//! here is stored. It states facts side by side and judges nothing: a
//! build that followed its checks and was thrown away, followed by a
//! spec version "from the review of its vN build", is for the reader to
//! read.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use crate::session::{get_int, get_str};
use crate::store::get_datetime;

fn record(v: &Value, key: &str) -> Option<RecordId> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::RecordId(r)) => Some(r.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn boolean(v: &Value, key: &str) -> Option<bool> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        },
        _ => None,
    }
}

fn when(v: &Value, key: &str) -> String {
    get_datetime(v, key).map_or_else(|| "?".to_string(), |d| d.format("%Y-%m-%d %H:%M").to_string())
}

fn short(session_key: &str) -> String {
    session_key.chars().take(8).collect()
}

/// Who did something: the model, and the session it did it in.
fn who(v: &Value) -> String {
    format!(
        "{} (session {})",
        get_str(v, "model").unwrap_or_else(|| "an unknown model".to_string()),
        short(&get_str(v, "session_key").unwrap_or_default())
    )
}

/// A clause's name for a reader: `gate G7`, `reads config/`, `owns`.
fn clause_name(kind: &str, ident: Option<&str>) -> String {
    match ident {
        Some(i) => format!("{kind} {i}"),
        None => kind.to_string(),
    }
}

/// The clauses of a spec file, by name, with their text.
async fn clauses(db: &Db, doc: &RecordId) -> Result<BTreeMap<String, String>> {
    let rows: Vec<Value> = db
        .query("SELECT kind, ident, text, position FROM spec_clause WHERE doc = $doc ORDER BY position ASC")
        .bind(("doc", doc.clone()))
        .await?
        .take(0)?;
    let mut out = BTreeMap::new();
    for r in rows {
        let name = clause_name(&get_str(&r, "kind").unwrap_or_default(), get_str(&r, "ident").as_deref());
        // Two clauses of one kind with no name (never in the format, but
        // possible): keep both, numbered.
        let mut key = name.clone();
        let mut n = 2;
        while out.contains_key(&key) {
            key = format!("{name} #{n}");
            n += 1;
        }
        out.insert(key, get_str(&r, "text").unwrap_or_default());
    }
    Ok(out)
}

/// The clauses that differ between two versions of a task file.
fn changed(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    names
        .into_iter()
        .filter_map(|n| match (before.get(n), after.get(n)) {
            (Some(a), Some(b)) if a == b => None,
            (Some(_), Some(_)) => Some(n.clone()),
            (None, Some(_)) => Some(format!("{n} (new)")),
            (Some(_), None) => Some(format!("{n} (gone)")),
            (None, None) => None,
        })
        .collect()
}

/// How much an event settles a branch's fate, for events of one call.
fn weight(event: Option<&str>) -> u8 {
    match event {
        Some("deleted-remote") => 4,
        Some("deleted") => 3,
        Some("merged") => 2,
        Some("pushed") => 1,
        _ => 0,
    }
}

/// The events about a branch, whether a call named it bare or by its
/// remote (`origin/<branch>`).
fn about<'a>(events: &'a [Value], branch: &str) -> Vec<&'a Value> {
    let remote = format!("/{branch}");
    events
        .iter()
        .filter(|e| get_str(e, "branch").is_some_and(|b| b == branch || b.ends_with(&remote)))
        .collect()
}

/// What became of a branch: its newest successful event, in words. One
/// call that deletes a branch here and on the remote reads as the
/// remote deletion.
fn fate(events: &[&Value]) -> String {
    let done: Vec<&&Value> = events.iter().filter(|e| boolean(e, "ok") != Some(false)).collect();
    let Some(last) = done.iter().max_by_key(|e| (get_datetime(e, "at"), weight(get_str(e, "event").as_deref()))) else {
        return "fate unknown: no session's git touched it".to_string();
    };
    let what = match get_str(last, "event").as_deref() {
        Some("deleted-remote") => "deleted on the remote",
        Some("deleted") => "deleted locally",
        Some("merged") => "merged",
        Some("pushed") => "pushed, then nothing a session did",
        Some("created") => "created, then nothing a session did",
        _ => "touched",
    };
    format!("{what} {} by {}", when(last, "at"), who(last))
}

/// The ledger, for one task key or all of them.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn render(db: &Db, only: Option<&str>) -> Result<String> {
    let revisions: Vec<Value> = db.query("SELECT * FROM spec_revision ORDER BY at ASC").await?.take(0)?;
    let events: Vec<Value> = db.query("SELECT * FROM branch_event ORDER BY at ASC").await?.take(0)?;
    let docs: Vec<Value> =
        db.query("SELECT id, key, version, first_read_at FROM spec_doc WHERE kind = 'task' ORDER BY first_read_at ASC")
            .await?
            .take(0)?;
    let runs = crate::store::current_runs(db).await?;

    let mut keys: BTreeSet<String> = BTreeSet::new();
    keys.extend(revisions.iter().filter_map(|r| get_str(r, "key")));
    keys.extend(runs.iter().filter_map(|(r, _)| get_str(r, "key")));
    keys.retain(|k| only.is_none_or(|o| o == k));
    if keys.is_empty() {
        return Ok("nothing recorded — run `superx specs ingest`\n".to_string());
    }

    let mut out = String::new();
    for key in keys {
        let _ = writeln!(out, "{key}");
        let mut versions: BTreeSet<i64> = BTreeSet::new();
        versions.extend(revisions.iter().filter(|r| get_str(r, "key").as_deref() == Some(&key)).filter_map(|r| get_int(r, "version")));
        versions.extend(runs.iter().filter(|(r, _)| get_str(r, "key").as_deref() == Some(&key)).filter_map(|(r, _)| get_int(r, "version")));

        let mut previous: Option<BTreeMap<String, String>> = None;
        let mut builds = 0usize;
        // Per check: (complied, breached).
        let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut merged = 0usize;
        let mut after_build: Vec<String> = Vec::new();
        let mut last_build_at: Option<DateTime<Utc>> = None;
        for v in versions {
            let commits: Vec<&Value> = revisions
                .iter()
                .filter(|r| get_str(r, "key").as_deref() == Some(&key) && get_int(r, "version") == Some(v))
                .collect();
            let these: Vec<&(Value, Vec<Value>)> = runs
                .iter()
                .filter(|(r, _)| get_str(r, "key").as_deref() == Some(&key) && get_int(r, "version") == Some(v))
                .collect();
            match commits.first() {
                Some(first) => {
                    let more = if commits.len() > 1 { format!(" ({} commits)", commits.len()) } else { String::new() };
                    let _ = writeln!(out, "  v{v}  committed {} by {}{more}", when(first, "at"), who(first));
                    let line = get_str(first, "line").unwrap_or_default();
                    let _ = writeln!(out, "      \"{line}\"");
                    // A subject that names only the version keeps its
                    // reason in the body (the design sessions' rule from
                    // 2026-10-13): show the body's first paragraph.
                    let message = get_str(first, "message").unwrap_or_default();
                    if let Some(body) = message.split("\n\n").map(str::trim).find(|p| !p.is_empty() && !p.contains(&line) && !line.contains(*p)) {
                        let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
                        let cut: String = flat.chars().take(400).collect();
                        let more = if flat.chars().count() > 400 { "…" } else { "" };
                        let _ = writeln!(out, "      why: {cut}{more}");
                    }
                    let first_at = get_datetime(first, "at");
                    if let (Some(built), Some(at)) = (last_build_at, first_at) {
                        if at > built {
                            after_build.push(get_str(first, "model").unwrap_or_else(|| "an unknown model".to_string()));
                        }
                    }
                }
                None => {
                    let _ = writeln!(out, "  v{v}  no commit of it was seen in a session");
                }
            }

            // The file this version's builds read, else any recorded copy.
            let doc = these
                .iter()
                .find_map(|(r, _)| record(r, "task_doc"))
                .or_else(|| {
                    docs.iter()
                        .filter(|d| get_str(d, "key").as_deref() == Some(&key) && get_int(d, "version") == Some(v))
                        .filter_map(|d| record(d, "id"))
                        .next_back()
                });
            let current = match &doc {
                Some(d) => Some(clauses(db, d).await?),
                None => None,
            };
            match (&previous, &current) {
                (Some(before), Some(after)) => {
                    let diff = changed(before, after);
                    if !diff.is_empty() {
                        let _ = writeln!(out, "      changed: {}", diff.join("; "));
                    }
                }
                (_, None) => {
                    let _ = writeln!(out, "      (no build or session read this version whole; its clauses are unknown)");
                }
                _ => {}
            }
            if current.is_some() {
                previous = current;
            }

            for (run, findings) in these {
                builds += 1;
                let label = get_str(run, "label").or_else(|| get_str(run, "model")).unwrap_or_else(|| "?".to_string());
                let _ = writeln!(
                    out,
                    "      build {}  {label}  (session {})",
                    when(run, "started_at"),
                    short(&get_str(run, "session_key").unwrap_or_default())
                );
                for f in findings {
                    let result = get_str(f, "result").unwrap_or_default();
                    let entry = tally.entry(get_str(f, "check").unwrap_or_default()).or_default();
                    match result.as_str() {
                        "complied" => entry.0 += 1,
                        "breached" => entry.1 += 1,
                        _ => {}
                    }
                    let _ = writeln!(
                        out,
                        "        {}: {result} — {}",
                        get_str(f, "check").unwrap_or_default(),
                        get_str(f, "note").unwrap_or_default()
                    );
                }
                let branch = get_str(run, "branch");
                let line = match &branch {
                    Some(b) => {
                        let mine = about(&events, b);
                        if mine.iter().any(|e| get_str(e, "event").as_deref() == Some("merged") && boolean(e, "ok") != Some(false)) {
                            merged += 1;
                        }
                        format!("branch {b}: {}", fate(&mine))
                    }
                    None => "no build branch named for this version was seen".to_string(),
                };
                let _ = writeln!(out, "        {line}");
                let started = get_datetime(run, "started_at");
                last_build_at = last_build_at.max(started);
            }
        }
        let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
        for m in &after_build {
            *by_model.entry(m.clone()).or_default() += 1;
        }
        let authors = if by_model.is_empty() {
            String::new()
        } else {
            format!(
                " — committed by {}",
                by_model.iter().map(|(m, n)| format!("{m} ×{n}")).collect::<Vec<_>>().join(", ")
            )
        };
        let checks = tally
            .iter()
            .map(|(check, (kept, broke))| format!("{check} kept by {kept}, broken by {broke}"))
            .collect::<Vec<_>>()
            .join("; ");
        let checks = if checks.is_empty() { String::new() } else { format!("; {checks}") };
        let _ = writeln!(
            out,
            "  so far: {builds} builds, {merged} merged{checks}; {} spec versions committed after a build{authors}\n",
            after_build.len()
        );
    }
    Ok(out)
}
