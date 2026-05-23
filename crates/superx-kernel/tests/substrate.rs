//! End-to-end tests for kernel substrate verbs.
//!
//! Each test spins up a fresh `mem://` engine, applies the locked
//! kernel DDL (with the `$SUPERX_KERNEL_PASSWORD` placeholder
//! substituted for a test value), signs in as `superx_kernel`, then
//! exercises the verbs.

use std::error::Error;

use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;
use surrealdb::types::{SurrealValue, Value};

use superx_kernel::{Kernel, SCHEMA_DDL};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";
const TEST_NS: &str = "superx";
const TEST_DB: &str = "kernel";

/// Spin up a mem:// substrate with the kernel schema applied + sign
/// in as superx_kernel. Returns a ready Kernel.
async fn fresh_kernel() -> Result<Kernel, Box<dyn Error>> {
    let db = connect("mem://").await?;
    db.use_ns(TEST_NS).use_db(TEST_DB).await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(Database {
        namespace: TEST_NS.to_string(),
        database: TEST_DB.to_string(),
        username: "superx_kernel".to_string(),
        password: TEST_PASSWORD.to_string(),
    })
    .await?;
    Ok(Kernel::from_db(db))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signin_succeeds_against_kernel_schema() -> Result<(), Box<dyn Error>> {
    let _kernel = fresh_kernel().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_type_definition_is_idempotent() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let first = kernel
        .ensure_type_definition("node_my_test_type", "node", "core")
        .await?;
    let second = kernel
        .ensure_type_definition("node_my_test_type", "node", "core")
        .await?;
    assert_eq!(first, second, "repeat call must return existing id");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_type_opt_returns_none_for_unseeded() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let result = kernel.find_type_opt("nonexistent_type").await?;
    assert!(result.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_type_returns_not_found_for_unseeded() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let err = kernel
        .find_type("nonexistent_type")
        .await
        .expect_err("must return NotFound");
    assert!(err.to_string().contains("not found"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_entity_writes_typed_row_with_uuidv7_id() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_my_kind", "node", "core")
        .await?;
    let id = kernel.create_entity("node_my_kind", "admin").await?;
    assert_eq!(id.table, "entity".into());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_entity_refuses_non_node_category() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    // An attribute-category type, not a node-category one.
    kernel
        .ensure_type_definition("attr_my_attr", "attribute", "working")
        .await?;
    let err = kernel
        .create_entity("attr_my_attr", "admin")
        .await
        .expect_err("engine must refuse non-node type for entity row");
    let msg = err.to_string();
    assert!(
        msg.contains("node") || msg.contains("category"),
        "expected ASSERT failure mentioning node/category, got: {msg}",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_relation_links_two_entities() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_thing", "node", "core")
        .await?;
    kernel
        .ensure_type_definition("edge_links", "edge", "core")
        .await?;

    let a = kernel.create_entity("node_thing", "admin").await?;
    let b = kernel.create_entity("node_thing", "admin").await?;
    let rel = kernel
        .create_relation(a.clone(), b.clone(), "edge_links", true)
        .await?;
    assert_eq!(rel.table, "relation".into());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_relation_refuses_node_type_as_edge() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_thing", "node", "core")
        .await?;
    let a = kernel.create_entity("node_thing", "admin").await?;
    let b = kernel.create_entity("node_thing", "admin").await?;
    let err = kernel
        .create_relation(a, b, "node_thing", true)
        .await
        .expect_err("engine must refuse node-category type for relation edge");
    let msg = err.to_string();
    assert!(
        msg.contains("edge") || msg.contains("category"),
        "expected ASSERT failure mentioning edge/category, got: {msg}",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersede_state_and_current_state_roundtrip() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_thing", "node", "core")
        .await?;
    kernel
        .ensure_type_definition("attr_thing_desc", "attribute", "working")
        .await?;

    let entity = kernel.create_entity("node_thing", "user").await?;

    // No state yet.
    let initial = kernel
        .current_state(entity.clone(), "attr_thing_desc")
        .await?;
    assert!(initial.is_none());

    // v1
    let mut p1 = surrealdb::types::Object::new();
    p1.insert("value".to_string(), Value::String("first".to_string()));
    kernel
        .supersede_state(entity.clone(), "attr_thing_desc", Value::Object(p1))
        .await?;

    let after_v1 = kernel
        .current_state(entity.clone(), "attr_thing_desc")
        .await?
        .expect("v1 present");
    match &after_v1 {
        Value::Object(o) => {
            assert_eq!(
                o.get("value"),
                Some(&Value::String("first".to_string())),
            );
        }
        other => panic!("expected Object, got {other:?}"),
    }

    // v2 — supersedes v1
    let mut p2 = surrealdb::types::Object::new();
    p2.insert("value".to_string(), Value::String("second".to_string()));
    kernel
        .supersede_state(entity.clone(), "attr_thing_desc", Value::Object(p2))
        .await?;

    let after_v2 = kernel
        .current_state(entity.clone(), "attr_thing_desc")
        .await?
        .expect("v2 present");
    match &after_v2 {
        Value::Object(o) => {
            assert_eq!(
                o.get("value"),
                Some(&Value::String("second".to_string())),
            );
        }
        other => panic!("expected Object, got {other:?}"),
    }

    // Append-only: chain has 2 rows for this (target, type) pair.
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
async fn kernel_struct_is_clone() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let cloned = kernel.clone();
    // Both clones see the same underlying db — both can find_type_opt.
    let result = cloned.find_type_opt("nonexistent").await?;
    assert!(result.is_none());
    drop(kernel);
    // After dropping the original, the clone still works (Arc-internal).
    let _ = cloned.find_type_opt("nonexistent").await?;
    Ok(())
}
