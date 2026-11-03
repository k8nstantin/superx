//! A repository on bitbucket.org (#538, step 18): every path asked in this
//! order, each under `/repositories/<workspace>/<repo>`, after the
//! repository itself, which the host crawl's start asked. A pull request
//! and a pipeline's steps are asked again only when their object changed,
//! or when an earlier host crawl did not settle them; statuses for the
//! commits step 17 names. Issues are not asked: Bitbucket removed its issue
//! tracker and its API on 2026-08-20.

use std::collections::BTreeSet;

use serde_json::Value as Json;
use superx_kernel::types::{RecordId, Value};

use crate::crawler::hosts::ci::{COMMIT_STATUS, PIPELINE, PIPELINE_STEP};
use crate::crawler::hosts::crawl::{pages_of, Beside, HostCrawl, Keep, Reading};
use crate::crawler::hosts::objects::{hash, key_text, key_value, Shape};
use crate::crawler::hosts::pulls::{n, s};
use crate::crawler::Stop;

fn id(o: &Json) -> Option<Value> {
    o.get("id").and_then(key_value)
}

fn uuid(o: &Json) -> Option<Value> {
    o.get("uuid").and_then(key_value)
}

/// A uuid as a path writes it: its braces `%7B` and `%7D`.
#[must_use]
pub fn in_path(uuid: &str) -> String {
    uuid.replace('{', "%7B").replace('}', "%7D")
}

/// Crawl a Bitbucket repository's paths after its own (step 18).
pub async fn crawl(c: &mut HostCrawl, beside: &Beside, stop: &mut Stop) {
    let base = format!("/repositories/{}", c.hp.path);
    if pull_requests(c, &base, stop).await.is_none() || statuses(c, &base, beside, stop).await.is_none() || pipelines(c, &base, stop).await.is_none() {
        return;
    }
    c.list(&format!("{base}/deployments"), Shape::Page, Some(&Keep { kind: "deployment", key: &uuid }), stop).await;
    c.list(&format!("{base}/environments"), Shape::Page, Some(&Keep { kind: "environment", key: &uuid }), stop).await;
    c.list(&format!("{base}/branch-restrictions"), Shape::Page, Some(&Keep { kind: "branch_restriction", key: &id }), stop).await;
    c.list(&format!("{base}/default-reviewers"), Shape::Page, Some(&Keep { kind: "default_reviewer", key: &uuid }), stop).await;
}

/// The pull requests in every state: the list, whose objects are not
/// kept; each pull request new or changed asked whole, with its commits,
/// diffstat, comments, activity and tasks; and what an earlier host crawl
/// left of them; then their rows (step 19). `None` once stopped.
async fn pull_requests(c: &mut HostCrawl, base: &str, stop: &mut Stop) -> Option<()> {
    let list = c
        .list(&format!("{base}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on"), Shape::Page, None, stop)
        .await;
    if list.stopped {
        return None;
    }
    let mut changed: BTreeSet<i64> = BTreeSet::new();
    for (item, _, _) in &list.objects {
        let (Some(key), Some(number)) = (id(item).map(|k| key_text(&k)), n(item, &["id"])) else { continue };
        let newest = c.memory.updated.get(&("pull_request".to_string(), key.clone())).map(String::as_str);
        if !c.memory.kept.holds("pull_request", &key) || s(item, &["updated_on"]) != newest {
            changed.insert(number);
        }
    }
    let mut wanted = changed.clone();
    for suffix in ["", "/commits", "/diffstat", "/comments", "/activity", "/tasks"] {
        wanted.extend(c.unsettled(&format!("{base}/pullrequests/"), suffix).iter().filter_map(|s| s.parse::<i64>().ok()));
    }
    let mut read: Vec<(Json, RecordId)> = Vec::new();
    let mut readings: Vec<Reading> = Vec::new();
    for number in wanted {
        let detail = format!("{base}/pullrequests/{number}");
        let ask = changed.contains(&number) || !c.settled(&detail);
        if ask {
            let one = c.one(&detail, &Keep { kind: "pull_request", key: &id }, false, stop).await;
            if let (Some(h), Some(taken)) = (one.held, one.taken) {
                if let Some(o) = h.objects.into_iter().next() {
                    if let (Some(key), Some(updated)) = (id(&o).map(|k| key_text(&k)), s(&o, &["updated_on"])) {
                        c.memory.updated.insert(("pull_request".to_string(), key), updated.to_string());
                    }
                    read.push((o, taken.row));
                }
            }
        }
        let commits = format!("{base}/pullrequests/{number}/commits");
        if ask || !c.settled(&c.first_page(&commits)) {
            let hash_of = |o: &Json| o.get("hash").and_then(key_value);
            let listed = c.list(&commits, Shape::Page, Some(&Keep { kind: "pull_request_commit", key: &hash_of }), stop).await;
            if listed.whole {
                let pages = pages_of(&listed, |o| s(o, &["hash"]).map(str::to_string));
                readings.push(Reading { number, pages, stated: None });
            }
        }
        // The diffstat's answer is a redirect within the host, which the
        // call follows.
        let diffstat = format!("{base}/pullrequests/{number}/diffstat");
        if ask || !c.settled(&c.first_page(&diffstat)) {
            let key = move |o: &Json| {
                let path = s(o, &["new", "path"]).or_else(|| s(o, &["old", "path"]))?;
                Some(Value::String(format!("{number}/{path}")))
            };
            c.list(&diffstat, Shape::Page, Some(&Keep { kind: "pull_request_file", key: &key }), stop).await;
        }
        let comments = format!("{base}/pullrequests/{number}/comments");
        if ask || !c.settled(&c.first_page(&comments)) {
            c.list(&comments, Shape::Page, Some(&Keep { kind: "pull_request_comment", key: &id }), stop).await;
        }
        let activity = format!("{base}/pullrequests/{number}/activity");
        if ask || !c.settled(&c.first_page(&activity)) {
            let own = |o: &Json| Some(Value::String(hash(o)));
            c.list(&activity, Shape::Page, Some(&Keep { kind: "pull_request_activity", key: &own }), stop).await;
        }
        let tasks = format!("{base}/pullrequests/{number}/tasks");
        if ask || !c.settled(&c.first_page(&tasks)) {
            c.list(&tasks, Shape::Page, Some(&Keep { kind: "pull_request_task", key: &id }), stop).await;
        }
        if c.stopped {
            return None;
        }
    }
    c.pull_rows(&read, &readings).await;
    Some(())
}

/// Commit statuses, keyed by their commit and key, for the commits and at
/// the host crawls step 17 names; a status pending while in progress.
async fn statuses(c: &mut HostCrawl, base: &str, beside: &Beside, stop: &mut Stop) -> Option<()> {
    let paths = |commit: &str| vec![format!("{base}/commit/{commit}/statuses")];
    let pending = c.pending_commits(&[COMMIT_STATUS]);
    for commit in c.commits_to_ask(beside, &paths, &pending) {
        if c.stopped {
            return None;
        }
        let asked = commit.clone();
        let key = move |o: &Json| s(o, &["key"]).map(|k| Value::String(format!("{asked}/{k}")));
        let listed = c.list(&format!("{base}/commit/{commit}/statuses"), Shape::Page, Some(&Keep { kind: COMMIT_STATUS, key: &key }), stop).await;
        c.ci_rows(&listed, COMMIT_STATUS, Some(&commit)).await;
    }
    (!c.stopped).then_some(())
}

/// Pipelines, newest first, and the steps of each pipeline new or changed
/// or left by an earlier host crawl.
async fn pipelines(c: &mut HostCrawl, base: &str, stop: &mut Stop) -> Option<()> {
    let pipelines = c.list(&format!("{base}/pipelines?sort=-created_on"), Shape::Page, Some(&Keep { kind: PIPELINE, key: &uuid }), stop).await;
    c.ci_rows(&pipelines, PIPELINE, None).await;
    let mut wanted: Vec<String> = Vec::new();
    for (o, taken, _) in &pipelines.objects {
        if let (Some(id), true) = (s(o, &["uuid"]), taken.as_ref().is_some_and(|t| t.written)) {
            wanted.push(in_path(id));
        }
    }
    for left in c.unsettled(&format!("{base}/pipelines/"), "/steps") {
        if !wanted.contains(&left) {
            wanted.push(left);
        }
    }
    for pipeline in wanted {
        if c.stopped {
            return None;
        }
        let steps = c.list(&format!("{base}/pipelines/{pipeline}/steps"), Shape::Page, Some(&Keep { kind: PIPELINE_STEP, key: &uuid }), stop).await;
        c.ci_rows(&steps, PIPELINE_STEP, None).await;
    }
    (!c.stopped).then_some(())
}
