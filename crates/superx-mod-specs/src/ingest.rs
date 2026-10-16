//! Read every captured session, find the builds, and record what each
//! build was told and what it did.
//!
//! A BUILD is a session whose first human message is a kickoff line
//! naming one file, and which read that file whole and found a task
//! file in it (rules.xml kickoff). Design sessions open with a
//! conversation, not a kickoff line, and are not builds.
//!
//! TWO PASSES. The first goes through EVERY session — design sessions
//! included — and records the spec files it read whole, the spec
//! versions its commits named (`spec_revision`) and what its git did to
//! branches (`branch_event`). The second judges each build. A build that
//! never read the root still owed it its order, so it is judged against
//! the root sessions had read by the time it started — which the first
//! pass makes sure is recorded, whatever order the sessions come in.
//!
//! Every row this writes is derived from the kernel's sessions, and a
//! session that has not changed since the last ingest writes nothing.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::RecordId;
use superx_kernel::{Db, Kernel, Result};

use crate::gates;
use crate::git;
use crate::ownership;
use crate::read_order::{self, Context, Judgement};
use crate::session::{self, get_str, Event, Read, Row};
use crate::spec::{self, Kind, SpecDoc};
use crate::store::{self, Run};

/// The checks this ingest makes, as `spec_finding.check` names them.
pub const READ_ORDER: &str = "read-order";
pub const OWNERSHIP: &str = "ownership";
pub const GATES: &str = "gates";

/// What an ingest did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub sessions: usize,
    pub builds: usize,
    pub runs_written: usize,
    pub runs_unchanged: usize,
    /// Unchanged runs whose check now says something else — the module
    /// learned to read better — and got a new finding.
    pub rejudged: usize,
    pub docs_written: usize,
    pub revisions_written: usize,
    pub branch_events_written: usize,
    /// `git revert`s whose commit no session ever printed with a
    /// `Merge branch '…'` subject: what they reverted is unknown.
    pub reverts_unresolved: usize,
    /// Builds longer than the read bound, judged on their first rows.
    pub cut_short: usize,
}

/// What pass 1 gathers across sessions to resolve reverts: every line any
/// session printed after a commit hash (hash → every such line: a log
/// subject, a reflog entry, …), and every revert awaiting its branch.
#[derive(Default)]
struct GitSeen {
    printed: HashMap<String, Vec<String>>,
    reverts: Vec<(String, store::NewBranchEvent)>,
}

/// A session that opened with a kickoff line.
struct Kickoff {
    session: RecordId,
    key: String,
    line: String,
    task_path: String,
    at: DateTime<Utc>,
    /// The directory the build was launched in: its repository.
    cwd: Option<String>,
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
    let mut git_seen = GitSeen::default();
    // Pass 1: every session — what it read, committed and did to branches.
    for s in kernel.list_named_entities("node_session", "attr_session_descriptor").await? {
        report.sessions += 1;
        let key = get_str(&s.payload, "session").unwrap_or_default();
        let reads = store::spec_reads(kernel, &s.entity_id).await?;
        seen_docs(db, &session::events(&reads), &mut report).await?;
        git_activity(kernel, db, &s.entity_id, &key, &mut git_seen, &mut report).await?;
        let Some((line, at, cwd)) = store::kickoff(kernel, &s.entity_id).await? else { continue };
        let Some(task_path) = spec::kickoff_task_path(&line) else { continue };
        kickoffs.push(Kickoff { session: s.entity_id.clone(), key, line, task_path, at, cwd });
    }
    // A revert names a commit; the branch is the one that commit's printed
    // subject merged — printed by whichever session showed it.
    for (sha, mut event) in git_seen.reverts {
        // The same hash prints as a log subject, a reflog entry, a list of
        // refs: the line that says which branch it merged is the one.
        let branch = git_seen
            .printed
            .iter()
            .filter(|(h, _)| h.starts_with(sha.as_str()) || sha.starts_with(h.as_str()))
            .flat_map(|(_, lines)| lines.iter())
            .find_map(|s| git::merged_branch(s));
        let Some(branch) = branch else {
            report.reverts_unresolved += 1;
            continue;
        };
        event.branch = branch;
        report.branch_events_written += usize::from(store::ensure_branch_event(db, &event).await?);
    }
    // Pass 2: judge each build.
    for k in &kickoffs {
        record_run(kernel, db, k, &mut report).await?;
    }
    tracing::info!(target: "specs", ?report, "ingest done");
    Ok(report)
}

/// Record what a session's git calls did: the spec versions its commits
/// named, and what it did to branches. A commit message given by file
/// (`-F`) is the one the session wrote last under that file name, with
/// the Write tool or a `cat > … <<EOF` in a shell call.
async fn git_activity(
    kernel: &Kernel,
    db: &Db,
    session: &RecordId,
    key: &str,
    git_seen: &mut GitSeen,
    report: &mut Report,
) -> Result<()> {
    let calls = store::git_calls(kernel, session).await?;
    if calls.is_empty() {
        return Ok(());
    }
    let bash: Vec<String> = calls
        .iter()
        .flat_map(|c| c.uses.iter())
        .filter(|u| get_str(u, "name").as_deref() == Some("Bash"))
        .filter_map(|u| get_str(u, "id"))
        .collect();
    let results = store::call_results(kernel, session, bash).await?;
    let mut written: HashMap<String, String> = HashMap::new();
    for c in &calls {
        for u in &c.uses {
            let input = field(u, "input");
            match get_str(u, "name").as_deref() {
                Some("Write") => {
                    if let (Some(path), Some(content)) = (
                        input.as_ref().and_then(|i| get_str(i, "file_path")),
                        input.as_ref().and_then(|i| get_str(i, "content")),
                    ) {
                        written.insert(basename(&path).to_string(), content);
                    }
                }
                Some("Bash") => {
                    let (Some(command), Some(call)) = (input.as_ref().and_then(|i| get_str(i, "command")), get_str(u, "id"))
                    else {
                        continue;
                    };
                    for (path, body) in git::written_files(&command) {
                        written.insert(basename(&path).to_string(), body);
                    }
                    let (failed, stdout) = results.get(&call).cloned().unwrap_or((None, String::new()));
                    let ok = failed.map(|f| !f);
                    let activity = git::activity(&command);
                    for (hash, subject) in git::printed_commits(&stdout) {
                        let lines = git_seen.printed.entry(hash.to_lowercase()).or_default();
                        if !lines.contains(&subject) {
                            lines.push(subject);
                        }
                    }
                    for r in &activity.reverts {
                        git_seen.reverts.push((
                            r.sha.clone(),
                            store::NewBranchEvent {
                                session: session.clone(),
                                session_key: key.to_string(),
                                model: c.model.clone(),
                                branch: String::new(),
                                event: "reverted",
                                command: r.command.clone(),
                                ok,
                                at: c.at,
                                call: call.clone(),
                            },
                        ));
                    }
                    for e in activity.branches {
                        let new = store::ensure_branch_event(
                            db,
                            &store::NewBranchEvent {
                                session: session.clone(),
                                session_key: key.to_string(),
                                model: c.model.clone(),
                                branch: e.branch,
                                event: e.event,
                                command: e.command,
                                ok,
                                at: c.at,
                                call: call.clone(),
                            },
                        )
                        .await?;
                        report.branch_events_written += usize::from(new);
                    }
                    if ok != Some(true) {
                        continue;
                    }
                    let printed = git::printed_commits(&stdout);
                    for commit in activity.commits {
                        let mut message = commit.message.clone();
                        if let Some(body) = commit.message_file.as_deref().and_then(|f| written.get(basename(f))) {
                            if !message.is_empty() {
                                message.push_str("\n\n");
                            }
                            message.push_str(body);
                        }
                        let mut revisions = git::revisions(&message);
                        if revisions.is_empty() {
                            // The message is out of reach; what the session
                            // printed of the commit is not.
                            let subjects: Vec<&str> = printed.iter().map(|(_, s)| s.as_str()).collect();
                            message = subjects.join("\n");
                            revisions = git::revisions(&message);
                        }
                        for r in revisions {
                            let named = format!("{} file version {}", r.key, r.version);
                            let commit = printed.iter().find(|(_, s)| s.contains(&named)).map(|(h, _)| h.clone());
                            let new = store::ensure_revision(
                                db,
                                &store::NewRevision {
                                    session: session.clone(),
                                    session_key: key.to_string(),
                                    model: c.model.clone(),
                                    key: r.key,
                                    version: r.version,
                                    reviewed: r.reviewed,
                                    line: r.line,
                                    message: message.clone(),
                                    commit,
                                    at: c.at,
                                    call: call.clone(),
                                },
                            )
                            .await?;
                            report.revisions_written += usize::from(new);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn field(v: &superx_kernel::types::Value, key: &str) -> Option<superx_kernel::types::Value> {
    match v {
        superx_kernel::types::Value::Object(o) => o.get(key).cloned(),
        _ => None,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
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
    let stored = store::latest_run(db, &k.session).await?;

    let read_order = if first_write.is_none() {
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
    // Level 2: every write of the session, against what the task owns.
    let writes: Vec<&session::Write> = events
        .iter()
        .filter_map(|e| match e {
            Event::Write(w) => Some(w),
            Event::Read(_) => None,
        })
        .collect();
    let mut owned = ownership::judge(&writes, k.cwd.as_deref(), &task.doc.owns, &task.doc.forbid);
    if cut {
        owned.note.push_str(&format!(" (judged on the session's first {} rows)", store::SESSION_ROWS));
    }
    let owns_clause = store::clause_of_kind(db, &task.id, "owns").await?;
    // Level 3: the gate table the build wrote, held against the task and
    // against the session's own commands and output.
    let documents = session::documents(&rows);
    let commands = session::commands(&rows);
    let outputs = session::outputs(&rows);
    let mut gated = gates::judge(&task.doc.gates, &strs(&documents), &strs(&commands), &strs(&outputs));
    if cut {
        gated.note.push_str(&format!(" (judged on the session's first {} rows)", store::SESSION_ROWS));
    }
    let checks = [(READ_ORDER, read_order.0, read_order.1), (OWNERSHIP, owns_clause, owned), (GATES, None, gated)];

    // The same facts: the run stands, and only a verdict that changed —
    // because the check did, or the session wrote more — is appended.
    if let Some((run_id, _)) = stored.filter(|(_, facts)| *facts == run) {
        report.runs_unchanged += 1;
        let mut changed = false;
        for (check, clause, judgement) in &checks {
            let same = store::latest_finding(db, &run_id, check).await?.is_some_and(|(result, note, evidence, cited)| {
                result == judgement.result && note == judgement.note && evidence == judgement.evidence && cited == *clause
            });
            if !same {
                store::create_finding(db, &run_id, clause.as_ref(), check, judgement).await?;
                changed = true;
            }
        }
        report.rejudged += usize::from(changed);
        return Ok(());
    }
    let run_id = store::create_run(db, &run).await?;
    for (check, clause, judgement) in &checks {
        store::create_finding(db, &run_id, clause.as_ref(), check, judgement).await?;
    }
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

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// A read path is the kickoff's task file.
fn path_matches(path: &str, task_path: &str) -> bool {
    let rel = task_path.trim_start_matches("./");
    path == rel || path.ends_with(&format!("/{rel}"))
}
