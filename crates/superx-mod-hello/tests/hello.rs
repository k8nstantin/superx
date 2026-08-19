//! The reference module's contract tests on the in-memory engine.
//! The full own-db path (provision → greet → own log file) is QA'd
//! live per the epic — mem engines can't host the second database's
//! auth. What IS testable here: descriptor + facilities declarations,
//! CLI routing + usage errors, and the honest failure when no real
//! instance backs module_db.

use superx_kernel::{Kernel, KernelModule, SCHEMA_DDL};

use superx_mod_hello::{HelloModule, MODULE_NAME};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn fresh_kernel() -> Kernel {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    Kernel::from_db(db)
}

#[test]
fn facilities_are_declared() {
    let m = HelloModule;
    assert_eq!(m.descriptor().name, MODULE_NAME);
    assert!(m.schema_ddl().is_some(), "owns data objects");
    assert!(m.needs_dir(), "owns a dir");
    assert!(
        m.schema_ddl().unwrap().contains("superx_mod_hello"),
        "schema defines the module's own service account"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_usage_error_and_module_db_honesty() {
    let kernel = fresh_kernel().await;
    let m = HelloModule;

    let err = m
        .cli(&kernel, &["bogus".to_string()])
        .await
        .expect_err("unknown subcommand errors");
    assert!(err.to_string().contains("usage"));

    // On a test handle there is no real instance behind module_db —
    // the failure must say so, never panic.
    let err = m
        .cli(&kernel, &["greet".to_string(), "hi".to_string()])
        .await
        .expect_err("no instance behind module_db");
    assert!(err.to_string().contains("connected instance"), "{err}");
}
