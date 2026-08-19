//! End-to-end tests for kernel substrate verbs against the locked v2
//! schema on a fresh `mem://` engine.

mod common;

use std::error::Error;

use surrealdb::types::{Object, SurrealValue, Value};

use common::fresh_kernel;

fn obj(key: &str, val: &str) -> Value {
    let mut payload = Object::new();
    payload.insert(key.to_string(), Value::String(val.to_string()));
    Value::Object(payload)
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
async fn unique_index_refuses_duplicate_type_uid() -> Result<(), Box<dyn Error>> {
    // The race-safety backstop from SUPERX_SCHEMA.md §4: a second row
    // with the same uid must be refused by the engine itself, not by
    // kernel courtesy. Bypass the verb and write raw.
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_uniq", "node", "core")
        .await?;
    let dup = kernel
        .db()
        .query(
            "CREATE type_definition CONTENT { \
               uid: 'node_uniq', category: 'node', is_acyclic: true, \
               sch_json: NONE, memory_tier: 'core', valid_from: time::now() }",
        )
        .await?
        .check();
    assert!(dup.is_err(), "engine must refuse the duplicate uid");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_type_opt_returns_none_for_unseeded() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    assert!(kernel.find_type_opt("nonexistent_type").await?.is_none());
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
async fn create_entity_writes_typed_row() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_my_kind", "node", "core")
        .await?;
    let id = kernel.create_entity("node_my_kind").await?;
    assert_eq!(id.table, "entity".into());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_entity_refuses_non_node_category() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("attr_my_attr", "attribute", "working")
        .await?;
    let err = kernel
        .create_entity("attr_my_attr")
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

    let a = kernel.create_entity("node_thing").await?;
    let b = kernel.create_entity("node_thing").await?;
    let rel = kernel.create_relation(a, b, "edge_links", true).await?;
    assert_eq!(rel.table, "relation".into());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_relation_refuses_non_edge_category() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_thing2", "node", "core")
        .await?;

    let a = kernel.create_entity("node_thing2").await?;
    let b = kernel.create_entity("node_thing2").await?;
    let err = kernel
        .create_relation(a, b, "node_thing2", true)
        .await
        .expect_err("engine must refuse node type for relation row");
    let msg = err.to_string();
    assert!(
        msg.contains("edge") || msg.contains("category"),
        "expected ASSERT failure mentioning edge/category, got: {msg}",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_state_is_none_before_any_write() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_x", "node", "core")
        .await?;
    kernel
        .ensure_type_definition("attr_desc", "attribute", "working")
        .await?;
    let e = kernel.create_entity("node_x").await?;
    assert!(kernel.current_state(e, "attr_desc").await?.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersede_state_latest_wins_and_history_is_preserved() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_x2", "node", "core")
        .await?;
    kernel
        .ensure_type_definition("attr_desc", "attribute", "working")
        .await?;
    let e = kernel.create_entity("node_x2").await?;

    kernel
        .supersede_state(e.clone(), "attr_desc", obj("text", "v1"))
        .await?;
    kernel
        .supersede_state(e.clone(), "attr_desc", obj("text", "v2"))
        .await?;

    // Latest wins at read time.
    let current = kernel
        .current_state(e.clone(), "attr_desc")
        .await?
        .expect("state exists");
    assert_eq!(current, obj("text", "v2"));

    // Append-only: both rows still exist — N writes → N rows.
    #[derive(SurrealValue)]
    struct CountRow {
        c: i64,
    }
    let rows: Vec<CountRow> = kernel
        .db()
        .query("SELECT count() AS c FROM state_ledger WHERE target = $target GROUP ALL")
        .bind(("target", e))
        .await?
        .take(0)?;
    assert_eq!(rows[0].c, 2, "history preserved, no row mutated");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersede_state_refuses_non_attribute_category() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_x3", "node", "core")
        .await?;
    let e = kernel.create_entity("node_x3").await?;
    let err = kernel
        .supersede_state(e, "node_x3", obj("text", "nope"))
        .await
        .expect_err("engine must refuse node type for state_ledger row");
    let msg = err.to_string();
    assert!(
        msg.contains("attribute") || msg.contains("category"),
        "expected ASSERT failure mentioning attribute/category, got: {msg}",
    );
    Ok(())
}

#[test]
fn already_exists_classifier() {
    use superx_kernel::provision::is_already_exists;
    assert!(is_already_exists("The table 'module' already exists"));
    assert!(!is_already_exists("Found type 'string' for field 'uid'"));
}
