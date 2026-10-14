//! Every query this module issues, in one place: SELECTs against the
//! kernel's captured sessions, and CREATEs into the module's own
//! database. Nothing here emits UPDATE or DELETE (§10).

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, Kernel, Result};

use crate::session::{get_str, Row};
use crate::spec::SpecDoc;

/// A session's first human message: the kickoff line, when it is one.
pub const KICKOFF: &str = "SELECT content, emitted_at, valid_from FROM message \
     WHERE session = $sess AND role = 'user' ORDER BY valid_from ASC LIMIT 1";

/// A session's replies and tool results, one page at a time, in the
/// order the kernel captured them. Only what the checks use is projected:
/// a Read's file, a reply's writing calls, the branch and the model.
pub const TIMELINE: &str = "SELECT id, role, emitted_at, valid_from, \
     raw.gitBranch AS branch, raw.message.model AS model, \
     raw.toolUseResult.type AS result_type, raw.toolUseResult.file AS file, \
     raw.message.content[0].tool_use_id AS tool_use_id, \
     raw.message.content[WHERE type = 'tool_use'] AS uses \
     FROM message WHERE session = $sess AND role INSIDE ['assistant', 'tool'] \
     ORDER BY valid_from ASC, id ASC START $start LIMIT $limit";

/// Rows per page of [`TIMELINE`].
pub const PAGE_ROWS: u32 = 500; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Rows read from one session at most: a session longer than this is
/// judged on its first rows, and the run says so.
pub const SESSION_ROWS: usize = 50_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A datetime column, or nothing.
#[must_use]
pub fn get_datetime(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Datetime(d)) => Some(DateTime::<Utc>::from(*d)),
            _ => None,
        },
        _ => None,
    }
}

fn get_record(v: &Value, key: &str) -> Option<RecordId> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::RecordId(r)) => Some(r.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The kickoff line and when it was sent, when the session has a human
/// message at all.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn kickoff(kernel: &Kernel, session: &RecordId) -> Result<Option<(String, DateTime<Utc>)>> {
    let rows: Vec<Value> = kernel.db().query(KICKOFF).bind(("sess", session.clone())).await?.take(0)?;
    Ok(rows.first().and_then(|r| {
        let at = get_datetime(r, "emitted_at").or_else(|| get_datetime(r, "valid_from"))?;
        Some((get_str(r, "content")?, at))
    }))
}

/// A session's timeline, paged, bounded by [`SESSION_ROWS`]. The flag
/// says whether the bound cut it short.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn timeline(kernel: &Kernel, session: &RecordId) -> Result<(Vec<Row>, bool)> {
    let mut rows: Vec<Row> = Vec::new();
    let mut start: u32 = 0;
    loop {
        let page: Vec<Value> = kernel
            .db()
            .query(TIMELINE)
            .bind(("sess", session.clone()))
            .bind(("start", start))
            .bind(("limit", PAGE_ROWS))
            .await?
            .take(0)?;
        let n = page.len();
        rows.extend(page.iter().map(row_of));
        if rows.len() >= SESSION_ROWS {
            rows.truncate(SESSION_ROWS);
            return Ok((rows, true));
        }
        if n < PAGE_ROWS as usize {
            return Ok((rows, false));
        }
        start += PAGE_ROWS;
    }
}

fn row_of(v: &Value) -> Row {
    let field = |key: &str| match v {
        Value::Object(o) => o.get(key).filter(|x| !matches!(x, Value::None | Value::Null)).cloned(),
        _ => None,
    };
    Row {
        role: get_str(v, "role").unwrap_or_default(),
        at: get_datetime(v, "emitted_at").or_else(|| get_datetime(v, "valid_from")),
        branch: get_str(v, "branch"),
        model: get_str(v, "model"),
        result_type: get_str(v, "result_type"),
        file: field("file").filter(|f| matches!(f, Value::Object(_))),
        tool_use_id: get_str(v, "tool_use_id"),
        uses: match field("uses") {
            Some(Value::Array(a)) => a.into_iter().collect(),
            _ => Vec::new(),
        },
    }
}

/// Hex sha256 of a file's content: a spec file's identity.
#[must_use]
pub fn sha256(content: &str) -> String {
    Sha256::digest(content.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A new uuid7 record id in this module's database.
#[must_use]
pub fn new_id(table: &'static str) -> RecordId {
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::now_v7()))
}

/// The spec file with this content, creating it and its clauses the
/// first time it is seen. The flag says whether it was created.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors, a refusal of the
/// UNIQUE content hash among them.
pub async fn ensure_doc(db: &Db, doc: &SpecDoc, path: &str, content: &str, at: DateTime<Utc>) -> Result<(RecordId, bool)> {
    let sha = sha256(content);
    let found: Vec<RecordId> = db
        .query("SELECT VALUE id FROM spec_doc WHERE sha256 = $sha LIMIT 1")
        .bind(("sha", sha.clone()))
        .await?
        .take(0)?;
    if let Some(id) = found.into_iter().next() {
        return Ok((id, false));
    }
    let id = new_id("spec_doc");
    let now = Utc::now();
    let mut row = Object::new();
    row.insert("sha256".to_string(), Value::String(sha));
    row.insert("kind".to_string(), Value::String(doc.kind.as_str().to_string()));
    if let Some(k) = &doc.key {
        row.insert("key".to_string(), Value::String(k.clone()));
    }
    if let Some(n) = &doc.name {
        row.insert("name".to_string(), Value::String(n.clone()));
    }
    if let Some(v) = doc.version {
        row.insert("version".to_string(), Value::Number(v.into()));
    }
    row.insert("path".to_string(), Value::String(path.to_string()));
    row.insert("content".to_string(), Value::String(content.to_string()));
    row.insert("first_read_at".to_string(), Value::Datetime(at.into()));
    row.insert("valid_from".to_string(), Value::Datetime(now.into()));
    db.query("CREATE $id CONTENT $row").bind(("id", id.clone())).bind(("row", Value::Object(row))).await?.check()?;

    for (position, clause) in doc.clauses.iter().enumerate() {
        let mut row = Object::new();
        row.insert("doc".to_string(), Value::RecordId(id.clone()));
        row.insert("kind".to_string(), Value::String(clause.kind.to_string()));
        if let Some(i) = &clause.ident {
            row.insert("ident".to_string(), Value::String(i.clone()));
        }
        row.insert("position".to_string(), Value::Number(i64::try_from(position).unwrap_or(i64::MAX).into()));
        row.insert("text".to_string(), Value::String(clause.text.clone()));
        row.insert("valid_from".to_string(), Value::Datetime(now.into()));
        db.query("CREATE $id CONTENT $row")
            .bind(("id", new_id("spec_clause")))
            .bind(("row", Value::Object(row)))
            .await?
            .check()?;
    }
    Ok((id, true))
}

/// The root most recently first read at or before `at`: the version a
/// build that never read the root was nonetheless bound by. Its id, its
/// content and when it was first read.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn root_before(db: &Db, at: DateTime<Utc>) -> Result<Option<(RecordId, String, DateTime<Utc>)>> {
    let rows: Vec<Value> = db
        .query(
            "SELECT id, content, first_read_at FROM spec_doc WHERE kind = 'root' AND first_read_at <= $at \
             ORDER BY first_read_at DESC LIMIT 1",
        )
        .bind(("at", at))
        .await?
        .take(0)?;
    Ok(rows.first().and_then(|r| Some((get_record(r, "id")?, get_str(r, "content")?, get_datetime(r, "first_read_at")?))))
}

/// A clause of a spec file, by kind and name.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn clause(db: &Db, doc: &RecordId, kind: &str, ident: &str) -> Result<Option<RecordId>> {
    let found: Vec<RecordId> = db
        .query("SELECT VALUE id FROM spec_clause WHERE doc = $doc AND kind = $kind AND ident = $ident LIMIT 1")
        .bind(("doc", doc.clone()))
        .bind(("kind", kind.to_string()))
        .bind(("ident", ident.to_string()))
        .await?
        .take(0)?;
    Ok(found.into_iter().next())
}

/// What a run says, as it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub session: RecordId,
    pub session_key: String,
    pub kickoff: String,
    pub task_path: String,
    pub task_doc: Option<RecordId>,
    pub root_doc: Option<RecordId>,
    pub rules_doc: Option<RecordId>,
    pub key: Option<String>,
    pub version: Option<i64>,
    pub branch: Option<String>,
    pub label: Option<String>,
    pub model: Option<String>,
    pub reads: Vec<String>,
    pub first_write: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// The facts of a stored run row, to compare with a fresh reading.
fn run_of(v: &Value) -> Option<Run> {
    Some(Run {
        session: get_record(v, "session")?,
        session_key: get_str(v, "session_key")?,
        kickoff: get_str(v, "kickoff")?,
        task_path: get_str(v, "task_path")?,
        task_doc: get_record(v, "task_doc"),
        root_doc: get_record(v, "root_doc"),
        rules_doc: get_record(v, "rules_doc"),
        key: get_str(v, "key"),
        version: crate::session::get_int(v, "version"),
        branch: get_str(v, "branch"),
        label: get_str(v, "label"),
        model: get_str(v, "model"),
        reads: match v {
            Value::Object(o) => match o.get("reads") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|x| match x {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        },
        first_write: get_str(v, "first_write"),
        started_at: get_datetime(v, "started_at")?,
    })
}

/// The newest stored run of a session: its id and its facts.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn latest_run(db: &Db, session: &RecordId) -> Result<Option<(RecordId, Run)>> {
    let rows: Vec<Value> = db
        .query("SELECT * FROM spec_run WHERE session = $sess ORDER BY valid_from DESC LIMIT 1")
        .bind(("sess", session.clone()))
        .await?
        .take(0)?;
    Ok(rows.first().and_then(|r| Some((get_record(r, "id")?, run_of(r)?))))
}

/// The newest finding of one check on one run, as a judgement and the
/// clause it cites.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn latest_finding(
    db: &Db,
    run: &RecordId,
    check: &str,
) -> Result<Option<(String, String, Vec<String>, Option<RecordId>)>> {
    let rows: Vec<Value> = db
        .query("SELECT * FROM spec_finding WHERE run = $run AND check = $check ORDER BY valid_from DESC LIMIT 1")
        .bind(("run", run.clone()))
        .bind(("check", check.to_string()))
        .await?
        .take(0)?;
    Ok(rows.first().map(|r| {
        let evidence = match r {
            Value::Object(o) => match o.get("evidence") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|x| match x {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        (
            get_str(r, "result").unwrap_or_default(),
            get_str(r, "note").unwrap_or_default(),
            evidence,
            get_record(r, "clause"),
        )
    }))
}

/// Append a run.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn create_run(db: &Db, run: &Run) -> Result<RecordId> {
    let id = new_id("spec_run");
    let mut row = Object::new();
    row.insert("session".to_string(), Value::RecordId(run.session.clone()));
    row.insert("session_key".to_string(), Value::String(run.session_key.clone()));
    row.insert("kickoff".to_string(), Value::String(run.kickoff.clone()));
    row.insert("task_path".to_string(), Value::String(run.task_path.clone()));
    for (name, doc) in [("task_doc", &run.task_doc), ("root_doc", &run.root_doc), ("rules_doc", &run.rules_doc)] {
        if let Some(d) = doc {
            row.insert(name.to_string(), Value::RecordId(d.clone()));
        }
    }
    for (name, text) in [("key", &run.key), ("branch", &run.branch), ("label", &run.label), ("model", &run.model), ("first_write", &run.first_write)] {
        if let Some(t) = text {
            row.insert(name.to_string(), Value::String(t.clone()));
        }
    }
    if let Some(v) = run.version {
        row.insert("version".to_string(), Value::Number(v.into()));
    }
    row.insert(
        "reads".to_string(),
        Value::Array(run.reads.iter().cloned().map(Value::String).collect::<Vec<_>>().into()),
    );
    row.insert("started_at".to_string(), Value::Datetime(run.started_at.into()));
    row.insert("valid_from".to_string(), Value::Datetime(Utc::now().into()));
    db.query("CREATE $id CONTENT $row").bind(("id", id.clone())).bind(("row", Value::Object(row))).await?.check()?;
    Ok(id)
}

/// Append a finding.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn create_finding(
    db: &Db,
    run: &RecordId,
    clause: Option<&RecordId>,
    check: &str,
    judgement: &crate::read_order::Judgement,
) -> Result<RecordId> {
    let id = new_id("spec_finding");
    let mut row = Object::new();
    row.insert("run".to_string(), Value::RecordId(run.clone()));
    if let Some(c) = clause {
        row.insert("clause".to_string(), Value::RecordId(c.clone()));
    }
    row.insert("check".to_string(), Value::String(check.to_string()));
    row.insert("result".to_string(), Value::String(judgement.result.to_string()));
    row.insert("note".to_string(), Value::String(judgement.note.clone()));
    row.insert(
        "evidence".to_string(),
        Value::Array(judgement.evidence.iter().cloned().map(Value::String).collect::<Vec<_>>().into()),
    );
    row.insert("valid_from".to_string(), Value::Datetime(Utc::now().into()));
    db.query("CREATE $id CONTENT $row").bind(("id", id.clone())).bind(("row", Value::Object(row))).await?.check()?;
    Ok(id)
}

/// Every session's newest run with the newest finding of each check,
/// oldest build first. A check re-judged after the module improved
/// appends a finding; the newest is the one that stands.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn current_runs(db: &Db) -> Result<Vec<(Value, Vec<Value>)>> {
    let rows: Vec<Value> = db.query("SELECT * FROM spec_run ORDER BY valid_from DESC").await?.take(0)?;
    let mut seen: Vec<RecordId> = Vec::new();
    let mut out: Vec<(Value, Vec<Value>)> = Vec::new();
    for row in rows {
        let Some(session) = get_record(&row, "session") else { continue };
        if seen.contains(&session) {
            continue;
        }
        seen.push(session);
        let Some(id) = get_record(&row, "id") else { continue };
        let all: Vec<Value> = db
            .query("SELECT * FROM spec_finding WHERE run = $run ORDER BY valid_from DESC")
            .bind(("run", id))
            .await?
            .take(0)?;
        let mut checks: Vec<String> = Vec::new();
        let mut findings: Vec<Value> = Vec::new();
        for f in all {
            let check = get_str(&f, "check").unwrap_or_default();
            if !checks.contains(&check) {
                checks.push(check);
                findings.push(f);
            }
        }
        out.push((row, findings));
    }
    out.sort_by_key(|(row, _)| get_datetime(row, "started_at"));
    Ok(out)
}

/// A session's replies that call git through the shell or write a file,
/// in captured order, with only those calls projected. Filtered in the
/// engine, so a session without one returns nothing.
pub const GIT_CALLS: &str = "SELECT id, emitted_at, valid_from, raw.gitBranch AS branch, raw.message.model AS model, \
     raw.message.content[WHERE type = 'tool_use' AND (name = 'Write' OR (name = 'Bash' AND string::contains(input.command ?? '', 'git ')))] AS uses \
     FROM message WHERE session = $sess AND role = 'assistant' \
     AND array::len(raw.message.content[WHERE type = 'tool_use' AND (name = 'Write' OR (name = 'Bash' AND string::contains(input.command ?? '', 'git ')))]) > 0 \
     ORDER BY valid_from ASC, id ASC";

/// The results of given calls: whether each failed, and what it printed.
pub const CALL_RESULTS: &str = "SELECT raw.message.content[0].tool_use_id AS call, raw.message.content[0].is_error AS is_error, \
     raw.toolUseResult.stdout AS stdout FROM message \
     WHERE session = $sess AND role = 'tool' AND raw.message.content[0].tool_use_id INSIDE $calls";

/// A session's whole-file Read results of XML files, in captured order.
pub const SPEC_READS: &str = "SELECT id, emitted_at, valid_from, raw.toolUseResult.type AS result_type, \
     raw.toolUseResult.file AS file, raw.message.content[0].tool_use_id AS tool_use_id, role \
     FROM message WHERE session = $sess AND role = 'tool' AND raw.toolUseResult.type = 'text' \
     AND string::ends_with(raw.toolUseResult.file.filePath ?? '', '.xml') ORDER BY valid_from ASC, id ASC";

/// One reply's git and write calls.
#[derive(Debug, Clone)]
pub struct Calls {
    pub at: DateTime<Utc>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub uses: Vec<Value>,
}

/// A session's git and write calls.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn git_calls(kernel: &Kernel, session: &RecordId) -> Result<Vec<Calls>> {
    let rows: Vec<Value> = kernel.db().query(GIT_CALLS).bind(("sess", session.clone())).await?.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Calls {
                at: get_datetime(r, "emitted_at").or_else(|| get_datetime(r, "valid_from"))?,
                branch: get_str(r, "branch"),
                model: get_str(r, "model"),
                uses: match r {
                    Value::Object(o) => match o.get("uses") {
                        Some(Value::Array(a)) => a.iter().cloned().collect(),
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                },
            })
        })
        .collect())
}

/// Whether each call failed and what it printed, by call id.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn call_results(
    kernel: &Kernel,
    session: &RecordId,
    calls: Vec<String>,
) -> Result<std::collections::HashMap<String, (Option<bool>, String)>> {
    if calls.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows: Vec<Value> =
        kernel.db().query(CALL_RESULTS).bind(("sess", session.clone())).bind(("calls", calls)).await?.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let failed = match r {
                Value::Object(o) => match o.get("is_error") {
                    Some(Value::Bool(b)) => Some(*b),
                    _ => None,
                },
                _ => None,
            };
            Some((get_str(r, "call")?, (failed, get_str(r, "stdout").unwrap_or_default())))
        })
        .collect())
}

/// A session's whole-file reads of XML files, as timeline rows.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn spec_reads(kernel: &Kernel, session: &RecordId) -> Result<Vec<Row>> {
    let rows: Vec<Value> = kernel.db().query(SPEC_READS).bind(("sess", session.clone())).await?.take(0)?;
    Ok(rows.iter().map(row_of).collect())
}

/// What one commit call says about one spec version.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRevision {
    pub session: RecordId,
    pub session_key: String,
    pub model: Option<String>,
    pub key: String,
    pub version: i64,
    pub reviewed: Option<i64>,
    pub line: String,
    pub message: String,
    pub commit: Option<String>,
    pub at: DateTime<Utc>,
    pub call: String,
}

/// Record a revision, once per call. The flag says whether it was new.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn ensure_revision(db: &Db, r: &NewRevision) -> Result<bool> {
    let found: Vec<RecordId> = db
        .query("SELECT VALUE id FROM spec_revision WHERE call = $call AND key = $key AND version = $version LIMIT 1")
        .bind(("call", r.call.clone()))
        .bind(("key", r.key.clone()))
        .bind(("version", r.version))
        .await?
        .take(0)?;
    if !found.is_empty() {
        return Ok(false);
    }
    let mut row = Object::new();
    row.insert("session".to_string(), Value::RecordId(r.session.clone()));
    row.insert("session_key".to_string(), Value::String(r.session_key.clone()));
    if let Some(m) = &r.model {
        row.insert("model".to_string(), Value::String(m.clone()));
    }
    row.insert("key".to_string(), Value::String(r.key.clone()));
    row.insert("version".to_string(), Value::Number(r.version.into()));
    if let Some(v) = r.reviewed {
        row.insert("reviewed".to_string(), Value::Number(v.into()));
    }
    row.insert("line".to_string(), Value::String(r.line.clone()));
    row.insert("message".to_string(), Value::String(r.message.clone()));
    if let Some(c) = &r.commit {
        row.insert("commit".to_string(), Value::String(c.clone()));
    }
    row.insert("at".to_string(), Value::Datetime(r.at.into()));
    row.insert("call".to_string(), Value::String(r.call.clone()));
    row.insert("valid_from".to_string(), Value::Datetime(Utc::now().into()));
    db.query("CREATE $id CONTENT $row")
        .bind(("id", new_id("spec_revision")))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(true)
}

/// What one call did to one branch.
#[derive(Debug, Clone, PartialEq)]
pub struct NewBranchEvent {
    pub session: RecordId,
    pub session_key: String,
    pub model: Option<String>,
    pub branch: String,
    pub event: &'static str,
    pub command: String,
    pub ok: Option<bool>,
    pub at: DateTime<Utc>,
    pub call: String,
}

/// Record a branch event, once per call. The flag says whether it was
/// new.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn ensure_branch_event(db: &Db, e: &NewBranchEvent) -> Result<bool> {
    let found: Vec<RecordId> = db
        .query("SELECT VALUE id FROM branch_event WHERE call = $call AND branch = $branch AND event = $event LIMIT 1")
        .bind(("call", e.call.clone()))
        .bind(("branch", e.branch.clone()))
        .bind(("event", e.event.to_string()))
        .await?
        .take(0)?;
    if !found.is_empty() {
        return Ok(false);
    }
    let mut row = Object::new();
    row.insert("session".to_string(), Value::RecordId(e.session.clone()));
    row.insert("session_key".to_string(), Value::String(e.session_key.clone()));
    if let Some(m) = &e.model {
        row.insert("model".to_string(), Value::String(m.clone()));
    }
    row.insert("branch".to_string(), Value::String(e.branch.clone()));
    row.insert("event".to_string(), Value::String(e.event.to_string()));
    row.insert("command".to_string(), Value::String(e.command.clone()));
    if let Some(ok) = e.ok {
        row.insert("ok".to_string(), Value::Bool(ok));
    }
    row.insert("at".to_string(), Value::Datetime(e.at.into()));
    row.insert("call".to_string(), Value::String(e.call.clone()));
    row.insert("valid_from".to_string(), Value::Datetime(Utc::now().into()));
    db.query("CREATE $id CONTENT $row")
        .bind(("id", new_id("branch_event")))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(true)
}
