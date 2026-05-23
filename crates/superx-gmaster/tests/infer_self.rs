//! End-to-end test: spin up an embedded `mem://` substrate, apply
//! the locked DDL, sign in as `superx`, then run `gmaster infer`
//! against the `superx-kernel` source tree itself. Assert the run
//! produces non-zero counts of every node + edge kind and that the
//! `gmaster_infer_complete` telemetry event lands.

use std::error::Error;
use std::path::PathBuf;

use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;
use surrealdb::types::SurrealValue;

use superx_gmaster::infer;
use superx_kernel::{Kernel, SCHEMA_DDL};

const TEST_SERVICE_PASSWORD: &str = "test-service-password-for-mem-engine";
const TEST_NS: &str = "superx";
const TEST_DB: &str = "v01";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gmaster_infer_self_extracts_entities_and_relations(
) -> Result<(), Box<dyn Error>> {
    let db = connect("mem://").await?;
    db.use_ns(TEST_NS).use_db(TEST_DB).await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_SERVICE_PASSWORD", TEST_SERVICE_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(Database {
        namespace: TEST_NS.to_string(),
        database: TEST_DB.to_string(),
        username: "superx".to_string(),
        password: TEST_SERVICE_PASSWORD.to_string(),
    })
    .await?;

    let kernel = Kernel::from_db(db);

    // Point gmaster at the superx-kernel crate's source tree — that's
    // an in-repo Rust codebase we can reason about deterministically.
    // Tests dir is `crates/superx-gmaster/tests/`, so kernel is at
    // `../superx-kernel/src`.
    let kernel_src: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "superx-kernel", "src"]
        .iter()
        .collect();

    let stats = infer(&kernel, &kernel_src).await?;

    assert!(stats.files >= 1, "must find at least one .rs file: {stats:?}");
    assert!(
        stats.functions >= 5,
        "expected several pub fns (connect_service, create_entity, …) in superx-kernel: {stats:?}",
    );
    assert!(
        stats.classes >= 1,
        "expected at least one struct/enum (Kernel, KernelError, …): {stats:?}",
    );
    assert!(
        stats.edges_defines >= stats.functions + stats.classes,
        "every fn/class should have an edge_defines from its file: {stats:?}",
    );

    // The pipeline emits a `gmaster_infer_complete` event at the end.
    // Verify it landed via the telemetry firehose.
    let events = kernel.recent_telemetry(500).await?;
    assert!(
        events
            .iter()
            .any(|e| e.lifecycle_event == "gmaster_infer_complete"),
        "must emit gmaster_infer_complete telemetry: {} events found",
        events.len(),
    );

    // And every node should have its attr_desc roundtripping.
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = kernel
        .db()
        .query("SELECT count() AS count FROM state_ledger GROUP ALL")
        .await?
        .take(0)?;
    let total_attr_rows = counts.first().map(|c| c.count).unwrap_or(0);
    let total_nodes = stats.files + stats.functions + stats.classes + stats.modules;
    assert!(
        total_attr_rows >= total_nodes as i64,
        "every entity should carry an attr_desc row — got {total_attr_rows} for {total_nodes} nodes",
    );

    Ok(())
}
