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


/// Build a document node the way the world before B4 did: a `document`
/// entity joined by an `attached` edge. `documents::attach_document` is
/// gone — a file is an attachment row now (§6) — so tests that need the
/// legacy shape build it directly, past the writer, which is how such a
/// row got there.
async fn legacy_document(
    db: &superx_kernel::Db,
    owner: &superx_kernel::types::RecordId,
    name: &str,
    path: &str,
    mime: &str,
    size: u64,
) -> superx_kernel::types::RecordId {
    let attributes = superx_kernel::message::value_from_json(&serde_json::json!({
        "file": path,
        "original_name": name,
        "mime": mime,
        "size": size,
    }));
    let _ = superx_mod_entities::registry::add_type(db, "document", "entity", None).await;
    let node = superx_mod_entities::nodes::create_entity(db, "document", name, None, Some(attributes))
        .await
        .expect("legacy document node");
    superx_mod_entities::edges::link(db, owner, &node, "attached").await.expect("attached edge");
    node
}


/// Build a text carrier the way the world before #268 did. `text` is no
/// longer a shipped entity type — B6 retired it — so the legacy shape
/// registers the legacy type first, which is the honest statement of
/// what it is: a row a fresh instance would never make.
async fn legacy_text(
    db: &superx_kernel::Db,
    name: &str,
    body: &str,
) -> superx_kernel::types::RecordId {
    let _ = superx_mod_entities::registry::add_type(db, "text", "entity", None).await;
    superx_mod_entities::nodes::create_entity(db, "text", name, Some(body.to_string()), None)
        .await
        .expect("legacy carrier")
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
    // No kind is a "system" type any more. `text` needed the flag
    // because writing a description created one behind your back; B6
    // ended that, so there is nothing the create form has to hide.
    assert!(types.iter().all(|t| !t.system), "no kind is hidden from the create form");

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
            based_on: None,
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
    let listed = api::list(&db, None, false).await.expect("list");
    assert!(listed.iter().any(|e| e.id == product));
    assert!(
        listed.iter().all(|e| e.entity_type != "text"),
        "text carriers must not show as entity rows"
    );
    // Writing prose no longer creates a carrier at all (#302), so
    // there is nothing of that type to return. The prose itself is on
    // the detail page above, under its dictionary label.
    let texts_only = api::list(&db, Some("text"), true).await.expect("list text");
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
    let carrier = legacy_text(&db, "an older description", "an older description").await;
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

    // Through the real writer: a file is an attachment ROW, never a
    // node (§6), so there is no `attached` edge to assert any more.
    let dir = std::env::temp_dir().join(format!("sx-att-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("dir");
    let uid = superx_mod_entities::attachments::attach_bytes(
        &db,
        &dir,
        superx_mod_entities::attachments::Upload {
            target: &superx_mod_entities::target::Target::Entity(owner_id.clone()),
            label: "attachments",
            filename: "notes.md",
            bytes: &vec![0u8; 4096],
            author: &Author::operator(),
        },
    )
    .await
    .expect("attach");

    // The owner's detail shows it as an attachment.
    let d = api::detail(&db, &owner).await.expect("detail");
    assert_eq!(d.attachments.len(), 1, "{:?}", d.attachments);
    let a = &d.attachments[0];
    assert_eq!(a.name, "notes.md");
    assert_eq!(a.mime, "text/markdown");
    assert_eq!(a.size, 4096);
    assert_eq!(a.id, uid);
    assert!(
        !d.edges.iter().any(|e| e.rel_type == "attached"),
        "a file makes no edge: it is content on the entity, not a member of the graph"
    );

    // The download route resolves the stored path from the row.
    let (path, name, mime) = api::attachment_file(&db, &uid).await.expect("file");
    assert!(path.ends_with("notes.md"), "{path}");
    assert_eq!((name.as_str(), mime.as_str()), ("notes.md", "text/markdown"));

    // And a LEGACY document node still resolves, so an instance that
    // has not run the migration keeps its download links working.
    let legacy = legacy_document(
        &db,
        &owner_id,
        "old.md",
        "/tmp/entities/files/01a0-old.md",
        documents::mime_for("old.md"),
        11,
    )
    .await;
    let (p2, n2, _) = api::attachment_file(&db, &superx_ops::record_uuid(&legacy))
        .await
        .expect("legacy file");
    assert_eq!(p2, "/tmp/entities/files/01a0-old.md");
    assert_eq!(n2, "old.md");

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
    // `text` and `document` are NOT here: B6 retired them, because
    // prose is a note and a file is an attachment row. An instance that
    // already has the rows keeps them — the registry is append-only.
    assert!(rows.iter().all(|r| r.name != "text" && r.name != "document"));
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
    let doc = legacy_document(
        &db,
        &product,
        "spec.pdf",
        "/instance/modules/entities/files/xyz-spec.pdf",
        "application/pdf",
        4096,
    )
    .await;

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

    // The property is the BATCHED per-level read: one request resolves
    // the whole frontier's name, attributes, version and notes, so a
    // walk costs the nodes it reaches rather than the table.
    //
    // It no longer carries `content`. The only kinds that had any were
    // `text` and `document`, and B4 took both out of the graph — so a
    // field that can never be populated is gone rather than left as a
    // column that is always null.
    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let attrs = superx_kernel::message::value_from_json(&serde_json::json!({"owner": "cal"}));
    let part = nodes::create_entity(&db, "product", "Frame", None, Some(attrs)).await.expect("c");
    edges::link(&db, &product, &part, "contains").await.expect("link");
    superx_mod_entities::texts::set_role_text(&db, &part, "describes", "the frame")
        .await
        .expect("prose");

    let sub = graph::subgraph(&db, &product, 3, false).await.expect("bfs");
    let child = sub.nodes.iter().find(|n| n.name == "Frame").expect("reached in one batch");
    assert!(child.attributes.is_some(), "attributes came with it");
    assert!(!child.version.is_empty(), "and the version it was read at");
    assert!(
        child.notes.iter().any(|n| n.body == "the frame"),
        "and its prose, from the same read — fetching notes separately would let one \
         written after the walk slip into a prompt"
    );
    let root = sub.nodes.iter().find(|n| n.entity_type == "product" && n.name == "Widget");
    assert_eq!(root.expect("root").depth, 0);
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
    let text = legacy_text(&db, "what to build", "what to build").await;
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

/// B4's END, which is what takes prose out of the graph (§13):
/// "Role edges are then retracted and old anchors archived rather than
/// deleted, so it reads correctly in both directions."
///
/// Not a reader-side filter. §6 is explicit: "every entity is a node
/// and no reader needs a filter — the special case does not get
/// documented, it stops existing."
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_retracts_the_role_edges_and_archives_the_anchors() {
    use superx_mod_entities::{edges, graph, migrate, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("structural");

    // Prose as the world before #268 stored it.
    let carrier = legacy_text(&db, "what this desk is", "what this desk is").await;
    edges::link(&db, &product, &carrier, "describes").await.expect("role edge");

    let before = graph::subgraph(&db, &product, 3, false).await.expect("before");
    assert_eq!(before.nodes.len(), 3, "prose is in the graph until B4 runs");

    let report = migrate::prose(&db, false).await.expect("migrate");
    assert_eq!(report.edges_retracted, 1);
    assert_eq!(report.anchors_archived, 1);

    let after = graph::subgraph(&db, &product, 3, false).await.expect("after");
    assert_eq!(after.nodes.len(), 2, "the product and its task, nothing else");
    assert!(after.nodes.iter().all(|n| n.entity_type != "text"));

    // The prose is READABLE, just not a member of the graph.
    let notes = superx_mod_entities::notes::for_entity(&db, &product, false).await.expect("notes");
    assert!(notes.iter().any(|n| n.body == "what this desk is"), "the words survived");

    // Nothing was deleted, and it is reversible by un-retracting: the
    // anchor is still there, archived, with its own history.
    let state = nodes::current_state(&db, &carrier).await.expect("read").expect("still there");
    assert!(state.archived, "hidden, not erased");
}

/// §6: "A file is attached content: it belongs to the entity and is
/// never a node." B3's exit says the same: "no new node appears in the
/// graph or the entity list."
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_becomes_an_attachment_row_and_leaves_the_graph() {
    use superx_mod_entities::{attachments, graph, migrate, nodes, target::Target};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let doc = legacy_document(
        &db,
        &product,
        "mandate.pdf",
        "attachments/abc/mandate.pdf",
        "application/pdf",
        1234,
    )
    .await;

    assert_eq!(
        graph::subgraph(&db, &product, 2, false).await.expect("before").nodes.len(),
        2,
        "the document is a node until B4 runs"
    );

    let report = migrate::prose(&db, false).await.expect("migrate");
    assert_eq!(report.documents, 1);

    // Gone from the graph…
    let after = graph::subgraph(&db, &product, 2, false).await.expect("after");
    assert_eq!(after.nodes.len(), 1, "only the product");

    // …and readable as an attachment on the entity, under §5.3's label.
    let files = attachments::for_target(&db, &Target::Entity(product.clone()), false)
        .await
        .expect("attachments");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].label, "attachments");
    assert_eq!(files[0].filename, "mandate.pdf");
    assert_eq!(files[0].path, "attachments/abc/mandate.pdf", "the same bytes, not a copy");

    // The anchor is archived, not deleted.
    let state = nodes::current_state(&db, &doc).await.expect("read").expect("still there");
    assert!(state.archived);
}

/// "Idempotent and re-runnable" — the spec's words. A second run must
/// not move anything again, retract an edge twice, or pad the anchor's
/// history with versions that say nothing new.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_the_migration_twice_changes_nothing_the_second_time() {
    use superx_mod_entities::{edges, migrate, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let carrier =
        legacy_text(&db, "words", "words").await;
    edges::link(&db, &product, &carrier, "describes").await.expect("edge");
    legacy_document(&db, &product, "f.pdf", "attachments/x/f.pdf", "application/pdf", 9).await;

    let first = migrate::prose(&db, false).await.expect("first");
    assert!(first.versions > 0 && first.documents == 1 && first.edges_retracted == 2);
    let history = nodes::state_history(&db, &carrier).await.expect("history").len();

    let second = migrate::prose(&db, false).await.expect("second");
    assert_eq!(second.versions, 0, "no prose moves twice");
    assert_eq!(second.documents, 0, "no file moves twice");
    assert_eq!(second.edges_retracted, 0, "an edge already retracted is not retracted again");
    assert_eq!(second.anchors_archived, 0, "an anchor already archived is left alone");
    assert_eq!(
        nodes::state_history(&db, &carrier).await.expect("history").len(),
        history,
        "the anchor's history did not grow"
    );
}

/// THE UPGRADE PATH, not the fresh-install path (#304).
///
/// Every test above applies the schema to an empty database, which is
/// the one case that cannot fail. The operator's instance is not empty:
/// it holds entity_state and edge rows written before these columns
/// existed, and provisioning re-applies this file verbatim over them.
///
/// So this writes rows under the OLD shape, applies the NEW schema on
/// top, and checks the old rows still read. #158 was exactly this
/// failure — a schema that was correct for a fresh install and wrong
/// for an instance that had been running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_new_columns_do_not_invalidate_rows_written_before_them() {
    use superx_mod_entities::{edges, nodes};

    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");

    // The schema WITHOUT the new columns: the shape the operator's
    // instance is running right now.
    let old = SCHEMA_DDL
        .replace("$SUPERX_MODULE_PASSWORD", "test-password")
        .lines()
        .filter(|l| {
            !(l.contains("ON TABLE entity_state") || l.contains("ON TABLE edge"))
                || !(l.contains("author_kind")
                    || l.contains("author_uid")
                    || l.contains("via_uid")
                    || l.contains("archived"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    // A filter that quietly matched nothing would leave this test
    // applying the NEW schema twice and proving nothing at all.
    let removed = SCHEMA_DDL.lines().count() - old.lines().count();
    assert_eq!(removed, 7, "4 on entity_state + 3 on edge were actually taken out");
    assert!(
        !old.contains("archived   ON TABLE entity_state"),
        "the old shape really is missing them"
    );
    assert!(!old.contains("author_kind ON TABLE edge"));

    db.query(old).await.expect("old ddl").check().expect("the shape that exists today");

    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");
    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Build it", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("edge");

    // Now provisioning runs again with the new file, verbatim.
    let new = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(new).await.expect("new ddl").check().expect("applies over a populated database");

    // The rows written before the columns existed still read.
    let state = nodes::current_state(&db, &product).await.expect("read").expect("still there");
    assert_eq!(state.name, "Widget");
    let out = edges::expand(&db, std::slice::from_ref(&product), false).await.expect("expand");
    assert_eq!(out.len(), 1, "the edge survives the upgrade");
    assert!(out[0].active);

    // And writing after the upgrade still works — the new columns are
    // optional, so a writer that says nothing about authorship is not
    // refused. Whether it SHOULD say something is a code question for
    // the PR that follows this one; the schema does not force a flag day.
    let another = nodes::create_entity(&db, "task", "Ship it", None, None).await.expect("t2");
    edges::link(&db, &product, &another, "contains").await.expect("write after upgrade");
    let out = edges::expand(&db, std::slice::from_ref(&product), false).await.expect("expand2");
    assert_eq!(out.len(), 2);
}

/// §6 compare-and-append: "Every write carries the `valid_from` it was
/// based on. If the chain head has moved, the write is refused and the
/// current version comes back with the refusal."
///
/// The scenario is the spec's: the operator saves an edit based on v3
/// while a role, also holding v3, saves a moment later. Without this the
/// role's row wins, the operator's is invisible, and nobody is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_against_a_version_that_has_moved_is_refused() {
    use superx_mod_entities::{api, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let id = api::create(
        &db,
        &api::CreateReq {
            entity_type: "product".into(),
            name: "Widget".into(),
            description: None,
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("create");
    let anchor = nodes::resolve_entity(&db, &id).await.expect("resolve");
    let v1 = nodes::current_state(&db, &anchor).await.expect("read").expect("state").valid_from;

    // A role writes first, holding v1.
    api::update(
        &db,
        &id,
        &api::UpdateReq {
            name: Some("Widget, by the role".into()),
            content: None,
            attributes_json: None,
            based_on: Some(v1.clone()),
        },
    )
    .await
    .expect("the first writer wins");

    // The operator, also holding v1, saves a moment later.
    let err = api::update(
        &db,
        &id,
        &api::UpdateReq {
            name: Some("Widget, by the operator".into()),
            content: None,
            attributes_json: None,
            based_on: Some(v1.clone()),
        },
    )
    .await
    .expect_err("the second writer is refused rather than silently winning");

    // The refusal CARRIES the version that beat it — a role told only
    // "no" has nothing to do; one handed the current version can
    // re-read, merge and retry.
    let text = err.to_string();
    assert!(text.contains(&v1), "it names what the write was based on: {text}");
    let head = nodes::current_state(&db, &anchor).await.expect("read").expect("state").valid_from;
    assert!(text.contains(&head), "and the version that is actually there: {text}");

    // Nothing was half-applied: the role's write stands untouched.
    assert_eq!(
        api::detail(&db, &id).await.expect("detail").name,
        "Widget, by the role",
        "the refused edit changed nothing"
    );

    // And re-reading, then writing against the CURRENT version, works.
    api::update(
        &db,
        &id,
        &api::UpdateReq {
            name: Some("Widget, merged".into()),
            content: None,
            attributes_json: None,
            based_on: Some(head),
        },
    )
    .await
    .expect("a writer that re-read is not blocked");
    assert_eq!(api::detail(&db, &id).await.expect("detail").name, "Widget, merged");
}

/// The guarantee is OFFERED, never imposed. A caller with no version to
/// quote — which is most callers today, and every older client — keeps
/// the latest-wins behaviour rather than being refused for not making a
/// claim it never made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_that_claims_nothing_is_not_refused() {
    use superx_mod_entities::api;

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let id = api::create(
        &db,
        &api::CreateReq {
            entity_type: "product".into(),
            name: "Widget".into(),
            description: None,
            content: None,
            attributes_json: None,
        },
    )
    .await
    .expect("create");

    for name in ["one", "two"] {
        api::update(
            &db,
            &id,
            &api::UpdateReq {
                name: Some(name.to_string()),
                content: None,
                attributes_json: None,
                based_on: None,
            },
        )
        .await
        .expect("no claim, no refusal");
    }
    assert_eq!(api::detail(&db, &id).await.expect("detail").name, "two");
}

/// §14, the read that matters: "the entity as it stood at an instant —
/// its state, every note, every attachment and every edge resolved at
/// the SAME moment."
///
/// A field-by-field picker answers "how did this text change". This
/// answers "what did the agent see when it did that", which cannot be
/// assembled from separate pickers because each one moves
/// independently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_instant_reaches_every_chain() {
    use superx_mod_entities::{api, asof, edges, nodes, texts};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);
    texts::set_role_text(&db, &product, "describes", "the first wording").await.expect("d1");
    let task = nodes::create_entity(&db, "task", "Build it", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("link");

    // THE INSTANT, taken from the LAST setup write rather than from
    // the clock: `set_role_text` appends a note and no state version,
    // so the anchor's own timestamp predates the prose and would make
    // this test assert something it never set up.
    let then = edges::expand(&db, std::slice::from_ref(&product), false)
        .await
        .expect("edges")
        .into_iter()
        .map(|e| e.valid_from)
        .max()
        .expect("the link is the last thing written above");

    // The world moves on: renamed, re-described, a comment added, the
    // task unlinked.
    nodes::update_entity(&db, &product, Some("Widget X2".into()), None, None).await.expect("rename");
    texts::set_role_text(&db, &product, "describes", "a later wording").await.expect("d2");
    texts::add_comment(&db, &product, "a remark", &Author::operator()).await.expect("c");
    edges::unlink(&db, &product, &task, "contains").await.expect("unlink");

    // Now is now.
    let now = api::detail(&db, &frag).await.expect("now");
    assert_eq!(now.name, "Widget X2");
    assert!(now.annotations.iter().any(|a| a.content == "a later wording"));
    assert!(now.annotations.iter().any(|a| a.label == "comments"));
    assert!(!now.edges.iter().any(|e| e.rel_type == "contains"), "the link is gone now");

    // Then is then — one instant, every chain.
    let past = api::detail_at(&db, &frag, asof::parse(Some(&then)).expect("parse"))
        .await
        .expect("as-of");
    assert_eq!(past.name, "Widget", "the name it had");
    assert!(
        past.annotations.iter().any(|a| a.content == "the first wording"),
        "the wording it had: {:?}",
        past.annotations.iter().map(|a| &a.content).collect::<Vec<_>>()
    );
    assert!(
        !past.annotations.iter().any(|a| a.content == "a later wording"),
        "and not the one written afterwards"
    );
    assert!(
        !past.annotations.iter().any(|a| a.label == "comments"),
        "a comment written afterwards was not there"
    );
    assert!(
        past.edges.iter().any(|e| e.rel_type == "contains"),
        "the edge was ACTIVE then, even though it is unlinked now"
    );
}

/// An instant before the entity existed is a question about a time when
/// it did not, and the refusal says so rather than reading as a broken
/// substrate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_instant_before_it_existed_says_so() {
    use superx_mod_entities::{api, asof, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    let err = api::detail_at(&db, &frag, asof::parse(Some("2000-01-01T00:00:00Z")).expect("p"))
        .await
        .expect_err("there was no Widget in 2000");
    assert!(err.to_string().contains("created later"), "{err}");

    // And an unreadable instant is refused before anything is read.
    asof::parse(Some("last tuesday")).expect_err("not an instant");
}

/// §14: archiving hides, it does not erase — and it is a versioned
/// change, so an as-of read from before it still shows the thing as
/// live.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_hides_by_default_and_is_itself_a_version() {
    use superx_mod_entities::{api, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Widget", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    assert!(api::list(&db, None, false).await.expect("list").iter().any(|e| e.id == frag));

    assert!(nodes::set_archived(&db, &product, true).await.expect("archive"));
    assert!(
        !api::list(&db, None, false).await.expect("list").iter().any(|e| e.id == frag),
        "hidden from the default list"
    );
    let shown = api::list(&db, None, true).await.expect("list");
    let row = shown.iter().find(|e| e.id == frag).expect("still there when asked for");
    assert!(row.archived, "and it says so, rather than looking ordinary");

    // Restoring changes it back, and restoring AGAIN appends nothing —
    // a repeated call must not pad the history with rows saying nothing.
    assert!(nodes::set_archived(&db, &product, false).await.expect("restore"));
    assert!(!nodes::set_archived(&db, &product, false).await.expect("again"), "no second version");

    // Restored.
    assert!(api::list(&db, None, false).await.expect("list").iter().any(|e| e.id == frag));
}

/// §5.5: "once cardinality, endpoints and acyclicity are DATA, 'does
/// this graph make sense?' derives from the dictionary alone … exactly
/// the check to run before dispatching agents at a graph one of them
/// designed."
///
/// The point is that a graph can be wrong without any single write
/// being wrong: a label narrowed after its edges exist, a type given a
/// required slot after its entities were made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_graph_is_checked_against_the_dictionary_it_was_built_under() {
    use superx_mod_entities::dictionary::{Definition, LINK};
    use superx_mod_entities::{dictionary, edges, nodes, validate};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("t");
    edges::link(&db, &product, &task, "contains").await.expect("legal when written");

    // Clean under the dictionary as it stands.
    assert!(
        validate::subgraph(&db, &product, 3).await.expect("check").is_empty(),
        "nothing wrong yet"
    );

    // NOW the operator narrows the label — every write so far was
    // legal, and the graph is wrong the moment the rule changes.
    dictionary::define(&db, Definition {
        key: "contains",
        kind: LINK,
        display: "contains",
        semantics: "composition",
        source_types: Some(&["role".to_string()]),
        ..Default::default()
    })
    .await
    .expect("narrow it");

    let findings = validate::subgraph(&db, &product, 3).await.expect("check");
    assert_eq!(findings.len(), 1, "{:?}", findings.iter().map(|f| &f.detail).collect::<Vec<_>>());
    assert!(findings[0].detail.contains("starts at role"), "{}", findings[0].detail);
}

/// A required slot added to a type after its entities were made: every
/// entity written before it is now missing something it was promised,
/// and no single write was wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entity_missing_what_its_type_requires_is_reported() {
    use superx_mod_entities::{dictionary, nodes, validate};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed_type_labels(&db).await.expect("slots");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    // The shipped bindings already require something of a product, so
    // the baseline is what they require — not zero. Naming it here
    // rather than asserting emptiness keeps the test about the ONE slot
    // it adds below.
    let baseline = validate::subgraph(&db, &product, 1).await.expect("check").len();

    dictionary::bind_slot(&db, "product", "spec", true, None, &Author::operator())
        .await
        .expect("now every product needs one");

    let findings = validate::subgraph(&db, &product, 1).await.expect("check");
    assert!(
        findings.iter().any(|f| f.detail.contains("carries no 'spec'")),
        "{:?}",
        findings.iter().map(|f| &f.detail).collect::<Vec<_>>()
    );

    // Writing one clears it.
    superx_mod_entities::notes::write(
        &db,
        &product,
        "spec",
        "what to build",
        &Author::operator(),
    )
    .await
    .expect("write the spec");
    assert_eq!(
        validate::subgraph(&db, &product, 1).await.expect("check").len(),
        baseline,
        "writing the spec cleared the finding it caused, and nothing else"
    );
}

/// A cycle written before the label was marked acyclic. `link` refuses
/// to create one now, so the only way in is data that predates the
/// rule — which is exactly the case this check exists for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cycle_that_predates_the_rule_is_found() {
    use superx_mod_entities::{edges, nodes, validate};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let a = nodes::create_entity(&db, "task", "A", None, None).await.expect("a");
    let b = nodes::create_entity(&db, "task", "B", None, None).await.expect("b");
    edges::link(&db, &a, &b, "depends_on").await.expect("a waits on b");

    // Straight into the substrate, past the guard — how such a row got
    // there before the rule existed.
    let uid = uuid::Uuid::now_v7().to_string();
    db.query(
        "RELATE $from->edge->$to SET edge_uid = $uid, rel_type = 'depends_on', \
         active = true, valid_from = time::now()",
    )
    .bind(("from", b.clone()))
    .bind(("to", a.clone()))
    .bind(("uid", uid))
    .await
    .expect("write")
    .check()
    .expect("the pre-rule shape");

    let findings = validate::subgraph(&db, &a, 5).await.expect("check");
    assert!(
        findings.iter().any(|f| f.detail.contains("acyclic")),
        "{:?}",
        findings.iter().map(|f| &f.detail).collect::<Vec<_>>()
    );
}
