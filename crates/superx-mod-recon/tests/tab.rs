//! The tab (#539): its page at the module's own URL, the crawler's stats per
//! day, and the parameters, read and set from the page as
//! `superx param set recon` sets them.

pub mod common;

use std::collections::BTreeMap;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use common::*;
use superx_kernel::types::Value;
use superx_mod_recon::api::{days, Day, WRITE_HEADER};
use superx_mod_recon::crawler::Crawler;
use superx_mod_recon::instance::Instance;
use superx_mod_recon::params::{
    self, ALL, BLAMES_AT_ONCE_PARAM, BLAME_FILES_PARAM, CRAWL_SECS_PARAM, INCLUDE_PARAM, NAMES,
};
use superx_mod_recon::{registry, server};
use tower::ServiceExt as _;

const PORT: u16 = 5152;

fn app(kernel: &superx_kernel::Kernel) -> axum::Router {
    let home = scratch();
    let instance = Instance {
        home: home.clone(),
        endpoint: "mem://".to_string(),
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        data_dir: home.join("db"),
        log_filter: "info".to_string(),
    };
    server::app(kernel.clone(), instance, PORT)
}

/// One request to the module's routes, addressed to it by `host`: the status
/// and the body, as JSON when it is.
async fn ask(app: &axum::Router, host: &str, method: &str, path: &str, write: bool, body: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path).header(header::HOST, host);
    if write {
        request = request.header(WRITE_HEADER, "write");
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).expect("a request");
    let answer = app.clone().oneshot(request).await.expect("an answer");
    let status = answer.status();
    let bytes = to_bytes(answer.into_body(), usize::MAX).await.expect("its body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn own() -> String {
    format!("127.0.0.1:{PORT}")
}

fn listed(body: &str) -> Vec<serde_json::Value> {
    let json: serde_json::Value = serde_json::from_str(body).expect("json");
    json["params"].as_array().expect("params").clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_is_served_at_the_modules_own_url_and_a_request_addressed_to_another_name_is_refused() {
    let kernel = kernel().await;
    let app = app(&kernel);
    let (status, page) = ask(&app, &own(), "GET", "/", false, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("<div id=\"root\"></div>") && page.contains("SuperX · Recon"), "{page}");
    let (status, again) = ask(&app, &own(), "GET", "/a/view/of/the/page", false, None).await;
    assert_eq!((status, again == page), (StatusCode::OK, true), "any other path is the page itself");
    for path in ["/", "/api/params", "/api/stats"] {
        let (status, _) = ask(&app, &format!("evil.example:{PORT}"), "GET", path, false, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stats_per_day_add_up_every_pass_by_outcome_and_each_sources_lines_git_calls_and_host_calls() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "one\n")], "first");
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir]))]);
    let mut crawler = Crawler::default();
    pass(&kernel, &db, &p, &mut crawler).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    // An earlier day's pass: partial, its hosts' calls one failed.
    db.query(
        "LET $p = CREATE ONLY type::record('pass', rand::uuid::v7()) SET started_at = d'2026-10-29T23:59:00Z', \
         finished_at = d'2026-10-29T23:59:59Z', rows_written = 7, outcome = 'partial', valid_from = d'2026-10-29T23:59:59Z';\n\
         CREATE type::record('pass_source', rand::uuid::v7()) SET pass = $p.id, source = 'hosts', started_at = d'2026-10-29T23:59:00Z', \
         finished_at = d'2026-10-29T23:59:59Z', items = 2, unread = 0, calls = 5, failed = 1, timed_out = 0, rows = {}, counts = {}, \
         crawls = [], outcome = 'partial', valid_from = d'2026-10-29T23:59:59Z';",
    )
    .await
    .expect("an earlier pass")
    .check()
    .expect("written");

    // The same, from a scan of every row.
    let day_of = |row: &Value| match field(row, "finished_at") {
        Some(Value::Datetime(d)) => chrono::DateTime::<chrono::Utc>::from(*d).format("%Y-%m-%d").to_string(),
        other => panic!("a time: {other:?}"),
    };
    let mut want: BTreeMap<String, Day> = BTreeMap::new();
    for row in rows(&db, "SELECT finished_at, outcome, rows_written FROM pass").await {
        let day = day_of(&row);
        let d = want.entry(day.clone()).or_insert_with(|| Day { day, ..Day::default() });
        *d.passes.entry(text(&row, "outcome").expect("outcome")).or_insert(0) += 1;
        d.rows += int(&row, "rows_written").expect("rows");
    }
    for row in rows(&db, "SELECT finished_at, source, items, calls, failed FROM pass_source").await {
        let day = day_of(&row);
        let d = want.entry(day.clone()).or_insert_with(|| Day { day, ..Day::default() });
        let (items, calls, failed) = (int(&row, "items").expect("items"), int(&row, "calls").expect("calls"), int(&row, "failed").expect("failed"));
        match text(&row, "source").as_deref() {
            Some("lines") => d.lines += items,
            Some("repos") => (d.git_calls, d.git_failed) = (d.git_calls + calls, d.git_failed + failed),
            Some("hosts") => (d.host_calls, d.host_failed) = (d.host_calls + calls, d.host_failed + failed),
            other => panic!("a source: {other:?}"),
        }
    }
    let read = days(&db).await.expect("days");
    assert_eq!(read, want.into_values().collect::<Vec<_>>(), "the stats are the rows, day by day");
    assert_eq!(read.len(), 2, "{read:?}");
    assert_eq!(
        (read[0].day.as_str(), read[0].passes.clone(), read[0].rows, read[0].host_calls, read[0].host_failed),
        ("2026-10-29", BTreeMap::from([("partial".to_string(), 1)]), 7, 5, 1)
    );
    assert_eq!(read[1].passes.values().sum::<i64>(), 2, "today's two passes: {read:?}");
    assert!(read[1].git_calls > 0, "the crawl's git calls: {read:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_parameter_is_listed_in_its_order_with_its_value_whether_it_is_set_its_shape_its_group_and_what_it_decides() {
    let kernel = kernel().await;
    let app = app(&kernel);
    let (status, body) = ask(&app, &own(), "GET", "/api/params", false, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let params = listed(&body);
    let names: Vec<&str> = params.iter().map(|p| p["name"].as_str().expect("a name")).collect();
    assert_eq!(names, NAMES, "in the order status lists them");
    for p in &params {
        assert_eq!(p["set"], false, "nothing set before the module is registered: {p}");
        for key in ["value", "wanted", "group", "decides", "input"] {
            assert!(p[key].as_str().is_some_and(|s| !s.is_empty()), "{key}: {p}");
        }
    }
    let crawl = params.iter().find(|p| p["name"] == CRAWL_SECS_PARAM).expect("listed");
    assert_eq!((crawl["value"].as_str(), crawl["group"].as_str(), crawl["input"].as_str()), (Some("600"), Some("when"), Some("number")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parameter_set_from_the_page_is_written_as_superx_param_set_writes_it_and_the_crawler_reads_it_at_its_next_pass() {
    let kernel = kernel().await;
    let entity = registry::starting(&kernel).await.expect("registers");
    let app = app(&kernel);
    let (status, body) = ask(&app, &own(), "PUT", &format!("/api/params/{BLAMES_AT_ONCE_PARAM}"), true, Some("3")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let shown = listed(&body).into_iter().find(|p| p["name"] == BLAMES_AT_ONCE_PARAM).expect("listed");
    assert_eq!((shown["value"].as_str(), shown["set"].as_bool()), (Some("3"), Some(true)), "the answer is the parameters as they now stand");
    assert_eq!(kernel.get_parameter(entity.clone(), BLAMES_AT_ONCE_PARAM).await.expect("reads"), Some(number(3)), "the row superx param set writes");
    assert_eq!(params::resolve(&kernel).await.expect("resolves").params.blames_at_once, 3, "what the next pass reads");

    // A limit set, then lifted again.
    let (status, body) = ask(&app, &own(), "PUT", &format!("/api/params/{BLAME_FILES_PARAM}"), true, Some("40")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(params::resolve(&kernel).await.expect("resolves").params.blame_files, 40);
    let (status, body) = ask(&app, &own(), "PUT", &format!("/api/params/{BLAME_FILES_PARAM}"), true, Some("\"all\"")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let resolved = params::resolve(&kernel).await.expect("resolves");
    assert_eq!(resolved.params.blame_files, ALL, "all again");
    assert!(resolved.problems.is_empty(), "{:?}", resolved.problems);

    // A list.
    let (status, body) = ask(&app, &own(), "PUT", &format!("/api/params/{INCLUDE_PARAM}"), true, Some("[\"/a\", \"/b\"]")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(params::resolve(&kernel).await.expect("resolves").params.include, ["/a", "/b"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_without_the_write_header_to_a_name_that_is_no_parameter_or_of_the_wrong_shape_is_refused_and_writes_nothing() {
    let kernel = kernel().await;
    registry::starting(&kernel).await.expect("registers");
    let app = app(&kernel);
    let path = format!("/api/params/{BLAMES_AT_ONCE_PARAM}");
    let (status, _) = ask(&app, &own(), "PUT", &path, false, Some("3")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a page of another site cannot send the header");
    let (status, body) = ask(&app, &own(), "PUT", "/api/params/attr_recon_none", true, Some("3")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    for (wrong, wanted) in [("0", "a whole number, 1 or more"), ("-1", "a whole number, 1 or more"), ("\"all\"", "a whole number, 1 or more")] {
        let (status, body) = ask(&app, &own(), "PUT", &path, true, Some(wrong)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{wrong}: {body}");
        assert!(body.contains(BLAMES_AT_ONCE_PARAM) && body.contains(wanted), "named, with the shape it wants: {body}");
    }
    let (status, body) = ask(&app, &own(), "PUT", &format!("/api/params/{CRAWL_SECS_PARAM}"), true, Some("\"soon\"")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let resolved = params::resolve(&kernel).await.expect("resolves");
    assert!(resolved.settings.iter().all(|s| !s.set), "nothing written: {:?}", resolved.settings);
}
