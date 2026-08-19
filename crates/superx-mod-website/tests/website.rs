//! Contract tests for the website module (epic #213): the minimal
//! data-less module — no schema, no dir, a CLI, an embedded site.

use superx_kernel::KernelModule as _;
use superx_mod_website::{WebsiteModule, MODULE_NAME, PUBLIC_URL};

#[test]
fn facilities_declared() {
    let d = WebsiteModule.descriptor();
    assert_eq!(d.name, MODULE_NAME);
    assert!(WebsiteModule.schema_ddl().is_none(), "data-less by design");
    assert!(!WebsiteModule.needs_dir(), "the site lives in git, not the instance");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_prints_both_urls() {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let kernel = superx_kernel::Kernel::from_db(db);
    let out = WebsiteModule
        .cli(&kernel, &["url".to_string()])
        .await
        .expect("url");
    assert!(out.contains("127.0.0.1:5151"), "{out}");
    assert!(out.contains(PUBLIC_URL), "{out}");

    let usage = WebsiteModule.cli(&kernel, &[]).await;
    assert!(usage.unwrap_err().to_string().contains("usage: superx website"));
}
