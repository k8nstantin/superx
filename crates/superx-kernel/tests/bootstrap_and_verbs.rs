//! End-to-end integration test: spin up the embedded `mem://` engine,
//! apply the locked DDL, sign in as `superx`, then exercise the kernel
//! verbs end-to-end — `seed_metamodel` + `create_entity` +
//! `log_telemetry` with the just-created entity as `run`.
//!
//! This is the proof the kernel works against the deployed schema:
//! engine accepts the typed FK, the metamodel uid lookup resolves, the
//! telemetry's `ASSERT $value.type.uid = 'node_run'` is satisfied by
//! the entity we just created.

use std::error::Error;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;
use surrealdb::types::{RecordId, SurrealValue};

use superx_kernel::{Kernel, SCHEMA_DDL};

const TEST_SERVICE_PASSWORD: &str = "test-service-password-for-mem-engine";
const TEST_NS: &str = "superx";
const TEST_DB: &str = "v01";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kernel_seeds_metamodel_creates_entity_and_logs_telemetry_end_to_end(
) -> Result<(), Box<dyn Error>> {
    // Provision the substrate: apply the locked DDL into a fresh mem
    // engine, then sign in as `superx`.
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

    // 1. Seed the metamodel — node_run, node_agent, node_source.
    kernel.seed_metamodel().await?;

    // Idempotent: re-running seed is a no-op (no duplicate rows).
    kernel.seed_metamodel().await?;

    // Both seed calls together produced exactly one row per uid (3 rows)
    // plus one `metamodel_seeded` telemetry row per call (2 rows).
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = kernel
        .db()
        .query("SELECT count() AS count FROM type_definition GROUP ALL")
        .await?
        .take(0)?;
    assert_eq!(counts.first().map(|c| c.count), Some(3));

    // 2. Create a node_run entity. find_type resolves uid='node_run'
    //    to the row seeded above; create_entity sets an explicit
    //    UUIDv7 id, the engine accepts the typed FK.
    let run_id = kernel.create_entity("node_run", "admin").await?;
    assert_eq!(run_id.table, "entity".into());

    // 3. Log telemetry with that run as the FK. The schema's
    //    `ASSERT $value = NONE OR $value.type.uid = 'node_run'` clause
    //    is satisfied because run_id points at a node_run entity.
    let mut payload = surrealdb::types::Object::new();
    payload.insert(
        "phase".to_string(),
        surrealdb::types::Value::String("end_to_end_smoke".to_string()),
    );
    let event_id = kernel
        .log_telemetry(
            "system_bootstrap",
            surrealdb::types::Value::Object(payload),
            Some(run_id.clone()),
        )
        .await?;
    assert_eq!(event_id.table, "telemetry_stream".into());

    // 4. Verify everything roundtrips via SELECT.
    #[derive(SurrealValue)]
    struct TelemetrySlice {
        lifecycle_event: String,
        run: Option<RecordId>,
    }
    let rows: Vec<TelemetrySlice> = kernel
        .db()
        .query("SELECT lifecycle_event, run FROM $id")
        .bind(("id", event_id))
        .await?
        .take(0)?;
    let row = rows.into_iter().next().expect("telemetry row roundtrips");
    assert_eq!(row.lifecycle_event, "system_bootstrap");
    assert_eq!(row.run, Some(run_id));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_entity_refuses_unknown_type_uid() -> Result<(), Box<dyn Error>> {
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

    // No seed yet — find_type for 'node_run' must return NotFound.
    let err = kernel
        .create_entity("node_run", "admin")
        .await
        .expect_err("must refuse when type_definition row is absent");

    let msg = err.to_string();
    assert!(
        msg.contains("not found"),
        "expected NotFound, got: {msg}"
    );

    Ok(())
}
