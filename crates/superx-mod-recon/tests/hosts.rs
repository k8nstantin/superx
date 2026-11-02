//! The hosts (#538, steps 15 to 21): a repository whose remotes name a
//! repository on github.com and one on bitbucket.org, a host of the test's
//! own on the loopback answering for both, a pass run and its rows and its
//! requests read back.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::*;
use superx_kernel::types::Value;
use superx_mod_recon::crawler::Crawler;

/// GitHub's own repository object of acme/widgets.
fn github_repository() -> serde_json::Value {
    serde_json::json!({ "id": 7, "full_name": "acme/widgets", "default_branch": "main", "owner": { "login": "acme", "id": 9, "type": "Organization" } })
}

/// Bitbucket's own repository object of ws/repo.
fn bitbucket_repository() -> serde_json::Value {
    serde_json::json!({ "uuid": "{r-1}", "full_name": "ws/repo", "mainbranch": { "name": "main" }, "owner": { "uuid": "{w-1}", "type": "team", "display_name": "WS" } })
}

/// A repository whose remote origin names github.com's acme/widgets and
/// whose remote up names bitbucket.org's ws/repo, each held by the host's
/// own copy in `<root>/hosts`; the repository and that folder.
fn on_both_hosts(root: &Path) -> (PathBuf, PathBuf) {
    let (a, git_base) = (root.join("a"), root.join("hosts"));
    repo(&a);
    hosted(&a, &git_base, "github.com", "acme/widgets");
    hosted(&a, &git_base, "bitbucket.org", "ws/repo");
    git(&a, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);
    git(&a, &["remote", "add", "up", "https://bitbucket.org/ws/repo.git"]);
    (a, git_base)
}

/// The host_call rows, by the path they asked.
async fn calls(db: &superx_kernel::Db) -> BTreeMap<String, Vec<Value>> {
    let mut by: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for c in rows(db, "SELECT * FROM host_call").await {
        by.entry(text(&c, "path").expect("a path")).or_default().push(c);
    }
    by
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_host_call_is_a_get_with_the_hosts_headers_and_bearer_token_sent_only_to_its_host_the_token_never_in_a_path_a_query_a_row_or_a_log_line_and_is_a_host_call_row_with_each_column() {
    let logged = logged();
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step15a");
    let (a, git_base) = on_both_hosts(&root);
    let server = Server::start().await;
    let elsewhere = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    bitbucket_quiet(&server, "ws/repo", bitbucket_repository());
    let reset = chrono::Utc::now().timestamp() + 3600;
    server.on(
        "/repos/acme/widgets",
        vec![Answer::json(github_repository())
            .header("etag", "W/\"r1\"")
            .header("x-ratelimit-limit", "5000")
            .header("x-ratelimit-remaining", "4999")
            .header("x-ratelimit-reset", &reset.to_string())],
    );
    server.on("/repos/acme/widgets/releases?per_page=100", vec![Answer::status(500, "{\"message\":\"boom\"}")]);
    // A next page on another host: never asked, and its call fails.
    let away = format!("{}/2.0/repositories/ws/repo/deployments?pagelen=50&page=2", elsewhere.base);
    server.on(
        "/2.0/repositories/ws/repo/deployments?pagelen=50",
        vec![Answer::json(serde_json::json!({ "values": [{ "uuid": "{d-1}", "name": "one" }], "next": away }))],
    );
    let mut lines = Lines::new(&kernel, "s15a").await;
    lines.worked_in(&a).await;
    pass(&kernel, &db, &params(&[]), &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com", "bitbucket.org"]))).await;

    let asked = server.asked();
    assert!(asked.iter().any(|q| q.path.starts_with("/repos/")) && asked.iter().any(|q| q.path.starts_with("/2.0/")), "both hosts asked");
    let bearer = format!("Bearer {TOKEN}");
    for q in &asked {
        assert_eq!(q.method, "GET", "{}", q.path);
        assert_eq!(q.header("authorization"), Some(bearer.as_str()), "{}", q.path);
        assert_eq!(q.header("user-agent"), Some("superx-recon"), "{}", q.path);
        if q.path.starts_with("/repos/") {
            assert_eq!(q.header("accept"), Some("application/vnd.github+json"), "{}", q.path);
            assert_eq!(q.header("x-github-api-version"), Some("2022-11-28"), "{}", q.path);
        } else {
            assert!(q.path.starts_with("/2.0/"), "{}", q.path);
            assert_eq!(q.header("accept"), Some("application/json"), "{}", q.path);
            assert_eq!(q.header("x-github-api-version"), None, "{}", q.path);
        }
        assert!(!q.path.contains(TOKEN));
    }
    assert!(elsewhere.asked().is_empty(), "sent only to its host");
    let by_path = calls(&db).await;
    let refused = &by_path["/2.0/repositories/ws/repo/deployments?pagelen=50&page=2"][0];
    assert_eq!(int(refused, "status"), None, "no answer came");
    assert!(text(refused, "error").is_some_and(|e| e.contains("not the host's address")), "{refused:?}");
    assert!(!every_row(&db).await.contains(TOKEN), "no row holds the token");
    let said = logged.0.lock().expect("logged").join("\n");
    assert!(said.contains("http1 handshake complete"), "the calls' own lines were caught");
    assert!(!said.contains(TOKEN), "no log line holds the token");

    // A host_call row, column by column.
    let itself = &by_path["/repos/acme/widgets"][0];
    let crawl = rows(&db, "SELECT VALUE id FROM host_crawl WHERE host_repository.path = 'acme/widgets'").await;
    assert_eq!(itself.as_object().and_then(|o| o.get("host_crawl")), crawl.first());
    assert_eq!(text(itself, "method").as_deref(), Some("GET"));
    assert!(time(itself, "started_at").is_some() && int(itself, "ms").is_some_and(|ms| ms >= 0));
    assert_eq!((int(itself, "status"), flag(itself, "not_modified")), (Some(200), Some(false)));
    assert_eq!(int(itself, "bytes"), Some(i64::try_from(github_repository().to_string().len()).expect("a length")));
    assert_eq!(text(itself, "etag").as_deref(), Some("W/\"r1\""));
    assert_eq!((int(itself, "rate_limit"), int(itself, "rate_remaining")), (Some(5000), Some(4999)));
    assert_eq!(time(itself, "rate_reset").map(|t| t.timestamp()), Some(reset));
    assert_eq!(int(itself, "retry_after_ms"), None);
    assert_eq!((int(itself, "objects"), flag(itself, "next")), (Some(1), Some(false)));
    assert_eq!(text(itself, "error"), None, "none for HTTP 200");
    let failed = &by_path["/repos/acme/widgets/releases?per_page=100"][0];
    assert_eq!(text(failed, "error").as_deref(), Some("HTTP 500: {\"message\":\"boom\"}"), "the status and the answer's body, whole");
    let paged = &by_path["/2.0/repositories/ws/repo/deployments?pagelen=50"][0];
    assert_eq!((int(paged, "objects"), flag(paged, "next")), (Some(1), Some(true)));
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The host_crawl rows, oldest first, sorted here: a datetime index
/// misorders times within a second.
async fn host_crawls(db: &superx_kernel::Db) -> Vec<Value> {
    let mut all = rows(db, "SELECT * FROM host_crawl").await;
    all.sort_by_key(|c| time(c, "started_at"));
    all
}

/// The host_objects of `kind`.
async fn objects_of(db: &superx_kernel::Db, kind: &str) -> Vec<Value> {
    rows_with(db, "SELECT * FROM host_object WHERE kind = $k", "k", Value::String(kind.to_string())).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_list_is_paged_by_githubs_link_rel_next_and_bitbuckets_next_as_given_asking_a_hundred_and_fifty_a_page_a_redirect_within_the_host_is_followed_with_the_token_and_one_to_another_host_is_not_followed_and_fails() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step15b");
    let (a, git_base) = on_both_hosts(&root);
    let server = Server::start().await;
    let elsewhere = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    bitbucket_quiet(&server, "ws/repo", bitbucket_repository());
    let issue = |id: i64, number: i64| serde_json::json!({ "id": id, "number": number, "title": format!("issue {number}"), "updated_at": "2026-11-01T10:00:00Z" });
    server.on(
        "/repos/acme/widgets/issues?state=all&sort=created&direction=asc&per_page=100",
        vec![Answer::json(serde_json::json!([issue(1001, 1)])).header(
            "link",
            &format!("<{0}/repositories/7/issues?page=2&per_page=100>; rel=\"next\", <{0}/repositories/7/issues?page=2&per_page=100>; rel=\"last\"", server.base),
        )],
    );
    server.on("/repositories/7/issues?page=2&per_page=100", vec![Answer::json(serde_json::json!([issue(1002, 2)]))]);
    for number in [1, 2] {
        server.on(&format!("/repos/acme/widgets/issues/{number}/timeline?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    let deployments = "/2.0/repositories/ws/repo/deployments";
    server.on(
        &format!("{deployments}?pagelen=50"),
        vec![Answer::json(serde_json::json!({ "values": [{ "uuid": "{d-1}" }], "pagelen": 50, "next": format!("{}{deployments}?pagelen=50&page=2", server.base) }))],
    );
    server.on(&format!("{deployments}?pagelen=50&page=2"), vec![Answer::json(serde_json::json!({ "values": [{ "uuid": "{d-2}" }], "pagelen": 50 }))]);
    // A redirect within the host, and one to another.
    server.on("/repos/acme/widgets/releases?per_page=100", vec![Answer::status(302, "").header("location", "/repos/acme/widgets/releases-moved?per_page=100")]);
    server.on("/repos/acme/widgets/releases-moved?per_page=100", vec![Answer::json(serde_json::json!([{ "id": 5, "tag_name": "v1" }]))]);
    server.on("/repos/acme/widgets/environments?per_page=100", vec![Answer::status(302, "").header("location", &format!("{}/environments", elsewhere.base))]);
    let mut lines = Lines::new(&kernel, "s15b").await;
    lines.worked_in(&a).await;
    pass(&kernel, &db, &params(&[]), &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com", "bitbucket.org"]))).await;

    let paths = server.paths();
    assert!(paths.contains(&"/repositories/7/issues?page=2&per_page=100".to_string()), "GitHub's next, as given");
    assert!(paths.contains(&format!("{deployments}?pagelen=50&page=2")), "Bitbucket's next, as given");
    assert_eq!(objects_of(&db, "issue").await.len(), 2, "both pages kept");
    assert_eq!(objects_of(&db, "deployment").await.len(), 2);
    for first in [
        "/repos/acme/widgets/pulls?state=all&sort=created&direction=asc&per_page=100",
        "/repos/acme/widgets/issues/comments?sort=created&direction=asc&per_page=100",
        "/repos/acme/widgets/actions/runs?per_page=100",
        "/repos/acme/widgets/rulesets?per_page=100",
        "/2.0/repositories/ws/repo/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50",
        "/2.0/repositories/ws/repo/pipelines?sort=-created_on&pagelen=50",
    ] {
        assert!(paths.contains(&first.to_string()), "{first}");
    }
    for path in paths.iter().filter(|p| p.contains("/statuses")) {
        assert!(path.ends_with("per_page=100") || path.ends_with("pagelen=50"), "{path}");
    }
    let moved = server.asked().into_iter().find(|q| q.path == "/repos/acme/widgets/releases-moved?per_page=100").expect("the redirect followed");
    assert_eq!(moved.header("authorization"), Some(format!("Bearer {TOKEN}").as_str()), "with the token");
    assert_eq!(objects_of(&db, "release").await.len(), 1);
    assert!(elsewhere.asked().is_empty(), "a redirect to another host is not followed");
    let away = &calls(&db).await["/repos/acme/widgets/environments?per_page=100"][0];
    assert!(text(away, "error").is_some_and(|e| e.contains("redirect")), "the call fails: {away:?}");
    let crawl = rows(&db, "SELECT outcome, failed FROM host_crawl WHERE host_repository.path = 'acme/widgets'").await;
    assert_eq!((text(&crawl[0], "outcome").as_deref(), int(&crawl[0], "failed")), (Some("partial"), Some(1)));
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// An answer as it came, for the crawler's own reading of a refusal.
fn refusal(status: u16, body: &str) -> superx_mod_recon::crawler::hosts::call::Answered {
    superx_mod_recon::crawler::hosts::call::Answered {
        path: "/x".to_string(),
        started_at: chrono::Utc::now(),
        ms: 0,
        status: Some(status),
        headers: reqwest::header::HeaderMap::new(),
        body: body.as_bytes().to_vec(),
        etag: None,
        rate_limit: None,
        rate_remaining: None,
        rate_reset: None,
        retry_after: None,
        timed_out: false,
        error: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_etag_is_sent_back_and_an_answer_http_304_is_not_modified_with_nothing_read_again_a_refusal_for_the_rate_waits_as_retry_after_x_ratelimit_reset_or_one_minute_says_counted_and_a_stop_ends_the_wait() {
    use superx_mod_recon::crawler::hosts::call::{rate_wait, refused_for_rate};
    let kernel = kernel().await;
    let root = scratch("step15c");
    let (a, git_base) = on_both_hosts(&root);
    git(&a, &["remote", "remove", "up"]);
    let mut lines = Lines::new(&kernel, "s15c").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    let releases = "/repos/acme/widgets/releases?per_page=100";
    let deployments = "/repos/acme/widgets/deployments?per_page=100";

    // An etag, sent back; HTTP 304 read as unchanged; after a host crawl
    // that did not end complete, asked again whole.
    let db = own_db().await;
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    server.on(releases, vec![Answer::json(serde_json::json!([{ "id": 5, "tag_name": "v1" }])).header("etag", "\"e1\"")]);
    server.on(deployments, vec![Answer::json(serde_json::json!([])), Answer::status(500, "{}"), Answer::json(serde_json::json!([]))]);
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]));
    let asked_releases = |s: &Server| s.asked().into_iter().filter(|q| q.path == releases).collect::<Vec<_>>();
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(asked_releases(&server)[0].header("if-none-match"), None);
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(asked_releases(&server)[1].header("if-none-match"), Some("\"e1\""), "sent back");
    let second = calls(&db).await[releases].iter().find(|c| int(c, "status") == Some(304)).cloned().expect("HTTP 304");
    assert_eq!((flag(&second, "not_modified"), int(&second, "objects")), (Some(true), Some(0)), "nothing read again");
    assert_eq!(objects_of(&db, "release").await.len(), 1);
    let crawls = host_crawls(&db).await;
    assert_eq!((int(&crawls[1], "not_modified"), text(&crawls[1], "outcome").as_deref()), (Some(1), Some("partial")));
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(asked_releases(&server)[2].header("if-none-match"), None, "after a partial host crawl, asked again whole");

    // Refused for the rate: retry-after, then x-ratelimit-reset; each wait
    // counted.
    let db = own_db().await;
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    server.on(deployments, vec![Answer::status(429, "{}").header("retry-after", "1"), Answer::json(serde_json::json!([]))]);
    let reset = chrono::Utc::now().timestamp() + 1;
    server.on(
        "/repos/acme/widgets/environments?per_page=100",
        vec![
            Answer::status(403, "{}").header("x-ratelimit-remaining", "0").header("x-ratelimit-reset", &reset.to_string()),
            Answer::json(serde_json::json!({ "total_count": 0, "environments": [] })),
        ],
    );
    let ran = pass(&kernel, &db, &p, &mut Crawler::new(places(&root.join("home2"), endpoints(&server, &git_base), &["github.com"]))).await;
    let crawl = rows(&db, "SELECT rate_waits, waited_ms, outcome FROM host_crawl").await;
    assert_eq!((int(&crawl[0], "rate_waits"), text(&crawl[0], "outcome").as_deref()), (Some(2), Some("complete")));
    assert!(int(&crawl[0], "waited_ms").is_some_and(|ms| ms >= 1000), "{crawl:?}");
    assert_eq!(ran.source("hosts").and_then(|s| s.counts.get("rate_waits").copied()), Some(2));
    let refused = &calls(&db).await[deployments];
    assert_eq!(refused.iter().map(|c| (int(c, "status"), int(c, "retry_after_ms"))).collect::<Vec<_>>(), [(Some(429), Some(1000)), (Some(200), None)]);
    // The minute, as the crawler computes it, never waited.
    let now = chrono::Utc::now();
    assert!(refused_for_rate(&refusal(429, "")));
    assert_eq!(rate_wait(&refusal(429, ""), now), std::time::Duration::from_secs(60));
    let said = refusal(403, "{\"message\":\"API rate limit exceeded for installation\"}");
    assert!(refused_for_rate(&said));
    assert_eq!(rate_wait(&said, now), std::time::Duration::from_secs(60));
    assert!(!refused_for_rate(&refusal(403, "{\"message\":\"Resource not accessible by integration\"}")));

    // A stop ends the wait: the crawl stopped writes nothing.
    let db = own_db().await;
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    server.on(releases, vec![Answer::status(429, "{}")]);
    let (asked, mut stop) = never_stop();
    let mut crawler = Crawler::new(places(&root.join("home3"), endpoints(&server, &git_base), &["github.com"]));
    let began = std::time::Instant::now();
    let stopper = async {
        while !server.paths().iter().any(|path| path == releases) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // skill-allow: §9-duration — test fixture
        }
        asked.send(true).expect("asked to stop");
    };
    let (ran, ()) = tokio::join!(superx_mod_recon::crawler::pass_on(&kernel, &db, &p, &mut crawler, &mut stop), stopper);
    assert!(began.elapsed() < std::time::Duration::from_secs(30), "not the minute");
    let ran = ran.expect("the pass is recorded");
    assert_eq!(ran.source("hosts").map(|s| s.outcome), Some(superx_mod_recon::crawler::Outcome::Stopped));
    assert_eq!((count(&db, "host_crawl").await, count(&db, "host_call").await, count(&db, "crawl").await), (0, 0, 0), "nothing of the stopped crawl");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn objects_take_their_kind_and_key_their_hash_the_sha_256_of_their_sorted_compact_json_the_same_content_again_writes_nothing_and_changed_content_one_row_more_accounts_at_any_depth_are_user_objects_and_host_user_rows_once_an_answer_not_json_is_unread() {
    use sha2::Digest as _;
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step16");
    let (a, git_base) = on_both_hosts(&root);
    git(&a, &["remote", "remove", "up"]);
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    let release = |b: i64| {
        serde_json::json!({ "id": 5, "b": b, "a": [true, null],
            "author": { "login": "octo", "id": 70, "type": "User", "deep": { "nested": { "login": "inner", "id": 71, "type": "Bot" } } } })
    };
    server.on("/repos/acme/widgets/releases?per_page=100", vec![Answer::json(serde_json::json!([release(1)])), Answer::json(serde_json::json!([release(1)])), Answer::json(serde_json::json!([release(2)]))]);
    server.on("/repos/acme/widgets/deployments?per_page=100", vec![Answer::status(200, "not json"), Answer::json(serde_json::json!([]))]);
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]));
    let mut lines = Lines::new(&kernel, "s16").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    let first = pass(&kernel, &db, &p, &mut crawler).await;

    let kept = objects_of(&db, "release").await;
    assert_eq!(kept.len(), 1);
    assert_eq!(int(&kept[0], "key"), Some(5), "its key, the id");
    let compact = r#"{"a":[true,null],"author":{"deep":{"nested":{"id":71,"login":"inner","type":"Bot"}},"id":70,"login":"octo","type":"User"},"b":1,"id":5}"#;
    let wanted: String = sha2::Sha256::digest(compact.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(text(&kept[0], "hash"), Some(wanted), "every object's keys sorted, compact");
    let call = calls(&db).await["/repos/acme/widgets/releases?per_page=100"][0].clone();
    assert_eq!(kept[0].as_object().and_then(|o| o.get("call")), call.as_object().and_then(|o| o.get("id")));
    assert!(time(&kept[0], "at").is_some());
    let users: BTreeMap<i64, (Option<String>, Option<String>)> = rows(&db, "SELECT host_id, login, kind FROM host_user")
        .await
        .iter()
        .map(|u| (int(u, "host_id").expect("an id"), (text(u, "login"), text(u, "kind"))))
        .collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(users, [(9, (some("acme"), some("Organization"))), (70, (some("octo"), some("User"))), (71, (some("inner"), some("Bot")))].into(), "at any depth");
    assert_eq!(objects_of(&db, "user").await.len(), 3);
    // Not JSON: unread, its call failed.
    let hosts = first.source("hosts").expect("the hosts source");
    assert_eq!((hosts.unread, hosts.failed), (1, 1));
    let unread = &calls(&db).await["/repos/acme/widgets/deployments?per_page=100"][0];
    assert!(text(unread, "error").is_some_and(|e| e.contains("not json")), "{unread:?}");

    // The same content again: nothing. Changed: one row more.
    let held = count(&db, "host_object").await;
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "host_object").await, held);
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "host_object").await, held + 1);
    assert_eq!(objects_of(&db, "release").await.len(), 2);
    assert_eq!(count(&db, "host_user").await, 3, "each account once");
    let crawls: Vec<(i64, i64)> = host_crawls(&db)
        .await
        .iter()
        .map(|c| (int(c, "objects_new").expect("new"), int(c, "objects_changed").expect("changed")))
        .collect();
    assert_eq!(crawls[1..], [(0, 0), (0, 1)]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The paths asked under `prefix` from the `from`th request on.
fn asked_under(server: &Server, prefix: &str, from: usize) -> Vec<String> {
    server.paths().into_iter().skip(from).filter(|p| p.starts_with(prefix)).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_github_crawl_asks_every_path_step_17_names_in_its_order_and_no_check_run_or_check_suite_a_pull_request_an_issues_timeline_and_a_runs_jobs_are_asked_again_only_when_their_object_changed_statuses_are_asked_for_the_commits_and_at_the_crawls_step_17_names_an_unprotected_default_branch_is_no_failure_an_unanswered_repository_makes_the_crawl_unreadable() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step17");
    let (a, git_base) = on_both_hosts(&root);
    git(&a, &["remote", "remove", "up"]);
    let first = head(&a);
    write(&a, "c1.txt", "c1\n");
    let c1 = commit(&a, "c1");
    git(&a, &["switch", "-q", "-c", "topic"]);
    write(&a, "c2.txt", "c2\n");
    let c2 = commit(&a, "c2");
    git(&a, &["switch", "-q", "main"]);
    let host = git_base.join("github.com/acme/widgets.git");
    git(&a, &["push", "-q", &host.to_string_lossy(), "main", "topic"]);
    let b = root.join("b");
    repo(&b);
    git(&b, &["remote", "add", "origin", "https://github.com/acme/gone.git"]);

    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    let base = "/repos/acme/widgets";
    let (u1, u2) = ("2026-11-01T10:00:00Z", "2026-11-01T12:00:00Z");
    let pull = |updated: &str| {
        serde_json::json!({ "number": 1, "id": 101, "state": "open", "merged": false, "title": "topic", "updated_at": updated,
            "created_at": u1, "html_url": "https://github.com/acme/widgets/pull/1", "user": { "login": "octo", "id": 70, "type": "User" },
            "head": { "ref": "topic", "sha": c2, "repo": { "full_name": "acme/widgets" } }, "base": { "ref": "main", "sha": c1 }, "commits": 1 })
    };
    let listed = |updated: &str| Answer::json(serde_json::json!([{ "number": 1, "id": 101, "updated_at": updated }]));
    server.on(&format!("{base}/pulls?state=all&sort=created&direction=asc&per_page=100"), vec![listed(u1), listed(u1), listed(u2)]);
    server.on(&format!("{base}/pulls/1"), vec![Answer::json(pull(u1)), Answer::json(pull(u2))]);
    server.on(&format!("{base}/pulls/1/commits?per_page=100"), vec![Answer::json(serde_json::json!([{ "sha": c2 }]))]);
    server.on(&format!("{base}/pulls/1/files?per_page=100"), vec![Answer::json(serde_json::json!([{ "filename": "c2.txt", "status": "added" }]))]);
    for list in ["pulls/1/reviews", "issues/1/timeline", "issues/2/timeline", "deployments/21/statuses"] {
        server.on(&format!("{base}/{list}?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    server.on(
        &format!("{base}/issues?state=all&sort=created&direction=asc&per_page=100"),
        vec![Answer::json(serde_json::json!([
            { "id": 1101, "number": 1, "updated_at": u1, "pull_request": { "url": "x" } },
            { "id": 1202, "number": 2, "updated_at": u1 }
        ]))],
    );
    server.on(
        &format!("{base}/actions/runs?per_page=100"),
        vec![Answer::json(serde_json::json!({ "total_count": 1, "workflow_runs": [{ "id": 11, "name": "ci", "head_sha": c1, "status": "completed", "conclusion": "success" }] }))],
    );
    server.on(&format!("{base}/actions/runs/11/jobs?filter=all&per_page=100"), vec![Answer::json(serde_json::json!({ "total_count": 1, "jobs": [{ "id": 111, "run_id": 11 }] }))]);
    let status = |state: &str| {
        Answer::json(serde_json::json!([{ "id": 41, "state": state, "context": "ci", "url": format!("https://api.github.com/repos/acme/widgets/statuses/{first}") }]))
    };
    server.on(&format!("{base}/commits/{first}/statuses?per_page=100"), vec![status("pending"), status("success")]);
    server.on(&format!("{base}/deployments?per_page=100"), vec![Answer::json(serde_json::json!([{ "id": 21 }]))]);
    server.on(&format!("{base}/rulesets?per_page=100"), vec![Answer::json(serde_json::json!([{ "id": 31, "updated_at": u1 }]))]);
    server.on(&format!("{base}/rulesets/31"), vec![Answer::json(serde_json::json!({ "id": 31, "updated_at": u1 }))]);
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]));
    let mut lines = Lines::new(&kernel, "s17").await;
    lines.worked_in(&a).await;
    lines.worked_in(&b).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut crawler).await;

    let mut heads = [c1.clone(), c2.clone()];
    heads.sort();
    let statuses = |shas: &[&String]| shas.iter().map(|s| format!("{base}/commits/{s}/statuses?per_page=100")).collect::<Vec<_>>();
    let path = |p: &str| format!("{base}{p}");
    let mut wanted: Vec<String> = [
        "",
        "/pulls?state=all&sort=created&direction=asc&per_page=100",
        "/pulls/1",
        "/pulls/1/commits?per_page=100",
        "/pulls/1/files?per_page=100",
        "/pulls/1/reviews?per_page=100",
        "/issues/1/timeline?per_page=100",
        "/issues?state=all&sort=created&direction=asc&per_page=100",
        "/issues/2/timeline?per_page=100",
        "/issues/comments?sort=created&direction=asc&per_page=100",
        "/pulls/comments?sort=created&direction=asc&per_page=100",
        "/issues/events?per_page=100",
        "/actions/runs?per_page=100",
        "/actions/runs/11/jobs?filter=all&per_page=100",
    ]
    .iter()
    .map(|p| path(p))
    .collect();
    wanted.extend(statuses(&[&heads[0], &heads[1], &first]));
    let tail: Vec<String> =
        ["/deployments?per_page=100", "/deployments/21/statuses?per_page=100", "/environments?per_page=100", "/releases?per_page=100", "/rulesets?per_page=100"]
            .iter()
            .map(|p| path(p))
            .collect();
    wanted.extend(tail.clone());
    wanted.extend([path("/rulesets/31"), path("/branches/main/protection")]);
    assert_eq!(asked_under(&server, "/repos/acme/widgets", 0), wanted, "the first crawl, in step 17's order");
    assert!(!server.paths().iter().any(|p| p.contains("check-runs") || p.contains("check-suites")));
    assert_eq!(asked_under(&server, "/repos/acme/gone", 0), ["/repos/acme/gone"], "nothing more asked");
    let outcome = |path: &'static str| {
        let db = &db;
        async move {
            let found = rows_with(db, "SELECT outcome, failed, started_at FROM host_crawl WHERE host_repository.path = $p", "p", Value::String(path.to_string())).await;
            found.into_iter().max_by_key(|c| time(c, "started_at")).map(|c| (text(&c, "outcome"), int(&c, "failed")))
        }
    };
    assert_eq!(outcome("acme/widgets").await, Some((Some("complete".to_string()), Some(0))), "an unprotected default branch is no failure");
    assert_eq!(outcome("acme/gone").await.and_then(|o| o.0).as_deref(), Some("unreadable"));

    // Nothing changed: nothing asked again but the lists, the statuses of
    // the open pull request's head, the tip and the pending one.
    let before = server.paths().len();
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let mut again: Vec<String> = [
        "",
        "/pulls?state=all&sort=created&direction=asc&per_page=100",
        "/issues?state=all&sort=created&direction=asc&per_page=100",
        "/issues/comments?sort=created&direction=asc&per_page=100",
        "/pulls/comments?sort=created&direction=asc&per_page=100",
        "/issues/events?per_page=100",
        "/actions/runs?per_page=100",
    ]
    .iter()
    .map(|p| path(p))
    .collect();
    again.extend(statuses(&[&heads[0], &heads[1], &first]));
    again.extend(tail.clone());
    again.push(path("/branches/main/protection"));
    assert_eq!(asked_under(&server, "/repos/acme/widgets", before), again);

    // The pull request changed: it is asked again whole; the status is no
    // longer pending.
    let before = server.paths().len();
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let mut third: Vec<String> = [
        "",
        "/pulls?state=all&sort=created&direction=asc&per_page=100",
        "/pulls/1",
        "/pulls/1/commits?per_page=100",
        "/pulls/1/files?per_page=100",
        "/pulls/1/reviews?per_page=100",
        "/issues/1/timeline?per_page=100",
        "/issues?state=all&sort=created&direction=asc&per_page=100",
        "/issues/comments?sort=created&direction=asc&per_page=100",
        "/pulls/comments?sort=created&direction=asc&per_page=100",
        "/issues/events?per_page=100",
        "/actions/runs?per_page=100",
    ]
    .iter()
    .map(|p| path(p))
    .collect();
    third.extend(statuses(&[&heads[0], &heads[1]]));
    third.extend(tail);
    third.push(path("/branches/main/protection"));
    assert_eq!(asked_under(&server, "/repos/acme/widgets", before), third);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_bitbucket_crawl_asks_every_path_step_18_names_in_its_order_with_every_pull_request_state_follows_the_diffstats_redirect_keys_statuses_by_commit_and_key_writes_a_uuids_braces_encoded_and_asks_no_issue() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step18");
    let (a, git_base) = on_both_hosts(&root);
    git(&a, &["remote", "remove", "origin"]);
    let first = head(&a);
    write(&a, "c1.txt", "c1\n");
    let c1 = commit(&a, "c1");
    git(&a, &["switch", "-q", "-c", "topic"]);
    write(&a, "c2.txt", "c2\n");
    let c2 = commit(&a, "c2");
    git(&a, &["switch", "-q", "main"]);

    let server = Server::start().await;
    bitbucket_quiet(&server, "ws/repo", bitbucket_repository());
    let base = "/2.0/repositories/ws/repo";
    let all_states = format!("{base}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50");
    server.on(&all_states, vec![page(serde_json::json!([{ "id": 1, "updated_on": "2026-11-01T10:00:00Z" }]))]);
    server.on(
        &format!("{base}/pullrequests/1"),
        vec![Answer::json(serde_json::json!({ "id": 1, "title": "topic", "state": "OPEN", "updated_on": "2026-11-01T10:00:00Z", "created_on": "2026-11-01T09:00:00Z",
            "author": { "uuid": "{u-1}", "type": "user", "nickname": "nick" },
            "source": { "branch": { "name": "topic" }, "commit": { "hash": c2 }, "repository": { "full_name": "ws/repo" } },
            "destination": { "branch": { "name": "main" }, "commit": { "hash": c1 } },
            "links": { "html": { "href": "https://bitbucket.org/ws/repo/pull-requests/1" } }, "comment_count": 0 }))],
    );
    server.on(&format!("{base}/pullrequests/1/commits?pagelen=50"), vec![page(serde_json::json!([{ "hash": c2 }]))]);
    let moved = format!("{base}/diffstat/ws/repo:{c2}%0D{c1}?from_pullrequest_id=1&pagelen=50");
    server.on(&format!("{base}/pullrequests/1/diffstat?pagelen=50"), vec![Answer::status(302, "").header("location", &moved)]);
    server.on(&moved, vec![page(serde_json::json!([{ "status": "added", "old": null, "new": { "path": "c2.txt" } }]))]);
    for list in ["comments", "activity", "tasks"] {
        server.on(&format!("{base}/pullrequests/1/{list}?pagelen=50"), vec![page(serde_json::json!([]))]);
    }
    server.on(
        &format!("{base}/commit/{c1}/statuses?pagelen=50"),
        vec![page(serde_json::json!([{ "key": "build", "state": "SUCCESSFUL", "name": "Build", "refname": "main", "url": "https://ci.test/1", "created_on": "2026-11-01T09:30:00Z" }]))],
    );
    server.on(
        &format!("{base}/pipelines?sort=-created_on&pagelen=50"),
        vec![page(serde_json::json!([{ "uuid": "{p-1}", "build_number": 3, "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } },
            "target": { "ref_name": "main", "commit": { "hash": c1 }, "selector": { "pattern": "default" } }, "created_on": "2026-11-01T09:00:00Z" }]))],
    );
    server.on(&format!("{base}/pipelines/%7Bp-1%7D/steps?pagelen=50"), vec![page(serde_json::json!([{ "uuid": "{s-1}", "name": "build" }]))]);
    let mut lines = Lines::new(&kernel, "s18").await;
    lines.worked_in(&a).await;
    pass(&kernel, &db, &params(&[]), &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["bitbucket.org"]))).await;

    let mut heads = [c1.clone(), c2.clone()];
    heads.sort();
    let path = |p: &str| format!("{base}{p}");
    let mut wanted = vec![
        path(""),
        all_states.clone(),
        path("/pullrequests/1"),
        path("/pullrequests/1/commits?pagelen=50"),
        path("/pullrequests/1/diffstat?pagelen=50"),
        moved.clone(),
        path("/pullrequests/1/comments?pagelen=50"),
        path("/pullrequests/1/activity?pagelen=50"),
        path("/pullrequests/1/tasks?pagelen=50"),
    ];
    for sha in [&heads[0], &heads[1], &first] {
        wanted.push(path(&format!("/commit/{sha}/statuses?pagelen=50")));
    }
    for p in ["/pipelines?sort=-created_on&pagelen=50", "/pipelines/%7Bp-1%7D/steps?pagelen=50", "/deployments?pagelen=50", "/environments?pagelen=50", "/branch-restrictions?pagelen=50", "/default-reviewers?pagelen=50"] {
        wanted.push(path(p));
    }
    assert_eq!(server.paths(), wanted, "step 18's order, the diffstat's redirect followed, a uuid's braces encoded");
    assert!(!server.paths().iter().any(|p| p.contains("issues")), "no issue asked");
    let key = |kind: &'static str| {
        let db = &db;
        async move { objects_of(db, kind).await.iter().map(|o| text(o, "key").expect("a text key")).collect::<Vec<_>>() }
    };
    assert_eq!(key("commit_status").await, [format!("{c1}/build")], "by commit and key");
    assert_eq!(key("pull_request_file").await, ["1/c2.txt"]);
    assert_eq!(key("pipeline_step").await, ["{s-1}"]);
    let crawl = host_crawls(&db).await;
    assert_eq!((text(&crawl[0], "outcome").as_deref(), int(&crawl[0], "failed")), (Some("complete"), Some(0)));
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The host_user row of the account `host_id` names on `host`.
async fn user_id(db: &superx_kernel::Db, host: &str, host_id: Value) -> Value {
    let mut vars = superx_kernel::types::Object::new();
    vars.insert("h".to_string(), Value::String(host.to_string()));
    vars.insert("i".to_string(), host_id);
    let found: Vec<Value> = db.query("SELECT VALUE id FROM host_user WHERE host = $h AND host_id = $i").bind(vars).await.expect("q").take(0).expect("rows");
    found.into_iter().next().expect("the account")
}

/// A row's columns, `object` and `id` and `valid_from` and the links aside,
/// as JSON text where they are set.
fn columns(row: &Value) -> BTreeMap<String, String> {
    let Value::Object(o) = row else { unreachable!("a row") };
    o.iter()
        .filter(|(k, v)| !matches!(k.as_str(), "id" | "valid_from" | "object" | "pull_request" | "host_repository") && !matches!(v, Value::None))
        .map(|(k, v)| (k.clone(), format!("{v:?}")))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_github_and_a_bitbucket_pull_request_each_make_their_pull_request_and_pull_request_state_rows_column_by_column_merge_sha_only_once_merged_closed_by_from_the_timeline_on_github_and_a_state_row_only_when_it_changes() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step19a");
    let (a, git_base) = on_both_hosts(&root);
    write(&a, "c1.txt", "c1\n");
    let c1 = commit(&a, "c1");
    let c2 = "2".repeat(40);
    let (merge, trial) = ("a".repeat(40), "f".repeat(40));
    let (t0, t1, t2, t3) = ("2026-11-01T09:00:00Z", "2026-11-01T10:00:00Z", "2026-11-01T11:00:00Z", "2026-11-01T12:00:00Z");
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    bitbucket_quiet(&server, "ws/repo", bitbucket_repository());

    let octo = serde_json::json!({ "login": "octo", "id": 70, "type": "User" });
    let gh_open = serde_json::json!({ "number": 1, "id": 101, "html_url": "https://github.com/acme/widgets/pull/1", "state": "open", "merged": false,
        "merge_commit_sha": trial, "title": "topic", "draft": true, "user": octo, "created_at": t0, "updated_at": t1,
        "head": { "ref": "topic", "sha": c2, "repo": { "full_name": "Acme/Widgets" } }, "base": { "ref": "main", "sha": c1 },
        "additions": 1, "deletions": 0, "changed_files": 1, "commits": 1, "comments": 2, "review_comments": 3 });
    let mut gh_merged = gh_open.clone();
    for (k, v) in [("state", serde_json::json!("closed")), ("merged", serde_json::json!(true)), ("merge_commit_sha", serde_json::json!(merge)), ("draft", serde_json::json!(false)),
        ("merged_at", serde_json::json!(t2)), ("closed_at", serde_json::json!(t2)), ("updated_at", serde_json::json!(t2)), ("merged_by", serde_json::json!({ "login": "merger", "id": 80, "type": "User" }))] {
        gh_merged[k] = v;
    }
    let gh = "/repos/acme/widgets";
    let listed = |updated: &str| Answer::json(serde_json::json!([{ "number": 1, "id": 101, "updated_at": updated }]));
    server.on(&format!("{gh}/pulls?state=all&sort=created&direction=asc&per_page=100"), vec![listed(t1), listed(t2), listed(t3)]);
    server.on(&format!("{gh}/pulls/1"), vec![Answer::json(gh_open), Answer::json(gh_merged)]);
    server.on(&format!("{gh}/pulls/1/commits?per_page=100"), vec![Answer::json(serde_json::json!([{ "sha": c2 }]))]);
    for list in ["pulls/1/files", "pulls/1/reviews"] {
        server.on(&format!("{gh}/{list}?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    let closed = serde_json::json!([{ "id": 9001, "event": "closed", "actor": { "login": "closer", "id": 81, "type": "User" }, "created_at": t2 }]);
    server.on(&format!("{gh}/issues/1/timeline?per_page=100"), vec![Answer::json(serde_json::json!([])), Answer::json(closed)]);

    let nick = serde_json::json!({ "uuid": "{u-1}", "type": "user", "nickname": "nick" });
    let bb_open = serde_json::json!({ "id": 1, "title": "topic", "state": "OPEN", "draft": false, "author": nick, "created_on": t0, "updated_on": t1,
        "source": { "branch": { "name": "topic" }, "commit": { "hash": c2 }, "repository": { "full_name": "Fork/Repo" } },
        "destination": { "branch": { "name": "main" }, "commit": { "hash": c1 } },
        "links": { "html": { "href": "https://bitbucket.org/ws/repo/pull-requests/1" } }, "comment_count": 4, "merge_commit": null, "closed_by": null });
    let mut bb_merged = bb_open.clone();
    for (k, v) in [("state", serde_json::json!("MERGED")), ("merge_commit", serde_json::json!({ "hash": merge })), ("updated_on", serde_json::json!(t2)),
        ("closed_by", serde_json::json!({ "uuid": "{u-2}", "type": "user", "nickname": "boss" })), ("reason", serde_json::json!("done"))] {
        bb_merged[k] = v;
    }
    let bb = "/2.0/repositories/ws/repo";
    let pages = |updated: &str| page(serde_json::json!([{ "id": 1, "updated_on": updated }]));
    server.on(&format!("{bb}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50"), vec![pages(t1), pages(t2), pages(t3)]);
    server.on(&format!("{bb}/pullrequests/1"), vec![Answer::json(bb_open), Answer::json(bb_merged)]);
    server.on(&format!("{bb}/pullrequests/1/commits?pagelen=50"), vec![page(serde_json::json!([{ "hash": c2 }]))]);
    for list in ["diffstat", "comments", "activity", "tasks"] {
        server.on(&format!("{bb}/pullrequests/1/{list}?pagelen=50"), vec![page(serde_json::json!([]))]);
    }
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com", "bitbucket.org"]));
    let mut lines = Lines::new(&kernel, "s19a").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    for _ in 0..3 {
        pass(&kernel, &db, &p, &mut crawler).await;
        lines.pushed(&a).await;
    }

    let pulls = rows(&db, "SELECT *, host_repository.host AS host FROM pull_request").await;
    assert_eq!(pulls.len(), 2, "one each, made the first time");
    let pull = |host: &str| pulls.iter().find(|p| text(p, "host").as_deref() == Some(host)).cloned().expect("a pull request");
    let s = |v: &str| format!("{:?}", Value::String(v.to_string()));
    let n = |v: i64| format!("{:?}", number(v));
    let at = |v: &str| format!("{:?}", Value::Datetime(chrono::DateTime::parse_from_rfc3339(v).expect("a time").with_timezone(&chrono::Utc).into()));
    let link = |v: &Value| format!("{v:?}");
    let (gh_pull, bb_pull) = (pull("github.com"), pull("bitbucket.org"));
    let wanted: BTreeMap<String, String> = [
        ("number", n(1)), ("host_id", n(101)), ("author", link(&user_id(&db, "github.com", number(70)).await)), ("created_at", at(t0)),
        ("source_branch", s("topic")), ("target_branch", s("main")), ("url", s("https://github.com/acme/widgets/pull/1")),
        ("host", s("github.com")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(columns(&gh_pull), wanted, "its own fork is no source_path");
    let wanted: BTreeMap<String, String> = [
        ("number", n(1)), ("host_id", n(1)), ("author", link(&user_id(&db, "bitbucket.org", Value::String("{u-1}".to_string())).await)), ("created_at", at(t0)),
        ("source_branch", s("topic")), ("target_branch", s("main")), ("source_path", s("fork/repo")), ("url", s("https://bitbucket.org/ws/repo/pull-requests/1")),
        ("host", s("bitbucket.org")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(columns(&bb_pull), wanted);

    let states = |pr: &Value| {
        let db = &db;
        let id = pr.as_object().and_then(|o| o.get("id")).cloned().expect("an id");
        async move {
            let mut all = rows_with(db, "SELECT * FROM pull_request_state WHERE pull_request = $p", "p", id).await;
            all.sort_by_key(|r| time(r, "valid_from"));
            all.iter().map(columns).collect::<Vec<_>>()
        }
    };
    let common = |rows: &[(&str, String)]| rows.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect::<BTreeMap<_, _>>();
    let gh_states = states(&gh_pull).await;
    assert_eq!(gh_states.len(), 2, "a state row only when it changes");
    assert_eq!(
        gh_states[0],
        common(&[("state", s("open")), ("merged", format!("{:?}", Value::Bool(false))), ("draft", format!("{:?}", Value::Bool(true))), ("title", s("topic")),
            ("head_sha", s(&c2)), ("base_sha", s(&c1)), ("additions", n(1)), ("deletions", n(0)), ("changed_files", n(1)), ("commits", n(1)),
            ("comments", n(2)), ("review_comments", n(3)), ("updated_at", at(t1))]),
        "a trial merge is no merge_sha"
    );
    assert_eq!(
        gh_states[1],
        common(&[("state", s("closed")), ("merged", format!("{:?}", Value::Bool(true))), ("draft", format!("{:?}", Value::Bool(false))), ("title", s("topic")),
            ("head_sha", s(&c2)), ("base_sha", s(&c1)), ("merge_sha", s(&merge)), ("merged_at", at(t2)),
            ("merged_by", link(&user_id(&db, "github.com", number(80)).await)), ("closed_at", at(t2)),
            ("closed_by", link(&user_id(&db, "github.com", number(81)).await)), ("additions", n(1)), ("deletions", n(0)), ("changed_files", n(1)),
            ("commits", n(1)), ("comments", n(2)), ("review_comments", n(3)), ("updated_at", at(t2))]),
        "closed_by, the actor of the timeline's closed event"
    );
    let bb_states = states(&bb_pull).await;
    assert_eq!(bb_states.len(), 2);
    assert_eq!(
        bb_states[0],
        common(&[("state", s("OPEN")), ("merged", format!("{:?}", Value::Bool(false))), ("draft", format!("{:?}", Value::Bool(false))), ("title", s("topic")),
            ("head_sha", s(&c2)), ("base_sha", s(&c1)), ("comments", n(4)), ("updated_at", at(t1))])
    );
    let boss = link(&user_id(&db, "bitbucket.org", Value::String("{u-2}".to_string())).await);
    assert_eq!(
        bb_states[1],
        common(&[("state", s("MERGED")), ("merged", format!("{:?}", Value::Bool(true))), ("draft", format!("{:?}", Value::Bool(false))), ("title", s("topic")),
            ("head_sha", s(&c2)), ("base_sha", s(&c1)), ("merge_sha", s(&merge)), ("merged_by", boss.clone()), ("closed_by", boss), ("reason", s("done")),
            ("comments", n(4)), ("updated_at", at(t2))])
    );
    assert_eq!(server.paths().iter().filter(|p| *p == "/repos/acme/widgets/pulls/1").count(), 3, "read a third time, unchanged");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A pull_request_commit row: when, its hash, present, its position, and
/// whether it names its object.
type Listed = (chrono::DateTime<chrono::Utc>, String, bool, Option<i64>, bool);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_leaving_a_pull_requests_list_and_coming_back_is_present_then_absent_then_present_again_with_its_position() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step19b");
    let (a, git_base) = on_both_hosts(&root);
    git(&a, &["remote", "remove", "up"]);
    let (c1, c2) = ("1".repeat(40), "2".repeat(40));
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    let gh = "/repos/acme/widgets";
    let pull = |commits: i64| {
        Answer::json(serde_json::json!({ "number": 1, "id": 101, "html_url": "https://github.com/acme/widgets/pull/1", "state": "open", "merged": false,
            "title": "topic", "user": { "login": "octo", "id": 70, "type": "User" }, "created_at": "2026-11-01T09:00:00Z", "updated_at": "2026-11-01T10:00:00Z",
            "head": { "ref": "topic", "sha": c2 }, "base": { "ref": "main", "sha": c1 }, "commits": commits }))
    };
    let listed = |updated: &str| Answer::json(serde_json::json!([{ "number": 1, "id": 101, "updated_at": updated }]));
    server.on(
        &format!("{gh}/pulls?state=all&sort=created&direction=asc&per_page=100"),
        vec![listed("2026-11-01T10:00:00Z"), listed("2026-11-01T11:00:00Z"), listed("2026-11-01T12:00:00Z")],
    );
    server.on(&format!("{gh}/pulls/1"), vec![pull(2), pull(1), pull(2)]);
    let both = Answer::json(serde_json::json!([{ "sha": c1 }, { "sha": c2 }]));
    server.on(&format!("{gh}/pulls/1/commits?per_page=100"), vec![both.clone(), Answer::json(serde_json::json!([{ "sha": c1 }])), both]);
    for list in ["pulls/1/files", "pulls/1/reviews", "issues/1/timeline"] {
        server.on(&format!("{gh}/{list}?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]));
    let mut lines = Lines::new(&kernel, "s19b").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    for _ in 0..3 {
        pass(&kernel, &db, &p, &mut crawler).await;
        lines.pushed(&a).await;
    }
    let mut listed: Vec<Listed> = rows(&db, "SELECT * FROM pull_request_commit")
        .await
        .iter()
        .map(|r| (time(r, "valid_from").expect("a time"), text(r, "hash").expect("a hash"), flag(r, "present").expect("present"), int(r, "position"), link(r, "object").is_some()))
        .collect();
    listed.sort();
    let seen: Vec<(String, bool, Option<i64>, bool)> = listed.into_iter().map(|(_, h, p, n, o)| (h, p, n, o)).collect();
    assert_eq!(
        seen,
        [(c1.clone(), true, Some(0), true), (c2.clone(), true, Some(1), true), (c2.clone(), false, None, false), (c2.clone(), true, Some(1), true)],
        "present, then absent, then present again, with its position"
    );
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A ci_run as the test reads it: host, kind, host_id, name, commit_sha,
/// ref, its parent's kind, its pull request's host, and whether it has a
/// created_at.
type Run = (String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, bool);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_kind_of_ci_object_makes_its_ci_run_with_its_parent_and_pull_request_and_a_ci_run_state_only_when_its_status_conclusion_times_attempt_number_actor_or_runner_change() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step20");
    let (a, git_base) = on_both_hosts(&root);
    write(&a, "c1.txt", "c1\n");
    let c1 = commit(&a, "c1");
    let t0 = "2026-11-01T09:00:00Z";
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    bitbucket_quiet(&server, "ws/repo", bitbucket_repository());
    let octo = serde_json::json!({ "login": "octo", "id": 70, "type": "User" });

    // GitHub: a pull request, a run of it with a job, a status of the tip.
    let gh = "/repos/acme/widgets";
    server.on(&format!("{gh}/pulls?state=all&sort=created&direction=asc&per_page=100"), vec![Answer::json(serde_json::json!([{ "number": 1, "id": 101, "updated_at": t0 }]))]);
    server.on(
        &format!("{gh}/pulls/1"),
        vec![Answer::json(serde_json::json!({ "number": 1, "id": 101, "html_url": "https://github.com/acme/widgets/pull/1", "state": "open", "merged": false,
            "title": "topic", "user": octo, "created_at": t0, "updated_at": t0, "head": { "ref": "topic", "sha": c1 }, "base": { "ref": "main", "sha": c1 } }))],
    );
    for list in ["pulls/1/commits", "pulls/1/files", "pulls/1/reviews", "issues/1/timeline"] {
        server.on(&format!("{gh}/{list}?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    let run = |status: &str, conclusion: Option<&str>| {
        Answer::json(serde_json::json!({ "total_count": 1, "workflow_runs": [{ "id": 11, "name": "ci", "head_sha": c1, "head_branch": "main", "status": status,
            "conclusion": conclusion, "html_url": "https://github.com/acme/widgets/actions/runs/11", "created_at": t0, "run_started_at": t0,
            "run_attempt": 1, "run_number": 3, "triggering_actor": octo, "pull_requests": [{ "number": 1 }] }] }))
    };
    server.on(&format!("{gh}/actions/runs?per_page=100"), vec![run("in_progress", None), run("in_progress", None), run("completed", Some("success"))]);
    server.on(
        &format!("{gh}/actions/runs/11/jobs?filter=all&per_page=100"),
        vec![Answer::json(serde_json::json!({ "total_count": 1, "jobs": [{ "id": 111, "run_id": 11, "name": "build", "head_sha": c1, "head_branch": "main",
            "status": "completed", "conclusion": "success", "html_url": "https://github.com/acme/widgets/actions/runs/11/job/111", "created_at": t0,
            "started_at": t0, "completed_at": t0, "run_attempt": 1, "runner_name": "runner-1" }] }))],
    );
    server.on(
        &format!("{gh}/commits/{c1}/statuses?per_page=100"),
        vec![Answer::json(serde_json::json!([{ "id": 41, "state": "success", "context": "ci/x", "target_url": "https://ci.test/x", "created_at": t0,
            "url": format!("https://api.github.com/repos/acme/widgets/statuses/{c1}") }]))],
    );

    // Bitbucket: a pull request, a pipeline of it with a step, a status.
    let bb = "/2.0/repositories/ws/repo";
    server.on(&format!("{bb}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50"), vec![page(serde_json::json!([{ "id": 1, "updated_on": t0 }]))]);
    server.on(
        &format!("{bb}/pullrequests/1"),
        vec![Answer::json(serde_json::json!({ "id": 1, "title": "topic", "state": "OPEN", "created_on": t0, "updated_on": t0,
            "source": { "branch": { "name": "topic" }, "commit": { "hash": c1 } }, "destination": { "branch": { "name": "main" }, "commit": { "hash": c1 } },
            "links": { "html": { "href": "https://bitbucket.org/ws/repo/pull-requests/1" } } }))],
    );
    for list in ["commits", "diffstat", "comments", "activity", "tasks"] {
        server.on(&format!("{bb}/pullrequests/1/{list}?pagelen=50"), vec![page(serde_json::json!([]))]);
    }
    let pipeline = |state: serde_json::Value| {
        page(serde_json::json!([{ "uuid": "{p-1}", "build_number": 7, "state": state, "created_on": t0, "creator": { "uuid": "{u-1}", "type": "user", "nickname": "nick" },
            "target": { "ref_name": "topic", "commit": { "hash": c1 }, "selector": { "pattern": "pr" }, "pullrequest": { "id": 1 } } }]))
    };
    let running = serde_json::json!({ "name": "IN_PROGRESS" });
    let done = serde_json::json!({ "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } });
    server.on(&format!("{bb}/pipelines?sort=-created_on&pagelen=50"), vec![pipeline(running.clone()), pipeline(running), pipeline(done)]);
    server.on(
        &format!("{bb}/pipelines/%7Bp-1%7D/steps?pagelen=50"),
        vec![page(serde_json::json!([{ "uuid": "{s-1}", "name": "build", "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } }, "started_on": t0, "completed_on": t0 }]))],
    );
    server.on(
        &format!("{bb}/commit/{c1}/statuses?pagelen=50"),
        vec![page(serde_json::json!([{ "key": "build", "state": "SUCCESSFUL", "name": "Build", "refname": "main", "url": "https://ci.test/b", "created_on": t0 }]))],
    );
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com", "bitbucket.org"]));
    let mut lines = Lines::new(&kernel, "s20").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut crawler).await;

    let runs = rows(&db, "SELECT *, host_repository.host AS host, parent.kind AS parent_kind, pull_request.number AS pull, pull_request.host_repository.host AS pull_host FROM ci_run").await;
    let mut got: Vec<Run> = runs
        .iter()
        .map(|r| {
            let id = match r.as_object().and_then(|o| o.get("host_id")) {
                Some(Value::String(s)) => s.clone(),
                Some(v) => format!("{v:?}"),
                None => String::new(),
            };
            (text(r, "host").expect("host"), text(r, "kind").expect("kind"), id, text(r, "name"), text(r, "commit_sha"), text(r, "ref"), text(r, "parent_kind"), text(r, "pull_host"), time(r, "created_at").is_some())
        })
        .collect();
    got.sort();
    let s = |v: &str| Some(v.to_string());
    let n = |v: i64| format!("{:?}", number(v));
    let mut wanted = vec![
        ("bitbucket.org".to_string(), "commit_status".to_string(), format!("{c1}/build"), s("Build"), s(&c1), s("main"), None, None, true),
        ("bitbucket.org".to_string(), "pipeline".to_string(), "{p-1}".to_string(), s("pr"), s(&c1), s("topic"), None, s("bitbucket.org"), true),
        ("bitbucket.org".to_string(), "pipeline_step".to_string(), "{s-1}".to_string(), s("build"), s(&c1), s("topic"), s("pipeline"), None, false),
        ("github.com".to_string(), "commit_status".to_string(), n(41), s("ci/x"), s(&c1), None, None, None, true),
        ("github.com".to_string(), "workflow_job".to_string(), n(111), s("build"), s(&c1), s("main"), s("workflow_run"), None, true),
        ("github.com".to_string(), "workflow_run".to_string(), n(11), s("ci"), s(&c1), s("main"), None, s("github.com"), true),
    ];
    wanted.sort();
    assert_eq!(got, wanted, "each kind with its parent and pull request");
    assert_eq!(count(&db, "ci_run_state").await, 6);

    // Read again unchanged: no state row. A run and a pipeline finishing:
    // one each.
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!((count(&db, "ci_run").await, count(&db, "ci_run_state").await), (6, 6));
    lines.pushed(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!((count(&db, "ci_run").await, count(&db, "ci_run_state").await), (6, 8));
    let history = |kind: &'static str| {
        let db = &db;
        async move {
            let mut all = rows_with(db, "SELECT status, conclusion, valid_from FROM ci_run_state WHERE ci_run.kind = $k", "k", Value::String(kind.to_string())).await;
            all.sort_by_key(|r| time(r, "valid_from"));
            all.iter().map(|r| (text(r, "status"), text(r, "conclusion"))).collect::<Vec<_>>()
        }
    };
    assert_eq!(history("workflow_run").await, [(s("in_progress"), None), (s("completed"), s("success"))]);
    assert_eq!(history("pipeline").await, [(s("IN_PROGRESS"), None), (s("COMPLETED"), s("SUCCESSFUL"))]);
    let job = rows(&db, "SELECT * FROM ci_run_state WHERE ci_run.kind = 'workflow_job'").await;
    assert_eq!((text(&job[0], "runner").as_deref(), int(&job[0], "attempt")), (Some("runner-1"), Some(1)));
    let gh_run = rows(&db, "SELECT * FROM ci_run_state WHERE ci_run.kind = 'workflow_run'").await;
    assert!(gh_run.iter().all(|r| int(r, "number") == Some(3) && link(r, "actor").is_some() && time(r, "started_at").is_some()));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_crawl_records_its_calls_waits_objects_and_rows_complete_partial_on_a_failed_call_or_a_stated_total_read_short_unreadable_or_no_credentials_with_nothing_asked_the_hosts_sources_pass_source_holds_its_counts_and_stays_complete_for_an_unreadable_or_uncredentialed_host() {
    let kernel = kernel().await;
    let root = scratch("step21");
    let git_base = root.join("hosts");
    let server = Server::start().await;
    let mut lines = Lines::new(&kernel, "s21").await;
    let mut made = BTreeMap::new();
    for (name, url, path, answered) in [
        ("w", "https://github.com/acme/widgets.git", "acme/widgets", true),
        ("s", "https://github.com/acme/short.git", "acme/short", true),
        ("f", "https://github.com/acme/failing.git", "acme/failing", true),
        ("g", "https://github.com/acme/gone.git", "acme/gone", false),
        ("u", "https://bitbucket.org/ws/repo.git", "ws/repo", false),
    ] {
        let dir = root.join(name);
        repo(&dir);
        git(&dir, &["remote", "add", "origin", url]);
        if answered {
            hosted(&dir, &git_base, "github.com", path);
            let object = if path == "acme/widgets" { github_repository() } else { serde_json::json!({ "id": 100 + made.len(), "default_branch": "main" }) };
            github_quiet(&server, path, object);
        }
        lines.worked_in(&dir).await;
        made.insert(name, dir);
    }
    server.on(
        "/repos/acme/short/environments?per_page=100",
        vec![Answer::json(serde_json::json!({ "total_count": 2, "environments": [{ "id": 1, "name": "prod" }] }))],
    );
    server.on("/repos/acme/failing/releases?per_page=100", vec![Answer::status(500, "{}")]);
    let p = params(&[]);
    let db = own_db().await;
    let ran = pass(&kernel, &db, &p, &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]))).await;

    let crawl_of = |path: &'static str| {
        let db = &db;
        async move {
            let found = rows_with(db, "SELECT *, id AS me FROM host_crawl WHERE host_repository.path = $p", "p", Value::String(path.to_string())).await;
            assert_eq!(found.len(), 1, "{path}");
            found.into_iter().next().expect("its host crawl")
        }
    };
    let asked = |prefix: &str| i64::try_from(server.paths().iter().filter(|p| p.starts_with(prefix)).count()).expect("a count");
    let w = crawl_of("acme/widgets").await;
    assert_eq!((text(&w, "outcome").as_deref(), text(&w, "note")), (Some("complete"), None));
    assert_eq!(int(&w, "calls"), Some(asked("/repos/acme/widgets")));
    let zeros = ["not_modified", "failed", "rate_waits", "waited_ms", "objects_changed"].map(|n| int(&w, n));
    assert_eq!(zeros, [Some(0); 5]);
    let objects = rows(&db, "SELECT VALUE id FROM host_object WHERE host_repository.path = 'acme/widgets'").await.len();
    assert_eq!(int(&w, "objects_new"), Some(i64::try_from(objects).expect("a count")), "the repository and its owner");
    let calls_of_w = rows_with(&db, "SELECT VALUE id FROM host_call WHERE host_crawl = $c", "c", w.as_object().and_then(|o| o.get("me")).cloned().expect("an id")).await.len();
    let accounts = rows(&db, "SELECT VALUE id FROM host_user").await.len();
    assert_eq!(accounts, 1, "only acme/widgets names an account");
    assert_eq!(int(&w, "rows_written"), Some(i64::try_from(objects + calls_of_w + accounts + 2).expect("a count")), "its objects, calls and account, the host repository and itself");

    let s = crawl_of("acme/short").await;
    assert_eq!((text(&s, "outcome").as_deref(), text(&s, "note").as_deref()), (Some("partial"), Some("1 objects read of 2 stated")));
    let f = crawl_of("acme/failing").await;
    assert_eq!((text(&f, "outcome").as_deref(), text(&f, "note").as_deref(), int(&f, "failed")), (Some("partial"), Some("/repos/acme/failing/releases?per_page=100: HTTP 500"), Some(1)));
    let g = crawl_of("acme/gone").await;
    assert_eq!((text(&g, "outcome").as_deref(), text(&g, "note").as_deref(), int(&g, "calls")), (Some("unreadable"), Some("/repos/acme/gone: HTTP 404"), Some(1)));
    let u = crawl_of("ws/repo").await;
    assert_eq!((text(&u, "outcome").as_deref(), text(&u, "note").as_deref(), int(&u, "calls")), (Some("no_credentials"), Some("no token"), Some(0)));
    assert_eq!(asked("/2.0/"), 0, "nothing asked");

    let hosts = ran.source("hosts").expect("the hosts source");
    assert_eq!((hosts.items, hosts.outcome), (5, superx_mod_recon::crawler::Outcome::Partial));
    assert_eq!(hosts.calls, asked("/repos/"));
    assert_eq!(hosts.failed, 2, "the release and the repository gone");
    let counts: BTreeMap<&str, i64> = hosts.counts.iter().map(|(k, v)| (*k, *v)).collect();
    let new_objects = i64::try_from(rows(&db, "SELECT VALUE id FROM host_object").await.len()).expect("a count");
    assert_eq!(
        counts,
        [("objects_new", new_objects), ("objects_changed", 0), ("not_modified", 0), ("rate_waits", 0), ("waited_ms", 0), ("totals_stated", 2), ("totals_read", 1)].into()
    );
    let note = hosts.note.clone().unwrap_or_default();
    for part in [
        "github.com/acme/short partial: 1 objects read of 2 stated",
        "github.com/acme/failing partial: /repos/acme/failing/releases?per_page=100: HTTP 500",
        "github.com/acme/gone unreadable: /repos/acme/gone: HTTP 404",
        "bitbucket.org/ws/repo no_credentials: no token",
    ] {
        assert!(note.contains(part), "{part} in {note}");
    }
    let stored = rows(&db, "SELECT items, outcome, calls FROM pass_source WHERE source = 'hosts'").await;
    assert_eq!((int(&stored[0], "items"), text(&stored[0], "outcome").as_deref()), (Some(5), Some("partial")));

    // An unreadable host and one without credentials leave the source
    // complete.
    let db = own_db().await;
    for name in ["w", "s", "f"] {
        git(&made[name], &["remote", "remove", "origin"]);
    }
    let ran = pass(&kernel, &db, &p, &mut Crawler::new(places(&root.join("home2"), endpoints(&server, &git_base), &["github.com"]))).await;
    let hosts = ran.source("hosts").expect("the hosts source");
    assert_eq!((hosts.items, hosts.outcome), (2, superx_mod_recon::crawler::Outcome::Complete));
    let note = hosts.note.clone().unwrap_or_default();
    assert!(note.contains("github.com/acme/gone unreadable") && note.contains("bitbucket.org/ws/repo no_credentials"), "{note}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_records_lines_repos_and_hosts_and_a_stop_mid_crawl_records_it_stopped_and_writes_nothing_of_the_stopped_crawl() {
    let kernel = kernel().await;
    let root = scratch("step22");
    let (a, git_base) = on_both_hosts(&root);
    git(&a, &["remote", "remove", "up"]);
    let mut lines = Lines::new(&kernel, "s22").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    let sources = |db: &superx_kernel::Db| {
        let db = db.clone();
        async move {
            let mut all: Vec<(String, String)> = rows(&db, "SELECT source, outcome FROM pass_source")
                .await
                .iter()
                .map(|r| (text(r, "source").expect("source"), text(r, "outcome").expect("outcome")))
                .collect();
            all.sort();
            all
        }
    };
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    let db = own_db().await;
    pass(&kernel, &db, &p, &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]))).await;
    let complete = |s: &str| (s.to_string(), "complete".to_string());
    assert_eq!(sources(&db).await, [complete("hosts"), complete("lines"), complete("repos")]);

    // Stopped while a host takes its time to answer.
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", github_repository());
    let releases = "/repos/acme/widgets/releases?per_page=100";
    server.on(releases, vec![Answer::json(serde_json::json!([])).late(std::time::Duration::from_secs(1))]); // skill-allow: §9-duration — test fixture
    let db = own_db().await;
    let (asked, mut stop) = never_stop();
    let mut crawler = Crawler::new(places(&root.join("home2"), endpoints(&server, &git_base), &["github.com"]));
    let stopper = async {
        while !server.paths().iter().any(|path| path == releases) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // skill-allow: §9-duration — test fixture
        }
        asked.send(true).expect("asked to stop");
    };
    let (ran, ()) = tokio::join!(superx_mod_recon::crawler::pass_on(&kernel, &db, &p, &mut crawler, &mut stop), stopper);
    let ran = ran.expect("the pass is recorded");
    let stopped = |s: &str| (s.to_string(), "stopped".to_string());
    assert_eq!(sources(&db).await, [stopped("hosts"), complete("lines"), stopped("repos")]);
    assert_eq!(ran.source("repos").map(|s| s.outcome), Some(superx_mod_recon::crawler::Outcome::Stopped));
    for table in ["crawl", "git_call", "commit", "commit_file", "ref_move", "placement", "survival", "remote_state", "mirror", "host_repository", "host_crawl", "host_call", "host_object", "host_user"] {
        assert_eq!(count(&db, table).await, 0, "nothing of the stopped crawl in {table}");
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}
