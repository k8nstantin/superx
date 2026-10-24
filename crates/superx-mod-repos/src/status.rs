//! `superx-repos status`, and the module's own URL: whether its database is
//! there, what it holds, and the parameters the crawler runs on.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;
use superx_kernel::{Db, Kernel, Result};

use crate::params::{self, Resolved};
use crate::MODULE_NAME;

/// What the module's own database holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Holdings {
    pub repositories: i64,
    pub checkouts: i64,
    pub commits: i64,
    /// When the newest crawl finished, and how it ended.
    pub last_crawl: Option<(DateTime<Utc>, String)>,
    /// The newest pass over the list.
    pub last_pass: Option<LastPass>,
}

/// The newest pass over the list, as `status` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastPass {
    pub finished_at: DateTime<Utc>,
    pub outcome: String,
    pub directories_read: i64,
    pub repositories_known: i64,
    pub repositories_stale: i64,
    pub checkouts_known: i64,
    pub rows_written: i64,
    pub git_calls: i64,
    pub git_timeouts: i64,
}

/// The counts, and the newest crawl. A count is NONE for an empty table:
/// `GROUP ALL` over no rows returns no row.
const HOLDINGS: &str = "\
    RETURN (SELECT count() AS n FROM repository GROUP ALL)[0].n;\
    RETURN (SELECT count() AS n FROM checkout GROUP ALL)[0].n;\
    RETURN (SELECT count() AS n FROM commit GROUP ALL)[0].n;\
    RETURN (SELECT VALUE finished_at FROM crawl ORDER BY valid_from DESC LIMIT 1)[0];\
    RETURN (SELECT VALUE outcome FROM crawl ORDER BY valid_from DESC LIMIT 1)[0];\
    SELECT finished_at, outcome, directories_read, repositories_known, repositories_stale, checkouts_known, \
        rows_written, git_calls, git_timeouts, valid_from FROM sweep ORDER BY valid_from DESC LIMIT 1;";

/// Read what the module's own database holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn holdings(db: &Db) -> Result<Holdings> {
    let mut res = db.query(HOLDINGS).await?;
    let repositories: Option<i64> = res.take(0)?;
    let checkouts: Option<i64> = res.take(1)?;
    let commits: Option<i64> = res.take(2)?;
    let finished: Option<DateTime<Utc>> = res.take(3)?;
    let outcome: Option<String> = res.take(4)?;
    let passes: Vec<Value> = res.take(5)?;
    Ok(Holdings {
        repositories: repositories.unwrap_or_default(),
        checkouts: checkouts.unwrap_or_default(),
        commits: commits.unwrap_or_default(),
        last_crawl: finished.zip(outcome),
        last_pass: passes.first().and_then(last_pass),
    })
}

fn last_pass(row: &Value) -> Option<LastPass> {
    let Value::Object(o) = row else { return None };
    let n = |key: &str| match o.get(key) {
        Some(Value::Number(n)) => n.to_int(),
        _ => None,
    };
    Some(LastPass {
        finished_at: match o.get("finished_at") {
            Some(Value::Datetime(d)) => DateTime::<Utc>::from(*d),
            _ => return None,
        },
        outcome: match o.get("outcome") {
            Some(Value::String(s)) => s.clone(),
            _ => return None,
        },
        directories_read: n("directories_read")?,
        repositories_known: n("repositories_known")?,
        repositories_stale: n("repositories_stale")?,
        checkouts_known: n("checkouts_known")?,
        rows_written: n("rows_written")?,
        git_calls: n("git_calls")?,
        git_timeouts: n("git_timeouts")?,
    })
}

/// The module's own database, as `status` found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Database {
    Ready(Holdings),
    /// Not provisioned, or not reachable: the kernel's own words.
    Unavailable(String),
}

/// The status, as `superx-repos status` prints it and its URL answers.
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
    Ok(text(&database, &resolved))
}

/// The kernel names `superx modules provision <name>` for a module database
/// it cannot sign in to: a command that knows only the modules linked into
/// `superx`. This module provisions its own.
fn own_hint(kernel_words: &str) -> String {
    let words = kernel_words.split(" — run `superx modules provision").next().unwrap_or(kernel_words);
    format!("{words}. Provision it with `superx-repos --initialize`")
}

/// The status text for what was found.
#[must_use]
pub fn text(database: &Database, resolved: &Resolved) -> String {
    let mut out = String::from("superx-repos: the repositories the agents work in, read with git, read-only\n");
    match database {
        Database::Ready(h) => {
            let crawl = h.last_crawl.as_ref().map_or_else(
                || "no crawl yet".to_string(),
                |(at, outcome)| format!("last crawl {} ({outcome})", at.to_rfc3339()),
            );
            let _ = writeln!(
                out,
                "database    superx/{MODULE_NAME}: {} repositories · {} checkouts · {} commits · {crawl}",
                h.repositories, h.checkouts, h.commits
            );
            match &h.last_pass {
                Some(p) => {
                    let _ = writeln!(
                        out,
                        "last pass   {} ({}): {} directories read · {} repositories, {} stale · {} checkouts · \
                         {} rows written · {} git calls, {} cut off",
                        p.finished_at.to_rfc3339(),
                        p.outcome,
                        p.directories_read,
                        p.repositories_known,
                        p.repositories_stale,
                        p.checkouts_known,
                        p.rows_written,
                        p.git_calls,
                        p.git_timeouts
                    );
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::params::{from_values, CRAWL_SECS_PARAM};

    #[test]
    fn a_fresh_database_says_it_holds_nothing_and_lists_every_parameter() {
        let t = text(&Database::Ready(Holdings::default()), &from_values(&BTreeMap::new()));
        assert!(t.contains("superx/repos: 0 repositories · 0 checkouts · 0 commits · no crawl yet"), "{t}");
        for name in crate::params::NAMES {
            assert!(t.contains(name), "{name} is listed:\n{t}");
        }
        assert!(!t.contains("problems"), "{t}");
    }

    #[test]
    fn the_last_pass_says_when_how_it_ended_and_what_it_found() {
        let h = Holdings {
            last_pass: Some(LastPass {
                finished_at: DateTime::parse_from_rfc3339("2026-10-24T15:00:00Z").expect("time").to_utc(),
                outcome: "partial".to_string(),
                directories_read: 52,
                repositories_known: 9,
                repositories_stale: 1,
                checkouts_known: 14,
                rows_written: 37,
                git_calls: 61,
                git_timeouts: 2,
            }),
            ..Holdings::default()
        };
        let t = text(&Database::Ready(h), &from_values(&BTreeMap::new()));
        assert!(
            t.contains(
                "last pass   2026-10-24T15:00:00+00:00 (partial): 52 directories read · 9 repositories, 1 stale · \
                 14 checkouts · 37 rows written · 61 git calls, 2 cut off"
            ),
            "{t}"
        );
        let none = text(&Database::Ready(Holdings::default()), &from_values(&BTreeMap::new()));
        assert!(none.contains("last pass   none yet"), "{none}");
    }

    #[test]
    fn an_unprovisioned_database_names_this_modules_own_command() {
        let kernel = "configuration error: module 'repos' db signin refused (There was a problem with authentication) \
                      — run `superx modules provision repos` first";
        assert_eq!(
            own_hint(kernel),
            "configuration error: module 'repos' db signin refused (There was a problem with authentication). \
             Provision it with `superx-repos --initialize`"
        );
        assert_eq!(own_hint("cannot connect"), "cannot connect. Provision it with `superx-repos --initialize`");
    }

    #[test]
    fn an_unprovisioned_database_says_why_and_a_wrong_value_says_what_runs_instead() {
        let mut values = BTreeMap::new();
        values.insert(CRAWL_SECS_PARAM, superx_kernel::types::Value::String("ten".to_string()));
        let why = "module 'repos' db signin refused — run `superx-repos --initialize` first";
        let t = text(&Database::Unavailable(why.to_string()), &from_values(&values));
        assert!(t.contains(&format!("superx/repos: not available. {why}")), "{t}");
        assert!(t.contains("attr_repos_crawl_secs is \"ten\", not a whole number, 0 or more: running on 600"), "{t}");
    }
}
