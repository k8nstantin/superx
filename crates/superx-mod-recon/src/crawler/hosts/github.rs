//! A repository on github.com (#538, step 17): every path asked in this
//! order, each under `/repos/<owner>/<repo>`, after the repository itself,
//! which the host crawl's start asked. A pull request, an issue's timeline,
//! a run's jobs and a ruleset are asked again only when their object
//! changed, or when an earlier host crawl did not settle them; statuses,
//! check suites and check runs for the commits step 17 names.

use std::collections::BTreeSet;

use serde_json::Value as Json;
use superx_kernel::types::{RecordId, Value};

use crate::crawler::hosts::ci::{CHECK_RUN, CHECK_SUITE, COMMIT_STATUS, WORKFLOW_JOB, WORKFLOW_RUN};
use crate::crawler::hosts::crawl::{pages_of, Beside, HostCrawl, Keep, Reading};
use crate::crawler::hosts::objects::{hash, key_text, key_value, Shape};
use crate::crawler::hosts::pulls::{n, s};
use crate::crawler::Stop;

fn id(o: &Json) -> Option<Value> {
    o.get("id").and_then(key_value)
}

fn sha(o: &Json) -> Option<Value> {
    o.get("sha").and_then(key_value)
}

/// A timeline event's key: its id, else a committed event's sha, else its
/// own hash.
fn timeline_key(o: &Json) -> Option<Value> {
    id(o).or_else(|| sha(o)).or_else(|| Some(Value::String(hash(o))))
}

/// The segments an earlier host crawl left unsettled, as numbers.
fn numbers(segments: BTreeSet<String>) -> BTreeSet<i64> {
    segments.iter().filter_map(|s| s.parse().ok()).collect()
}

/// What the pull requests' paths read, for the rows that wait on their
/// timelines.
struct Pulled {
    /// The numbers of the pull requests asked.
    asked: BTreeSet<i64>,
    /// Each pull request object read, with its row.
    read: Vec<(Json, RecordId)>,
    readings: Vec<Reading>,
}

/// Crawl a GitHub repository's paths after its own (step 17).
pub async fn crawl(c: &mut HostCrawl, beside: &Beside, stop: &mut Stop) {
    let base = format!("/repos/{}", c.hp.path);
    let Some(pulled) = pull_requests(c, &base, stop).await else { return };
    if timelines(c, &base, pulled, stop).await.is_none() {
        return;
    }
    c.list(&format!("{base}/issues/comments?sort=created&direction=asc"), Shape::Array, Some(&Keep { kind: "issue_comment", key: &id }), stop)
        .await;
    c.list(&format!("{base}/pulls/comments?sort=created&direction=asc"), Shape::Array, Some(&Keep { kind: "review_comment", key: &id }), stop)
        .await;
    c.list(&format!("{base}/issues/events"), Shape::Array, Some(&Keep { kind: "issue_event", key: &id }), stop).await;
    if c.stopped || runs(c, &base, stop).await.is_none() || checks(c, &base, beside, stop).await.is_none() {
        return;
    }

    // Deployments and the statuses of every deployment kept, environments,
    // releases.
    c.list(&format!("{base}/deployments"), Shape::Array, Some(&Keep { kind: "deployment", key: &id }), stop).await;
    let mut deployments: Vec<(i64, String)> =
        c.memory.kept.keys("deployment").into_iter().map(|k| (k.parse().unwrap_or(i64::MAX), k)).collect();
    deployments.sort();
    for (_, deployment) in deployments {
        if c.stopped {
            return;
        }
        c.list(&format!("{base}/deployments/{deployment}/statuses"), Shape::Array, Some(&Keep { kind: "deployment_status", key: &id }), stop)
            .await;
    }
    c.list(&format!("{base}/environments"), Shape::Wrapped("environments"), Some(&Keep { kind: "environment", key: &id }), stop).await;
    c.list(&format!("{base}/releases"), Shape::Array, Some(&Keep { kind: "release", key: &id }), stop).await;
    if c.stopped || rulesets(c, &base, stop).await.is_none() {
        return;
    }

    // The default branch's protection: GitHub answers HTTP 404, the host's
    // answer, when the branch is not protected and when the token is not an
    // admin of the repository.
    if let Some(branch) = c.default_branch() {
        let name = branch.clone();
        let key = move |_: &Json| Some(Value::String(name.clone()));
        c.one(&format!("{base}/branches/{branch}/protection"), &Keep { kind: "branch_protection", key: &key }, false, stop).await;
    }
}

/// The pull requests: the list, whose objects are not kept; each pull
/// request new or changed asked whole, with its commits, files and
/// reviews; and what an earlier host crawl left of them. `None` once
/// stopped.
async fn pull_requests(c: &mut HostCrawl, base: &str, stop: &mut Stop) -> Option<Pulled> {
    let list = c.list(&format!("{base}/pulls?state=all&sort=created&direction=asc"), Shape::Array, None, stop).await;
    if list.stopped {
        return None;
    }
    let mut changed: BTreeSet<i64> = BTreeSet::new();
    for (item, _, _) in &list.objects {
        let (Some(key), Some(number)) = (id(item).map(|k| key_text(&k)), n(item, &["number"])) else { continue };
        let newest = c.memory.updated.get(&("pull_request".to_string(), key.clone())).map(String::as_str);
        if !c.memory.kept.holds("pull_request", &key) || s(item, &["updated_at"]) != newest {
            changed.insert(number);
        }
    }
    let mut wanted = changed.clone();
    for suffix in ["", "/commits", "/files", "/reviews"] {
        wanted.extend(numbers(c.unsettled(&format!("{base}/pulls/"), suffix)));
    }
    let mut pulled = Pulled { asked: BTreeSet::new(), read: Vec::new(), readings: Vec::new() };
    for number in wanted {
        let detail = format!("{base}/pulls/{number}");
        let ask = changed.contains(&number) || !c.settled(&detail);
        let mut object = None;
        if ask {
            pulled.asked.insert(number);
            let one = c.one(&detail, &Keep { kind: "pull_request", key: &id }, false, stop).await;
            if let (Some(h), Some(taken)) = (one.held, one.taken) {
                if let Some(o) = h.objects.into_iter().next() {
                    if let (Some(key), Some(updated)) = (id(&o).map(|k| key_text(&k)), s(&o, &["updated_at"])) {
                        c.memory.updated.insert(("pull_request".to_string(), key), updated.to_string());
                    }
                    object = n(&o, &["commits"]);
                    pulled.read.push((o, taken.row));
                }
            }
        }
        let commits = format!("{base}/pulls/{number}/commits");
        if ask || !c.settled(&c.first_page(&commits)) {
            let listed = c.list(&commits, Shape::Array, Some(&Keep { kind: "pull_request_commit", key: &sha }), stop).await;
            if listed.whole {
                let held = c.memory.pulls.get(&number).and_then(|p| p.state.as_ref()).and_then(|st| st.commits);
                let pages = pages_of(&listed, |o| s(o, &["sha"]).map(str::to_string));
                pulled.readings.push(Reading { number, pages, stated: object.or(held) });
            }
        }
        let files = format!("{base}/pulls/{number}/files");
        if ask || !c.settled(&c.first_page(&files)) {
            let key = move |o: &Json| s(o, &["filename"]).map(|f| Value::String(format!("{number}/{f}")));
            c.list(&files, Shape::Array, Some(&Keep { kind: "pull_request_file", key: &key }), stop).await;
        }
        let reviews = format!("{base}/pulls/{number}/reviews");
        if ask || !c.settled(&c.first_page(&reviews)) {
            c.list(&reviews, Shape::Array, Some(&Keep { kind: "review", key: &id }), stop).await;
        }
        if c.stopped {
            return None;
        }
    }
    Some(pulled)
}

/// The issues, a pull request being an issue there too, and the timeline
/// of each issue new or changed, of each pull request asked, and of each
/// an earlier host crawl left; then the pull requests' rows (step 19),
/// whose closer the timelines name.
async fn timelines(c: &mut HostCrawl, base: &str, pulled: Pulled, stop: &mut Stop) -> Option<()> {
    let issues = c.list(&format!("{base}/issues?state=all&sort=created&direction=asc"), Shape::Array, Some(&Keep { kind: "issue", key: &id }), stop).await;
    let mut wanted: BTreeSet<i64> = issues
        .objects
        .iter()
        .filter(|(_, taken, _)| taken.as_ref().is_some_and(|t| t.written))
        .filter_map(|(o, _, _)| n(o, &["number"]))
        .collect();
    wanted.extend(pulled.asked.iter().copied());
    wanted.extend(numbers(c.unsettled(&format!("{base}/issues/"), "/timeline")));
    for number in wanted {
        if c.stopped {
            return None;
        }
        let timeline =
            c.list(&format!("{base}/issues/{number}/timeline"), Shape::Array, Some(&Keep { kind: "timeline_event", key: &timeline_key }), stop).await;
        for (event, _, _) in &timeline.objects {
            if let (Some("closed"), Some(at)) = (s(event, &["event"]), s(event, &["created_at"])) {
                c.memory.close(number, at.to_string(), event.get("actor").cloned().unwrap_or(Json::Null));
            }
        }
    }
    if c.stopped {
        return None;
    }
    c.pull_rows(&pulled.read, &pulled.readings).await;
    Some(())
}

/// Workflow runs, and the jobs of each run new or changed or left by an
/// earlier host crawl.
async fn runs(c: &mut HostCrawl, base: &str, stop: &mut Stop) -> Option<()> {
    let runs = c.list(&format!("{base}/actions/runs"), Shape::Wrapped("workflow_runs"), Some(&Keep { kind: WORKFLOW_RUN, key: &id }), stop).await;
    c.ci_rows(&runs, WORKFLOW_RUN, None).await;
    let mut wanted: BTreeSet<i64> = runs
        .objects
        .iter()
        .filter(|(_, taken, _)| taken.as_ref().is_some_and(|t| t.written))
        .filter_map(|(o, _, _)| n(o, &["id"]))
        .collect();
    wanted.extend(numbers(c.unsettled(&format!("{base}/actions/runs/"), "/jobs")));
    for run in wanted {
        if c.stopped {
            return None;
        }
        let jobs =
            c.list(&format!("{base}/actions/runs/{run}/jobs?filter=all"), Shape::Wrapped("jobs"), Some(&Keep { kind: WORKFLOW_JOB, key: &id }), stop).await;
        c.ci_rows(&jobs, WORKFLOW_JOB, None).await;
    }
    (!c.stopped).then_some(())
}

/// Commit statuses, then each commit's check suites and check runs, for
/// the commits step 17 names.
async fn checks(c: &mut HostCrawl, base: &str, beside: &Beside, stop: &mut Stop) -> Option<()> {
    let statuses = |commit: &str| vec![format!("{base}/commits/{commit}/statuses")];
    let pending = c.pending_commits(&[COMMIT_STATUS]);
    for commit in c.commits_to_ask(beside, &statuses, &pending) {
        if c.stopped {
            return None;
        }
        let listed = c.list(&format!("{base}/commits/{commit}/statuses"), Shape::Array, Some(&Keep { kind: COMMIT_STATUS, key: &id }), stop).await;
        c.ci_rows(&listed, COMMIT_STATUS, Some(&commit)).await;
    }
    let suites_and_runs =
        |commit: &str| vec![format!("{base}/commits/{commit}/check-suites"), format!("{base}/commits/{commit}/check-runs?filter=all")];
    let pending = c.pending_commits(&[CHECK_SUITE, CHECK_RUN]);
    for commit in c.commits_to_ask(beside, &suites_and_runs, &pending) {
        if c.stopped {
            return None;
        }
        let suites = c
            .list(&format!("{base}/commits/{commit}/check-suites"), Shape::Wrapped("check_suites"), Some(&Keep { kind: CHECK_SUITE, key: &id }), stop)
            .await;
        c.ci_rows(&suites, CHECK_SUITE, Some(&commit)).await;
        let checks = c
            .list(&format!("{base}/commits/{commit}/check-runs?filter=all"), Shape::Wrapped("check_runs"), Some(&Keep { kind: CHECK_RUN, key: &id }), stop)
            .await;
        c.ci_rows(&checks, CHECK_RUN, Some(&commit)).await;
    }
    (!c.stopped).then_some(())
}

/// The rulesets: the list, whose objects are not kept, and each ruleset
/// new or changed, or left by an earlier host crawl, kept whole.
async fn rulesets(c: &mut HostCrawl, base: &str, stop: &mut Stop) -> Option<()> {
    let list = c.list(&format!("{base}/rulesets"), Shape::Array, None, stop).await;
    let mut changed: BTreeSet<i64> = BTreeSet::new();
    for (item, _, _) in &list.objects {
        let (Some(key), Some(number)) = (id(item).map(|k| key_text(&k)), n(item, &["id"])) else { continue };
        let newest = c.memory.updated.get(&("ruleset".to_string(), key.clone())).map(String::as_str);
        if !c.memory.kept.holds("ruleset", &key) || s(item, &["updated_at"]) != newest {
            changed.insert(number);
        }
    }
    let mut wanted = changed.clone();
    wanted.extend(numbers(c.unsettled(&format!("{base}/rulesets/"), "")));
    for ruleset in wanted {
        if c.stopped {
            return None;
        }
        let path = format!("{base}/rulesets/{ruleset}");
        if !changed.contains(&ruleset) && c.settled(&path) {
            continue;
        }
        let one = c.one(&path, &Keep { kind: "ruleset", key: &id }, false, stop).await;
        if let Some(o) = one.held.and_then(|h| h.objects.into_iter().next()) {
            if let Some(updated) = s(&o, &["updated_at"]) {
                c.memory.updated.insert(("ruleset".to_string(), ruleset.to_string()), updated.to_string());
            }
        }
    }
    (!c.stopped).then_some(())
}
