//! The module's own service: the ledger as JSON, and the page.

use tower::ServiceExt as _;

async fn specs_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    db.query(superx_mod_specs::SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");
    db
}

async fn get(app: axum::Router, path: &str) -> (axum::http::StatusCode, String, String) {
    let r = app
        .oneshot(axum::http::Request::get(path).body(axum::body::Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = r.status();
    let kind = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.expect("body");
    (status, kind, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_service_answers_the_ledger_and_the_page() {
    let app = superx_mod_specs::server::router(specs_db().await, Some("http://127.0.0.1:5150".into()));

    let (status, kind, body) = get(app.clone(), "/api/ping").await;
    assert_eq!(status, 200);
    assert!(kind.starts_with("application/json"));
    assert!(body.contains(r#""module":"specs""#) && body.contains("5150"), "{body}");

    let (status, _, body) = get(app.clone(), "/api/ledger").await;
    assert_eq!((status.as_u16(), body.as_str()), (200, "[]"), "an empty database is an empty ledger");

    // Any other path is the page, served as HTML whatever the URL.
    let (status, kind, _) = get(app, "/task/ENG-1").await;
    assert_eq!(status, 200);
    assert!(kind.starts_with("text/html"), "{kind}");
}
