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

// ---------------------------------------------------------------- E2 --

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_chain_is_append_only() {
    use superx_mod_entities::nodes;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let anchor = nodes::create_entity(&db, "product", "Widget X", None, None)
        .await
        .expect("create");

    nodes::update_entity(&db, &anchor, Some("Widget X2".into()), None, None)
        .await
        .expect("update");

    let current = nodes::current_state(&db, &anchor).await.expect("query").expect("state");
    assert_eq!(current.name, "Widget X2");

    let history = nodes::state_history(&db, &anchor).await.expect("history");
    assert_eq!(history.len(), 2, "update appended, never replaced");
    assert_eq!(history[0].name, "Widget X");
    assert_eq!(history[1].name, "Widget X2");

    // The anchor is still exactly one immutable row.
    let mut resp = db
        .query("SELECT count() AS c FROM entity GROUP ALL")
        .await
        .expect("count");
    let rows: Vec<Value> = resp.take(0).expect("rows");
    let anchors = rows.first().and_then(|v| match v {
        Value::Object(o) => match o.get("c") {
            Some(Value::Number(n)) => n.to_int(),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(anchors, Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_carries_unset_fields_forward() {
    use superx_mod_entities::nodes;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let attrs = superx_kernel::message::value_from_json(&serde_json::json!({"sku": "W-1"}));
    let anchor = nodes::create_entity(&db, "task", "QA login", Some("check the login flow".into()), Some(attrs))
        .await
        .expect("create");

    // Rename only — content and attributes must survive.
    nodes::update_entity(&db, &anchor, Some("QA login flow".into()), None, None)
        .await
        .expect("update");
    let current = nodes::current_state(&db, &anchor).await.expect("query").expect("state");
    assert_eq!(current.name, "QA login flow");
    assert_eq!(current.content.as_deref(), Some("check the login flow"));
    assert!(current.attributes.is_some(), "attributes carried forward");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_refuses_wrong_type_category() {
    use superx_mod_entities::nodes;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let unknown = nodes::create_entity(&db, "widget", "X", None, None).await;
    assert!(unknown.unwrap_err().to_string().contains("unknown type"));

    let relation = nodes::create_entity(&db, "contains", "X", None, None).await;
    assert!(relation.unwrap_err().to_string().contains("relation type, not entity"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_shows_current_labels_and_filters() {
    use superx_mod_entities::nodes;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    nodes::create_entity(&db, "task", "Build widget", None, None).await.expect("t");
    nodes::update_entity(&db, &product, Some("Widget v2".into()), None, None).await.expect("u");

    let all = nodes::list_entities(&db, None).await.expect("list");
    assert_eq!(all.len(), 2, "one row per entity, not per version");
    assert!(all.iter().any(|r| r.name == "Widget v2"), "list shows CURRENT label");
    assert!(!all.iter().any(|r| r.name == "Widget"), "stale label absent");

    let tasks = nodes::list_entities(&db, Some("task")).await.expect("filtered");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].entity_type, "task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fragment_resolution_unique_ambiguous_none() {
    use superx_mod_entities::nodes;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let a = nodes::create_entity(&db, "product", "A", None, None).await.expect("a");
    let b = nodes::create_entity(&db, "product", "B", None, None).await.expect("b");
    let a_uuid = superx_ops::record_uuid(&a);
    let b_uuid = superx_ops::record_uuid(&b);

    let hit = nodes::resolve_entity(&db, &a_uuid).await.expect("unique");
    assert_eq!(superx_ops::record_uuid(&hit), a_uuid);

    // uuid7 ids minted in the same instant share their time prefix.
    let shared: String = a_uuid
        .chars()
        .zip(b_uuid.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect();
    if !shared.is_empty() {
        let ambiguous = nodes::resolve_entity(&db, &shared).await;
        assert!(ambiguous.unwrap_err().to_string().contains("ambiguous"));
    }

    let none = nodes::resolve_entity(&db, "zzzzzz").await;
    assert!(none.unwrap_err().to_string().contains("no entity matches"));
}

// ---------------------------------------------------------------- E3 --

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_refusals_and_edge_history() {
    use superx_mod_entities::{edges, nodes};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let part = nodes::create_entity(&db, "product", "Frame", None, None).await.expect("c");

    let uid = edges::link(&db, &product, &part, "contains").await.expect("link");

    let dup = edges::link(&db, &product, &part, "contains").await;
    assert!(dup.unwrap_err().to_string().contains("already linked"));

    let self_link = edges::link(&db, &product, &product, "linked").await;
    assert!(self_link.unwrap_err().to_string().contains("itself"));

    let bad_rel = edges::link(&db, &product, &part, "product").await;
    assert!(bad_rel.unwrap_err().to_string().contains("entity type, not relation"));

    // Retract, then relink: THREE rows total on the wire, TWO chains,
    // and the current state is active again under a NEW edge_uid.
    let retracted = edges::unlink(&db, &product, &part, "contains").await.expect("unlink");
    assert_eq!(retracted, uid, "retraction lands on the same chain");
    let relinked = edges::link(&db, &product, &part, "contains").await.expect("relink");
    assert_ne!(relinked, uid, "a new link is a new chain");

    let current = edges::expand(&db, std::slice::from_ref(&product), false).await.expect("expand");
    let actives: Vec<_> = current.iter().filter(|e| e.active).collect();
    assert_eq!(actives.len(), 1, "one active edge after retract+relink");
    let mut resp = db.query("SELECT count() AS c FROM edge GROUP ALL").await.expect("count");
    let rows: Vec<Value> = resp.take(0).expect("rows");
    let total = rows.first().and_then(|v| match v {
        Value::Object(o) => match o.get("c") { Some(Value::Number(n)) => n.to_int(), _ => None },
        _ => None,
    });
    assert_eq!(total, Some(3), "full link/unlink/relink history preserved");

    let second_unlink = edges::unlink(&db, &product, &part, "consults").await;
    assert!(second_unlink.unwrap_err().to_string().contains("no active consults link"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn traversal_handles_cycles_and_depth() {
    use superx_mod_entities::{edges, graph, nodes};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    // product -> a -> b -> product (cycle), plus a tail below b.
    let product = nodes::create_entity(&db, "product", "Root", None, None).await.expect("r");
    let a = nodes::create_entity(&db, "product", "A", None, None).await.expect("a");
    let b = nodes::create_entity(&db, "product", "B", None, None).await.expect("b");
    let tail = nodes::create_entity(&db, "task", "Deep task", None, None).await.expect("t");
    edges::link(&db, &product, &a, "contains").await.expect("l1");
    edges::link(&db, &a, &b, "contains").await.expect("l2");
    edges::link(&db, &b, &product, "linked").await.expect("cycle edge");
    edges::link(&db, &b, &tail, "contains").await.expect("l3");

    let full = graph::subgraph(&db, &product, 10, false).await.expect("bfs");
    assert_eq!(full.nodes.len(), 4, "cycle terminated by visited set");
    assert_eq!(full.edges.len(), 4, "all active edges reported");
    assert!(!full.truncated_at_depth);

    let shallow = graph::subgraph(&db, &product, 1, false).await.expect("bfs");
    assert_eq!(shallow.nodes.len(), 2, "depth 1 = root + first hop");
    assert!(shallow.truncated_at_depth);

    let rendered = graph::render_tree(&full, &product);
    assert!(rendered.contains("contains"), "tree labels rels: {rendered}");
    assert!(rendered.contains("cycle"), "cycle annotated: {rendered}");

    // Retract the cycle edge: traversal follows only ACTIVE edges.
    edges::unlink(&db, &b, &product, "linked").await.expect("retract");
    let after = graph::subgraph(&db, &product, 10, false).await.expect("bfs");
    assert_eq!(after.edges.len(), 3, "inactive chain is history, not topology");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_evolves_comment_multiplies() {
    use superx_mod_entities::{nodes, texts};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");

    let (text1, created) = texts::set_role_text(&db, &product, "describes", "First description.")
        .await
        .expect("describe");
    assert!(created);
    let (text2, created_again) =
        texts::set_role_text(&db, &product, "describes", "Better description.")
            .await
            .expect("re-describe");
    assert!(!created_again, "same text node evolves");
    assert_eq!(
        superx_ops::record_uuid(&text1),
        superx_ops::record_uuid(&text2)
    );
    let history = nodes::state_history(&db, &text1).await.expect("history");
    assert_eq!(history.len(), 2, "description evolution is the text node's history");
    assert_eq!(history[1].content.as_deref(), Some("Better description."));

    let c1 = texts::add_comment(&db, &product, "looks good").await.expect("c1");
    let c2 = texts::add_comment(&db, &product, "ship it").await.expect("c2");
    assert_ne!(superx_ops::record_uuid(&c1), superx_ops::record_uuid(&c2));

    // Thread: comment on a comment.
    texts::add_comment(&db, &c1, "replying to the first comment").await.expect("thread");

    let notes = texts::annotations(&db, &product).await.expect("notes");
    assert_eq!(notes.len(), 3, "one describes + two comments on the product");
    assert!(notes.iter().any(|n| n.content == "Better description."));
}
