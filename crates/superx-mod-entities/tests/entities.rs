//! Contract tests for the entities module (epic #166, phase E1): the
//! declared facilities, the schema's teeth (ASSERT, ENFORCED), and
//! the type registry's idempotent seeding — all on `mem://`.

use superx_kernel::types::Value;
use superx_kernel::KernelModule as _;
use superx_mod_entities::{registry, EntitiesModule, MODULE_NAME, SCHEMA_DDL};

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema applies clean");
    db
}

#[test]
fn facilities_declared() {
    let d = EntitiesModule.descriptor();
    assert_eq!(d.name, MODULE_NAME);
    assert!(EntitiesModule.needs_dir(), "owns modules/entities/ for document files");
    assert!(EntitiesModule.schema_ddl().is_some(), "owns superx/entities");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeding_is_idempotent() {
    let db = fresh_db().await;
    let first = registry::seed_types(&db).await.expect("seed");
    assert_eq!(first, registry::SEEDED_TYPES.len(), "all contract types created");
    let second = registry::seed_types(&db).await.expect("re-seed");
    assert_eq!(second, 0, "re-seed creates nothing");
    let rows = registry::list_types(&db).await.expect("list");
    assert_eq!(rows.len(), registry::SEEDED_TYPES.len());
    assert!(rows.iter().any(|r| r.name == "text" && r.category == "entity"));
    assert!(rows.iter().any(|r| r.name == "instructs" && r.category == "relation"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_type_extension_and_refusals() {
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    registry::add_type(&db, "review", "relation", Some("qa reviews an artifact"))
        .await
        .expect("new type registers");
    let rows = registry::list_types(&db).await.expect("list");
    assert_eq!(rows.len(), registry::SEEDED_TYPES.len() + 1);

    let dup = registry::add_type(&db, "product", "entity", None).await;
    assert!(dup.unwrap_err().to_string().contains("already exists"));

    let bad_cat = registry::add_type(&db, "widget", "thing", None).await;
    assert!(bad_cat.unwrap_err().to_string().contains("entity | relation"));

    let bad_name = registry::add_type(&db, "Bad Name", "entity", None).await;
    assert!(bad_name.unwrap_err().to_string().contains("lowercase"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_asserts_category() {
    let db = fresh_db().await;
    let refused = db
        .query("CREATE entity_type SET name = 'x', category = 'bogus', valid_from = time::now()")
        .await
        .expect("query runs")
        .check();
    assert!(refused.is_err(), "ASSERT INSIDE rejects unknown categories");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edge_relation_is_enforced() {
    let db = fresh_db().await;

    // Two real anchors.
    let a = registry::new_id("entity");
    let b = registry::new_id("entity");
    for id in [&a, &b] {
        db.query("CREATE $id SET entity_type = 'product', created_at = time::now()")
            .bind(("id", id.clone()))
            .await
            .expect("query")
            .check()
            .expect("anchor created");
    }

    // A real edge between them, with the D19 field contract.
    db.query(
        "RELATE $a->edge->$b SET edge_uid = $uid, rel_type = 'contains', \
         active = true, valid_from = time::now()",
    )
    .bind(("a", a.clone()))
    .bind(("b", b.clone()))
    .bind(("uid", uuid::Uuid::now_v7().to_string()))
    .await
    .expect("query")
    .check()
    .expect("edge between real anchors is accepted");

    // ENFORCED: an edge to a non-existent anchor is refused.
    let ghost = registry::new_id("entity");
    let refused = db
        .query(
            "RELATE $a->edge->$ghost SET edge_uid = $uid, rel_type = 'contains', \
             active = true, valid_from = time::now()",
        )
        .bind(("a", a))
        .bind(("ghost", ghost))
        .bind(("uid", uuid::Uuid::now_v7().to_string()))
        .await
        .expect("query")
        .check();
    assert!(refused.is_err(), "ENFORCED refuses dangling edges");

    // Pointer traversal sees the edge.
    let mut resp = db
        .query("SELECT ->edge->entity AS reached FROM $b_from")
        .bind(("b_from", b))
        .await
        .expect("traversal query");
    let _rows: Vec<Value> = resp.take(0).expect("traversal parses");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_usage_and_unprovisioned_honesty() {
    let db = fresh_db().await;
    let kernel = superx_kernel::Kernel::from_db(db);

    let usage = EntitiesModule.cli(&kernel, &[]).await;
    assert!(usage.unwrap_err().to_string().contains("usage: superx entities"));

    // A test handle has no endpoint/credentials: the module's own-db
    // commands must fail honestly, pointing at provisioning.
    let honest = EntitiesModule.cli(&kernel, &["types".to_string()]).await;
    assert!(honest.is_err());
}
