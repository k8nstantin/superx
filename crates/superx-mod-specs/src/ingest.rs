//! Read every captured session, find the builds, and record what each
//! build was told and what it did.
//!
//! A BUILD is a session whose first human message is a kickoff line
//! naming one file, and which read that file whole and found a task
//! file in it (rules.xml kickoff). Design sessions open with a
//! conversation, not a kickoff line, and are not builds.
//!
//! TWO PASSES. The first records every spec file every build read; the
//! second judges each build. A build that never read the root still owed
//! it its order, so it is judged against the root other sessions had
//! read by the time it started — which the first pass makes sure is
//! recorded, whatever order the sessions come in.
//!
//! Every row this writes is derived from the kernel's sessions, and a
//! session that has not changed since the last ingest writes nothing.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::RecordId;
use superx_kernel::{Db, Kernel, Result};

use crate::read_order::{self, Context, Judgement};
use crate::session::{self, get_str, Event, Read, Row};
use crate::spec::{self, Kind, SpecDoc};
use crate::store::{self, Run};

/// The check this ingest makes, as `spec_finding.check` names it.
pub const READ_ORDER: &str = "read-order";

/// What an ingest did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub sessions: usize,
    pub builds: usize,
    pub runs_written: usize,
    pub runs_unchanged: usize,
    pub docs_written: usize,
    /// Builds longer than the read bound, judged on their first rows.
    pub cut_short: usize,
}

/// A session that opened with a kickoff line.
struct Kickoff {
    session: RecordId,
    key: String,
    line: String,
    task_path: String,
    at: DateTime<Utc>,
}

/// A spec file one read of a session found.
struct Seen {
    id: RecordId,
    doc: SpecDoc,
    path: String,
}

/// Ingest every session the kernel holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for engine errors.
pub async fn ingest(kernel: &Kernel, db: &Db) -> Result<Report> {
    let mut report = Report::default();
    let mut kickoffs: Vec<Kickoff> = Vec::new();
    for s in kernel.list_named_entities("node_session", "attr_session_descriptor").await? {
        report.sessions += 1;
        let Some((line, at)) = store::kickoff(kernel, &s.entity_id).await? else { continue };
        let Some(task_path) = spec::kickoff_task_path(&line) else { continue };
        kickoffs.push(Kickoff {
            session: s.entity_id.clone(),
            key: get_str(&s.payload, "session").unwrap_or_default(),
            line,
            task_path,
            at,
        });
    }
    // Pass 1: every spec file every build read.
    for k in &kickoffs {
        let (rows, _) = store::timeline(kernel, &k.session).await?;
        seen_docs(db, &session::events(&rows), &mut report).await?;
    }
    // Pass 2: judge each build.
    for k in &kickoffs {
        record_run(kernel, db, k, &mut report).await?;
    }
    tracing::info!(target: "specs", ?report, "ingest done");
    Ok(report)
}

/// The spec files a session read whole, recorded once per content, with
/// the position of the read that found each.
async fn seen_docs(db: &Db, events: &[Event], report: &mut Report) -> Result<Vec<(usize, Seen)>> {
    let mut seen = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let Event::Read(r) = e else { continue };
        if !r.whole || !r.path.ends_with(".xml") {
            continue;
        }
        let Some(content) = &r.content else { continue };
        let Some(doc) = spec::parse(content) else { continue };
        let (id, created) = store::ensure_doc(db, &doc, &r.path, content, r.at).await?;
        if created {
            report.docs_written += 1;
        }
        seen.push((i, Seen { id, doc, path: r.path.clone() }));
    }
    Ok(seen)
}

async fn record_run(kernel: &Kernel, db: &Db, k: &Kickoff, report: &mut Report) -> Result<()> {
    let (rows, cut) = store::timeline(kernel, &k.session).await?;
    let events = session::events(&rows);
    let mut ignored = Report::default();
    let seen = seen_docs(db, &events, &mut ignored).await?;

    let (before, first_write) = session::before_first_write(&events);
    let write_at = before.len() + usize::from(first_write.is_some());
    // The last whole read of a kind before the first write, else the
    // first anywhere: the version the build was working from.
    let pick = |want: &dyn Fn(&Seen) -> bool| -> Option<&Seen> {
        seen.iter()
            .filter(|(i, s)| *i < write_at && want(s))
            .map(|(_, s)| s)
            .next_back()
            .or_else(|| seen.iter().map(|(_, s)| s).find(|s| want(s)))
    };
    let Some(task) = pick(&|s| s.doc.kind == Kind::Task && path_matches(&s.path, &k.task_path)) else { return Ok(()) };
    let root = pick(&|s| s.doc.kind == Kind::Root);
    let rules = pick(&|s| s.doc.kind == Kind::Rules);
    report.builds += 1;
    if cut {
        report.cut_short += 1;
    }

    let run = run_of(k, &rows, task, root, rules, &before, first_write.map(|w| w.path.clone()));
    if store::latest_run(db, &k.session).await?.as_ref() == Some(&run) {
        report.runs_unchanged += 1;
        return Ok(());
    }

    let (clause, judgement) = if first_write.is_none() {
        // The order binds what is read "before it writes anything": a
        // session that wrote nothing — a build that stopped at its
        // kickoff, as the kickoff rule asks of one that holds anything
        // else — cannot have broken it.
        let note = "the session wrote nothing; the read order binds the reads before a write".to_string();
        (None, Judgement { result: "no-evidence", note, evidence: Vec::new() })
    } else {
        let kinds: HashMap<String, Kind> = seen.iter().map(|(_, s)| (s.path.clone(), s.doc.kind)).collect();
        let mut j = judge_read_order(db, root, k, &task.doc, &before, &kinds).await?;
        if cut {
            j.1.note.push_str(&format!(" (judged on the session's first {} rows)", store::SESSION_ROWS));
        }
        j
    };
    let run_id = store::create_run(db, &run).await?;
    store::create_finding(db, &run_id, clause.as_ref(), READ_ORDER, &judgement).await?;
    report.runs_written += 1;
    Ok(())
}

fn run_of(
    k: &Kickoff,
    rows: &[Row],
    task: &Seen,
    root: Option<&Seen>,
    rules: Option<&Seen>,
    before: &[&Read],
    first_write: Option<String>,
) -> Run {
    let (branch, label) = match (&task.doc.key, &task.doc.name, task.doc.version) {
        (Some(key), Some(name), Some(v)) => session::build_branch(rows, key, name, v).unzip(),
        _ => (None, None),
    };
    Run {
        session: k.session.clone(),
        session_key: k.key.clone(),
        kickoff: k.line.clone(),
        task_path: k.task_path.clone(),
        task_doc: Some(task.id.clone()),
        root_doc: root.map(|s| s.id.clone()),
        rules_doc: rules.map(|s| s.id.clone()),
        key: task.doc.key.clone(),
        version: task.doc.version,
        branch,
        label,
        model: session::dominant_model(rows),
        reads: before.iter().map(|r| r.path.clone()).collect(),
        first_write,
        started_at: k.at,
    }
}

async fn judge_read_order(
    db: &Db,
    root: Option<&Seen>,
    k: &Kickoff,
    task: &SpecDoc,
    before: &[&Read],
    kinds: &HashMap<String, Kind>,
) -> Result<(Option<RecordId>, Judgement)> {
    // The session's own root; else the root other sessions had read by
    // the time this build started — the order was owed either way.
    let (root_id, root_doc, borrowed) = match root {
        Some(s) => (s.id.clone(), s.doc.clone(), None),
        None => match store::root_before(db, k.at).await? {
            Some((id, content, read_at)) => match spec::parse(&content) {
                Some(doc) => (id, doc, Some(read_at)),
                None => return Ok((None, no_root())),
            },
            None => return Ok((None, no_root())),
        },
    };
    let clause = store::clause(db, &root_id, "method", "read").await?;
    let Some(steps) = root_doc.method_step("read").and_then(|t| read_order::steps(t, root_doc.platform.as_deref())) else {
        return Ok((
            clause,
            Judgement {
                result: "no-evidence",
                note: "the root's read step gives no order this check recognises".to_string(),
                evidence: Vec::new(),
            },
        ));
    };
    let kind_of = |p: &str| kinds.get(p).copied();
    let cx = Context { platform: root_doc.platform.as_deref(), task_path: &k.task_path, named: &task.named, kind_of: &kind_of };
    let mut judgement = read_order::judge(before, &steps, !task.named.is_empty(), &cx);
    if let Some(at) = borrowed {
        judgement.note.push_str(&format!(
            " (this session never read root.xml; judged against the root first read by another session at {})",
            at.format("%Y-%m-%d %H:%M")
        ));
    }
    Ok((clause, judgement))
}

fn no_root() -> Judgement {
    Judgement {
        result: "no-evidence",
        note: "no session had read root.xml by the time this build started, so there is no read order to judge against"
            .to_string(),
        evidence: Vec::new(),
    }
}

/// A read path is the kickoff's task file.
fn path_matches(path: &str, task_path: &str) -> bool {
    let rel = task_path.trim_start_matches("./");
    path == rel || path.ends_with(&format!("/{rel}"))
}
