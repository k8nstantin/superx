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

/// The newest stored run of a session.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn latest_run(db: &Db, session: &RecordId) -> Result<Option<Run>> {
    let rows: Vec<Value> = db
        .query("SELECT * FROM spec_run WHERE session = $sess ORDER BY valid_from DESC LIMIT 1")
        .bind(("sess", session.clone()))
        .await?
        .take(0)?;
    Ok(rows.first().and_then(run_of))
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

/// Every session's newest run with its findings, oldest build first.
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
        let findings: Vec<Value> = db
            .query("SELECT * FROM spec_finding WHERE run = $run ORDER BY valid_from ASC")
            .bind(("run", id))
            .await?
            .take(0)?;
        out.push((row, findings));
    }
    out.sort_by_key(|(row, _)| get_datetime(row, "started_at"));
    Ok(out)
}
