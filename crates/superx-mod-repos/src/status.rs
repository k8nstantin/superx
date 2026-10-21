//! `superx repos status`: whether the module's own database is there, what
//! it holds, and the parameters the crawler runs on.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
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
}

/// The counts, and the newest crawl. A count is NONE for an empty table:
/// `GROUP ALL` over no rows returns no row.
const HOLDINGS: &str = "\
    RETURN (SELECT count() AS n FROM repository GROUP ALL)[0].n;\
    RETURN (SELECT count() AS n FROM checkout GROUP ALL)[0].n;\
    RETURN (SELECT count() AS n FROM commit GROUP ALL)[0].n;\
    RETURN (SELECT VALUE finished_at FROM crawl ORDER BY valid_from DESC LIMIT 1)[0];\
    RETURN (SELECT VALUE outcome FROM crawl ORDER BY valid_from DESC LIMIT 1)[0];";

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
    Ok(Holdings {
        repositories: repositories.unwrap_or_default(),
        checkouts: checkouts.unwrap_or_default(),
        commits: commits.unwrap_or_default(),
        last_crawl: finished.zip(outcome),
    })
}

/// The module's own database, as `status` found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Database {
    Ready(Holdings),
    /// Not provisioned, or not reachable: the kernel's own words.
    Unavailable(String),
}

/// The status, as `superx repos status` prints it.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer
/// for the parameters, or the module's own database for its contents.
pub async fn render(kernel: &Kernel) -> Result<String> {
    let resolved = params::resolve(kernel).await?;
    let database = match kernel.module_db(MODULE_NAME).await {
        Ok(db) => Database::Ready(holdings(&db).await?),
        Err(e) => Database::Unavailable(e.to_string()),
    };
    Ok(text(&database, &resolved))
}

/// The status text for what was found.
#[must_use]
pub fn text(database: &Database, resolved: &Resolved) -> String {
    let mut out = String::from("repos: the repositories the agents work in, read with git, read-only\n");
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
    use crate::params::{from_values, MAX_REPOS_PARAM};

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
    fn an_unprovisioned_database_says_why_and_a_wrong_value_says_what_runs_instead() {
        let mut values = BTreeMap::new();
        values.insert(MAX_REPOS_PARAM, superx_kernel::types::Value::String("eight".to_string()));
        let why = "module 'repos' db signin refused — run `superx modules provision repos` first";
        let t = text(&Database::Unavailable(why.to_string()), &from_values(&values));
        assert!(t.contains(&format!("superx/repos: not available. {why}")), "{t}");
        assert!(t.contains("attr_repos_max is \"eight\", not a whole number, 1 or more: running on 8"), "{t}");
    }
}
