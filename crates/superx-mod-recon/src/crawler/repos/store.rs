//! Every query the `repos` source issues against the module's own
//! database for its list, in one place: the list as it stands, the working
//! directories the sessions worked in, and the git events the lines showed
//! (#538); a crawl reads what it needs itself. The rows it writes go through
//! [`crate::crawler::store`]. Of several rows for one thing, the newest is
//! picked here by `valid_from`, never by an index's order (#486).

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Uuid, Value};
use superx_kernel::{Db, Result};

use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::repos::crawl::Event;
use crate::crawler::repos::operations::Operations;
use crate::crawler::store::Row;
use crate::crawler::value::{at, flag, list, text};

/// A checkout's state, as a `checkout_state` row says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutState {
    pub branch: Option<String>,
    pub first_branch: Option<String>,
    pub present: bool,
    /// What git is in the middle of there (#538, step 12); none when not
    /// read, as for a checkout that is gone.
    pub operations: Option<Operations>,
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
    /// Each checkout's newest state, by the checkout row's UUID: a map
    /// keyed by a record id is one clippy refuses (`mutable_key_type`).
    pub states: HashMap<Uuid, CheckoutState>,
    /// Each directory's newest row.
    pub directories: HashMap<String, DirectoryRow>,
}

const HELD: &str = "SELECT id, common_dir FROM repository;\
     SELECT id, repository, repository.common_dir AS common, toplevel FROM checkout;\
     SELECT checkout, branch, first_branch, present, operations, operation_commits, rebase_branch, rebase_onto, rebase_done, \
     rebase_left, bisect_start, valid_from FROM checkout_state;\
     SELECT path, source, checkout, exact, valid_from FROM directory;";

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
    let mut held = Held::default();
    for r in &repositories {
        if let (Some(id), Some(common)) = (record(r, "id"), text(r, &["common_dir"])) {
            held.repositories.insert(common.to_string(), id);
        }
    }
    for r in &checkouts {
        if let (Some(id), Some(repository), Some(common), Some(toplevel)) =
            (record(r, "id"), record(r, "repository"), text(r, &["common"]), text(r, &["toplevel"]))
        {
            held.checkouts.insert(
                toplevel.to_string(),
                Checkout { id, repository, common: common.to_string(), toplevel: toplevel.to_string() },
            );
        }
    }
    for (checkout, state) in newest(states.iter().filter_map(|r| {
        Some((
            record(r, "checkout").as_ref().and_then(record_uuid_of)?,
            CheckoutState {
                branch: text(r, &["branch"]).map(str::to_string),
                first_branch: text(r, &["first_branch"]).map(str::to_string),
                present: flag(r, &["present"])?,
                operations: operations_of(r),
            },
            time(r, "valid_from")?,
        ))
    })) {
        held.states.insert(checkout, state);
    }
    for (path, row) in newest(directories.iter().filter_map(|r| {
        Some((
            text(r, &["path"])?.to_string(),
            DirectoryRow {
                source: if text(r, &["source"])? == "include" { "include" } else { "session" },
                checkout: record(r, "checkout"),
                exact: flag(r, &["exact"])?,
            },
            time(r, "valid_from")?,
        ))
    })) {
        held.directories.insert(path, row);
    }
    Ok(held)
}

/// Every working directory a session worked in, as the `lines` source
/// wrote them.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the module's database cannot answer.
pub async fn worked_in(db: &Db) -> Result<BTreeSet<String>> {
    let rows: Vec<Value> = db.query("SELECT VALUE cwd FROM session_place").await?.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|v| match v {
            Value::String(s) if !s.is_empty() => Some(s),
            _ => None,
        })
        .collect())
}

/// The newest of each key's rows, by `valid_from`.
fn newest<K: std::hash::Hash + Eq, V>(rows: impl Iterator<Item = (K, V, DateTime<Utc>)>) -> Vec<(K, V)> {
    let mut best: HashMap<K, (V, DateTime<Utc>)> = HashMap::new();
    for (k, v, valid_from) in rows {
        match best.get(&k) {
            Some((_, seen)) if *seen >= valid_from => {}
            _ => {
                best.insert(k, (v, valid_from));
            }
        }
    }
    best.into_iter().map(|(k, (v, _))| (k, v)).collect()
}

/// A repository found for the first time.
#[must_use]
pub fn repository(common: &str, name: &str, at_time: DateTime<Utc>) -> Row {
    Row::new().text("common_dir", common).text("name", name).time("found_at", at_time)
}

/// A checkout found for the first time.
#[must_use]
pub fn checkout(repository: &RecordId, toplevel: &str, at_time: DateTime<Utc>) -> Row {
    Row::new().link("repository", repository).text("toplevel", toplevel).time("found_at", at_time)
}

/// A checkout's state, when it changed.
#[must_use]
pub fn checkout_state(checkout: &RecordId, state: &CheckoutState) -> Row {
    let row = Row::new()
        .link("checkout", checkout)
        .some_text("branch", state.branch.clone())
        .some_text("first_branch", state.first_branch.clone())
        .flag("present", state.present);
    match &state.operations {
        None => row,
        Some(o) => {
            let row = row.texts("operations", &o.operations);
            let row = match &o.operation_commits {
                Some(c) => row.texts("operation_commits", c),
                None => row,
            };
            row.some_text("rebase_branch", o.rebase_branch.clone())
                .some_text("rebase_onto", o.rebase_onto.clone())
                .some_int("rebase_done", o.rebase_done)
                .some_int("rebase_left", o.rebase_left)
                .some_text("bisect_start", o.bisect_start.clone())
        }
    }
}

/// The operations a checkout_state row holds; none when it holds none.
fn operations_of(r: &Value) -> Option<Operations> {
    let strings = |key: &str| -> Vec<String> {
        list(r, &[key]).iter().filter_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        }).collect()
    };
    let number = |key: &str| match at(r, &[key]) {
        Some(Value::Number(n)) => n.to_int(),
        _ => None,
    };
    match at(r, &["operations"]) {
        Some(Value::Array(_)) => Some(Operations {
            operations: strings("operations"),
            operation_commits: matches!(at(r, &["operation_commits"]), Some(Value::Array(_))).then(|| strings("operation_commits")),
            rebase_branch: text(r, &["rebase_branch"]).map(str::to_string),
            rebase_onto: text(r, &["rebase_onto"]).map(str::to_string),
            rebase_done: number("rebase_done"),
            rebase_left: number("rebase_left"),
            bisect_start: text(r, &["bisect_start"]).map(str::to_string),
        }),
        _ => None,
    }
}

/// The git actions that send the crawler to a repository (step 1), which
/// hold the ones that tie a commit to a session (step 13).
pub const ACTIONS: [&str; 10] =
    ["commit", "push", "merge", "rebase", "revert", "cherry_pick", "reset", "branch_deleted", "pr_created", "pr_merged"];

/// Every git event the lines source wrote of those actions, with the
/// session of its tool call's reply.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the module's database cannot answer.
pub async fn events(db: &Db) -> Result<Vec<Event>> {
    let rows: Vec<Value> = db
        .query(
            "SELECT action, hash, dir, url, pr_number, at, tool_call, tool_call.reply.session AS session, valid_from \
             FROM git_event WHERE action INSIDE $actions",
        )
        .bind(("actions", ACTIONS.to_vec()))
        .await?
        .take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Event {
                action: text(r, &["action"])?.to_string(),
                hash: text(r, &["hash"]).map(str::to_string),
                dir: text(r, &["dir"]).map(str::to_string),
                url: text(r, &["url"]).map(str::to_string),
                pr_number: match at(r, &["pr_number"]) {
                    Some(Value::Number(n)) => n.to_int(),
                    _ => None,
                },
                at: time(r, "at")?,
                tool_call: record(r, "tool_call")?,
                session: record(r, "session"),
                valid_from: time(r, "valid_from")?,
            })
        })
        .collect())
}

/// A directory's row, when it changed.
#[must_use]
pub fn directory(path: &str, dir: &DirectoryRow) -> Row {
    let row = Row::new().text("path", path).text("source", dir.source).flag("exact", dir.exact);
    match &dir.checkout {
        Some(c) => row.link("checkout", c),
        None => row,
    }
}

fn record(v: &Value, key: &str) -> Option<RecordId> {
    match at(v, &[key])? {
        Value::RecordId(id) => Some(id.clone()),
        _ => None,
    }
}

fn time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match at(v, &[key])? {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}
