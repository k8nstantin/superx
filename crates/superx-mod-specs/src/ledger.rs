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
use serde::Serialize;
use ts_rs::TS;
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
        Some("deleted-remote") => 5,
        Some("deleted") => 4,
        Some("reverted") => 3,
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

/// What became of a branch: its newest successful event, in words, and
/// that event's kind. One call that deletes a branch here and on the
/// remote reads as the remote deletion.
fn fate(events: &[&Value]) -> (String, Option<String>) {
    let done: Vec<&&Value> = events.iter().filter(|e| boolean(e, "ok") != Some(false)).collect();
    let Some(last) = done.iter().max_by_key(|e| (get_datetime(e, "at"), weight(get_str(e, "event").as_deref()))) else {
        return ("fate unknown: no session's git touched it".to_string(), None);
    };
    let event = get_str(last, "event");
    let what = match event.as_deref() {
        Some("deleted-remote") => "deleted on the remote",
        Some("deleted") => "deleted locally",
        Some("merged") => "merged",
        Some("reverted") => "merged, then its merge reverted",
        Some("pushed") => "pushed, then nothing a session did",
        Some("created") => "created, then nothing a session did",
        _ => "touched",
    };
    (format!("{what} {} by {}", when(last, "at"), who(last)), event)
}

/// One task's ledger: its versions in order, and what they add up to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TaskLedger {
    pub key: String,
    pub versions: Vec<VersionLedger>,
    pub summary: Summary,
}

/// One spec version: its commit, what changed, and its builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct VersionLedger {
    #[ts(type = "number")]
    pub version: i64,
    /// The first commit a session made of it, when one was seen.
    pub commit: Option<CommitLine>,
    /// The clauses that differ from the version before; `None` when no
    /// build or session read this version whole, so its clauses are
    /// unknown.
    pub changed: Option<Vec<String>>,
    pub builds: Vec<BuildLine>,
}

/// Who committed a version, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct CommitLine {
    pub at: String,
    pub model: Option<String>,
    pub session: String,
    /// The message line that names the version.
    pub line: String,
    /// The body's first paragraph, when the subject names only the
    /// version.
    pub why: Option<String>,
    /// How many commits named this version.
    #[ts(type = "number")]
    pub commits: usize,
}

/// One build of a version: its checks, and its branch's fate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct BuildLine {
    pub started_at: String,
    pub label: String,
    pub session: String,
    pub checks: Vec<CheckLine>,
    pub branch: Option<String>,
    pub fate: String,
    /// The kind of the event the fate rests on (`merged`, `reverted`,
    /// `deleted-remote`, …).
    pub fate_event: Option<String>,
    pub merged: bool,
}

/// One check's newest finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct CheckLine {
    pub check: String,
    /// `complied`, `breached` or `no-evidence`.
    pub result: String,
    pub note: String,
}

/// What a task's ledger adds up to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct Summary {
    #[ts(type = "number")]
    pub builds: usize,
    #[ts(type = "number")]
    pub merged: usize,
    /// Per check that judged anything: kept and broken.
    pub checks: Vec<CheckTally>,
    /// Spec versions first committed after a build of the task.
    #[ts(type = "number")]
    pub after_build: usize,
    /// Who committed those, by model.
    pub authors: Vec<AuthorCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct CheckTally {
    pub check: String,
    #[ts(type = "number")]
    pub kept: usize,
    #[ts(type = "number")]
    pub broken: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AuthorCount {
    pub model: String,
    #[ts(type = "number")]
    pub versions: usize,
}

/// The ledger, for one task key or all of them, as data.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn build(db: &Db, only: Option<&str>) -> Result<Vec<TaskLedger>> {
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

    let mut out = Vec::new();
    for key in keys {
        let mut numbers: BTreeSet<i64> = BTreeSet::new();
        numbers.extend(revisions.iter().filter(|r| get_str(r, "key").as_deref() == Some(&key)).filter_map(|r| get_int(r, "version")));
        numbers.extend(runs.iter().filter(|(r, _)| get_str(r, "key").as_deref() == Some(&key)).filter_map(|(r, _)| get_int(r, "version")));

        let mut versions: Vec<VersionLedger> = Vec::new();
        let mut previous: Option<BTreeMap<String, String>> = None;
        let mut builds = 0usize;
        // Per check: (complied, breached).
        let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut merged = 0usize;
        let mut after_build: Vec<String> = Vec::new();
        let mut last_build_at: Option<DateTime<Utc>> = None;
        for v in numbers {
            let commits: Vec<&Value> = revisions
                .iter()
                .filter(|r| get_str(r, "key").as_deref() == Some(&key) && get_int(r, "version") == Some(v))
                .collect();
            let these: Vec<&(Value, Vec<Value>)> = runs
                .iter()
                .filter(|(r, _)| get_str(r, "key").as_deref() == Some(&key) && get_int(r, "version") == Some(v))
                .collect();
            let commit = commits.first().map(|first| {
                let line = get_str(first, "line").unwrap_or_default();
                // A subject that names only the version keeps its reason
                // in the body (the design sessions' rule from 2026-10-13):
                // the body's first paragraph is the why.
                let message = get_str(first, "message").unwrap_or_default();
                let why = message
                    .split("\n\n")
                    .map(str::trim)
                    .find(|p| !p.is_empty() && !p.contains(&line) && !line.contains(*p))
                    .map(|body| {
                        let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
                        let cut: String = flat.chars().take(400).collect();
                        if flat.chars().count() > 400 { format!("{cut}…") } else { cut }
                    });
                if let (Some(built), Some(at)) = (last_build_at, get_datetime(first, "at")) {
                    if at > built {
                        after_build.push(get_str(first, "model").unwrap_or_else(|| "an unknown model".to_string()));
                    }
                }
                CommitLine {
                    at: when(first, "at"),
                    model: get_str(first, "model"),
                    session: short(&get_str(first, "session_key").unwrap_or_default()),
                    line,
                    why,
                    commits: commits.len(),
                }
            });

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
            let changed_clauses = match (&previous, &current) {
                (Some(before), Some(after)) => Some(changed(before, after)),
                (None, Some(_)) => Some(Vec::new()),
                (_, None) => None,
            };
            if current.is_some() {
                previous = current;
            }

            let mut lines = Vec::new();
            for (run, findings) in these {
                builds += 1;
                let mut checks = Vec::new();
                for f in findings {
                    let result = get_str(f, "result").unwrap_or_default();
                    let check = get_str(f, "check").unwrap_or_default();
                    let entry = tally.entry(check.clone()).or_default();
                    match result.as_str() {
                        "complied" => entry.0 += 1,
                        "breached" => entry.1 += 1,
                        _ => {}
                    }
                    checks.push(CheckLine { check, result, note: get_str(f, "note").unwrap_or_default() });
                }
                let branch = get_str(run, "branch");
                let (fate_text, fate_event, is_merged) = match &branch {
                    Some(b) => {
                        let mine = about(&events, b);
                        // Merged, unless its newest merge or revert is a revert.
                        let last = mine
                            .iter()
                            .filter(|e| boolean(e, "ok") != Some(false))
                            .filter(|e| matches!(get_str(e, "event").as_deref(), Some("merged" | "reverted")))
                            .max_by_key(|e| get_datetime(e, "at"));
                        let is_merged = last.is_some_and(|e| get_str(e, "event").as_deref() == Some("merged"));
                        let (text, event) = fate(&mine);
                        (text, event, is_merged)
                    }
                    None => ("no build branch named for this version was seen".to_string(), None, false),
                };
                merged += usize::from(is_merged);
                lines.push(BuildLine {
                    started_at: when(run, "started_at"),
                    label: get_str(run, "label").or_else(|| get_str(run, "model")).unwrap_or_else(|| "?".to_string()),
                    session: short(&get_str(run, "session_key").unwrap_or_default()),
                    checks,
                    branch,
                    fate: fate_text,
                    fate_event,
                    merged: is_merged,
                });
                last_build_at = last_build_at.max(get_datetime(run, "started_at"));
            }
            versions.push(VersionLedger { version: v, commit, changed: changed_clauses, builds: lines });
        }
        let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
        for m in &after_build {
            *by_model.entry(m.clone()).or_default() += 1;
        }
        out.push(TaskLedger {
            key,
            versions,
            summary: Summary {
                builds,
                merged,
                checks: tally
                    .into_iter()
                    .filter(|(_, (kept, broke))| kept + broke > 0)
                    .map(|(check, (kept, broken))| CheckTally { check, kept, broken })
                    .collect(),
                after_build: after_build.len(),
                authors: by_model.into_iter().map(|(model, versions)| AuthorCount { model, versions }).collect(),
            },
        });
    }
    Ok(out)
}

/// The ledger, for one task key or all of them, as text.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn render(db: &Db, only: Option<&str>) -> Result<String> {
    let tasks = build(db, only).await?;
    if tasks.is_empty() {
        return Ok("nothing recorded — run `superx specs ingest`\n".to_string());
    }
    let mut out = String::new();
    for task in &tasks {
        let _ = writeln!(out, "{}", task.key);
        for v in &task.versions {
            match &v.commit {
                Some(c) => {
                    let more = if c.commits > 1 { format!(" ({} commits)", c.commits) } else { String::new() };
                    let by = c.model.as_deref().unwrap_or("an unknown model");
                    let _ = writeln!(out, "  v{}  committed {} by {by} (session {}){more}", v.version, c.at, c.session);
                    let _ = writeln!(out, "      \"{}\"", c.line);
                    if let Some(why) = &c.why {
                        let _ = writeln!(out, "      why: {why}");
                    }
                }
                None => {
                    let _ = writeln!(out, "  v{}  no commit of it was seen in a session", v.version);
                }
            }
            match &v.changed {
                Some(diff) if !diff.is_empty() => {
                    let _ = writeln!(out, "      changed: {}", diff.join("; "));
                }
                Some(_) => {}
                None => {
                    let _ = writeln!(out, "      (no build or session read this version whole; its clauses are unknown)");
                }
            }
            for b in &v.builds {
                let _ = writeln!(out, "      build {}  {}  (session {})", b.started_at, b.label, b.session);
                for c in &b.checks {
                    let _ = writeln!(out, "        {}: {} — {}", c.check, c.result, c.note);
                }
                let line = match &b.branch {
                    Some(branch) => format!("branch {branch}: {}", b.fate),
                    None => b.fate.clone(),
                };
                let _ = writeln!(out, "        {line}");
            }
        }
        let s = &task.summary;
        let authors = if s.authors.is_empty() {
            String::new()
        } else {
            format!(
                " — committed by {}",
                s.authors.iter().map(|a| format!("{} ×{}", a.model, a.versions)).collect::<Vec<_>>().join(", ")
            )
        };
        let checks = s
            .checks
            .iter()
            .map(|c| format!("{} kept by {}, broken by {}", c.check, c.kept, c.broken))
            .collect::<Vec<_>>()
            .join("; ");
        let checks = if checks.is_empty() { String::new() } else { format!("; {checks}") };
        let _ = writeln!(
            out,
            "  so far: {} builds, {} merged{checks}; {} spec versions committed after a build{authors}\n",
            s.builds, s.merged, s.after_build
        );
    }
    Ok(out)
}
