//! The hosts crawled beside the repositories (#538, steps 15 to 22), end to
//! end: a repository whose remotes name GitHub and Bitbucket, hosts of the
//! test's own on the loopback that record every request, the mirror fetched
//! from a bare repository of the test's own, and the module's own database
//! on the in-memory engine. One test per item of the task's list, named by
//! the item it proves.

pub mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::*;
use superx_kernel::types::Value;
use superx_kernel::Kernel;
use superx_mod_recon::crawler::{pass_on, Crawler, Outcome, Pass, Places};
use superx_mod_recon::params::{CRAWL_SECS_PARAM, INCLUDE_PARAM};

const GITHUB: &str = "/repos/acme/widget";
const BITBUCKET: &str = "/repositories/team/repo";

/// A repository on GitHub, acme/widget, or on Bitbucket, team/repo, with
/// its host answering the repository and its mirror's source in place.
struct World {
    root: PathBuf,
    kernel: Kernel,
    db: superx_kernel::Db,
    dir: PathBuf,
    github: Fake,
    bitbucket: Fake,
    places: Places,
    /// Its commits on `main`, oldest first.
    commits: Vec<String>,
}

impl World {
    async fn new(host: &str) -> Self {
        let root = scratch();
        let dir = root.join("work");
        repo(&dir);
        let commits = vec![commit(&dir, &[("a.txt", "a\n")], "first"), commit(&dir, &[("a.txt", "a\nb\n")], "second")];
        let (github, bitbucket) = (Fake::start("").await, Fake::start("/2.0").await);
        match host {
            "github.com" => {
                git(&dir, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
                hosted(&root, "github.com", "acme/widget", &dir);
                github.json(GITHUB, &github_repository(77, "main"));
            }
            _ => {
                git(&dir, &["remote", "add", "origin", "https://bitbucket.org/team/repo.git"]);
                hosted(&root, "bitbucket.org", "team/repo", &dir);
                bitbucket.json(BITBUCKET, &bitbucket_repository());
            }
        }
        let places = places(&root, Some(credentials(&root, Some(TOKEN), Some(BB_TOKEN))), &github, &bitbucket);
        Self { root, kernel: kernel().await, db: own_db().await, dir, github, bitbucket, places, commits }
    }

    /// Its repository, every pass a full one a second after the last.
    fn params(&self) -> superx_mod_recon::params::Params {
        params(&[(INCLUDE_PARAM, strings(&[&self.dir])), (CRAWL_SECS_PARAM, number(1))])
    }

    async fn pass(&self, crawler: &mut Crawler) -> Pass {
        pass(&self.kernel, &self.db, &self.params(), crawler).await
    }

    /// Another pass by the same crawler, once a full pass is due.
    async fn again(&self, crawler: &mut Crawler) -> Pass {
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        self.pass(crawler).await
    }

    fn crawler(&self) -> Crawler {
        Crawler::new(self.places.clone())
    }

    /// The newest host crawl.
    async fn host_crawl(&self) -> Value {
        rows(&self.db, "SELECT *, valid_from FROM host_crawl ORDER BY valid_from DESC LIMIT 1").await.into_iter().next().expect("a host crawl")
    }

    fn done(self) {
        std::fs::remove_dir_all(&self.root).expect("clean up");
    }
}

fn bitbucket_repository() -> serde_json::Value {
    serde_json::json!({
        "uuid": "{b0b5e7a1-0000-4000-8000-000000000001}", "full_name": "team/repo", "mainbranch": { "name": "main" },
        "owner": { "uuid": "{team-0001}", "type": "team", "display_name": "Team", "nickname": "team" }
    })
}

/// Every row of every table, written out.
async fn every_row(db: &superx_kernel::Db) -> String {
    let mut out = String::new();
    for table in superx_mod_recon::status::fact_tables().chain(["pass", "pass_source"]) {
        for r in rows(db, &format!("SELECT * FROM {table}")).await {
            out.push_str(&written(&r));
        }
    }
    out
}

/// What the crawler logs, every field of every event, while it is set.
#[derive(Clone, Default)]
struct Logged(Arc<Mutex<Vec<String>>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Logged {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Fields(&mut line));
        self.0.lock().expect("log").push(line);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

impl Logged {
    fn lines(&self) -> Vec<String> {
        self.0.lock().expect("log").clone()
    }
}

#[tokio::test]
async fn every_host_call_is_a_get_with_the_hosts_headers_and_bearer_token_sent_only_to_its_host_the_token_never_in_a_path_a_query_a_row_or_a_log_line_and_is_a_host_call_row_with_each_column()
{
    let logged = Logged::default();
    let _logging = tracing::subscriber::set_default(logged.clone());
    let world = World::new("github.com").await;
    let reset = chrono::Utc::now().timestamp() + 3600;
    world.github.on(
        GITHUB,
        vec![Answer::json(&github_repository(77, "main"))
            .with("etag", "\"r1\"")
            .with("x-ratelimit-limit", "5000")
            .with("x-ratelimit-remaining", "4999")
            .with("x-ratelimit-reset", &reset.to_string())],
    );
    // A second repository on Bitbucket, crawled in the same pass.
    let bb = world.root.join("bb");
    repo(&bb);
    commit(&bb, &[("a.txt", "a\n")], "first");
    git(&bb, &["remote", "add", "origin", "https://bitbucket.org/team/repo.git"]);
    hosted(&world.root, "bitbucket.org", "team/repo", &bb);
    world.bitbucket.json(BITBUCKET, &bitbucket_repository());
    let p = params(&[(INCLUDE_PARAM, strings(&[&world.dir, &bb]))]);
    let done = pass(&world.kernel, &world.db, &p, &mut world.crawler()).await;
    superx_mod_recon::crawler::log(Ok(done.clone()));

    let (github, bitbucket) = (world.github.seen(), world.bitbucket.seen());
    assert!(github.len() > 10 && bitbucket.len() > 5, "{github:?} {bitbucket:?}");
    for s in &github {
        assert_eq!(s.method, "GET");
        assert_eq!(
            (s.header("accept"), s.header("x-github-api-version"), s.header("user-agent"), s.header("authorization")),
            (Some("application/vnd.github+json"), Some("2022-11-28"), Some("superx-recon"), Some(format!("Bearer {TOKEN}").as_str())),
            "{s:?}"
        );
        assert!(!s.path.contains(TOKEN) && !s.path.contains(BB_TOKEN), "never in a path or a query");
    }
    for s in &bitbucket {
        assert_eq!(s.method, "GET");
        assert_eq!(
            (s.header("accept"), s.header("x-github-api-version"), s.header("user-agent"), s.header("authorization")),
            (Some("application/json"), None, Some("superx-recon"), Some(format!("Bearer {BB_TOKEN}").as_str())),
            "{s:?}"
        );
        assert!(!s.path.contains(TOKEN) && !s.path.contains(BB_TOKEN));
    }
    let all = every_row(&world.db).await;
    assert!(!all.contains(TOKEN) && !all.contains(BB_TOKEN), "never in a row");
    let lines = logged.lines();
    assert!(lines.iter().any(|l| l.contains("host_calls")), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains(TOKEN) && !l.contains(BB_TOKEN)), "never in a log line: {lines:?}");

    let call = rows(&world.db, &format!("SELECT * FROM host_call WHERE path = '{GITHUB}'")).await;
    let call = &call[0];
    assert!(matches!(field(call, "host_crawl"), Some(Value::RecordId(_))));
    assert_eq!((text(call, "method").as_deref(), text(call, "path").as_deref()), (Some("GET"), Some(GITHUB)));
    assert!(field(call, "started_at").is_some() && int(call, "ms").is_some());
    assert_eq!((int(call, "status"), flag(call, "not_modified"), text(call, "etag").as_deref()), (Some(200), Some(false), Some("\"r1\"")));
    assert_eq!(int(call, "bytes"), Some(i64::try_from(github_repository(77, "main").to_string().len()).expect("a size")));
    assert_eq!((int(call, "rate_limit"), int(call, "rate_remaining")), (Some(5000), Some(4999)));
    assert!(matches!(field(call, "rate_reset"), Some(Value::Datetime(_))));
    assert_eq!((int(call, "retry_after_ms"), int(call, "objects"), flag(call, "next"), text(call, "error")), (None, Some(1), Some(false), None));
    let missing = rows(&world.db, &format!("SELECT * FROM host_call WHERE path = '{GITHUB}/releases?per_page=100'")).await;
    assert_eq!((int(&missing[0], "status"), int(&missing[0], "objects")), (Some(404), Some(0)));
    assert_eq!(text(&missing[0], "error").as_deref(), Some(r#"HTTP 404: {"message":"Not Found"}"#));
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_list_is_paged_by_githubs_link_rel_next_and_bitbuckets_next_as_given_asking_a_hundred_and_fifty_a_page_a_redirect_within_the_host_is_followed_with_the_token_and_one_to_another_host_is_not_followed_and_fails()
{
    let world = World::new("github.com").await;
    let elsewhere = Fake::start("").await;
    let issue = |n: i64| serde_json::json!({ "id": 1000 + n, "number": n, "title": format!("issue {n}"), "user": github_user("dev", 11) });
    world.github.on(
        &format!("{GITHUB}/issues?state=all&sort=created&direction=asc&per_page=100"),
        vec![Answer::json(&serde_json::json!([issue(1)])).with("link", &format!("<{}{GITHUB}/issues?state=all&page=2&per_page=100>; rel=\"next\", <{}{GITHUB}/issues?page=2>; rel=\"last\"", world.github.url, world.github.url))],
    );
    world.github.json(&format!("{GITHUB}/issues?state=all&page=2&per_page=100"), &serde_json::json!([issue(2)]));
    world.github.on(&format!("{GITHUB}/releases?per_page=100"), vec![Answer::status(302, "").with("location", &format!("{}/stolen", elsewhere.url))]);
    world.github.on(&format!("{GITHUB}/environments?per_page=100"), vec![Answer::status(301, "").with("location", &format!("{}{GITHUB}/moved-environments", world.github.url))]);
    world.github.json(&format!("{GITHUB}/moved-environments"), &serde_json::json!({ "total_count": 1, "environments": [{ "id": 31, "name": "prod" }] }));
    // Bitbucket, paged by its answers' next.
    let bb = world.root.join("bb");
    repo(&bb);
    commit(&bb, &[("a.txt", "a\n")], "first");
    git(&bb, &["remote", "add", "origin", "https://bitbucket.org/team/repo.git"]);
    hosted(&world.root, "bitbucket.org", "team/repo", &bb);
    world.bitbucket.json(BITBUCKET, &bitbucket_repository());
    world.bitbucket.json(
        &format!("{BITBUCKET}/deployments?pagelen=50"),
        &serde_json::json!({ "values": [{ "uuid": "{d1}" }], "size": 2, "next": format!("{}{BITBUCKET}/deployments?pagelen=50&page=2", world.bitbucket.url) }),
    );
    world.bitbucket.json(&format!("{BITBUCKET}/deployments?pagelen=50&page=2"), &serde_json::json!({ "values": [{ "uuid": "{d2}" }], "size": 2 }));
    let p = params(&[(INCLUDE_PARAM, strings(&[&world.dir, &bb]))]);
    pass(&world.kernel, &world.db, &p, &mut world.crawler()).await;

    let paths = world.github.paths();
    let at = paths.iter().position(|p| p.starts_with(&format!("{GITHUB}/issues?state=all&sort"))).expect("the first page");
    assert_eq!(paths[at + 1], format!("{GITHUB}/issues?state=all&page=2&per_page=100"), "the next page as given");
    assert!(paths.iter().filter(|p| p.contains("?")).all(|p| !p.contains("per_page") || p.contains("per_page=100")));
    assert_eq!(count_of(&world.db, "issue").await, 2);
    let bb_paths = world.bitbucket.paths();
    assert!(bb_paths.contains(&format!("{BITBUCKET}/deployments?pagelen=50")) && bb_paths.contains(&format!("{BITBUCKET}/deployments?pagelen=50&page=2")), "{bb_paths:?}");
    assert_eq!(count_of(&world.db, "deployment").await, 2);
    // Within the host: followed, the token with it.
    let moved = world.github.seen().into_iter().find(|s| s.path == format!("{GITHUB}/moved-environments")).expect("followed");
    assert_eq!(moved.header("authorization"), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(count_of(&world.db, "environment").await, 1);
    // To another host: not followed, and the call failed.
    assert!(elsewhere.seen().is_empty(), "nothing sent elsewhere");
    let refused = rows(&world.db, &format!("SELECT status, error FROM host_call WHERE path = '{GITHUB}/releases?per_page=100'")).await;
    assert_eq!(int(&refused[0], "status"), Some(302));
    assert!(text(&refused[0], "error").is_some_and(|e| e.starts_with("HTTP 302")));
    let crawls = rows(&world.db, "SELECT outcome, failed, host_repository.host AS host FROM host_crawl").await;
    let github_crawl = crawls.iter().find(|c| text(c, "host").as_deref() == Some("github.com")).expect("github's");
    assert_eq!((text(github_crawl, "outcome").as_deref(), int(github_crawl, "failed")), (Some("partial"), Some(1)));
    world.done();
}

/// The `host_object` rows of a kind.
async fn count_of(db: &superx_kernel::Db, kind: &str) -> i64 {
    let n = rows(db, &format!("SELECT count() AS n FROM host_object WHERE kind = '{kind}' GROUP ALL")).await;
    n.first().and_then(|r| int(r, "n")).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_etag_is_sent_back_and_an_answer_http_304_is_not_modified_with_nothing_read_again_a_refusal_for_the_rate_waits_as_retry_after_x_ratelimit_reset_or_one_minute_says_counted_and_a_stop_ends_the_wait()
{
    use superx_mod_recon::crawler::hosts::call::{rate_wait, Answer as Answered, UNSAID_WAIT};

    let world = World::new("github.com").await;
    world.github.on(GITHUB, vec![Answer::json(&github_repository(77, "main")).with("etag", "\"r1\"")]);
    let release = serde_json::json!([{ "id": 1, "name": "r1" }]);
    world.github.on(&format!("{GITHUB}/releases?per_page=100"), vec![Answer::json(&release).with("etag", "\"rel1\"")]);
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    assert_eq!(text(&world.host_crawl().await, "outcome").as_deref(), Some("complete"));
    let objects = count(&world.db, "host_object").await;
    world.github.forget();
    world.again(&mut crawler).await;
    let sent: Vec<Seen> = world.github.seen();
    let asked = sent.iter().find(|s| s.path == format!("{GITHUB}/releases?per_page=100")).expect("asked again");
    assert_eq!(asked.header("if-none-match"), Some("\"rel1\""), "its etag sent back");
    let calls = rows(&world.db, &format!("SELECT not_modified, status, objects, error, valid_from FROM host_call WHERE path = '{GITHUB}/releases?per_page=100' ORDER BY valid_from DESC")).await;
    assert_eq!((flag(&calls[0], "not_modified"), int(&calls[0], "status"), int(&calls[0], "objects"), text(&calls[0], "error")), (Some(true), Some(304), Some(0), None));
    assert_eq!(count(&world.db, "host_object").await, objects, "nothing read again");
    assert_eq!(int(&world.host_crawl().await, "not_modified"), Some(2));

    // Refused for the rate: retry-after, then x-ratelimit-reset.
    world.github.on(
        &format!("{GITHUB}/deployments?per_page=100"),
        vec![Answer::status(429, "slow down").with("retry-after", "1"), Answer::json(&serde_json::json!([]))],
    );
    world.github.on(
        &format!("{GITHUB}/environments?per_page=100"),
        vec![
            Answer::status(403, r#"{"message":"API rate limit exceeded"}"#).with("x-ratelimit-remaining", "0").with("x-ratelimit-reset", "@now+1"),
            Answer::json(&serde_json::json!({ "total_count": 0, "environments": [] })),
        ],
    );
    world.github.forget();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let started = std::time::Instant::now();
    world.pass(&mut crawler).await;
    assert!(started.elapsed() >= std::time::Duration::from_millis(1000), "it waited");
    let crawl = world.host_crawl().await;
    assert_eq!((int(&crawl, "rate_waits"), text(&crawl, "outcome").as_deref()), (Some(2), Some("complete")), "{crawl:?}");
    assert!(int(&crawl, "waited_ms").is_some_and(|ms| ms >= 1500), "a second, then until the reset: {crawl:?}");
    let paths = world.github.paths();
    assert_eq!(paths.iter().filter(|p| **p == format!("{GITHUB}/deployments?per_page=100")).count(), 2, "asked again");
    let refused = rows(&world.db, &format!("SELECT status, retry_after_ms FROM host_call WHERE path = '{GITHUB}/deployments?per_page=100' AND status = 429")).await;
    assert_eq!(int(&refused[0], "retry_after_ms"), Some(1000), "the refused call is kept");
    // The minute a refusal that names neither waits, as the crawler
    // computes it, never waited.
    let now = chrono::Utc::now();
    let unsaid = Answered { status: Some(429), ..Answered::default() };
    assert_eq!((rate_wait(&unsaid, now), UNSAID_WAIT), (std::time::Duration::from_secs(60), std::time::Duration::from_secs(60)));

    // A stop ends the wait: sent once the refusal that makes it wait a
    // minute has come.
    let refused = format!("{GITHUB}/releases?per_page=100");
    world.github.on(&refused, vec![Answer::status(429, "slow down")]);
    world.github.forget();
    let (asked, mut stop) = never_stop();
    let host = world.github.clone();
    let stopping = tokio::spawn(async move {
        while !host.paths().contains(&refused) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        asked.send_replace(true);
    });
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let started = std::time::Instant::now();
    let stopped = pass_on(&world.kernel, &world.db, &world.params(), &mut crawler, &mut stop).await.expect("pass");
    stopping.await.expect("the stop was sent");
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "the minute was not waited out");
    assert_eq!(stopped.outcome, "stopped", "{stopped:?}");
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_made_while_the_hosts_newest_answer_said_none_of_its_limit_is_left_waits_until_its_reset_first() {
    let world = World::new("github.com").await;
    // The pull requests' answer says none of the limit is left, its reset
    // a second away: the issues, asked next, wait for it.
    let pulls = format!("{GITHUB}/pulls?state=all&sort=created&direction=asc&per_page=100");
    world.github.on(&pulls, vec![Answer::json(&serde_json::json!([])).with("x-ratelimit-remaining", "0").with("x-ratelimit-reset", "@now+1")]);
    let done = world.pass(&mut world.crawler()).await;
    let said = rows(&world.db, &format!("SELECT rate_reset FROM host_call WHERE path = '{pulls}'")).await;
    let reset = said.first().and_then(|r| time_of(r, "rate_reset")).expect("the reset its answer named");
    let next = rows(&world.db, &format!("SELECT started_at FROM host_call WHERE path = '{GITHUB}/issues?state=all&sort=created&direction=asc&per_page=100'")).await;
    let asked = next.first().and_then(|r| time_of(r, "started_at")).expect("the next call");
    assert!(asked >= reset, "it waited for the reset: {said:?} {next:?}");
    let crawl = world.host_crawl().await;
    assert!(int(&crawl, "rate_waits").is_some_and(|n| n >= 1), "{crawl:?} {done:?}");
    world.done();
}

fn time_of(v: &Value, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    match field(v, key) {
        Some(Value::Datetime(d)) => Some(chrono::DateTime::<chrono::Utc>::from(*d)),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_of_a_list_answered_http_304_is_followed_by_the_page_the_newest_earlier_answer_named_asked_with_that_pages_own_etag()
{
    let world = World::new("github.com").await;
    let first = format!("{GITHUB}/releases?per_page=100");
    let second = format!("{GITHUB}/releases?per_page=100&page=2");
    world.github.on(
        &first,
        vec![Answer::json(&serde_json::json!([{ "id": 1 }])).with("etag", "\"p1\"").with("link", &format!("<{}{second}>; rel=\"next\"", world.github.url))],
    );
    world.github.on(&second, vec![Answer::json(&serde_json::json!([{ "id": 2 }])).with("etag", "\"p2\"")]);
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    world.github.forget();
    world.again(&mut crawler).await;
    let seen = world.github.seen();
    let one = seen.iter().find(|s| s.path == first).expect("the first page");
    let two = seen.iter().find(|s| s.path == second).expect("the page it named");
    assert_eq!((one.header("if-none-match"), two.header("if-none-match")), (Some("\"p1\""), Some("\"p2\"")));
    let calls = rows(&world.db, &format!("SELECT path, not_modified, next, valid_from FROM host_call WHERE path INSIDE ['{first}', '{second}'] ORDER BY valid_from DESC LIMIT 2")).await;
    assert!(calls.iter().all(|c| flag(c, "not_modified") == Some(true)), "{calls:?}");
    assert_eq!(count_of(&world.db, "release").await, 2, "nothing read again");
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_http_400_http_403_or_http_404_to_a_call_after_the_repositorys_own_is_the_hosts_answer_no_object_and_no_failure_its_call_kept_with_its_status_and_body_its_path_and_status_in_the_note_the_crawl_complete_the_same_path_is_asked_at_the_next_host_crawl_and_read_once_the_credentials_file_gives_a_token_the_host_answers_nothing_restarted()
{
    let world = World::new("github.com").await;
    let protection = format!("{GITHUB}/branches/main/protection");
    let rulesets = format!("{GITHUB}/rulesets?per_page=100");
    let runs = format!("{GITHUB}/actions/runs?per_page=100");
    world.github.on(&protection, vec![Answer::status(404, r#"{"message":"Branch not protected"}"#)]);
    world.github.on(&rulesets, vec![Answer::status(403, r#"{"message":"Resource not accessible by personal access token"}"#)]);
    world.github.on(&runs, vec![Answer::status(400, r#"{"message":"Bad request"}"#)]);
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    let crawl = world.host_crawl().await;
    assert_eq!((text(&crawl, "outcome").as_deref(), int(&crawl, "failed")), (Some("complete"), Some(0)), "{crawl:?}");
    let note = text(&crawl, "note").expect("a note");
    for (path, status) in [(&protection, 404), (&rulesets, 403), (&runs, 400)] {
        assert!(note.contains(&format!("{path}: HTTP {status}")), "{note}");
        let call = rows(&world.db, &format!("SELECT status, objects, error FROM host_call WHERE path = '{path}'")).await;
        assert_eq!((int(&call[0], "status"), int(&call[0], "objects")), (Some(status), Some(0)));
        assert!(text(&call[0], "error").is_some_and(|e| e.starts_with(&format!("HTTP {status}: {{\"message\""))), "its body: {call:?}");
    }
    assert_eq!(count_of(&world.db, "branch_protection").await, 0);

    // A token given more: the same paths asked at the next host crawl, and
    // read, nothing restarted.
    let better = "recon-test-better-token-1d2e";
    credentials(&world.root, Some(better), Some(BB_TOKEN));
    world.github.json(&protection, &serde_json::json!({ "url": "x", "required_linear_history": { "enabled": true } }));
    world.github.json(&rulesets, &serde_json::json!([]));
    world.github.forget();
    world.again(&mut crawler).await;
    let asked = world.github.seen().into_iter().find(|s| s.path == protection).expect("asked again");
    assert_eq!(asked.header("authorization"), Some(format!("Bearer {better}").as_str()), "the credentials file read afresh");
    assert_eq!(count_of(&world.db, "branch_protection").await, 1);
    let key = rows(&world.db, "SELECT key FROM host_object WHERE kind = 'branch_protection'").await;
    assert_eq!(text(&key[0], "key").as_deref(), Some("main"), "keyed by the branch's name");
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_http_200_that_is_not_json_or_not_of_the_shape_asked_is_unread_its_call_failed_its_error_an_answer_not_of_the_shape_asked_and_the_body()
{
    let world = World::new("github.com").await;
    world.github.on(&format!("{GITHUB}/releases?per_page=100"), vec![Answer::status(200, "not json")]);
    world.github.on(&format!("{GITHUB}/deployments?per_page=100"), vec![Answer::status(200, r#"{"message":"an object, not a list"}"#)]);
    let done = world.pass(&mut world.crawler()).await;
    for (path, body) in [("releases", "not json"), ("deployments", r#"{"message":"an object, not a list"}"#)] {
        let call = rows(&world.db, &format!("SELECT status, error, objects FROM host_call WHERE path = '{GITHUB}/{path}?per_page=100'")).await;
        assert_eq!(
            (int(&call[0], "status"), text(&call[0], "error"), int(&call[0], "objects")),
            (Some(200), Some(format!("an answer not of the shape asked: {body}")), Some(0))
        );
    }
    let crawl = world.host_crawl().await;
    assert_eq!((text(&crawl, "outcome").as_deref(), int(&crawl, "failed")), (Some("partial"), Some(2)));
    let hosts = done.source("hosts").expect("the hosts source");
    assert_eq!((hosts.unread, hosts.failed, hosts.outcome), (2, 2, Outcome::Partial));
    world.done();
}

/// An object written as the step says its hash is taken: serde_json's
/// compact form, every object's keys sorted.
fn sorted(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(o) => {
            let keys: std::collections::BTreeMap<&String, &serde_json::Value> = o.iter().collect();
            let fields: Vec<String> = keys.iter().map(|(k, v)| format!("{}:{}", serde_json::to_string(k).expect("a key"), sorted(v))).collect();
            format!("{{{}}}", fields.join(","))
        }
        serde_json::Value::Array(items) => format!("[{}]", items.iter().map(sorted).collect::<Vec<_>>().join(",")),
        other => serde_json::to_string(other).expect("a value"),
    }
}

fn sha256(text: &str) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn objects_take_their_kind_and_key_their_hash_the_sha_256_of_their_sorted_compact_json_the_same_content_again_writes_nothing_and_changed_content_one_row_more_accounts_at_any_depth_are_user_objects_and_host_user_rows_once()
{
    let world = World::new("github.com").await;
    let release = |name: &str| {
        serde_json::json!({
            "name": name, "id": 41, "zeta": null, "alpha": [3, 1.5, "x"],
            "author": { "type": "User", "login": "dev", "id": 11 },
            "assets": [{ "id": 5, "uploader": { "login": "bot", "id": 12, "type": "Bot", "name": "a bot" } }]
        })
    };
    world.github.json(&format!("{GITHUB}/releases?per_page=100"), &serde_json::json!([release("v1")]));
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    let kept = rows(&world.db, "SELECT kind, key, hash, object FROM host_object WHERE kind = 'release'").await;
    assert_eq!(kept.len(), 1);
    assert_eq!((int(&kept[0], "key"), text(&kept[0], "hash")), (Some(41), Some(sha256(&sorted(&release("v1"))))), "{kept:?}");
    let users = rows(&world.db, "SELECT key FROM host_object WHERE kind = 'user'").await;
    let mut keys: Vec<i64> = users.iter().filter_map(|u| int(u, "key")).collect();
    keys.sort_unstable();
    assert_eq!(keys, [11, 12, 9001], "every account, at any depth, the repository's owner among them");
    let accounts = rows(&world.db, "SELECT host, host_id, login, name, kind FROM host_user").await;
    let mut seen: Vec<(i64, Option<String>, Option<String>)> = accounts.iter().map(|a| (int(a, "host_id").expect("id"), text(a, "login"), text(a, "kind"))).collect();
    seen.sort();
    assert_eq!(
        seen,
        [(11, Some("dev".into()), Some("User".into())), (12, Some("bot".into()), Some("Bot".into())), (9001, Some("acme".into()), Some("Organization".into()))]
    );
    assert!(accounts.iter().all(|a| text(a, "host").as_deref() == Some("github.com")));
    let objects = count(&world.db, "host_object").await;

    // The same content again: nothing.
    world.again(&mut crawler).await;
    assert_eq!(count(&world.db, "host_object").await, objects);
    let crawl = world.host_crawl().await;
    assert_eq!((int(&crawl, "objects_new"), int(&crawl, "objects_changed")), (Some(0), Some(0)));

    // Changed content: one row more, the accounts' rows once.
    world.github.json(&format!("{GITHUB}/releases?per_page=100"), &serde_json::json!([release("v1 renamed")]));
    world.again(&mut crawler).await;
    assert_eq!(count(&world.db, "host_object").await, objects + 1);
    assert_eq!(count_of(&world.db, "release").await, 2);
    let crawl = world.host_crawl().await;
    assert_eq!((int(&crawl, "objects_new"), int(&crawl, "objects_changed")), (Some(0), Some(1)));
    assert_eq!(count(&world.db, "host_user").await, 3, "an account is a row once");
    world.done();
}

/// GitHub's answers for a repository with an open pull request, two
/// issues, a workflow run, statuses and checks, a deployment, a release
/// and a ruleset. The pull request's head is `head`.
fn github_answers(world: &World, head: &str) {
    let [c1, c2] = [&world.commits[0], &world.commits[1]];
    let g = &world.github;
    let pull = github_pull(1, head, c2, None, "2026-10-02T10:00:00Z");
    g.json(&format!("{GITHUB}/pulls?state=all&sort=created&direction=asc&per_page=100"), &serde_json::json!([pull]));
    g.json(&format!("{GITHUB}/pulls/1"), &pull);
    g.json(&format!("{GITHUB}/pulls/1/commits?per_page=100"), &serde_json::json!([{ "sha": head }]));
    g.json(&format!("{GITHUB}/pulls/1/files?per_page=100"), &serde_json::json!([{ "filename": "f.txt", "status": "added" }]));
    g.json(&format!("{GITHUB}/pulls/1/reviews?per_page=100"), &serde_json::json!([{ "id": 70, "user": github_user("reviewer", 13), "state": "APPROVED" }]));
    g.json(
        &format!("{GITHUB}/issues?state=all&sort=created&direction=asc&per_page=100"),
        &serde_json::json!([{ "id": 1001, "number": 1, "pull_request": { "url": "x" } }, { "id": 1002, "number": 2, "title": "a bug" }]),
    );
    g.json(&format!("{GITHUB}/issues/1/timeline?per_page=100"), &serde_json::json!([{ "event": "committed", "sha": head }]));
    g.json(&format!("{GITHUB}/issues/2/timeline?per_page=100"), &serde_json::json!([{ "id": 501, "event": "labeled" }]));
    for path in ["issues/comments?sort=created&direction=asc&per_page=100", "pulls/comments?sort=created&direction=asc&per_page=100", "issues/events?per_page=100"] {
        g.json(&format!("{GITHUB}/{path}"), &serde_json::json!([]));
    }
    g.json(
        &format!("{GITHUB}/actions/runs?per_page=100"),
        &serde_json::json!({ "total_count": 1, "workflow_runs": [{ "id": 10, "name": "ci", "head_sha": c2, "head_branch": "main", "status": "completed", "conclusion": "success", "run_attempt": 1, "run_number": 3, "html_url": "https://github.com/acme/widget/actions/runs/10", "created_at": "2026-10-02T10:00:00Z", "run_started_at": "2026-10-02T10:00:05Z", "triggering_actor": github_user("dev", 11), "pull_requests": [] }] }),
    );
    g.json(
        &format!("{GITHUB}/actions/runs/10/jobs?filter=all&per_page=100"),
        &serde_json::json!({ "total_count": 1, "jobs": [{ "id": 100, "run_id": 10, "name": "build", "head_sha": c2, "head_branch": "main", "status": "completed", "conclusion": "success", "started_at": "2026-10-02T10:00:06Z", "completed_at": "2026-10-02T10:01:00Z", "runner_name": "runner-1", "run_attempt": 1, "html_url": "https://github.com/acme/widget/actions/runs/10/job/100", "created_at": "2026-10-02T10:00:05Z" }] }),
    );
    for (i, (commit, state)) in [(c2.as_str(), "success"), (head, "success"), (c1.as_str(), "pending")].into_iter().enumerate() {
        let n = i64::try_from(i).expect("small");
        g.json(&format!("{GITHUB}/commits/{commit}/statuses?per_page=100"), &serde_json::json!([{ "id": 1 + n, "state": state, "context": "ci", "target_url": "https://ci", "created_at": "2026-10-02T10:00:00Z" }]));
        g.json(
            &format!("{GITHUB}/commits/{commit}/check-suites?per_page=100"),
            &serde_json::json!({ "total_count": 1, "check_suites": [{ "id": 20 + n, "status": "completed", "conclusion": "success", "head_sha": commit, "head_branch": "main", "app": { "name": "GitHub Actions" }, "pull_requests": [], "url": "https://api/suites", "created_at": "2026-10-02T10:00:00Z" }] }),
        );
        g.json(
            &format!("{GITHUB}/commits/{commit}/check-runs?filter=all&per_page=100"),
            &serde_json::json!({ "total_count": 1, "check_runs": [{ "id": 30 + n, "name": "build", "head_sha": commit, "status": "completed", "conclusion": "success", "started_at": "2026-10-02T10:00:01Z", "completed_at": "2026-10-02T10:00:09Z", "check_suite": { "id": 20 + n }, "html_url": "https://github.com/acme/widget/runs/x", "pull_requests": [] }] }),
        );
    }
    g.json(&format!("{GITHUB}/deployments?per_page=100"), &serde_json::json!([{ "id": 7, "sha": c2, "creator": github_user("dev", 11) }]));
    g.json(&format!("{GITHUB}/deployments/7/statuses?per_page=100"), &serde_json::json!([{ "id": 71, "state": "success" }]));
    g.json(&format!("{GITHUB}/environments?per_page=100"), &serde_json::json!({ "total_count": 1, "environments": [{ "id": 31, "name": "prod" }] }));
    g.json(&format!("{GITHUB}/releases?per_page=100"), &serde_json::json!([{ "id": 41, "name": "v1" }]));
    g.json(&format!("{GITHUB}/rulesets?per_page=100"), &serde_json::json!([{ "id": 3, "name": "main", "updated_at": "2026-10-01T00:00:00Z" }]));
    g.json(&format!("{GITHUB}/rulesets/3"), &serde_json::json!({ "id": 3, "name": "main", "updated_at": "2026-10-01T00:00:00Z", "rules": [] }));
    g.on(&format!("{GITHUB}/branches/main/protection"), vec![Answer::status(404, r#"{"message":"Branch not protected","documentation_url":"x"}"#)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_github_crawl_asks_every_path_step_17_names_in_its_order_the_check_suites_and_check_runs_among_them_a_pull_request_an_issues_timeline_and_a_runs_jobs_are_asked_again_only_when_their_object_changed_statuses_check_suites_and_check_runs_are_asked_for_the_commits_and_at_the_crawls_step_17_names_a_protection_answered_http_404_whatever_its_message_is_the_hosts_answer_an_unanswered_repository_makes_the_crawl_unreadable()
{
    let world = World::new("github.com").await;
    git(&world.dir, &["switch", "-q", "-c", "feat"]);
    let head = commit(&world.dir, &[("f.txt", "f\n")], "head");
    git(&world.dir, &["switch", "-q", "main"]);
    let [c1, c2] = [world.commits[0].clone(), world.commits[1].clone()];
    github_answers(&world, &head);
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    let checks = |c: &str| [format!("/commits/{c}/check-suites?per_page=100"), format!("/commits/{c}/check-runs?filter=all&per_page=100")];
    let mut want: Vec<String> = [
        "",
        "/pulls?state=all&sort=created&direction=asc&per_page=100",
        "/pulls/1",
        "/pulls/1/commits?per_page=100",
        "/pulls/1/files?per_page=100",
        "/pulls/1/reviews?per_page=100",
        "/issues?state=all&sort=created&direction=asc&per_page=100",
        "/issues/1/timeline?per_page=100",
        "/issues/2/timeline?per_page=100",
        "/issues/comments?sort=created&direction=asc&per_page=100",
        "/pulls/comments?sort=created&direction=asc&per_page=100",
        "/issues/events?per_page=100",
        "/actions/runs?per_page=100",
        "/actions/runs/10/jobs?filter=all&per_page=100",
    ]
    .iter()
    .map(|p| (*p).to_string())
    .collect();
    // The main line's tip, the open pull request's head, then the main
    // line's first-parent history at the first crawl that knows it.
    for c in [&c2, &head, &c1] {
        want.push(format!("/commits/{c}/statuses?per_page=100"));
    }
    for c in [&c2, &head, &c1] {
        want.extend(checks(c));
    }
    want.extend(
        [
            "/deployments?per_page=100",
            "/deployments/7/statuses?per_page=100",
            "/environments?per_page=100",
            "/releases?per_page=100",
            "/rulesets?per_page=100",
            "/rulesets/3",
            "/branches/main/protection",
        ]
        .iter()
        .map(|p| (*p).to_string()),
    );
    let want: Vec<String> = want.into_iter().map(|p| format!("{GITHUB}{p}")).collect();
    assert_eq!(world.github.paths(), want);
    let crawl = world.host_crawl().await;
    assert_eq!(text(&crawl, "outcome").as_deref(), Some("complete"), "{crawl:?}");
    assert!(text(&crawl, "note").is_some_and(|n| n.contains(&format!("{GITHUB}/branches/main/protection: HTTP 404"))), "the host's answer");
    for (kind, n) in [("check_suite", 3), ("check_run", 3), ("workflow_run", 1), ("workflow_job", 1), ("commit_status", 3), ("pull_request", 1), ("timeline_event", 2), ("ruleset", 1)] {
        assert_eq!(count_of(&world.db, kind).await, n, "{kind}");
    }

    // Again, nothing changed: no pull request, timeline, jobs or ruleset;
    // the tip's and the open head's statuses and checks; the statuses still
    // pending.
    world.github.forget();
    world.again(&mut crawler).await;
    let mut want: Vec<String> = [
        "",
        "/pulls?state=all&sort=created&direction=asc&per_page=100",
        "/issues?state=all&sort=created&direction=asc&per_page=100",
        "/issues/comments?sort=created&direction=asc&per_page=100",
        "/pulls/comments?sort=created&direction=asc&per_page=100",
        "/issues/events?per_page=100",
        "/actions/runs?per_page=100",
    ]
    .iter()
    .map(|p| (*p).to_string())
    .collect();
    for c in [&c2, &head, &c1] {
        want.push(format!("/commits/{c}/statuses?per_page=100"));
    }
    for c in [&c2, &head] {
        want.extend(checks(c));
    }
    want.extend(
        ["/deployments?per_page=100", "/deployments/7/statuses?per_page=100", "/environments?per_page=100", "/releases?per_page=100", "/rulesets?per_page=100", "/branches/main/protection"]
            .iter()
            .map(|p| (*p).to_string()),
    );
    let want: Vec<String> = want.into_iter().map(|p| format!("{GITHUB}{p}")).collect();
    assert_eq!(world.github.paths(), want);

    // A repository the host does not answer: unreadable, nothing more asked.
    let other = World::new("github.com").await;
    other.github.on(GITHUB, vec![Answer::status(404, r#"{"message":"Not Found"}"#)]);
    let done = other.pass(&mut other.crawler()).await;
    let crawl = other.host_crawl().await;
    assert_eq!((text(&crawl, "outcome").as_deref(), int(&crawl, "calls")), (Some("unreadable"), Some(1)), "{crawl:?}");
    assert_eq!(other.github.paths(), [GITHUB.to_string()]);
    assert_eq!(done.source("hosts").map(|s| s.outcome), Some(Outcome::Complete), "an unreadable host leaves the source complete");
    other.done();
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_bitbucket_crawl_asks_every_path_step_18_names_in_its_order_with_every_pull_request_state_follows_the_diffstats_redirect_keys_statuses_by_commit_and_key_writes_a_uuids_braces_encoded_and_asks_no_issue()
{
    let world = World::new("bitbucket.org").await;
    git(&world.dir, &["switch", "-q", "-c", "feat"]);
    let head = commit(&world.dir, &[("f.txt", "f\n")], "head");
    git(&world.dir, &["switch", "-q", "main"]);
    let [c1, c2] = [world.commits[0].clone(), world.commits[1].clone()];
    let b = &world.bitbucket;
    let author = serde_json::json!({ "uuid": "{u-1}", "type": "user", "display_name": "Dev", "nickname": "dev" });
    let pull = serde_json::json!({
        "id": 1, "title": "feat", "state": "OPEN", "author": author, "created_on": "2026-10-01T10:00:00+00:00", "updated_on": "2026-10-02T10:00:00+00:00",
        "source": { "branch": { "name": "feat" }, "commit": { "hash": head }, "repository": { "full_name": "team/repo" } },
        "destination": { "branch": { "name": "main" }, "commit": { "hash": c2 } },
        "links": { "html": { "href": "https://bitbucket.org/team/repo/pull-requests/1" } }, "comment_count": 2
    });
    b.json(&format!("{BITBUCKET}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50"), &serde_json::json!({ "values": [pull], "size": 1 }));
    b.json(&format!("{BITBUCKET}/pullrequests/1"), &pull);
    b.json(&format!("{BITBUCKET}/pullrequests/1/commits?pagelen=50"), &serde_json::json!({ "values": [{ "hash": head }] }));
    b.on(&format!("{BITBUCKET}/pullrequests/1/diffstat?pagelen=50"), vec![Answer::status(302, "").with("location", &format!("{}{BITBUCKET}/diffstat/abc?pagelen=50", b.url))]);
    b.json(&format!("{BITBUCKET}/diffstat/abc?pagelen=50"), &serde_json::json!({ "values": [{ "new": { "path": "f.txt" }, "old": null, "status": "added" }] }));
    b.json(&format!("{BITBUCKET}/pullrequests/1/comments?pagelen=50"), &serde_json::json!({ "values": [{ "id": 81 }] }));
    b.json(&format!("{BITBUCKET}/pullrequests/1/activity?pagelen=50"), &serde_json::json!({ "values": [{ "approval": { "user": author } }] }));
    b.json(&format!("{BITBUCKET}/pullrequests/1/tasks?pagelen=50"), &serde_json::json!({ "values": [{ "id": 91 }] }));
    for c in [&c2, &head] {
        b.json(
            &format!("{BITBUCKET}/commit/{c}/statuses?pagelen=50"),
            &serde_json::json!({ "values": [{ "key": "build", "state": "SUCCESSFUL", "name": "ci", "refname": "main", "url": "https://ci", "created_on": "2026-10-02T10:00:00+00:00" }] }),
        );
    }
    b.json(&format!("{BITBUCKET}/commit/{c1}/statuses?pagelen=50"), &serde_json::json!({ "values": [] }));
    b.json(
        &format!("{BITBUCKET}/pipelines?sort=-created_on&pagelen=50"),
        &serde_json::json!({ "values": [{ "uuid": "{pipe-1}", "build_number": 5, "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } }, "target": { "ref_name": "main", "commit": { "hash": c2 }, "selector": { "pattern": "main" } }, "creator": author, "created_on": "2026-10-02T10:00:00+00:00", "completed_on": "2026-10-02T10:05:00+00:00" }] }),
    );
    b.json(
        &format!("{BITBUCKET}/pipelines/%7Bpipe-1%7D/steps?pagelen=50"),
        &serde_json::json!({ "values": [{ "uuid": "{step-1}", "name": "build", "pipeline": { "uuid": "{pipe-1}" }, "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } }, "started_on": "2026-10-02T10:01:00+00:00", "completed_on": "2026-10-02T10:04:00+00:00" }] }),
    );
    b.json(&format!("{BITBUCKET}/deployments?pagelen=50"), &serde_json::json!({ "values": [{ "uuid": "{dep-1}" }] }));
    b.json(&format!("{BITBUCKET}/environments?pagelen=50"), &serde_json::json!({ "values": [{ "uuid": "{env-1}" }] }));
    b.json(&format!("{BITBUCKET}/branch-restrictions?pagelen=50"), &serde_json::json!({ "values": [{ "id": 61, "kind": "push" }] }));
    b.json(&format!("{BITBUCKET}/default-reviewers?pagelen=50"), &serde_json::json!({ "values": [{ "uuid": "{u-2}", "type": "user", "display_name": "Reviewer" }] }));
    world.pass(&mut world.crawler()).await;

    let mut want: Vec<String> = [
        "",
        "/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50",
        "/pullrequests/1",
        "/pullrequests/1/commits?pagelen=50",
        "/pullrequests/1/diffstat?pagelen=50",
        "/diffstat/abc?pagelen=50",
        "/pullrequests/1/comments?pagelen=50",
        "/pullrequests/1/activity?pagelen=50",
        "/pullrequests/1/tasks?pagelen=50",
    ]
    .iter()
    .map(|p| (*p).to_string())
    .collect();
    for c in [&c2, &head, &c1] {
        want.push(format!("/commit/{c}/statuses?pagelen=50"));
    }
    want.extend(
        [
            "/pipelines?sort=-created_on&pagelen=50",
            "/pipelines/%7Bpipe-1%7D/steps?pagelen=50",
            "/deployments?pagelen=50",
            "/environments?pagelen=50",
            "/branch-restrictions?pagelen=50",
            "/default-reviewers?pagelen=50",
        ]
        .iter()
        .map(|p| (*p).to_string()),
    );
    let want: Vec<String> = want.into_iter().map(|p| format!("{BITBUCKET}{p}")).collect();
    assert_eq!(world.bitbucket.paths(), want);
    assert!(world.bitbucket.paths().iter().all(|p| !p.contains("/issues")), "no issue asked");
    assert_eq!(count_of(&world.db, "pull_request_file").await, 1, "the diffstat read through its redirect");
    let keys: Vec<String> = rows(&world.db, "SELECT VALUE key FROM host_object WHERE kind = 'commit_status'").await.iter().filter_map(|k| match k {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }).collect();
    let mut keys = keys;
    keys.sort();
    let mut want_keys = vec![format!("{c2}/build"), format!("{head}/build")];
    want_keys.sort();
    assert_eq!(keys, want_keys, "a status by its commit and key");
    let crawl = world.host_crawl().await;
    assert_eq!(text(&crawl, "outcome").as_deref(), Some("complete"), "{crawl:?}");
    world.done();
}

/// The `pull_request_state` rows of the pull request numbered `number`,
/// oldest first.
async fn states(db: &superx_kernel::Db, number: i64) -> Vec<Value> {
    rows(
        db,
        &format!(
            "SELECT state, merged, draft, title, head_sha, base_sha, merge_sha, merged_at, merged_by.login AS merged_by, closed_at, closed_by.login AS closed_by, \
             reason, additions, deletions, changed_files, commits, comments, review_comments, updated_at, object.kind AS object, valid_from \
             FROM pull_request_state WHERE pull_request.number = {number} ORDER BY valid_from"
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_github_and_a_bitbucket_pull_request_each_make_their_pull_request_and_pull_request_state_rows_column_by_column_merge_sha_only_once_merged_closed_by_from_the_timeline_on_github_and_a_state_row_only_when_it_changes()
{
    use chrono::TimeZone as _;

    let world = World::new("github.com").await;
    let (base, head, merge) = (world.commits[0].clone(), world.commits[1].clone(), "1111111111111111111111111111111111111111".to_string());
    let list = format!("{GITHUB}/pulls?state=all&sort=created&direction=asc&per_page=100");
    let open = github_pull(1, &head, &base, None, "2026-10-02T10:00:00Z");
    let mut fork = github_pull(2, &head, &base, None, "2026-10-02T10:00:00Z");
    fork["head"]["repo"]["full_name"] = serde_json::json!("Fork/Widget");
    world.github.json(&list, &serde_json::json!([open, fork]));
    world.github.json(&format!("{GITHUB}/pulls/1"), &open);
    world.github.json(&format!("{GITHUB}/pulls/2"), &fork);
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    let pull = rows(&world.db, "SELECT number, host_id, author.login AS author, created_at, source_branch, target_branch, source_path, url, object.kind AS object FROM pull_request WHERE number = 1").await;
    let p = &pull[0];
    assert_eq!(
        (int(p, "number"), int(p, "host_id"), text(p, "author"), text(p, "source_branch"), text(p, "target_branch"), text(p, "source_path"), text(p, "url"), text(p, "object")),
        (Some(1), Some(100), Some("dev".into()), Some("feat".into()), Some("main".into()), None, Some("https://github.com/acme/widget/pull/1".into()), Some("pull_request".into()))
    );
    assert_eq!(time_of(p, "created_at"), chrono::Utc.with_ymd_and_hms(2026, 10, 1, 10, 0, 0).single());
    let forked = rows(&world.db, "SELECT source_path FROM pull_request WHERE number = 2").await;
    assert_eq!(text(&forked[0], "source_path").as_deref(), Some("fork/widget"), "the repository its branch lives in, lowercased");
    let s = &states(&world.db, 1).await[0];
    assert_eq!(
        (text(s, "state"), flag(s, "merged"), flag(s, "draft"), text(s, "title"), text(s, "head_sha"), text(s, "base_sha"), text(s, "merge_sha")),
        (Some("open".into()), Some(false), Some(false), Some("pull 1".into()), Some(head.clone()), Some(base.clone()), None),
        "a trial merge is no merge: {s:?}"
    );
    assert_eq!((time_of(s, "merged_at"), text(s, "merged_by"), time_of(s, "closed_at"), text(s, "closed_by"), text(s, "reason")), (None, None, None, None, None));
    assert_eq!(
        ["additions", "deletions", "changed_files", "commits", "comments", "review_comments"].map(|k| int(s, k)),
        [Some(3), Some(1), Some(2), Some(2), Some(1), Some(0)]
    );
    assert_eq!((time_of(s, "updated_at"), text(s, "object")), (chrono::Utc.with_ymd_and_hms(2026, 10, 2, 10, 0, 0).single(), Some("pull_request".into())));

    // Merged, and closed as its timeline says.
    let merged = github_pull(1, &head, &base, Some(&merge), "2026-10-03T10:00:00Z");
    world.github.json(&list, &serde_json::json!([merged, fork]));
    world.github.json(&format!("{GITHUB}/pulls/1"), &merged);
    world.github.json(
        &format!("{GITHUB}/issues/1/timeline?per_page=100"),
        &serde_json::json!([{ "id": 900, "event": "closed", "actor": github_user("closer", 21), "created_at": "2026-10-02T10:00:00Z" }]),
    );
    world.again(&mut crawler).await;
    let all = states(&world.db, 1).await;
    assert_eq!(all.len(), 2, "{all:?}");
    let s = &all[1];
    assert_eq!(
        (text(s, "state"), flag(s, "merged"), text(s, "merge_sha"), text(s, "merged_by"), text(s, "closed_by")),
        (Some("closed".into()), Some(true), Some(merge.clone()), Some("lead".into()), Some("closer".into()))
    );
    assert_eq!((time_of(s, "merged_at"), time_of(s, "closed_at")), (chrono::Utc.with_ymd_and_hms(2026, 10, 2, 10, 0, 0).single(), chrono::Utc.with_ymd_and_hms(2026, 10, 2, 10, 0, 0).single()));

    // Nothing changed: no state row.
    world.again(&mut crawler).await;
    assert_eq!(states(&world.db, 1).await.len(), 2);
    assert_eq!(states(&world.db, 2).await.len(), 1);

    // Bitbucket: merged and closed by the same account.
    let bb = World::new("bitbucket.org").await;
    let closer = serde_json::json!({ "uuid": "{u-9}", "type": "user", "display_name": "Closer", "nickname": "closer" });
    let pull = serde_json::json!({
        "id": 4, "title": "bb pull", "state": "MERGED", "draft": false, "reason": "",
        "author": { "uuid": "{u-1}", "type": "user", "display_name": "Dev", "nickname": "dev" }, "closed_by": closer,
        "created_on": "2026-10-01T10:00:00+00:00", "updated_on": "2026-10-03T10:00:00+00:00",
        "source": { "branch": { "name": "feat" }, "commit": { "hash": head }, "repository": { "full_name": "team/repo" } },
        "destination": { "branch": { "name": "main" }, "commit": { "hash": base } },
        "merge_commit": { "hash": merge }, "links": { "html": { "href": "https://bitbucket.org/team/repo/pull-requests/4" } }, "comment_count": 3
    });
    bb.bitbucket.json(&format!("{BITBUCKET}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50"), &serde_json::json!({ "values": [pull] }));
    bb.bitbucket.json(&format!("{BITBUCKET}/pullrequests/4"), &pull);
    bb.pass(&mut bb.crawler()).await;
    let row = rows(&bb.db, "SELECT number, host_id, author.login AS author, source_branch, target_branch, source_path, url FROM pull_request").await;
    assert_eq!(
        (int(&row[0], "number"), int(&row[0], "host_id"), text(&row[0], "author"), text(&row[0], "source_path"), text(&row[0], "url")),
        (Some(4), Some(4), Some("dev".into()), None, Some("https://bitbucket.org/team/repo/pull-requests/4".into()))
    );
    let s = &states(&bb.db, 4).await[0];
    assert_eq!(
        (text(s, "state"), flag(s, "merged"), text(s, "merge_sha"), text(s, "merged_by"), text(s, "closed_by"), time_of(s, "merged_at"), time_of(s, "closed_at")),
        (Some("MERGED".into()), Some(true), Some(merge.clone()), Some("closer".into()), Some("closer".into()), None, None)
    );
    assert_eq!(
        ["additions", "deletions", "changed_files", "commits", "comments", "review_comments"].map(|k| int(s, k)),
        [None, None, None, None, Some(3), None],
        "Bitbucket's comment count as comments, the others NONE"
    );
    assert_eq!(time_of(s, "updated_at"), chrono::Utc.with_ymd_and_hms(2026, 10, 3, 10, 0, 0).single());
    bb.done();
    world.done();
}

/// The `pull_request_commit` rows, oldest first: hash, present, position.
async fn listed(db: &superx_kernel::Db) -> Vec<(String, bool, Option<i64>)> {
    rows(db, "SELECT hash, present, position, valid_from FROM pull_request_commit ORDER BY valid_from, hash")
        .await
        .iter()
        .map(|r| (text(r, "hash").expect("hash"), flag(r, "present").expect("present"), int(r, "position")))
        .collect()
}

/// A pull request listing `commits`, its own count `stated`, changed at
/// `updated_at`.
fn pull_listing(world: &World, commits: &[String], stated: i64, updated_at: &str) {
    let mut pull = github_pull(1, &world.commits[1], &world.commits[0], None, updated_at);
    pull["commits"] = serde_json::json!(stated);
    world.github.json(&format!("{GITHUB}/pulls?state=all&sort=created&direction=asc&per_page=100"), &serde_json::json!([pull]));
    world.github.json(&format!("{GITHUB}/pulls/1"), &pull);
    let pages: Vec<&[String]> = commits.chunks(100).collect();
    for (i, page) in pages.iter().enumerate() {
        let path = if i == 0 { format!("{GITHUB}/pulls/1/commits?per_page=100") } else { format!("{GITHUB}/pulls/1/commits?per_page=100&page={}", i + 1) };
        let body = serde_json::Value::Array(page.iter().map(|c| serde_json::json!({ "sha": c })).collect());
        let mut answer = Answer::json(&body);
        if i + 1 < pages.len() {
            answer = answer.with("link", &format!("<{}{GITHUB}/pulls/1/commits?per_page=100&page={}>; rel=\"next\"", world.github.url, i + 2));
        }
        world.github.on(&path, vec![answer]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_leaving_a_pull_requests_list_and_coming_back_is_present_then_absent_then_present_again_with_its_position() {
    let world = World::new("github.com").await;
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    pull_listing(&world, &[a.clone(), b.clone()], 2, "2026-10-02T10:00:00Z");
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    assert_eq!(listed(&world.db).await, [(a.clone(), true, Some(0)), (b.clone(), true, Some(1))]);
    pull_listing(&world, std::slice::from_ref(&a), 1, "2026-10-03T10:00:00Z");
    world.again(&mut crawler).await;
    pull_listing(&world, &[a.clone(), b.clone()], 2, "2026-10-04T10:00:00Z");
    world.again(&mut crawler).await;
    assert_eq!(listed(&world.db).await, [(a.clone(), true, Some(0)), (b.clone(), true, Some(1)), (b.clone(), false, None), (b.clone(), true, Some(1))]);
    let objects = rows(&world.db, "SELECT present, object.kind AS kind FROM pull_request_commit").await;
    assert!(objects.iter().all(|o| (flag(o, "present") == Some(true)) == (text(o, "kind").as_deref() == Some("pull_request_commit"))), "{objects:?}");
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_request_whose_own_count_of_commits_is_above_the_two_hundred_and_fifty_github_lists_marks_no_commit_absent() {
    let world = World::new("github.com").await;
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    pull_listing(&world, &[a.clone(), b.clone()], 2, "2026-10-02T10:00:00Z");
    let mut crawler = world.crawler();
    world.pass(&mut crawler).await;
    // Three hundred commits now; GitHub lists two hundred and fifty, and
    // not the first two.
    let shown: Vec<String> = (0..250).map(|i| format!("{i:040x}")).collect();
    pull_listing(&world, &shown, 300, "2026-10-03T10:00:00Z");
    world.again(&mut crawler).await;
    let rows_now = listed(&world.db).await;
    assert_eq!(rows_now.len(), 252, "every commit listed, none marked absent");
    assert!(rows_now.iter().all(|(_, present, _)| *present));
    assert!(world.github.paths().iter().any(|p| p.ends_with("/pulls/1/commits?per_page=100&page=3")), "all three pages read");
    world.done();
}

/// The `ci_run` rows of a kind, with their newest state's columns.
async fn runs_of(db: &superx_kernel::Db, kind: &str) -> Vec<Value> {
    rows(
        db,
        &format!(
            "SELECT host_id, name, commit_sha, ref, parent.host_id AS parent, pull_request.number AS pull, url, created_at, object.kind AS object, \
             (SELECT status, conclusion, started_at, completed_at, attempt, number, actor.login AS actor, runner, object.kind AS object, valid_from \
              FROM ci_run_state WHERE ci_run = $parent.id ORDER BY valid_from) AS states FROM ci_run WHERE kind = '{kind}'"
        ),
    )
    .await
}

fn states_in(run: &Value) -> Vec<Value> {
    match field(run, "states") {
        Some(Value::Array(items)) => items.iter().cloned().collect(),
        _ => Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_kind_of_ci_object_check_suites_and_check_runs_among_them_makes_its_ci_run_with_its_parent_and_pull_request_and_a_ci_run_state_only_when_its_status_conclusion_times_attempt_number_actor_or_runner_change_never_for_its_object_alone()
{
    let world = World::new("github.com").await;
    git(&world.dir, &["switch", "-q", "-c", "feat"]);
    let head = commit(&world.dir, &[("f.txt", "f\n")], "head");
    git(&world.dir, &["switch", "-q", "main"]);
    let c2 = world.commits[1].clone();
    github_answers(&world, &head);
    let run = |title: &str| serde_json::json!({ "total_count": 1, "workflow_runs": [{ "id": 10, "name": "ci", "display_title": title, "head_sha": c2, "head_branch": "main", "status": "completed", "conclusion": "success", "run_attempt": 1, "run_number": 3, "html_url": "https://github.com/acme/widget/actions/runs/10", "created_at": "2026-10-02T10:00:00Z", "run_started_at": "2026-10-02T10:00:05Z", "triggering_actor": github_user("dev", 11), "pull_requests": [{ "number": 1 }] }] });
    world.github.json(&format!("{GITHUB}/actions/runs?per_page=100"), &run("first title"));
    // Bitbucket beside it.
    let bb = world.root.join("bb");
    repo(&bb);
    let bb_tip = commit(&bb, &[("a.txt", "a\n")], "first");
    git(&bb, &["remote", "add", "origin", "https://bitbucket.org/team/repo.git"]);
    hosted(&world.root, "bitbucket.org", "team/repo", &bb);
    world.bitbucket.json(BITBUCKET, &bitbucket_repository());
    let creator = serde_json::json!({ "uuid": "{u-1}", "type": "user", "display_name": "Dev", "nickname": "dev" });
    world.bitbucket.json(
        &format!("{BITBUCKET}/pipelines?sort=-created_on&pagelen=50"),
        &serde_json::json!({ "values": [{ "uuid": "{pipe-1}", "build_number": 5, "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } }, "target": { "ref_name": "main", "commit": { "hash": bb_tip }, "selector": { "pattern": "main" } }, "creator": creator, "created_on": "2026-10-02T10:00:00+00:00", "completed_on": "2026-10-02T10:05:00+00:00" }] }),
    );
    world.bitbucket.json(
        &format!("{BITBUCKET}/pipelines/%7Bpipe-1%7D/steps?pagelen=50"),
        &serde_json::json!({ "values": [{ "uuid": "{step-1}", "name": "build", "pipeline": { "uuid": "{pipe-1}" }, "state": { "name": "COMPLETED", "result": { "name": "SUCCESSFUL" } }, "started_on": "2026-10-02T10:01:00+00:00", "completed_on": "2026-10-02T10:04:00+00:00" }] }),
    );
    world.bitbucket.json(
        &format!("{BITBUCKET}/commit/{bb_tip}/statuses?pagelen=50"),
        &serde_json::json!({ "values": [{ "key": "build", "state": "INPROGRESS", "name": "ci", "refname": "main", "url": "https://ci/1", "created_on": "2026-10-02T10:00:00+00:00" }] }),
    );
    let p = params(&[(INCLUDE_PARAM, strings(&[&world.dir, &bb])), (CRAWL_SECS_PARAM, number(1))]);
    let mut crawler = world.crawler();
    pass(&world.kernel, &world.db, &p, &mut crawler).await;

    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc));
    let suite = runs_of(&world.db, "check_suite").await.into_iter().find(|r| int(r, "host_id") == Some(20)).expect("a suite");
    assert_eq!(
        (text(&suite, "name"), text(&suite, "commit_sha"), text(&suite, "ref"), int(&suite, "parent"), text(&suite, "url"), time_of(&suite, "created_at"), text(&suite, "object")),
        (Some("GitHub Actions".into()), Some(c2.clone()), Some("main".into()), None, Some("https://api/suites".into()), at("2026-10-02T10:00:00Z"), Some("check_suite".into()))
    );
    let check = runs_of(&world.db, "check_run").await.into_iter().find(|r| int(r, "host_id") == Some(30)).expect("a check run");
    assert_eq!((text(&check, "name"), int(&check, "parent"), time_of(&check, "created_at")), (Some("build".into()), Some(20), None), "its suite's ci_run");
    let s = &states_in(&check)[0];
    assert_eq!(
        (text(s, "status"), text(s, "conclusion"), time_of(s, "started_at"), time_of(s, "completed_at")),
        (Some("completed".into()), Some("success".into()), at("2026-10-02T10:00:01Z"), at("2026-10-02T10:00:09Z"))
    );
    let wr = &runs_of(&world.db, "workflow_run").await[0];
    assert_eq!((text(wr, "name"), text(wr, "ref"), int(wr, "pull")), (Some("ci".into()), Some("main".into()), Some(1)), "the pull request it names");
    let s = &states_in(wr)[0];
    assert_eq!(
        (int(s, "attempt"), int(s, "number"), text(s, "actor"), time_of(s, "started_at"), text(s, "runner")),
        (Some(1), Some(3), Some("dev".into()), at("2026-10-02T10:00:05Z"), None)
    );
    let job = &runs_of(&world.db, "workflow_job").await[0];
    assert_eq!((int(job, "parent"), text(job, "name")), (Some(10), Some("build".into())), "its run's ci_run");
    assert_eq!(text(&states_in(job)[0], "runner").as_deref(), Some("runner-1"));
    let status = runs_of(&world.db, "commit_status").await.into_iter().find(|r| int(r, "host_id") == Some(1)).expect("a status");
    assert_eq!((text(&status, "name"), text(&status, "commit_sha"), text(&status, "url")), (Some("ci".into()), Some(c2.clone()), Some("https://ci".into())));
    let s = &states_in(&status)[0];
    assert_eq!((text(s, "status"), text(s, "conclusion")), (Some("success".into()), None));
    let pipeline = &runs_of(&world.db, "pipeline").await[0];
    assert_eq!(
        (text(pipeline, "host_id"), text(pipeline, "name"), text(pipeline, "commit_sha"), text(pipeline, "ref"), time_of(pipeline, "created_at")),
        (Some("{pipe-1}".into()), Some("main".into()), Some(bb_tip.clone()), Some("main".into()), at("2026-10-02T10:00:00+00:00"))
    );
    let s = &states_in(pipeline)[0];
    assert_eq!(
        (text(s, "status"), text(s, "conclusion"), int(s, "number"), text(s, "actor"), time_of(s, "completed_at")),
        (Some("COMPLETED".into()), Some("SUCCESSFUL".into()), Some(5), Some("dev".into()), at("2026-10-02T10:05:00+00:00"))
    );
    let step = &runs_of(&world.db, "pipeline_step").await[0];
    assert_eq!(
        (text(step, "name"), text(step, "parent"), text(step, "commit_sha"), text(step, "ref")),
        (Some("build".into()), Some("{pipe-1}".into()), Some(bb_tip.clone()), Some("main".into())),
        "its pipeline's ci_run, commit and ref"
    );
    let bb_status = runs_of(&world.db, "commit_status").await.into_iter().find(|r| text(r, "host_id").is_some()).expect("Bitbucket's status");
    assert_eq!(
        (text(&bb_status, "host_id"), text(&bb_status, "name"), text(&bb_status, "commit_sha"), text(&bb_status, "ref"), text(&bb_status, "url")),
        (Some(format!("{bb_tip}/build")), Some("ci".into()), Some(bb_tip.clone()), Some("main".into()), Some("https://ci/1".into()))
    );
    assert_eq!(text(&states_in(&bb_status)[0], "status").as_deref(), Some("INPROGRESS"));

    // The run's object changes alone; its job's conclusion changes.
    world.github.json(&format!("{GITHUB}/actions/runs?per_page=100"), &run("second title"));
    world.github.json(
        &format!("{GITHUB}/actions/runs/10/jobs?filter=all&per_page=100"),
        &serde_json::json!({ "total_count": 1, "jobs": [{ "id": 100, "run_id": 10, "name": "build", "head_sha": c2, "head_branch": "main", "status": "completed", "conclusion": "failure", "started_at": "2026-10-02T10:00:06Z", "completed_at": "2026-10-02T10:01:00Z", "runner_name": "runner-1", "run_attempt": 1, "html_url": "https://github.com/acme/widget/actions/runs/10/job/100", "created_at": "2026-10-02T10:00:05Z" }] }),
    );
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    pass(&world.kernel, &world.db, &p, &mut crawler).await;
    assert_eq!(count_of(&world.db, "workflow_run").await, 2, "its object changed");
    assert_eq!(states_in(&runs_of(&world.db, "workflow_run").await[0]).len(), 1, "never a state for its object alone");
    let job_states = states_in(&runs_of(&world.db, "workflow_job").await[0]);
    assert_eq!(job_states.iter().map(|s| text(s, "conclusion")).collect::<Vec<_>>(), [Some("success".to_string()), Some("failure".to_string())]);
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_crawl_records_its_calls_waits_objects_and_rows_complete_partial_on_a_failed_call_or_a_stated_total_read_short_unreadable_or_no_credentials_with_nothing_asked_the_hosts_sources_pass_source_holds_its_counts_and_stays_complete_for_an_unreadable_or_uncredentialed_host()
{
    // An unreadable host and one without credentials: the source stays
    // complete, and names them.
    let world = World::new("github.com").await;
    world.github.on(GITHUB, vec![Answer::status(403, r#"{"message":"Must have admin rights"}"#)]);
    let bb = world.root.join("bb");
    repo(&bb);
    commit(&bb, &[("a.txt", "a\n")], "first");
    git(&bb, &["remote", "add", "origin", "https://bitbucket.org/team/repo.git"]);
    let unprovided = places(&world.root, Some(credentials(&world.root, Some(TOKEN), None)), &world.github, &world.bitbucket);
    let p = params(&[(INCLUDE_PARAM, strings(&[&world.dir, &bb]))]);
    let done = pass(&world.kernel, &world.db, &p, &mut Crawler::new(unprovided)).await;
    let crawls = rows(&world.db, "SELECT host_repository.path AS path, outcome, note, calls FROM host_crawl").await;
    let of = |path: &str| crawls.iter().find(|c| text(c, "path").as_deref() == Some(path)).cloned().expect("a host crawl");
    let gh = of("acme/widget");
    assert_eq!((text(&gh, "outcome").as_deref(), int(&gh, "calls")), (Some("unreadable"), Some(1)));
    assert!(text(&gh, "note").is_some_and(|n| n.contains("HTTP 403")), "{gh:?}");
    let bb_crawl = of("team/repo");
    assert_eq!((text(&bb_crawl, "outcome").as_deref(), int(&bb_crawl, "calls"), text(&bb_crawl, "note").as_deref()), (Some("no_credentials"), Some(0), Some("no token")));
    assert!(world.bitbucket.seen().is_empty(), "nothing asked");
    let hosts = done.source("hosts").expect("the hosts source");
    assert_eq!((hosts.items, hosts.outcome), (2, Outcome::Complete));
    let note = hosts.note.clone().expect("a note");
    assert!(note.contains("github.com/acme/widget unreadable") && note.contains("bitbucket.org/team/repo no_credentials"), "{note}");
    world.done();

    // A failed call, and a stated total read short: partial.
    let world = World::new("github.com").await;
    world.github.on(&format!("{GITHUB}/releases?per_page=100"), vec![Answer::status(500, "down")]);
    world.github.json(&format!("{GITHUB}/actions/runs?per_page=100"), &serde_json::json!({ "total_count": 3, "workflow_runs": [{ "id": 10, "name": "ci", "status": "completed" }] }));
    world.github.json(&format!("{GITHUB}/environments?per_page=100"), &serde_json::json!({ "total_count": 2, "environments": [{ "id": 1 }, { "id": 2 }] }));
    world.github.on(&format!("{GITHUB}/deployments?per_page=100"), vec![Answer::status(429, "").with("retry-after", "1"), Answer::json(&serde_json::json!([]))]);
    let done = world.pass(&mut world.crawler()).await;
    let crawl = world.host_crawl().await;
    assert_eq!(text(&crawl, "outcome").as_deref(), Some("partial"));
    let note = text(&crawl, "note").expect("a note");
    assert!(note.contains(&format!("{GITHUB}/releases?per_page=100: HTTP 500")), "{note}");
    assert!(note.contains(&format!("{GITHUB}/actions/runs?per_page=100: 1 objects read of 3 stated")), "{note}");
    let calls = count(&world.db, "host_call").await;
    assert_eq!(
        ["calls", "failed", "rate_waits", "not_modified"].map(|k| int(&crawl, k)),
        [Some(calls), Some(1), Some(1), Some(0)]
    );
    assert!(int(&crawl, "waited_ms").is_some_and(|ms| ms >= 1000));
    assert!(int(&crawl, "objects_new").is_some_and(|n| n >= 4) && int(&crawl, "objects_changed") == Some(0), "{crawl:?}");
    let hosts = done.source("hosts").expect("the hosts source");
    assert_eq!((hosts.items, hosts.calls, hosts.failed, hosts.outcome), (1, calls, 1, Outcome::Partial));
    assert!(hosts.note.as_deref().is_some_and(|n| n.contains("github.com/acme/widget partial") && n.contains("1 objects read of 3 stated")), "{hosts:?}");
    let recorded = rows(&world.db, "SELECT counts, rows FROM pass_source WHERE source = 'hosts'").await;
    let counts = field(&recorded[0], "counts").expect("counts");
    assert_eq!(
        ["objects_new", "objects_changed", "not_modified", "rate_waits", "totals_stated", "totals_read"].map(|k| int(counts, k)),
        [int(&crawl, "objects_new"), Some(0), Some(0), Some(1), Some(5), Some(3)],
        "{counts:?}"
    );
    assert!(int(counts, "waited_ms").is_some_and(|ms| ms >= 1000));
    let written = field(&recorded[0], "rows").expect("rows");
    assert_eq!((int(written, "host_crawl"), int(written, "host_call")), (Some(1), Some(calls)));
    // Its own rows: every one the source wrote but the accounts, which
    // every worker shares.
    let own: i64 = ["host_repository", "host_crawl", "host_call", "host_object", "ci_run", "ci_run_state"].iter().filter_map(|t| int(written, t)).sum();
    assert_eq!(int(&crawl, "rows_written"), Some(own), "{written:?}");
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_records_lines_repos_and_hosts_and_a_stop_mid_crawl_records_it_stopped_and_writes_nothing_of_the_stopped_crawl() {
    let world = World::new("github.com").await;
    let mut crawler = world.crawler();
    let first = world.pass(&mut crawler).await;
    assert_eq!(first.sources.iter().map(|s| s.source).collect::<Vec<_>>(), ["lines", "repos", "hosts"]);
    let recorded = rows(&world.db, "SELECT VALUE source FROM pass_source").await;
    assert_eq!(recorded.len(), 3);
    let held = ["crawl", "git_call", "host_crawl", "host_call", "commit", "placement"];
    let mut before = Vec::new();
    for t in held {
        before.push(count(&world.db, t).await);
    }

    // The host refuses a call for the rate, naming no wait: the crawler
    // waits a minute, and a stop ends it there.
    let refused = format!("{GITHUB}/releases?per_page=100");
    world.github.on(&refused, vec![Answer::status(429, "")]);
    world.github.forget();
    let (asked, mut stop) = never_stop();
    let host = world.github.clone();
    let stopping = tokio::spawn(async move {
        while !host.paths().contains(&refused) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        asked.send_replace(true);
    });
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let stopped = pass_on(&world.kernel, &world.db, &world.params(), &mut crawler, &mut stop).await.expect("pass");
    stopping.await.expect("stopped");
    assert_eq!(stopped.outcome, "stopped", "{stopped:?}");
    assert_eq!(stopped.source("repos").map(|s| s.outcome), Some(Outcome::Stopped));
    assert_eq!(stopped.source("hosts").map(|s| s.outcome), Some(Outcome::Stopped));
    let mut after = Vec::new();
    for t in held {
        after.push(count(&world.db, t).await);
    }
    assert_eq!(after, before, "nothing of the stopped crawl written");
    let last = rows(&world.db, "SELECT outcome, valid_from FROM pass ORDER BY valid_from DESC LIMIT 1").await;
    assert_eq!(text(&last[0], "outcome").as_deref(), Some("stopped"));
    world.done();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_passs_log_line_names_hosts_items_and_host_calls() {
    let world = World::new("github.com").await;
    let done = world.pass(&mut world.crawler()).await;
    let logged = Logged::default();
    let hosts = done.source("hosts").expect("the hosts source").clone();
    tracing::subscriber::with_default(logged.clone(), || superx_mod_recon::crawler::log(Ok(done.clone())));
    let lines = logged.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(&format!(" hosts_items={}", hosts.items)) && lines[0].contains(&format!(" host_calls={}", hosts.calls)), "{lines:?}");
    assert!(hosts.items == 1 && hosts.calls > 1);
    world.done();
}
