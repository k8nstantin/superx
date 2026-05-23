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
    // 6 rows: node_run, node_agent, node_source, edge_owns,
    // attr_desc, attr_score.
    assert_eq!(counts.first().map(|c| c.count), Some(6));

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
async fn recent_telemetry_returns_events_newest_first() -> Result<(), Box<dyn Error>> {
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

    // Empty substrate — no telemetry yet.
    let events_empty = kernel.recent_telemetry(25).await?;
    assert!(events_empty.is_empty(), "fresh substrate must have no telemetry");

    // Seed + create + log produces a chain of events.
    kernel.seed_metamodel().await?;
    let _run = kernel.create_entity("node_run", "admin").await?;
    let _agent = kernel.create_entity("node_agent", "user").await?;
    let mut payload = surrealdb::types::Object::new();
    payload.insert(
        "phase".to_string(),
        surrealdb::types::Value::String("manual".to_string()),
    );
    let _ = kernel
        .log_telemetry(
            "system_pulse",
            surrealdb::types::Value::Object(payload),
            None,
        )
        .await?;

    // recent_telemetry returns the chain newest-first.
    let events = kernel.recent_telemetry(25).await?;
    assert!(
        events.len() >= 4,
        "expected at least 4 events: metamodel_seeded + 2 entity_created + 1 system_pulse, got {}",
        events.len()
    );
    assert_eq!(events[0].lifecycle_event, "system_pulse");

    // Limit clamps the row count.
    let events_limited = kernel.recent_telemetry(2).await?;
    assert_eq!(events_limited.len(), 2);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_relation_links_two_entities_via_edge_owns(
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
    kernel.seed_metamodel().await?;

    // Create a "parent" run and a "child" agent, then link them via
    // edge_owns. The schema's `relation.type` ASSERT requires the
    // type_definition to have category='edge'; seed_metamodel just
    // seeded edge_owns with that category.
    let parent = kernel.create_entity("node_run", "admin").await?;
    let child = kernel.create_entity("node_agent", "user").await?;

    let rel_id = kernel
        .create_relation(parent.clone(), child.clone(), "edge_owns", true)
        .await?;

    assert_eq!(rel_id.table, "relation".into());

    // Verify the row landed with the right typed FKs.
    #[derive(SurrealValue)]
    struct RelSlice {
        #[surreal(rename = "in")]
        in_: RecordId,
        out: RecordId,
        is_acyclic: bool,
    }
    let rows: Vec<RelSlice> = kernel
        .db()
        .query("SELECT in, out, is_acyclic FROM $id")
        .bind(("id", rel_id))
        .await?
        .take(0)?;
    let row = rows.into_iter().next().expect("relation row roundtrips");
    assert_eq!(row.in_, parent);
    assert_eq!(row.out, child);
    assert!(row.is_acyclic);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_relation_refuses_node_type_uid() -> Result<(), Box<dyn Error>> {
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
    kernel.seed_metamodel().await?;
    let a = kernel.create_entity("node_run", "admin").await?;
    let b = kernel.create_entity("node_agent", "user").await?;

    // Try to use a node-category type_definition as an edge — engine
    // must refuse via `relation.type ASSERT $value.category = 'edge'`.
    let err = kernel
        .create_relation(a, b, "node_run", true)
        .await
        .expect_err("must refuse when type's category is not 'edge'");

    let msg = err.to_string();
    assert!(
        msg.contains("edge") || msg.contains("category"),
        "expected engine ASSERT refusal mentioning category/edge, got: {msg}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersede_state_appends_and_current_state_returns_latest(
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
    kernel.seed_metamodel().await?;
    let entity = kernel.create_entity("node_agent", "user").await?;

    // Before any supersede_state, current_state is None.
    let initial = kernel.current_state(entity.clone(), "attr_desc").await?;
    assert!(initial.is_none());

    // First write: attr_desc with text "first description".
    let mut payload_v1 = surrealdb::types::Object::new();
    payload_v1.insert(
        "text".to_string(),
        surrealdb::types::Value::String("first description".to_string()),
    );
    let _id_v1 = kernel
        .supersede_state(
            entity.clone(),
            "attr_desc",
            surrealdb::types::Value::Object(payload_v1),
        )
        .await?;

    // current_state must return the v1 payload.
    let after_v1 = kernel
        .current_state(entity.clone(), "attr_desc")
        .await?
        .expect("v1 payload present");
    match &after_v1 {
        surrealdb::types::Value::Object(obj) => {
            let text = obj.get("text").expect("text field");
            assert_eq!(
                text,
                &surrealdb::types::Value::String("first description".to_string())
            );
        }
        other => panic!("expected Object payload, got {other:?}"),
    }

    // Supersede with v2.
    let mut payload_v2 = surrealdb::types::Object::new();
    payload_v2.insert(
        "text".to_string(),
        surrealdb::types::Value::String("revised description".to_string()),
    );
    let _id_v2 = kernel
        .supersede_state(
            entity.clone(),
            "attr_desc",
            surrealdb::types::Value::Object(payload_v2),
        )
        .await?;

    // current_state must now return v2 (latest by valid_from).
    let after_v2 = kernel
        .current_state(entity.clone(), "attr_desc")
        .await?
        .expect("v2 payload present");
    match &after_v2 {
        surrealdb::types::Value::Object(obj) => {
            let text = obj.get("text").expect("text field");
            assert_eq!(
                text,
                &surrealdb::types::Value::String("revised description".to_string())
            );
        }
        other => panic!("expected Object payload, got {other:?}"),
    }

    // Full history: 2 rows for this (target, type) chain.
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = kernel
        .db()
        .query(
            "SELECT count() AS count FROM state_ledger \
             WHERE target = $target \
             GROUP ALL",
        )
        .bind(("target", entity))
        .await?
        .take(0)?;
    assert_eq!(counts.first().map(|c| c.count), Some(2));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersede_state_refuses_node_type_uid() -> Result<(), Box<dyn Error>> {
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
    kernel.seed_metamodel().await?;
    let entity = kernel.create_entity("node_run", "admin").await?;

    // Try to use a node-category type as an attribute — engine must
    // refuse via `state_ledger.type ASSERT $value.category = 'attribute'`.
    let err = kernel
        .supersede_state(
            entity,
            "node_run",
            surrealdb::types::Value::Object(surrealdb::types::Object::new()),
        )
        .await
        .expect_err("must refuse when type's category is not 'attribute'");

    let msg = err.to_string();
    assert!(
        msg.contains("attribute") || msg.contains("category"),
        "expected engine ASSERT refusal mentioning category/attribute, got: {msg}"
    );

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
