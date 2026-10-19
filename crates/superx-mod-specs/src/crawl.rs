//! The crawl: every build's branch, read in its repository (read-only).
//!
//! Each build's repository is the directory its kickoff line was sent from
//! (the kickoff line's `cwd`), up to its top level. Each repository is
//! read once per crawl; each build branch in it is looked up and recorded
//! when what the repository shows of it has changed.

use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use superx_kernel::types::RecordId;
use superx_kernel::{Db, Kernel, Result};

use crate::repo;
use crate::session::get_str;
use crate::store::{self, RepoBranch};

/// What a crawl did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CrawlReport {
    pub repos: usize,
    pub branches: usize,
    pub written: usize,
    pub unchanged: usize,
    /// Builds whose launch directory is no repository (any more).
    pub unreadable: usize,
}

/// The remote branches a merge may be said to have gone into, for every
/// repository (`attr_specs_mainline_refs`: a list, or one comma-separated
/// string); empty means each repository's `origin/HEAD`.
pub const MAINLINES_PARAM: &str = "attr_specs_mainline_refs";

async fn mainlines(kernel: &Kernel) -> Vec<String> {
    use superx_kernel::types::Value;
    let Ok(Some(entity)) = kernel.find_module_by_name(superx_kernel::NodeKind::KernelModule, crate::MODULE_NAME).await else {
        return Vec::new();
    };
    match kernel.get_parameter(entity, MAINLINES_PARAM).await.ok().flatten() {
        Some(Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.trim().to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Crawl every build branch the ledger knows.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn crawl(kernel: &Kernel, db: &Db) -> Result<CrawlReport> {
    let mut report = CrawlReport::default();
    let mainlines = mainlines(kernel).await;
    let mut repos: HashMap<String, Option<repo::Repo>> = HashMap::new();
    for (run, _) in store::current_runs(db).await? {
        let Some(branch) = get_str(&run, "branch") else { continue };
        let Some(session) = record(&run, "session") else { continue };
        let Some((_, _, Some(cwd))) = store::kickoff(kernel, &session).await? else {
            report.unreadable += 1;
            continue;
        };
        if !repos.contains_key(&cwd) {
            let read = repo::read(Path::new(&cwd)).await;
            report.repos += usize::from(read.is_some());
            repos.insert(cwd.clone(), read);
        }
        let Some(Some(r)) = repos.get(&cwd) else {
            report.unreadable += 1;
            continue;
        };
        let state = repo::branch(r, &branch, &mainlines).await;
        report.branches += 1;
        let found = RepoBranch {
            repo: r.top.clone(),
            host: r.host.clone(),
            branch,
            on_local: state.on_local,
            on_remote: state.on_remote,
            merge_commit: state.merge.as_ref().map(|m| m.commit.clone()),
            merged_at: state.merge.as_ref().and_then(|m| m.at),
            merged_into: state.merged_into,
            pull_request: state.merge.as_ref().and_then(|m| m.pull_request.clone()),
            revert_commit: state.revert.as_ref().map(|v| v.commit.clone()),
            reverted_at: state.revert.as_ref().and_then(|v| v.at),
        };
        if store::ensure_repo_branch(db, &found, Utc::now()).await? {
            report.written += 1;
        } else {
            report.unchanged += 1;
        }
    }
    tracing::info!(target: "specs", ?report, "crawl done");
    Ok(report)
}

fn record(v: &superx_kernel::types::Value, key: &str) -> Option<RecordId> {
    match v {
        superx_kernel::types::Value::Object(o) => match o.get(key) {
            Some(superx_kernel::types::Value::RecordId(r)) => Some(r.clone()),
            _ => None,
        },
        _ => None,
    }
}
