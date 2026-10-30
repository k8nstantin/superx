//! `superx-recon status`, and the module's own URL: whether its
//! database is there, what it holds, what the credentials file holds by
//! host and where the repos folder is, the crawler's newest pass and what
//! each of its sources did in it, and the parameters it runs on.

use std::fmt::Write as _;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;
use superx_kernel::{Db, Kernel, Result};

use crate::credentials::{self, Credentials, BITBUCKET, GITHUB};
use crate::instance::Instance;
use crate::params::{self, Resolved};
use crate::MODULE_NAME;

/// The tables of facts, as `status` lists them: what happened in the
/// sessions, what the models answered, what their tools did, what the
/// repositories and their hosts hold, and every field the crawler has seen;
/// each table with what its rows are called there. The crawler's own record
/// (`pass`, `pass_source`) is said apart.
pub const FACTS: &[(&str, &[(&str, &str)])] = &[
    (
        "sessions",
        &[
            ("session", "sessions"),
            ("session_place", "places"),
            ("prompt", "prompts"),
            ("trigger", "triggers"),
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
            ("commit", "commits"),
            ("commit_file", "commit files"),
            ("placement", "placements"),
            ("landed_via", "landings"),
            ("survival", "survival records"),
            ("crawl", "crawls"),
            ("git_call", "git calls"),
            ("remote_state", "remote states"),
            ("ref_move", "reflog entries"),
            ("commit_session", "commits tied to sessions"),
        ],
    ),
    (
        "hosts",
        &[
            ("host_repository", "repositories on hosts"),
            ("host_user", "host accounts"),
            ("host_crawl", "host crawls"),
            ("host_call", "host calls"),
            ("host_object", "host objects"),
            ("mirror", "mirrors"),
            ("pull_request", "pull requests"),
            ("pull_request_state", "pull request states"),
            ("pull_request_commit", "pull request commits"),
            ("ci_run", "CI runs"),
            ("ci_run_state", "CI run states"),
        ],
    ),
    ("census", &[("line_field", "fields")]),
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

/// The module folder, as `status` shows it: the credentials file and what
/// it holds, and the repos folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub credentials_path: PathBuf,
    pub credentials: Credentials,
    pub repos: PathBuf,
}

impl Folder {
    /// The instance's module folder, its credentials file read now.
    #[must_use]
    pub fn read(instance: &Instance) -> Self {
        let credentials_path = instance.credentials_path();
        Self { credentials: credentials::read(&credentials_path), credentials_path, repos: instance.repos_dir() }
    }
}

/// The status, as `superx-recon status` prints it and its URL
/// answers.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer
/// for the parameters, or the module's own database for its contents.
pub async fn render(kernel: &Kernel, instance: &Instance) -> Result<String> {
    let resolved = params::resolve(kernel).await?;
    let database = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => Database::Ready(holdings(&db).await?),
        Err(e) => Database::Unavailable(own_hint(&e.to_string())),
    };
    Ok(text_of(&database, &Folder::read(instance), &resolved))
}

/// The kernel names `superx modules provision <name>` for a module database
/// it cannot sign in to: a command that knows only the modules linked into
/// `superx`. This module provisions its own.
pub(crate) fn own_hint(kernel_words: &str) -> String {
    let words = kernel_words.split(" — run `superx modules provision").next().unwrap_or(kernel_words);
    format!("{words}. Provision it with `superx-recon --initialize`")
}

/// The status text for what was found.
#[must_use]
pub fn text_of(database: &Database, folder: &Folder, resolved: &Resolved) -> String {
    let mut out = String::from(
        "superx-recon: the models, the agents, thinking effort and the operator, measured from every captured \
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
            folder_lines(&mut out, folder);
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
            folder_lines(&mut out, folder);
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

/// The credentials line and the repos line.
fn folder_lines(out: &mut String, folder: &Folder) {
    let path = folder.credentials_path.display();
    let held = match &folder.credentials {
        Credentials::None => "none".to_string(),
        Credentials::Refused { mode } => format!("refused, mode {mode:03o}; chmod 600 {path}"),
        Credentials::Unreadable(why) => format!("unreadable: {why}"),
        Credentials::Hosts(hosts) => {
            let token = |host: &str, set: bool| format!("{host} {}", if set { "token set" } else { "no token" });
            let mut said = vec![token(GITHUB, hosts.github.is_some()), token(BITBUCKET, hosts.bitbucket.is_some())];
            said.extend(hosts.unknown.iter().map(|host| format!("{host} unknown")));
            said.join(" · ")
        }
    };
    let _ = writeln!(out, "credentials {path}: {held}");
    let _ = writeln!(out, "repos       {}", folder.repos.display());
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
    use crate::credentials::Hosts;
    use crate::params::{from_values, PORT_PARAM};

    /// A module folder with no credentials file.
    fn folder() -> Folder {
        folder_with(Credentials::None)
    }

    fn folder_with(credentials: Credentials) -> Folder {
        Folder {
            credentials_path: PathBuf::from("/home/op/modules/recon/credentials.json"),
            credentials,
            repos: PathBuf::from("/home/op/modules/recon/repos"),
        }
    }

    /// Every table of facts is counted once, and the crawler's own record
    /// is said apart. That these are exactly the tables the schema defines
    /// is read from the engine in `tests/module.rs`.
    #[test]
    fn every_table_of_facts_is_counted_once_and_the_crawlers_own_record_apart() {
        let tables: Vec<&str> = fact_tables().collect();
        let unique: std::collections::BTreeSet<&str> = tables.iter().copied().collect();
        assert_eq!(tables.len(), 42, "{tables:?}");
        assert_eq!(unique.len(), tables.len(), "a table counted twice: {tables:?}");
        assert!(!unique.contains("pass") && !unique.contains("pass_source"));
    }

    #[test]
    fn every_group_lists_the_tables_step_7_gives_it_in_their_order_with_their_names_and_none_the_structure_lacks() {
        let want: &[(&str, &[(&str, &str)])] = &[
            (
                "sessions",
                &[
                    ("session", "sessions"),
                    ("session_place", "places"),
                    ("prompt", "prompts"),
                    ("trigger", "triggers"),
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
                    ("commit", "commits"),
                    ("commit_file", "commit files"),
                    ("placement", "placements"),
                    ("landed_via", "landings"),
                    ("survival", "survival records"),
                    ("crawl", "crawls"),
                    ("git_call", "git calls"),
                    ("remote_state", "remote states"),
                    ("ref_move", "reflog entries"),
                    ("commit_session", "commits tied to sessions"),
                ],
            ),
            (
                "hosts",
                &[
                    ("host_repository", "repositories on hosts"),
                    ("host_user", "host accounts"),
                    ("host_crawl", "host crawls"),
                    ("host_call", "host calls"),
                    ("host_object", "host objects"),
                    ("mirror", "mirrors"),
                    ("pull_request", "pull requests"),
                    ("pull_request_state", "pull request states"),
                    ("pull_request_commit", "pull request commits"),
                    ("ci_run", "CI runs"),
                    ("ci_run_state", "CI run states"),
                ],
            ),
            ("census", &[("line_field", "fields")]),
        ];
        assert_eq!(FACTS, want);
        // Each one a table the structure gives fields to.
        for table in fact_tables() {
            assert!(crate::SCHEMA_DDL.contains(&format!(" ON TABLE {table} ")), "the structure has no table {table}");
        }
    }

    #[test]
    fn a_fresh_database_says_it_holds_nothing_and_lists_every_parameter() {
        let t = text_of(&Database::Ready(Holdings::default()), &folder(), &from_values(&BTreeMap::new()));
        assert!(
            t.contains(
                "database    superx/recon\n\
                 \x20 sessions  0 sessions · 0 places · 0 prompts · 0 triggers · 0 turns · 0 compactions · 0 recaps\n\
                 \x20 models    0 replies · 0 reply states · 0 thinking spans\n\
                 \x20 tools     0 tool calls · 0 tool results · 0 edits · 0 git events · 0 test runs · 0 build runs\n\
                 \x20 repos     0 repositories · 0 checkouts · 0 checkout states"
            ),
            "{t}"
        );
        assert!(
            t.contains(
                "  hosts     0 repositories on hosts · 0 host accounts · 0 host crawls · 0 host calls · 0 host objects · \
                 0 mirrors · 0 pull requests · 0 pull request states · 0 pull request commits · 0 CI runs · \
                 0 CI run states\n\
                 \x20 census    0 fields\n"
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
        let t = text_of(&Database::Ready(Holdings { rows, last_pass: None }), &folder(), &from_values(&BTreeMap::new()));
        assert!(t.contains("1 sessions · 2 places · 3 prompts · 4 triggers · 5 turns · 6 compactions · 7 recaps\n"), "{t}");
        assert!(t.contains("8 replies · 9 reply states · 10 thinking spans\n"), "{t}");
        assert!(t.contains("11 tool calls · 12 tool results · 13 edits · 14 git events · 15 test runs · 16 build runs\n"), "{t}");
        assert!(t.contains("17 repositories · 18 checkouts · 19 checkout states · 20 directory records · 21 commits"), "{t}");
        assert!(t.contains("26 crawls · 27 git calls · 28 remote states · 29 reflog entries · 30 commits tied to sessions\n"), "{t}");
        assert!(t.contains("31 repositories on hosts · 32 host accounts · 33 host crawls · 34 host calls · 35 host objects"), "{t}");
        assert!(t.contains("40 CI runs · 41 CI run states\n"), "{t}");
        assert!(t.contains("  census    42 fields\n"), "{t}");
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
        let t = text_of(&Database::Ready(h), &folder(), &from_values(&BTreeMap::new()));
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
        let kernel = "configuration error: module 'recon' db signin refused (There was a problem with \
                      authentication) — run `superx modules provision recon` first";
        assert_eq!(
            own_hint(kernel),
            "configuration error: module 'recon' db signin refused (There was a problem with authentication). \
             Provision it with `superx-recon --initialize`"
        );
        assert_eq!(own_hint("cannot connect"), "cannot connect. Provision it with `superx-recon --initialize`");
    }

    #[test]
    fn an_unprovisioned_database_says_why_and_a_wrong_value_says_what_runs_instead() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, Value::String("five".to_string()));
        let why = "module 'recon' db signin refused. Provision it with `superx-recon --initialize`";
        let t = text_of(&Database::Unavailable(why.to_string()), &folder(), &from_values(&values));
        assert!(t.contains(&format!("superx/recon: not available. {why}")), "{t}");
        assert!(t.contains("attr_recon_port is \"five\", not a port, 1 to 65535: running on 5153"), "{t}");
    }

    #[test]
    fn status_first_line_names_superx_recon_and_an_unprovisioned_hint_names_superx_recon_initialize() {
        let resolved = from_values(&BTreeMap::new());
        for database in [Database::Ready(Holdings::default()), Database::Unavailable(own_hint("cannot connect"))] {
            let t = text_of(&database, &folder(), &resolved);
            assert!(t.lines().next().is_some_and(|first| first.starts_with("superx-recon: ")), "{t}");
        }
        assert!(own_hint("signin refused").ends_with("Provision it with `superx-recon --initialize`"));
    }

    #[test]
    fn the_credentials_line_names_each_host_in_order_or_none_refused_unreadable_and_the_repos_line_follows() {
        let hosts = |github: Option<&str>, bitbucket: Option<&str>, unknown: &[&str]| {
            Credentials::Hosts(Hosts {
                github: github.map(credentials::token),
                bitbucket: bitbucket.map(credentials::token),
                unknown: unknown.iter().map(|u| (*u).to_string()).collect(),
            })
        };
        let path = "/home/op/modules/recon/credentials.json";
        for (credentials, said) in [
            (Credentials::None, "none".to_string()),
            (Credentials::Refused { mode: 0o644 }, format!("refused, mode 644; chmod 600 {path}")),
            (Credentials::Refused { mode: 0o004 }, format!("refused, mode 004; chmod 600 {path}")),
            (Credentials::Unreadable("expected value at line 1 column 1".to_string()), "unreadable: expected value at line 1 column 1".to_string()),
            (hosts(None, None, &[]), "github.com no token · bitbucket.org no token".to_string()),
            (hosts(Some("a"), None, &[]), "github.com token set · bitbucket.org no token".to_string()),
            (
                hosts(None, Some("b"), &["gitlab.example", "codeberg.example"]),
                "github.com no token · bitbucket.org token set · gitlab.example unknown · codeberg.example unknown".to_string(),
            ),
        ] {
            let want = format!("credentials {path}: {said}\nrepos       /home/op/modules/recon/repos\n");
            let resolved = from_values(&BTreeMap::new());
            // After the groups and before the last pass.
            let ready = text_of(&Database::Ready(Holdings::default()), &folder_with(credentials.clone()), &resolved);
            assert!(ready.contains(&format!("  census    0 fields\n{want}last pass   none yet\n")), "{ready}");
            // After the database line, when the database is not available.
            let unavailable = text_of(&Database::Unavailable("no".to_string()), &folder_with(credentials), &resolved);
            assert!(unavailable.contains(&format!("superx/recon: not available. no\n{want}parameters")), "{unavailable}");
        }
    }
}
