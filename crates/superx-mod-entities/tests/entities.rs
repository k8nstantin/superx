//! Contract tests for the entities module (epic #166, phase E1): the
//! declared facilities, the schema's teeth (ASSERT, ENFORCED), and
//! the type registry's idempotent seeding — all on `mem://`.

use superx_kernel::types::Value;
use superx_kernel::KernelModule as _;
use superx_mod_entities::notes::Author;
use superx_mod_entities::{dictionary, registry, EntitiesModule, MODULE_NAME, SCHEMA_DDL};

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema applies clean");
    // Startup seeds the dictionary before serving (#266/#268), and prose
    // writes refuse a label it does not define — so a test database
    // without it is not a smaller instance, it is a broken one.
    dictionary::seed(&db).await.expect("dictionary");
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
async fn ui_api_round_trip_create_detail_update_comment_link_types() {
    use superx_mod_entities::api;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    // Types: ENTITY kinds only — relations are not "types" (operator
    // model); they surface separately for the link picker.
    let types = api::types_list(&db).await.expect("types");
    assert!(types.iter().any(|t| t.name == "product"));
    assert!(
        !types.iter().any(|t| t.name == "depends_on"),
        "relation kinds must not appear as types"
    );
    let rels = api::rel_types(&db).await.expect("rels");
    assert!(rels.contains(&"depends_on".to_string()));
    // The text carrier is registered but never hand-created: flagged
    // so the create form drops it.
    assert!(types.iter().any(|t| t.name == "text" && t.system));
    assert!(types.iter().all(|t| t.name == "text" || !t.system));

    // Create with a markdown description + JSON attributes.
    let product = api::create(
        &db,
        &api::CreateReq {
            entity_type: "product".into(),
            name: "Widget X".into(),
            description: Some("**Widget X** is the composable widget product.".into()),
            content: None,
            attributes_json: Some(r#"{"owner": "calexander"}"#.into()),
        },
    )
    .await
    .expect("create");
    let task = api::create(
        &db,
        &api::CreateReq {
            entity_type: "task".into(),
            name: "Build the widget".into(),
            description: None,
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("create task");

    // Detail: description annotation + attributes round-trip.
    let d = api::detail(&db, &product).await.expect("detail");
    assert_eq!(d.name, "Widget X");
    assert_eq!(d.entity_type, "product");
    // Prose arrives from the note store under its dictionary label (#278),
    // not from an edge's rel_type.
    assert!(d.annotations.iter().any(|a| a.label == "description"
        && a.content.contains("composable")));
    assert!(d.attributes_json.as_deref().unwrap_or("").contains("calexander"));

    // Update = a new version; history grows.
    api::update(
        &db,
        &product,
        &api::UpdateReq {
            name: Some("Widget X2".into()),
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("update");
    let h = api::history(&db, &product).await.expect("history");
    assert!(h.len() >= 2, "update appended a version: {}", h.len());
    assert_eq!(api::detail(&db, &product).await.expect("detail2").name, "Widget X2");

    // Comment lands as an annotation.
    api::comment(&db, &product, "Priority: high", &Author::operator()).await.expect("comment");
    let d = api::detail(&db, &product).await.expect("detail3");
    assert!(d.annotations.iter().any(|a| a.label == "comments"));

    // The list is entities, not their annotations: the description and
    // comment text nodes stay out of it unless asked for by name.
    let listed = api::list(&db, None).await.expect("list");
    assert!(listed.iter().any(|e| e.id == product));
    assert!(
        listed.iter().all(|e| e.entity_type != "text"),
        "text carriers must not show as entity rows"
    );
    // Writing prose no longer creates a carrier at all (#302), so
    // there is nothing of that type to return. The prose itself is on
    // the detail page above, under its dictionary label.
    let texts_only = api::list(&db, Some("text")).await.expect("list text");
    assert!(
        texts_only.is_empty(),
        "a description and a comment made no entities: {:?}",
        texts_only.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Link task depends_on... task depends on nothing here — link the
    // product to the task and check both directions + unlink history.
    api::link(&db, &task, &api::LinkReq { to: product.clone(), rel: "depends_on".into() })
        .await
        .expect("link");
    let dt = api::detail(&db, &task).await.expect("task detail");
    assert!(dt.edges.iter().any(|e| e.rel_type == "depends_on"
        && e.outbound
        && e.other_name == "Widget X2"));
    let dp = api::detail(&db, &product).await.expect("product detail");
    assert!(dp.edges.iter().any(|e| e.rel_type == "depends_on" && !e.outbound));
    api::unlink(&db, &task, &api::LinkReq { to: product.clone(), rel: "depends_on".into() })
        .await
        .expect("unlink");
    let dt = api::detail(&db, &task).await.expect("task detail 2");
    assert!(!dt.edges.iter().any(|e| e.rel_type == "depends_on"), "unlinked edge hidden");

    // New entity type via the UI path — always an entity kind.
    api::types_add(&db, &api::TypeReq { name: "dashboard".into(), description: Some("a BI dashboard".into()) })
        .await
        .expect("type add");
    assert!(api::types_list(&db).await.expect("types2").iter().any(|t| t.name == "dashboard"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_graph_is_rooted_at_the_entity_and_walks_both_ways() {
    use superx_mod_entities::api;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    // product —contains→ task —depends_on→ repo, and a rag pointing AT
    // the task: a chain plus an inbound edge, so direction matters.
    let mk = |t: &'static str, n: &'static str| {
        let db = &db;
        async move {
            api::create(
                db,
                &api::CreateReq {
                    entity_type: t.into(),
                    name: n.into(),
                    description: None,
                    content: None,
                    attributes_json: None,
                },
            )
            .await
            .expect("create")
        }
    };
    let product = mk("product", "Widget").await;
    let task = mk("task", "Build it").await;
    let repo = mk("repo", "widget-src").await;
    let rag = mk("rag", "widget docs").await;
    for (from, rel, to) in [
        (&product, "contains", &task),
        (&task, "depends_on", &repo),
        (&rag, "consults", &task),
    ] {
        api::link(&db, from, &api::LinkReq { to: to.clone(), rel: rel.into() })
            .await
            .expect("link");
    }

    // Rooted at the task, both ways: reaches product (in), repo (out),
    // rag (in) — its whole neighbourhood.
    let g = api::graph_view(&db, &task, 2, "both").await.expect("graph");
    assert_eq!(g.root, task);
    let names: Vec<&str> = g.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(g.nodes.len(), 4, "the task and its three neighbours: {names:?}");
    assert_eq!(
        g.nodes.iter().find(|n| n.id == task).map(|n| n.depth),
        Some(0),
        "the root sits at depth 0"
    );
    assert!(g.edges.iter().any(|e| e.rel_type == "depends_on"));
    assert!(g.edges.iter().any(|e| e.rel_type == "consults"));

    // Outbound only: the task reaches the repo, and nothing that
    // merely points at it.
    let out = api::graph_view(&db, &task, 2, "out").await.expect("out");
    let out_names: Vec<&str> = out.nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(out_names.contains(&"widget-src"), "{out_names:?}");
    assert!(!out_names.contains(&"widget docs"), "inbound must stay out: {out_names:?}");

    // Depth 1 from the product sees the task but not the repo behind
    // it, and says the walk was cut short.
    let shallow = api::graph_view(&db, &product, 1, "out").await.expect("shallow");
    let shallow_names: Vec<&str> = shallow.nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(shallow_names.contains(&"Build it"));
    assert!(!shallow_names.contains(&"widget-src"), "{shallow_names:?}");
    assert!(shallow.truncated, "there IS more past depth 1");

    // Every edge returned connects two nodes that were returned —
    // a dangling edge would render as a stub.
    let ids: std::collections::HashSet<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(g.edges.iter().all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_graph_leaves_descriptions_and_comments_out_of_it() {
    use superx_mod_entities::api;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    // A product with a description and a comment — both text carriers —
    // plus one real neighbour.
    let product = api::create(
        &db,
        &api::CreateReq {
            entity_type: "product".into(),
            name: "Widget".into(),
            description: Some("the composable widget".into()),
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("create");
    api::comment(&db, &product, "ship it by friday", &Author::operator()).await.expect("comment");
    let task = api::create(
        &db,
        &api::CreateReq {
            entity_type: "task".into(),
            name: "Build it".into(),
            description: None,
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("task");
    api::link(&db, &product, &api::LinkReq { to: task.clone(), rel: "contains".into() })
        .await
        .expect("link");

    let g = api::graph_view(&db, &product, 3, "both").await.expect("graph");
    assert!(
        g.nodes.iter().all(|n| n.entity_type != "text"),
        "texts organize descriptions and comments — they are not graph members: {:?}",
        g.nodes.iter().map(|n| (&n.entity_type, &n.name)).collect::<Vec<_>>()
    );
    let names: Vec<&str> = g.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names.len(), 2, "the product and its task, nothing else: {names:?}");
    assert!(names.contains(&"Build it"));
    // …and no edge is left pointing at a node that is no longer there.
    let ids: std::collections::HashSet<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(g.edges.iter().all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())));
    assert!(g.edges.iter().all(|e| e.rel_type != "describes" && e.rel_type != "comments"));

    // They are still THERE, on the detail page, which is their place.
    let d = api::detail(&db, &product).await.expect("detail");
    assert!(d.annotations.iter().any(|a| a.label == "description"));
    assert!(d.annotations.iter().any(|a| a.label == "comments"));

    // Opening the graph ON a legacy carrier keeps it as the root — you
    // are looking at it — rather than handing back an empty canvas.
    // Writing prose no longer makes one (#302), so this builds the
    // pre-#268 shape directly: ~41 exist on the live instance and
    // clicking one must still open something.
    let carrier = superx_mod_entities::nodes::create_entity(
        &db,
        "text",
        "an older description",
        Some("an older description".to_string()),
        None,
    )
    .await
    .expect("legacy carrier");
    let text_id = &superx_ops::record_uuid(&carrier);
    let tg = api::graph_view(&db, text_id, 2, "both").await.expect("text graph");
    assert_eq!(&tg.root, text_id);
    assert!(tg.nodes.iter().any(|n| n.id == *text_id), "the root survives");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_attachments_surface_on_the_owner_and_resolve_to_their_file() {
    use superx_mod_entities::{api, documents};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let owner = api::create(
        &db,
        &api::CreateReq {
            entity_type: "task".into(),
            name: "Ship it".into(),
            description: None,
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("owner");
    let owner_id = superx_mod_entities::nodes::resolve_entity(&db, &owner).await.expect("resolve");

    let doc = documents::attach_document(
        &db,
        &owner_id,
        "notes.md",
        "/tmp/entities/files/01a0-notes.md",
        documents::mime_for("notes.md"),
        4096,
    )
    .await
    .expect("attach");

    // The owner's detail shows it as an attachment, not just an edge.
    let d = api::detail(&db, &owner).await.expect("detail");
    assert_eq!(d.attachments.len(), 1, "{:?}", d.attachments);
    let a = &d.attachments[0];
    assert_eq!(a.name, "notes.md");
    assert_eq!(a.mime, "text/markdown");
    assert_eq!(a.size, 4096);
    assert!(d.edges.iter().any(|e| e.rel_type == "attached" && e.outbound));

    // The download route resolves the stored path from the substrate.
    let (path, name, mime) = api::attachment_file(&db, &superx_ops::record_uuid(&doc))
        .await
        .expect("file");
    assert_eq!(path, "/tmp/entities/files/01a0-notes.md");
    assert_eq!((name.as_str(), mime.as_str()), ("notes.md", "text/markdown"));

    // An entity that is not an attachment has no file to serve.
    assert!(
        api::attachment_file(&db, &owner).await.is_err(),
        "a task is not a document"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_limit_defaults_then_follows_the_parameter() {
    use superx_kernel::{KernelModule, SCHEMA_DDL as KERNEL_DDL};
    use superx_mod_entities::{resolved_upload_limit, UPLOAD_LIMIT_PARAM};

    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = KERNEL_DDL.replace("$SUPERX_KERNEL_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    let kernel = superx_kernel::Kernel::from_db(db);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await
            .expect("seed");
    }
    let mb = 1024 * 1024;
    assert_eq!(resolved_upload_limit(&kernel).await, 25 * mb, "unregistered → default");
    let entity = kernel
        .register_module(&EntitiesModule.descriptor())
        .await
        .expect("register");
    kernel
        .set_parameter(entity, UPLOAD_LIMIT_PARAM, superx_kernel::types::Value::Number(4.into()))
        .await
        .expect("param");
    assert_eq!(resolved_upload_limit(&kernel).await, 4 * mb, "parameter wins");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_port_defaults_then_follows_the_parameter() {
    use superx_mod_entities::{resolved_ui_port, resolved_ui_url, DEFAULT_UI_PORT, UI_PORT_PARAM};
    // A KERNEL substrate (not the module db): port parameters live on
    // the module's registry entity.
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = superx_kernel::SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    let kernel = superx_kernel::Kernel::from_db(db);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await
            .expect("seed");
    }
    assert_eq!(resolved_ui_port(&kernel).await, DEFAULT_UI_PORT, "unregistered → default");
    let entity = kernel
        .register_module(&EntitiesModule.descriptor())
        .await
        .expect("register");
    kernel
        .set_parameter(entity, UI_PORT_PARAM, Value::Number(7272.into()))
        .await
        .expect("param");
    assert_eq!(resolved_ui_port(&kernel).await, 7272, "parameter wins");
    assert_eq!(resolved_ui_url(&kernel).await, "http://127.0.0.1:7272");
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
    // Newest first (#257): the task was created after the product, so
    // whatever you just made is the FIRST row, never buried last.
    assert_eq!(all[0].name, "Build widget", "newest first");
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

    // CARDINALITY IS THE PROPERTY, and it comes from the dictionary:
    // `description` is `one` so it amends in place, `comments` is `many`
    // so each adds a chain. That was true when both stores were written
    // and it is true now that only the note store is (#302) — what
    // changed is that these uids name notes, not text entities.
    let (d1, created) = texts::set_role_text(&db, &product, "describes", "First description.")
        .await
        .expect("describe");
    assert!(created);
    let (d2, created_again) =
        texts::set_role_text(&db, &product, "describes", "Better description.")
            .await
            .expect("re-describe");
    assert!(!created_again, "the same chain is amended");
    assert_eq!(d1, d2, "a `one` label keeps its uid across versions");

    let history = superx_mod_entities::notes::history(&db, &d1).await.expect("history");
    assert_eq!(history.len(), 2, "the wording that was replaced is still readable");
    assert_eq!(history[1].body, "Better description.");

    let c1 = texts::add_comment(&db, &product, "looks good", &Author::operator()).await.expect("c1");
    let c2 = texts::add_comment(&db, &product, "ship it", &Author::operator()).await.expect("c2");
    assert_ne!(c1, c2, "a `many` label adds rather than amends");

    // Thread: a reply to a comment, which is a note on the same entity
    // carrying its parent — not a comment on a carrier, since there is
    // no carrier any more.
    superx_mod_entities::notes::reply(&db, &c1, "replying to the first", &Author::operator())
        .await
        .expect("thread");

    let stored = superx_mod_entities::notes::for_entity(&db, &product, false).await.expect("notes");
    assert_eq!(stored.len(), 4, "one description + two comments + one reply");
    assert!(stored.iter().any(|n| n.body == "Better description."));
    assert!(
        stored.iter().any(|n| n.parent_uid.as_deref() == Some(c1.as_str())),
        "the reply says what it is replying to"
    );

    // And no carrier was created for any of it.
    assert!(
        superx_mod_entities::nodes::list_entities(&db, Some("text")).await.expect("list").is_empty(),
        "prose is not an entity any more"
    );
}

// ---------------------------------------------------------------- E4 --

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn documents_are_graph_nodes() {
    use superx_mod_entities::{documents, graph, nodes};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let doc = documents::attach_document(
        &db,
        &product,
        "spec.pdf",
        "/instance/modules/entities/files/xyz-spec.pdf",
        "application/pdf",
        4096,
    )
    .await
    .expect("attach");

    // The document rides the graph like any node.
    let sub = graph::subgraph(&db, &product, 3, false).await.expect("bfs");
    assert!(sub.nodes.iter().any(|n| n.entity_type == "document" && n.name == "spec.pdf"));
    assert!(sub.edges.iter().any(|e| e.rel_type == "attached"));

    // Metadata lives in attributes; the stored path is recoverable.
    let state = nodes::current_state(&db, &doc).await.expect("q").expect("state");
    let attrs = state.attributes.expect("attrs");
    assert_eq!(
        documents::stored_path(&attrs).as_deref(),
        Some("/instance/modules/entities/files/xyz-spec.pdf")
    );
    assert_eq!(documents::mime_for("Spec.PDF"), "application/pdf");
    assert_eq!(documents::mime_for("noext"), "application/octet-stream");
}

// -------------------------------------------------------------- #179 --

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_version_chains_list_only_currents() {
    use superx_mod_entities::nodes;
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    let anchor = nodes::create_entity(&db, "product", "v0", None, None).await.expect("create");
    for n in 1..=25 {
        nodes::update_entity(&db, &anchor, Some(format!("v{n}")), None, None)
            .await
            .expect("update");
    }
    let rows = nodes::list_entities(&db, None).await.expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "v25", "windowed read returns the current label");
    assert_eq!(
        nodes::state_history(&db, &anchor).await.expect("history").len(),
        26,
        "the full chain still exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subgraph_carries_text_content_via_batched_meta() {
    use superx_mod_entities::{edges, graph, nodes};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    // A DOCUMENT, not a text carrier. The property under test is that
    // the batched per-level meta read carries content for
    // content-bearing kinds — which is unchanged. What changed is that
    // prose is no longer a member of the graph (#300), so proving this
    // on a `describes` text node would be proving it about a node the
    // walk correctly never reaches.
    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let doc = nodes::create_entity(
        &db,
        "document",
        "spec.md",
        Some("The whole point.".to_string()),
        None,
    )
    .await
    .expect("d");
    edges::link(&db, &product, &doc, "attached").await.expect("attach");

    let sub = graph::subgraph(&db, &product, 3, false).await.expect("bfs");
    let file = sub.nodes.iter().find(|n| n.entity_type == "document").expect("document node");
    assert_eq!(file.content.as_deref(), Some("The whole point."));
    let root = sub.nodes.iter().find(|n| n.entity_type == "product").expect("root");
    assert_eq!(root.name, "Widget");
    assert!(root.content.is_none(), "non-content kinds stay lean");
}

// -------------------------------------------------------------- #253 --

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ancestor_path_is_root_first_priority_ordered_and_cycle_safe() {
    use superx_mod_entities::{edges, graph, nodes};
    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("seed");

    // product ─contains→ component ─contains→ task
    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let part = nodes::create_entity(&db, "product", "Frame", None, None).await.expect("c");
    let task = nodes::create_entity(&db, "task", "Build frame", None, None).await.expect("t");
    edges::link(&db, &product, &part, "contains").await.expect("l1");
    edges::link(&db, &part, &task, "contains").await.expect("l2");

    let trail = graph::ancestors(&db, &task, 12).await.expect("walk");
    let names: Vec<&str> = trail.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Widget", "Frame"], "root first, self excluded");
    assert_eq!(trail[1].rel_type, "contains");

    // A second, non-structural incoming edge must not outrank `contains`.
    let rag = nodes::create_entity(&db, "rag", "Datasheets", None, None).await.expect("r");
    edges::link(&db, &rag, &task, "linked").await.expect("l3");
    let trail = graph::ancestors(&db, &task, 12).await.expect("walk");
    assert_eq!(
        trail.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["Widget", "Frame"],
        "contains wins the parent slot"
    );

    // A LEGACY text carrier's parent is the entity it annotates. Prose
    // stopped being an entity in #302, so this shape can no longer be
    // made through `set_role_text` — but ~41 of them exist on the live
    // instance and clicking one must still say where it hangs. Built
    // the way the world before #268 built it, straight past the writer.
    let text = nodes::create_entity(
        &db,
        "text",
        "what to build",
        Some("what to build".to_string()),
        None,
    )
    .await
    .expect("legacy carrier");
    edges::link(&db, &task, &text, "describes").await.expect("legacy role edge");
    let trail = graph::ancestors(&db, &text, 12).await.expect("walk");
    assert_eq!(
        trail.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["Widget", "Frame", "Build frame"]
    );

    // Depth cap honoured.
    let capped = graph::ancestors(&db, &task, 1).await.expect("walk");
    assert_eq!(capped.len(), 1);

    // A root — nothing points at it — has no ancestors.
    assert!(graph::ancestors(&db, &product, 12).await.expect("walk").is_empty());

    // A cycle terminates instead of climbing forever (asserted last:
    // it gives the root an incoming edge).
    //
    // `link` refuses to CREATE one now (#298) — `contains` is acyclic —
    // but data written before that rule may hold one, and the walk must
    // still come back rather than climb forever. So the fixture is
    // written the way such a row got there: straight into the substrate,
    // past the guard.
    let edge_uid = uuid::Uuid::now_v7().to_string();
    db.query(
        "RELATE $from->edge->$to SET edge_uid = $uid, rel_type = 'contains', \
         active = true, valid_from = time::now()",
    )
    .bind(("from", task.clone()))
    .bind(("to", product.clone()))
    .bind(("uid", edge_uid))
    .await
    .expect("cycle")
    .check()
    .expect("a cycle that predates the rule");
    let trail = graph::ancestors(&db, &task, 12).await.expect("walk");
    assert!(trail.len() <= 3, "visited set stops the cycle: {}", trail.len());
}

/// THE INVARIANT THIS FILE EXISTS FOR (#300): the operator's view and
/// the runner's view are the same graph.
///
/// They were not. Measured on the live instance, one product at depth
/// 3: the CLI walk returned 4 nodes and 3 edges, the API returned 1 and
/// 0. The type check lived in `api.rs` and nowhere in the CLI path, so
/// a design approved from the API view was a design nobody had seen the
/// whole of — and the runner builds its prompt by walking the graph.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_views_walk_the_same_graph() {
    use superx_mod_entities::{api, dictionary, edges, graph, nodes, registry, texts};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("structural");

    // Prose as the world before #268 stored it: a `text` entity on a
    // `describes` edge. Writing prose stopped making one in #302, so
    // this is built directly — and it has to be, because the shape
    // still exists on the live instance and is exactly what the two
    // views used to disagree about.
    let carrier = nodes::create_entity(
        &db,
        "text",
        "what this desk is",
        Some("what this desk is".to_string()),
        None,
    )
    .await
    .expect("legacy carrier");
    edges::link(&db, &product, &carrier, "describes").await.expect("legacy role edge");

    // And prose as it is stored now, which creates no node to filter.
    texts::set_role_text(&db, &product, "describes", "what this desk is").await.expect("prose");
    texts::add_comment(&db, &product, "a remark", &superx_mod_entities::notes::Author::operator())
        .await
        .expect("comment");

    let walk = graph::subgraph(&db, &product, 3, false).await.expect("walk");
    let view = api::graph_view(&db, &superx_ops::record_uuid(&product), 3, "out").await.expect("view");

    assert_eq!(
        walk.nodes.len(),
        view.nodes.len(),
        "the two views disagree on how many nodes are in the graph: CLI {:?} vs API {:?}",
        walk.nodes.iter().map(|n| (&n.entity_type, &n.name)).collect::<Vec<_>>(),
        view.nodes.iter().map(|n| (&n.entity_type, &n.name)).collect::<Vec<_>>(),
    );
    assert_eq!(walk.edges.len(), view.edges.len(), "and on how many edges");

    // Two nodes, one edge: the product and its task. The description
    // and the comment are attached to the product, not members of it.
    assert_eq!(walk.nodes.len(), 2, "prose is not in the shape");
    assert_eq!(walk.edges.len(), 1);
    assert!(
        walk.nodes.iter().all(|n| n.entity_type != "text"),
        "no prose carrier reached the walk"
    );

    // And the walk SAYS what it did not follow, rather than swallowing
    // it — a silently vanishing edge is the original bug.
    assert!(
        walk.unwalked_labels.iter().any(|l| l == "describes"),
        "the walk reports the prose labels it skipped: {:?}",
        walk.unwalked_labels
    );
    assert_eq!(view.unwalked_labels, walk.unwalked_labels, "and both views report the same");
}

/// `attached` was a registered relation type that the dictionary never
/// declared, while holding two of the operator's documents. A walk that
/// followed only declared labels would have deleted those documents
/// from BOTH views — the fix quietly causing the very loss it exists to
/// prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_document_stays_in_the_shape() {
    use superx_mod_entities::{dictionary, edges, graph, nodes, registry};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let doc = nodes::create_entity(&db, "document", "mandate.pdf", None, None).await.expect("d");
    edges::link(&db, &product, &doc, "attached").await.expect("attach");

    let walk = graph::subgraph(&db, &product, 2, false).await.expect("walk");
    assert!(
        walk.nodes.iter().any(|n| n.entity_type == "document"),
        "the document is part of the shape — a file labelled a mandate IS the mandate (§5.4)"
    );
    assert!(walk.unwalked_labels.is_empty(), "and nothing was skipped: {:?}", walk.unwalked_labels);
}

/// An unprovisioned database declares nothing. A walk that filtered on
/// an empty dictionary would return a bare root and read as an empty
/// instance rather than an unseeded one — enforcement arrives with the
/// declaration, never before it (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undeclared_dictionary_does_not_blank_the_graph() {
    use superx_mod_entities::{edges, graph, nodes, registry};

    // NOT `fresh_db` — that seeds the dictionary, which is the whole
    // thing this test needs absent. A test that claims to prove the
    // unseeded path while running the seeded one proves nothing.
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema");
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("link");

    let walk = graph::subgraph(&db, &product, 2, false).await.expect("walk");
    assert_eq!(walk.nodes.len(), 2, "the walk still walks");
    assert_eq!(walk.edges.len(), 1);
}

/// Archiving a label stops it being OFFERED. It does not make the edges
/// it already made stop being part of the shape — otherwise archiving
/// `contains` would erase every hierarchy from both views at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_a_link_label_does_not_erase_its_edges_from_the_graph() {
    use superx_mod_entities::{dictionary, edges, graph, nodes, registry};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("link");

    dictionary::archive(&db, "contains", dictionary::LINK, true).await.expect("archive");

    let walk = graph::subgraph(&db, &product, 2, false).await.expect("walk");
    assert_eq!(walk.nodes.len(), 2, "the hierarchy survives archiving the label");
    assert_eq!(walk.edges.len(), 1);
}
