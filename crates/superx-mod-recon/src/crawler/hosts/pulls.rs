//! Pull requests (#538, step 19): a `pull_request` row the first time a
//! pull request object is read, a `pull_request_state` row whenever one of
//! its columns differs from the newest, and a `pull_request_commit` row each
//! time a reading of its commits lists a commit for the first time, finds
//! it gone, or finds it back.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;
use superx_kernel::types::{RecordId, Value};

use crate::crawler::hosts::objects::{key_value, Accounts};
use crate::crawler::hosts::Host;
use crate::crawler::store::{Batch, Row};
use crate::crawler::value::{at, flag, int, text};

/// GitHub lists two hundred and fifty commits of a pull request at most.
pub const GITHUB_LISTED: i64 = 250; // skill-allow: §9-const — the host's own limit, not a policy tunable

/// A pull request the crawler holds.
#[derive(Debug, Clone)]
pub struct Pull {
    pub row: RecordId,
    /// Its url, as its `pull_request` row holds it.
    pub url: Option<String>,
    pub state: Option<PullState>,
    /// Each commit its readings have listed, as the newest row says it.
    pub commits: HashMap<String, PullCommit>,
}

/// A commit of a pull request, as its newest row says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullCommit {
    pub present: bool,
    pub position: Option<i64>,
}

/// A pull request's state, as its newest row says it: every column but
/// the object it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullState {
    pub state: String,
    pub merged: bool,
    pub draft: Option<bool>,
    pub title: String,
    pub head_sha: Option<String>,
    pub base_sha: Option<String>,
    pub merge_sha: Option<String>,
    pub merged_at: Option<DateTime<Utc>>,
    pub merged_by: Option<RecordId>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_by: Option<RecordId>,
    pub reason: Option<String>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_files: Option<i64>,
    pub commits: Option<i64>,
    pub comments: Option<i64>,
    pub review_comments: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

impl PullState {
    /// A `pull_request_state` row, as the database gives it back.
    #[must_use]
    pub fn from_row(v: &Value) -> Self {
        let time = |key: &str| match at(v, &[key]) {
            Some(Value::Datetime(d)) => Some(DateTime::<Utc>::from(*d)),
            _ => None,
        };
        let link = |key: &str| match at(v, &[key]) {
            Some(Value::RecordId(id)) => Some(id.clone()),
            _ => None,
        };
        Self {
            state: text(v, &["state"]).unwrap_or_default().to_string(),
            merged: flag(v, &["merged"]).unwrap_or(false),
            draft: flag(v, &["draft"]),
            title: text(v, &["title"]).unwrap_or_default().to_string(),
            head_sha: text(v, &["head_sha"]).map(str::to_string),
            base_sha: text(v, &["base_sha"]).map(str::to_string),
            merge_sha: text(v, &["merge_sha"]).map(str::to_string),
            merged_at: time("merged_at"),
            merged_by: link("merged_by"),
            closed_at: time("closed_at"),
            closed_by: link("closed_by"),
            reason: text(v, &["reason"]).map(str::to_string),
            additions: int(v, &["additions"]),
            deletions: int(v, &["deletions"]),
            changed_files: int(v, &["changed_files"]),
            commits: int(v, &["commits"]),
            comments: int(v, &["comments"]),
            review_comments: int(v, &["review_comments"]),
            updated_at: time("updated_at").unwrap_or_default(),
        }
    }

    /// Whether the pull request is open, as its host writes it.
    #[must_use]
    pub fn open(&self) -> bool {
        self.state.eq_ignore_ascii_case("open")
    }
}

/// Text at a path of a JSON object.
#[must_use]
pub fn s<'a>(v: &'a Json, path: &[&str]) -> Option<&'a str> {
    let mut here = v;
    for key in path {
        here = here.get(*key)?;
    }
    here.as_str().filter(|s| !s.is_empty())
}

/// A whole number at a path of a JSON object.
#[must_use]
pub fn n(v: &Json, path: &[&str]) -> Option<i64> {
    let mut here = v;
    for key in path {
        here = here.get(*key)?;
    }
    here.as_i64()
}

/// A flag at a path of a JSON object.
#[must_use]
pub fn b(v: &Json, path: &[&str]) -> Option<bool> {
    let mut here = v;
    for key in path {
        here = here.get(*key)?;
    }
    here.as_bool()
}

/// A time a host wrote, at a path of a JSON object.
#[must_use]
pub fn t(v: &Json, path: &[&str]) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s(v, path)?).ok().map(|d| d.with_timezone(&Utc))
}

/// The value at a path of a JSON object.
#[must_use]
pub fn j<'a>(v: &'a Json, path: &[&str]) -> Option<&'a Json> {
    let mut here = v;
    for key in path {
        here = here.get(*key)?;
    }
    (!here.is_null()).then_some(here)
}

/// A pull request's number on its host: GitHub's `number`, Bitbucket's
/// `id`.
#[must_use]
pub fn number(host: Host, pull: &Json) -> Option<i64> {
    match host {
        Host::GitHub => n(pull, &["number"]),
        Host::Bitbucket => n(pull, &["id"]),
    }
}

/// What the rows of one pull request object need besides the object.
pub struct Read<'a> {
    pub host: Host,
    /// The host repository's own path, lowercased.
    pub path: &'a str,
    pub host_repository: &'a RecordId,
    /// The object's own `host_object` row.
    pub object: &'a RecordId,
    /// The actor of the newest closed event of its timeline as kept (GitHub).
    pub closed_actor: Option<&'a Json>,
}

/// One pull request object read: its `pull_request` row the first time,
/// and a `pull_request_state` row when its state changed. The pull request
/// as held after it, or `None` when the object lacks what a row needs.
pub fn read(rows: &mut Batch, accounts: &mut Accounts, held: Option<&Pull>, pull: &Json, r: &Read<'_>) -> Option<Pull> {
    let number = number(r.host, pull)?;
    let mut account = |v: Option<&Json>| v.and_then(|v| accounts.row(r.host, v));
    let (row, url) = match held {
        Some(p) => (p.row.clone(), p.url.clone()),
        None => {
            let (host_id, author, created_at, source_branch, target_branch, source_path, url) = match r.host {
                Host::GitHub => (
                    pull.get("id").and_then(key_value)?,
                    account(j(pull, &["user"])),
                    t(pull, &["created_at"])?,
                    s(pull, &["head", "ref"])?.to_string(),
                    s(pull, &["base", "ref"])?.to_string(),
                    s(pull, &["head", "repo", "full_name"]).map(str::to_lowercase),
                    s(pull, &["html_url"])?.to_string(),
                ),
                Host::Bitbucket => (
                    pull.get("id").and_then(key_value)?,
                    account(j(pull, &["author"])),
                    t(pull, &["created_on"])?,
                    s(pull, &["source", "branch", "name"])?.to_string(),
                    s(pull, &["destination", "branch", "name"])?.to_string(),
                    s(pull, &["source", "repository", "full_name"]).map(str::to_lowercase),
                    s(pull, &["links", "html", "href"])?.to_string(),
                ),
            };
            let row = rows.create(
                "pull_request",
                Row::new()
                    .link("host_repository", r.host_repository)
                    .int("number", number)
                    .value("host_id", host_id)
                    .some_link("author", author.as_ref())
                    .time("created_at", created_at)
                    .text("source_branch", source_branch)
                    .text("target_branch", target_branch)
                    .some_text("source_path", source_path.filter(|p| p != r.path))
                    .text("url", url.clone())
                    .link("object", r.object),
            );
            (row, Some(url))
        }
    };
    let state = match r.host {
        Host::GitHub => {
            let merged = b(pull, &["merged"]).unwrap_or(false);
            PullState {
                state: s(pull, &["state"])?.to_string(),
                merged,
                draft: b(pull, &["draft"]),
                title: s(pull, &["title"]).unwrap_or_default().to_string(),
                head_sha: s(pull, &["head", "sha"]).map(str::to_string),
                base_sha: s(pull, &["base", "sha"]).map(str::to_string),
                merge_sha: if merged { s(pull, &["merge_commit_sha"]).map(str::to_string) } else { None },
                merged_at: t(pull, &["merged_at"]),
                merged_by: account(j(pull, &["merged_by"])),
                closed_at: t(pull, &["closed_at"]),
                closed_by: account(r.closed_actor),
                reason: None,
                additions: n(pull, &["additions"]),
                deletions: n(pull, &["deletions"]),
                changed_files: n(pull, &["changed_files"]),
                commits: n(pull, &["commits"]),
                comments: n(pull, &["comments"]),
                review_comments: n(pull, &["review_comments"]),
                updated_at: t(pull, &["updated_at"])?,
            }
        }
        Host::Bitbucket => {
            let state = s(pull, &["state"])?.to_string();
            let merged = state == "MERGED";
            let closed_by = account(j(pull, &["closed_by"]));
            PullState {
                merged,
                draft: b(pull, &["draft"]),
                title: s(pull, &["title"]).unwrap_or_default().to_string(),
                head_sha: s(pull, &["source", "commit", "hash"]).map(str::to_string),
                base_sha: s(pull, &["destination", "commit", "hash"]).map(str::to_string),
                merge_sha: s(pull, &["merge_commit", "hash"]).map(str::to_string),
                merged_at: None,
                merged_by: if merged { closed_by.clone() } else { None },
                closed_at: None,
                closed_by,
                reason: s(pull, &["reason"]).map(str::to_string),
                additions: None,
                deletions: None,
                changed_files: None,
                commits: None,
                comments: n(pull, &["comment_count"]),
                review_comments: None,
                updated_at: t(pull, &["updated_on"])?,
                state,
            }
        }
    };
    let state = PullState { head_sha: state.head_sha.filter(|h| sha(h)), base_sha: state.base_sha.filter(|h| sha(h)), merge_sha: state.merge_sha.filter(|h| sha(h)), ..state };
    let newest = held.and_then(|p| p.state.clone());
    if newest.as_ref() != Some(&state) {
        rows.create(
            "pull_request_state",
            Row::new()
                .link("pull_request", &row)
                .text("state", state.state.clone())
                .flag("merged", state.merged)
                .some_flag("draft", state.draft)
                .text("title", state.title.clone())
                .some_text("head_sha", state.head_sha.clone())
                .some_text("base_sha", state.base_sha.clone())
                .some_text("merge_sha", state.merge_sha.clone())
                .some_time("merged_at", state.merged_at)
                .some_link("merged_by", state.merged_by.as_ref())
                .some_time("closed_at", state.closed_at)
                .some_link("closed_by", state.closed_by.as_ref())
                .some_text("reason", state.reason.clone())
                .some_int("additions", state.additions)
                .some_int("deletions", state.deletions)
                .some_int("changed_files", state.changed_files)
                .some_int("commits", state.commits)
                .some_int("comments", state.comments)
                .some_int("review_comments", state.review_comments)
                .time("updated_at", state.updated_at)
                .link("object", r.object),
        );
    }
    Some(Pull { row, url, state: Some(state), commits: held.map(|p| p.commits.clone()).unwrap_or_default() })
}

/// A commit hash a host gives: 7 to 64 hexadecimal digits.
#[must_use]
pub fn sha(h: &str) -> bool {
    (7..=64).contains(&h.len()) && h.bytes().all(|c| c.is_ascii_hexdigit())
}

/// One page of a reading of a pull request's commits: its commits with
/// their objects' rows, or, answered HTTP 304, unchanged.
#[derive(Debug, Clone)]
pub enum Page {
    Read(Vec<(String, RecordId)>),
    Unchanged,
}

/// One reading of a pull request's commits whose every page was answered:
/// the rows it makes. `page_size` is how many a page lists; `stated` is the
/// pull request's own count of its commits, when its host gives one.
pub fn commits(rows: &mut Batch, pull: &mut Pull, pages: &[Page], page_size: usize, stated: Option<i64>, host: Host) {
    let mut listed: Vec<(String, i64, Option<RecordId>)> = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        let offset = i64::try_from(i * page_size).unwrap_or(i64::MAX);
        match page {
            Page::Read(commits) => {
                for (k, (hash, object)) in commits.iter().enumerate() {
                    listed.push((hash.clone(), offset + i64::try_from(k).unwrap_or(i64::MAX), Some(object.clone())));
                }
            }
            Page::Unchanged => {
                // What the page listed before, it lists still.
                let end = offset + i64::try_from(page_size).unwrap_or(i64::MAX);
                let mut kept: Vec<(String, i64)> = pull
                    .commits
                    .iter()
                    .filter(|(_, c)| c.present && c.position.is_some_and(|p| p >= offset && p < end))
                    .map(|(h, c)| (h.clone(), c.position.unwrap_or(offset)))
                    .collect();
                kept.sort_by_key(|(_, p)| *p);
                listed.extend(kept.into_iter().map(|(h, p)| (h, p, None)));
            }
        }
    }
    let seen: HashSet<&str> = listed.iter().map(|(h, _, _)| h.as_str()).collect();
    let capped = host == Host::GitHub && stated.is_some_and(|n| n > i64::try_from(listed.len()).unwrap_or(i64::MAX));
    let mut changes: Vec<(String, PullCommit, Option<RecordId>)> = Vec::new();
    for (hash, position, object) in &listed {
        match pull.commits.get(hash) {
            Some(c) if c.present => {}
            _ => changes.push((hash.clone(), PullCommit { present: true, position: Some(*position) }, object.clone())),
        }
    }
    if !capped {
        let mut gone: Vec<&String> = pull.commits.iter().filter(|(h, c)| c.present && !seen.contains(h.as_str())).map(|(h, _)| h).collect();
        gone.sort();
        for hash in gone {
            changes.push((hash.clone(), PullCommit { present: false, position: None }, None));
        }
    }
    for (hash, c, object) in changes {
        rows.create(
            "pull_request_commit",
            Row::new()
                .link("pull_request", &pull.row)
                .text("hash", hash.clone())
                .some_int("position", c.position)
                .flag("present", c.present)
                .some_link("object", object.as_ref()),
        );
        pull.commits.insert(hash, c);
    }
}
