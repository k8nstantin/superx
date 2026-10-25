//! `superx-performance status`, and the module's own URL: whether its
//! database is there, what it holds, the crawler's newest pass and what
//! each of its sources did in it, and the parameters it runs on.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;
use superx_kernel::{Db, Kernel, Result};

use crate::params::{self, Resolved};
use crate::MODULE_NAME;

/// The tables of facts, as `status` lists them: what happened in the
/// sessions, what the models answered, what their tools did, and what the
/// repositories hold; each table with what its rows are called there. The
/// crawler's own record (`pass`, `pass_source`) is said apart.
pub const FACTS: &[(&str, &[(&str, &str)])] = &[
    (
        "sessions",
        &[
            ("session", "sessions"),
            ("session_place", "places"),
            ("prompt", "prompts"),
            ("turn", "turns"),
            ("compaction", "compactions"),
            ("away", "recaps"),
        ],
    ),
    ("models", &[("reply", "replies"), ("reply_state", "reply states"), ("thinking", "thinking spans")]),
    (
        "tools",
        &[
            ("tool_call", "tool calls"),
            ("tool_result", "tool results"),
            ("edit", "edits"),
            ("git_event", "git events"),
            ("test_run", "test runs"),
            ("build_run", "build runs"),
        ],
    ),
    (
        "repos",
        &[
            ("repository", "repositories"),
            ("checkout", "checkouts"),
            ("checkout_state", "checkout states"),
            ("directory", "directory records"),
            ("head_move", "head moves"),
            ("commit", "commits"),
            ("commit_file", "commit files"),
            ("placement", "placements"),
            ("landed_via", "landings"),
            ("survival", "survival records"),
            ("survival_file", "survival by file"),
            ("ref_state", "ref states"),
            ("worktree_file", "uncommitted files"),
            ("crawl", "crawls"),
            ("crawl_step", "crawl steps"),
            ("git_call", "git calls"),
        ],
    ),
];

/// Every table of facts, in the order `status` lists them.
pub fn fact_tables() -> impl Iterator<Item = &'static str> {
    FACTS.iter().flat_map(|(_, tables)| tables.iter().map(|(table, _)| *table))
}

/// What the module's own database holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Holdings {
    /// Rows in each table of facts, in the order of [`fact_tables`].
    pub rows: Vec<(&'static str, i64)>,
    /// The crawler's newest pass.
    pub last_pass: Option<LastPass>,
}

/// The crawler's newest pass, as `status` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastPass {
    pub finished_at: DateTime<Utc>,
    pub outcome: String,
    pub rows_written: i64,
    pub sources: Vec<LastSource>,
}

/// What one source did in the newest pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastSource {
    pub source: String,
    pub items: i64,
    pub unread: i64,
    pub calls: i64,
    pub failed: i64,
    pub timed_out: i64,
    pub rows_written: i64,
    pub outcome: String,
    pub note: Option<String>,
}

/// One count per table of facts, then the newest pass and its sources. A
/// count is NONE for an empty table: `GROUP ALL` over no rows returns no
/// row. Passes are seconds apart, so the index's misorder within a second
/// (#486) cannot pick the wrong one.
fn holdings_query() -> String {
    let mut sql: String =
        fact_tables().map(|table| format!("RETURN (SELECT count() AS n FROM {table} GROUP ALL)[0].n;")).collect();
    sql.push_str(
        "LET $last = (SELECT id, finished_at, outcome, rows_written, valid_from FROM pass \
         ORDER BY valid_from DESC LIMIT 1)[0];\
         RETURN $last;\
         SELECT source, items, unread, calls, failed, timed_out, rows, outcome, note FROM pass_source \
         WHERE pass = $last.id ORDER BY source;",
    );
    sql
}

/// Read what the module's own database holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn holdings(db: &Db) -> Result<Holdings> {
    let mut res = db.query(holdings_query()).await?;
    let mut rows = Vec::new();
    for (i, table) in fact_tables().enumerate() {
        let count: Option<i64> = res.take(i)?;
        rows.push((table, count.unwrap_or_default()));
    }
    let last: Option<Value> = res.take(rows.len() + 1)?;
    let sources: Vec<Value> = res.take(rows.len() + 2)?;
    Ok(Holdings { rows, last_pass: last.as_ref().and_then(|p| last_pass(p, &sources)) })
}

fn last_pass(row: &Value, sources: &[Value]) -> Option<LastPass> {
    Some(LastPass {
        finished_at: match field(row, "finished_at") {
            Some(Value::Datetime(d)) => DateTime::<Utc>::from(*d),
            _ => return None,
        },
        outcome: text(row, "outcome")?,
        rows_written: int(row, "rows_written")?,
        sources: sources
            .iter()
            .filter_map(|s| {
                Some(LastSource {
                    source: text(s, "source")?,
                    items: int(s, "items")?,
                    unread: int(s, "unread")?,
                    calls: int(s, "calls")?,
                    failed: int(s, "failed")?,
                    timed_out: int(s, "timed_out")?,
                    rows_written: match field(s, "rows") {
                        Some(Value::Object(o)) => {
                            o.values().filter_map(|v| if let Value::Number(n) = v { n.to_int() } else { None }).sum()
                        }
                        _ => 0,
                    },
                    outcome: text(s, "outcome")?,
                    note: text(s, "note"),
                })
            })
            .collect(),
    })
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key),
        _ => None,
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    match field(v, key)? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn int(v: &Value, key: &str) -> Option<i64> {
    match field(v, key)? {
        Value::Number(n) => n.to_int(),
        _ => None,
    }
}

/// The module's own database, as `status` found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Database {
    Ready(Holdings),
    /// Not provisioned, or not reachable: the kernel's own words.
    Unavailable(String),
}

/// The status, as `superx-performance status` prints it and its URL
/// answers.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer
/// for the parameters, or the module's own database for its contents.
pub async fn render(kernel: &Kernel) -> Result<String> {
    let resolved = params::resolve(kernel).await?;
    let database = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => Database::Ready(holdings(&db).await?),
        Err(e) => Database::Unavailable(own_hint(&e.to_string())),
    };
    Ok(text_of(&database, &resolved))
}

/// The kernel names `superx modules provision <name>` for a module database
/// it cannot sign in to: a command that knows only the modules linked into
/// `superx`. This module provisions its own.
pub(crate) fn own_hint(kernel_words: &str) -> String {
    let words = kernel_words.split(" — run `superx modules provision").next().unwrap_or(kernel_words);
    format!("{words}. Provision it with `superx-performance --initialize`")
}

/// The status text for what was found.
#[must_use]
pub fn text_of(database: &Database, resolved: &Resolved) -> String {
    let mut out = String::from(
        "superx-performance: the models, the agents, thinking effort and the operator, measured from every captured \
         line and every repository the agents worked in\n",
    );
    match database {
        Database::Ready(h) => {
            let _ = writeln!(out, "database    superx/{MODULE_NAME}");
            let width = FACTS.iter().map(|(group, _)| group.len()).max().unwrap_or(0);
            for (group, tables) in FACTS {
                let counts: Vec<String> = tables
                    .iter()
                    .map(|(table, called)| {
                        let n = h.rows.iter().find(|(t, _)| t == table).map_or(0, |(_, n)| *n);
                        format!("{n} {called}")
                    })
                    .collect();
                let _ = writeln!(out, "  {group:<width$}  {}", counts.join(" · "));
            }
            match &h.last_pass {
                Some(p) => {
                    let _ = writeln!(
                        out,
                        "last pass   {} ({}): {} rows written",
                        p.finished_at.to_rfc3339(),
                        p.outcome,
                        p.rows_written
                    );
                    for s in &p.sources {
                        let _ = writeln!(out, "  {:<width$}  {}", s.source, source_line(s));
                    }
                }
                None => {
                    let _ = writeln!(out, "last pass   none yet");
                }
            }
        }
        Database::Unavailable(why) => {
            let _ = writeln!(out, "database    superx/{MODULE_NAME}: not available. {why}");
        }
    }
    let _ = writeln!(out, "parameters  set with `superx param set {MODULE_NAME} <name> <json>`");
    let width = resolved.settings.iter().map(|s| s.name.len()).max().unwrap_or(0);
    for s in &resolved.settings {
        let origin = if s.set { "set" } else { "fallback" };
        let _ = writeln!(out, "  {:<width$}  {:<8}  {}", s.name, origin, s.value);
    }
    if !resolved.problems.is_empty() {
        let _ = writeln!(out, "problems");
        for p in &resolved.problems {
            let running = resolved.settings.iter().find(|s| s.name == p.name).map_or("", |s| s.value.as_str());
            let _ = writeln!(out, "  {} is {}, not {}: running on {running}", p.name, p.found, p.wanted);
        }
    }
    out
}

/// What one source did, in a line: what it worked through, its outside
/// calls when it made any, the rows it added, and how it ended when it did
/// not end well.
fn source_line(s: &LastSource) -> String {
    let mut parts = vec![format!("{} read, {} in a shape it does not know", s.items, s.unread)];
    if s.calls > 0 {
        parts.push(format!("{} calls, {} failed, {} cut off", s.calls, s.failed, s.timed_out));
    }
    parts.push(format!("{} rows", s.rows_written));
    let mut line = parts.join(" · ");
    if s.outcome != "complete" {
        let _ = write!(line, " ({}{})", s.outcome, s.note.as_ref().map_or(String::new(), |n| format!(": {n}")));
    }
    line
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::params::{from_values, PORT_PARAM};

    #[test]
    fn every_table_of_facts_is_counted_once_and_the_crawlers_own_record_apart() {
        let tables: Vec<&str> = fact_tables().collect();
        let unique: std::collections::BTreeSet<&str> = tables.iter().copied().collect();
        assert_eq!(tables.len(), 31, "{tables:?}");
        assert_eq!(unique.len(), tables.len(), "a table counted twice: {tables:?}");
        assert!(!unique.contains("pass") && !unique.contains("pass_source"));
    }

    #[test]
    fn a_fresh_database_says_it_holds_nothing_and_lists_every_parameter() {
        let t = text_of(&Database::Ready(Holdings::default()), &from_values(&BTreeMap::new()));
        assert!(
            t.contains(
                "database    superx/performance\n\
                 \x20 sessions  0 sessions · 0 places · 0 prompts · 0 turns · 0 compactions · 0 recaps\n\
                 \x20 models    0 replies · 0 reply states · 0 thinking spans\n\
                 \x20 tools     0 tool calls · 0 tool results · 0 edits · 0 git events · 0 test runs · 0 build runs\n\
                 \x20 repos     0 repositories · 0 checkouts · 0 checkout states"
            ),
            "{t}"
        );
        assert!(t.contains("last pass   none yet\n"), "{t}");
        for name in crate::params::NAMES {
            assert!(t.contains(name), "{name} is listed:\n{t}");
        }
        assert!(!t.contains("problems"), "{t}");
    }

    #[test]
    fn every_count_sits_beside_its_own_table() {
        let rows = fact_tables().zip(1..).collect();
        let t = text_of(&Database::Ready(Holdings { rows, last_pass: None }), &from_values(&BTreeMap::new()));
        assert!(t.contains("1 sessions · 2 places · 3 prompts · 4 turns · 5 compactions · 6 recaps"), "{t}");
        assert!(t.contains("7 replies · 8 reply states · 9 thinking spans"), "{t}");
        assert!(t.contains("10 tool calls · 11 tool results · 12 edits · 13 git events · 14 test runs · 15 build runs"), "{t}");
        assert!(t.contains("16 repositories · 17 checkouts · 18 checkout states · 19 directory records"), "{t}");
        assert!(t.contains("29 crawls · 30 crawl steps · 31 git calls"), "{t}");
    }

    #[test]
    fn the_last_pass_says_when_how_it_ended_and_what_each_source_did() {
        let source = |name: &str, items, unread, calls, failed, rows, outcome: &str, note: Option<&str>| LastSource {
            source: name.to_string(),
            items,
            unread,
            calls,
            failed,
            timed_out: 0,
            rows_written: rows,
            outcome: outcome.to_string(),
            note: note.map(str::to_string),
        };
        let h = Holdings {
            last_pass: Some(LastPass {
                finished_at: DateTime::parse_from_rfc3339("2026-10-25T15:00:00Z").expect("time").to_utc(),
                outcome: "partial".to_string(),
                rows_written: 1_204,
                sources: vec![
                    source("lines", 500, 3, 0, 0, 1_190, "complete", None),
                    source("repos", 125, 0, 149, 1, 14, "partial", Some("1 git calls cut off")),
                ],
            }),
            ..Holdings::default()
        };
        let t = text_of(&Database::Ready(h), &from_values(&BTreeMap::new()));
        assert!(t.contains("last pass   2026-10-25T15:00:00+00:00 (partial): 1204 rows written\n"), "{t}");
        assert!(t.contains("  lines     500 read, 3 in a shape it does not know · 1190 rows\n"), "{t}");
        assert!(
            t.contains(
                "  repos     125 read, 0 in a shape it does not know · 149 calls, 1 failed, 0 cut off · 14 rows \
                 (partial: 1 git calls cut off)\n"
            ),
            "{t}"
        );
    }

    #[test]
    fn an_unprovisioned_database_names_this_modules_own_command() {
        let kernel = "configuration error: module 'performance' db signin refused (There was a problem with \
                      authentication) — run `superx modules provision performance` first";
        assert_eq!(
            own_hint(kernel),
            "configuration error: module 'performance' db signin refused (There was a problem with authentication). \
             Provision it with `superx-performance --initialize`"
        );
        assert_eq!(own_hint("cannot connect"), "cannot connect. Provision it with `superx-performance --initialize`");
    }

    #[test]
    fn an_unprovisioned_database_says_why_and_a_wrong_value_says_what_runs_instead() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, Value::String("five".to_string()));
        let why = "module 'performance' db signin refused. Provision it with `superx-performance --initialize`";
        let t = text_of(&Database::Unavailable(why.to_string()), &from_values(&values));
        assert!(t.contains(&format!("superx/performance: not available. {why}")), "{t}");
        assert!(t.contains("attr_performance_port is \"five\", not a port, 1 to 65535: running on 5153"), "{t}");
    }
}
