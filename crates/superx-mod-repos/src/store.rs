//! Every query this module issues, in one place: SELECTs against the
//! kernel's captured sessions, reads of the module's own database, and
//! CREATEs into it. Nothing here emits UPDATE or DELETE (§10). Row ids are
//! UUIDv7, set here (§11).
//!
//! The one thing it knows of an agent's transcript format is where a line
//! keeps its working directory: `raw.cwd`, which Claude Code writes on
//! every line. A line without one names no directory.
//!
//! Of several rows for one thing, the newest is picked here by `valid_from`
//! and never by an index's order: SurrealDB 3.1's index on a datetime
//! orders rows within a second wrongly (#486).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::{Array, Object, RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Db, Kernel, Result};

/// One captured line, as far as the crawler reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub id: RecordId,
    pub session: Option<RecordId>,
    /// The working directory the line carries, when it carries one.
    pub cwd: Option<String>,
}

/// Where a page of lines starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// At this id, or the first after it.
    At(Uuid),
    /// After this id.
    After(Uuid),
}

/// The kernel's captured lines, a page at a time, in the order they were
/// captured: a line's id is a UUIDv7 the kernel set when it captured it. A
/// page is a seek on the table's own keys and at most `$limit` lines. No
/// datetime index is read: SurrealDB 3.1's misreads range bounds and drops
/// rows (#486). On a copy of the live database, a walk by capture time
/// through the agent index found 16 of the 112 working directories; this
/// one found all of them, every line, in 1.4 s.
const LINES_AT: &str = "SELECT id, session, raw.cwd AS cwd FROM type::record('message', $from..) LIMIT $limit";
const LINES_AFTER: &str = "SELECT id, session, raw.cwd AS cwd FROM type::record('message', $from>..) LIMIT $limit";

/// One page of the kernel's captured lines, from `cursor` on.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer.
pub async fn lines(kernel: &Kernel, cursor: Cursor, limit: u32) -> Result<Vec<Line>> {
    let (sql, from) = match cursor {
        Cursor::At(u) => (LINES_AT, u),
        Cursor::After(u) => (LINES_AFTER, u),
    };
    let rows: Vec<Value> = kernel.db().query(sql).bind(("from", from)).bind(("limit", limit)).await?.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Line {
                id: field(r, "id").and_then(record)?,
                session: field(r, "session").and_then(record),
                cwd: field(r, "cwd").and_then(string),
            })
        })
        .collect())
}

/// The smallest id a line captured at `at` can have: a UUIDv7 holding that
/// millisecond and nothing else.
#[must_use]
pub fn first_id_at(at: DateTime<Utc>) -> Uuid {
    let ms = u64::try_from(at.timestamp_millis()).unwrap_or(0);
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    Uuid::from(uuid::Uuid::from_bytes(bytes))
}

/// The first id there is: where the first pass starts.
#[must_use]
pub fn first_id() -> Uuid {
    Uuid::from(uuid::Uuid::nil())
}

/// A line's id as a place to page on from: its UUID, when its key is one.
#[must_use]
pub fn uuid_of(id: &RecordId) -> Option<Uuid> {
    match &id.key {
        RecordIdKey::Uuid(u) => Some(*u),
        _ => None,
    }
}

/// A checkout's state, as a `checkout_state` row says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutState {
    pub branch: Option<String>,
    pub first_branch: Option<String>,
    pub present: bool,
}

/// A directory, as a `directory` row says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryRow {
    /// `session` or `include`.
    pub source: &'static str,
    pub checkout: Option<RecordId>,
    pub exact: bool,
}

/// A checkout on the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub id: RecordId,
    pub repository: RecordId,
    /// Its repository's common git directory.
    pub common: String,
    pub toplevel: String,
}

/// The list as the module's own database holds it.
#[derive(Debug, Default)]
pub struct Held {
    /// Every repository, by its common git directory.
    pub repositories: HashMap<String, RecordId>,
    /// Every checkout, by its root.
    pub checkouts: HashMap<String, Checkout>,
    /// Each checkout's newest state.
    pub states: HashMap<RecordId, CheckoutState>,
    /// Each directory's newest row.
    pub directories: HashMap<String, DirectoryRow>,
    /// When the newest pass started.
    pub last_pass: Option<DateTime<Utc>>,
}

const HELD: &str = "SELECT id, common_dir FROM repository;\
     SELECT id, repository, repository.common_dir AS common, toplevel FROM checkout;\
     SELECT checkout, branch, first_branch, present, valid_from FROM checkout_state;\
     SELECT path, source, checkout, exact, valid_from FROM directory;\
     SELECT started_at, valid_from FROM sweep;";

/// Read the list as it stands.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the module's database cannot answer.
pub async fn held(db: &Db) -> Result<Held> {
    let mut res = db.query(HELD).await?;
    let repositories: Vec<Value> = res.take(0)?;
    let checkouts: Vec<Value> = res.take(1)?;
    let states: Vec<Value> = res.take(2)?;
    let directories: Vec<Value> = res.take(3)?;
    let sweeps: Vec<Value> = res.take(4)?;
    let mut held = Held::default();
    for r in &repositories {
        if let (Some(id), Some(common)) = (field(r, "id").and_then(record), field(r, "common_dir").and_then(string)) {
            held.repositories.insert(common, id);
        }
    }
    for r in &checkouts {
        if let (Some(id), Some(repository), Some(common), Some(toplevel)) = (
            field(r, "id").and_then(record),
            field(r, "repository").and_then(record),
            field(r, "common").and_then(string),
            field(r, "toplevel").and_then(string),
        ) {
            held.checkouts.insert(toplevel.clone(), Checkout { id, repository, common, toplevel });
        }
    }
    for (checkout, state, _) in newest(states.iter().filter_map(|r| {
        Some((
            field(r, "checkout").and_then(record)?,
            CheckoutState {
                branch: field(r, "branch").and_then(string),
                first_branch: field(r, "first_branch").and_then(string),
                present: field(r, "present").and_then(boolean)?,
            },
            field(r, "valid_from").and_then(datetime)?,
        ))
    })) {
        held.states.insert(checkout, state);
    }
    for (path, row, _) in newest(directories.iter().filter_map(|r| {
        Some((
            field(r, "path").and_then(string)?,
            DirectoryRow {
                source: match field(r, "source").and_then(string)?.as_str() {
                    "include" => "include",
                    _ => "session",
                },
                checkout: field(r, "checkout").and_then(record),
                exact: field(r, "exact").and_then(boolean)?,
            },
            field(r, "valid_from").and_then(datetime)?,
        ))
    })) {
        held.directories.insert(path, row);
    }
    held.last_pass = sweeps
        .iter()
        .filter_map(|r| Some((field(r, "valid_from").and_then(datetime)?, field(r, "started_at").and_then(datetime)?)))
        .max_by_key(|(at, _)| *at)
        .map(|(_, started)| started);
    Ok(held)
}

/// The newest of each key's rows, by `valid_from`.
fn newest<K: std::hash::Hash + Eq, V>(rows: impl Iterator<Item = (K, V, DateTime<Utc>)>) -> Vec<(K, V, DateTime<Utc>)> {
    let mut best: HashMap<K, (V, DateTime<Utc>)> = HashMap::new();
    for (k, v, at) in rows {
        match best.get(&k) {
            Some((_, seen)) if *seen >= at => {}
            _ => {
                best.insert(k, (v, at));
            }
        }
    }
    best.into_iter().map(|(k, (v, at))| (k, v, at)).collect()
}

/// A new UUIDv7 record id in this module's database.
fn new_id(table: &'static str) -> RecordId {
    RecordId::new(table, Uuid::new_v7())
}

/// Append one row.
async fn create(db: &Db, table: &'static str, row: Object) -> Result<RecordId> {
    let id = new_id(table);
    db.query("CREATE $id CONTENT $row").bind(("id", id.clone())).bind(("row", Value::Object(row))).await?.check()?;
    Ok(id)
}

fn time(at: DateTime<Utc>) -> Value {
    Value::Datetime(at.into())
}

fn count(n: u64) -> Value {
    Value::Number(i64::try_from(n).unwrap_or(i64::MAX).into())
}

/// A repository found for the first time.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for engine errors, a second row for one
/// common git directory among them.
pub async fn create_repository(db: &Db, common: &str, name: &str, at: DateTime<Utc>) -> Result<RecordId> {
    let mut row = Object::new();
    row.insert("common_dir".to_string(), Value::String(common.to_string()));
    row.insert("name".to_string(), Value::String(name.to_string()));
    row.insert("found_at".to_string(), time(at));
    row.insert("valid_from".to_string(), time(at));
    create(db, "repository", row).await
}

/// A checkout found for the first time.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for engine errors, a second row for one
/// root among them.
pub async fn create_checkout(db: &Db, repository: &RecordId, toplevel: &str, at: DateTime<Utc>) -> Result<RecordId> {
    let mut row = Object::new();
    row.insert("repository".to_string(), Value::RecordId(repository.clone()));
    row.insert("toplevel".to_string(), Value::String(toplevel.to_string()));
    row.insert("found_at".to_string(), time(at));
    row.insert("valid_from".to_string(), time(at));
    create(db, "checkout", row).await
}

/// A checkout's state, when it changed.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for engine errors.
pub async fn create_checkout_state(db: &Db, checkout: &RecordId, state: &CheckoutState, at: DateTime<Utc>) -> Result<()> {
    let mut row = Object::new();
    row.insert("checkout".to_string(), Value::RecordId(checkout.clone()));
    if let Some(b) = &state.branch {
        row.insert("branch".to_string(), Value::String(b.clone()));
    }
    if let Some(b) = &state.first_branch {
        row.insert("first_branch".to_string(), Value::String(b.clone()));
    }
    row.insert("present".to_string(), Value::Bool(state.present));
    row.insert("valid_from".to_string(), time(at));
    create(db, "checkout_state", row).await.map(|_| ())
}

/// A directory's row, when it changed.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for engine errors.
pub async fn create_directory(db: &Db, path: &str, dir: &DirectoryRow, at: DateTime<Utc>) -> Result<()> {
    let mut row = Object::new();
    row.insert("path".to_string(), Value::String(path.to_string()));
    row.insert("source".to_string(), Value::String(dir.source.to_string()));
    if let Some(c) = &dir.checkout {
        row.insert("checkout".to_string(), Value::RecordId(c.clone()));
    }
    row.insert("exact".to_string(), Value::Bool(dir.exact));
    row.insert("valid_from".to_string(), time(at));
    create(db, "directory", row).await.map(|_| ())
}

/// What one pass did, as its `sweep` row records it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub started_at: Option<DateTime<Utc>>,
    pub interval_secs: u64,
    pub sessions_read: u64,
    pub directories_read: u64,
    pub directories_new: u64,
    pub directories_moved: u64,
    pub repositories_known: u64,
    pub repositories_new: u64,
    pub repositories_stale: u64,
    pub checkouts_known: u64,
    pub checkouts_new: u64,
    pub checkouts_gone: u64,
    pub checkouts_back: u64,
    pub excluded: u64,
    pub crawls: Vec<RecordId>,
    pub crawls_complete: u64,
    pub crawls_partial: u64,
    pub crawls_unreadable: u64,
    pub commits_new: u64,
    pub rows_written: u64,
    pub git_calls: u64,
    pub git_failed: u64,
    pub git_timeouts: u64,
    /// `complete`, `partial` or `stopped`.
    pub outcome: &'static str,
    pub note: Option<String>,
}

/// Record a pass, once it ended.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for engine errors.
pub async fn create_sweep(db: &Db, sweep: &Sweep, finished_at: DateTime<Utc>) -> Result<RecordId> {
    let mut row = Object::new();
    row.insert("started_at".to_string(), time(sweep.started_at.unwrap_or(finished_at)));
    row.insert("finished_at".to_string(), time(finished_at));
    for (name, n) in [
        ("interval_secs", sweep.interval_secs),
        ("sessions_read", sweep.sessions_read),
        ("directories_read", sweep.directories_read),
        ("directories_new", sweep.directories_new),
        ("directories_moved", sweep.directories_moved),
        ("repositories_known", sweep.repositories_known),
        ("repositories_new", sweep.repositories_new),
        ("repositories_stale", sweep.repositories_stale),
        ("checkouts_known", sweep.checkouts_known),
        ("checkouts_new", sweep.checkouts_new),
        ("checkouts_gone", sweep.checkouts_gone),
        ("checkouts_back", sweep.checkouts_back),
        ("excluded", sweep.excluded),
        ("crawls_complete", sweep.crawls_complete),
        ("crawls_partial", sweep.crawls_partial),
        ("crawls_unreadable", sweep.crawls_unreadable),
        ("commits_new", sweep.commits_new),
        ("rows_written", sweep.rows_written),
        ("git_calls", sweep.git_calls),
        ("git_failed", sweep.git_failed),
        ("git_timeouts", sweep.git_timeouts),
    ] {
        row.insert(name.to_string(), count(n));
    }
    let crawls: Vec<Value> = sweep.crawls.iter().cloned().map(Value::RecordId).collect();
    row.insert("crawls".to_string(), Value::Array(Array::from(crawls)));
    row.insert("outcome".to_string(), Value::String(sweep.outcome.to_string()));
    if let Some(note) = &sweep.note {
        row.insert("note".to_string(), Value::String(note.clone()));
    }
    row.insert("valid_from".to_string(), time(finished_at));
    create(db, "sweep", row).await
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key),
        _ => None,
    }
}

fn string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn record(v: &Value) -> Option<RecordId> {
    match v {
        Value::RecordId(r) => Some(r.clone()),
        _ => None,
    }
}

fn boolean(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn datetime(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_id_of_a_moment_is_below_every_line_captured_then_or_later() {
        let at = DateTime::parse_from_rfc3339("2026-10-24T15:00:00.123456Z").expect("time").to_utc();
        let floor = first_id_at(at);
        let floor = uuid::Uuid::from(floor);
        assert_eq!(floor.get_version_num(), 7);
        let ms = u64::try_from(at.timestamp_millis()).expect("ms");
        let same = uuid::Uuid::new_v7(uuid::Timestamp::from_unix(uuid::NoContext, ms / 1000, ((ms % 1000) * 1_000_000) as u32));
        let later = uuid::Uuid::new_v7(uuid::Timestamp::from_unix(uuid::NoContext, ms / 1000 + 1, 0));
        let earlier = uuid::Uuid::new_v7(uuid::Timestamp::from_unix(uuid::NoContext, ms / 1000 - 1, 0));
        assert!(floor <= same && floor < later && earlier < floor, "{earlier} < {floor} <= {same} < {later}");
        assert!(uuid::Uuid::from(first_id()) < earlier);
    }

    #[test]
    fn a_line_pages_on_from_its_uuid_and_never_from_another_key() {
        let u = Uuid::new_v7();
        assert_eq!(uuid_of(&RecordId::new("message", u)), Some(u));
        assert_eq!(uuid_of(&RecordId::new("message", "text")), None);
    }
}
