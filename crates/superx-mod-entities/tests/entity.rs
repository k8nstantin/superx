//! Contract tests: entities and their attributes (epic #353).
//!
//! An entity is a uuid7. Everything else — its name, what it is, what it
//! holds, whether it is archived — is an attribute. These pin the
//! properties the rest of the module and every reader depend on.

use superx_kernel::types::Value;
use superx_mod_entities::attribute::{self, Write};
use superx_mod_entities::author::Author;
use superx_mod_entities::{api, edge, entity, SCHEMA_DDL};

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema applies clean");
    db
}

fn text(s: &str) -> Option<Value> {
    Some(Value::String(s.to_string()))
}

/// THE IDENTITY DOES NOT MOVE. A rename is a new version of one
/// attribute; the uuid7 is untouched, which is what lets everything in
/// the graph point at it forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_never_touches_the_identity() {
    let db = fresh_db().await;
    let dba = entity::create(&db, "DBA", &Author::operator()).await.expect("create");

    assert_eq!(entity::name_of(&db, &dba).await.expect("name").as_deref(), Some("DBA"));

    entity::rename(&db, &dba, "Database Administrator", &Author::operator())
        .await
        .expect("rename");

    assert_eq!(
        entity::name_of(&db, &dba).await.expect("name").as_deref(),
        Some("Database Administrator")
    );
    assert_eq!(entity::list(&db, false).await.expect("list"), vec![dba.clone()]);
    assert!(entity::exists(&db, &dba).await.expect("exists"), "same row, same id");

    // And the old name is still on the record.
    let name_attr = attribute::of(&db, &dba, false).await.expect("attrs");
    let history = attribute::history(&db, &name_attr[0].uid).await.expect("history");
    assert_eq!(history.len(), 2, "both names are kept");
}

/// THE DATATYPE GATE — the only thing standing where a column type
/// normally stands, because `content` is `any` and the engine cannot
/// check it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_must_be_what_its_datatype_says() {
    let db = fresh_db().await;
    let e = entity::create(&db, "Thing", &Author::operator()).await.expect("create");

    // Every datatype, with content that fits.
    let ok: Vec<(&str, Value)> = vec![
        ("text", Value::String("prose".into())),
        ("number", Value::Number(3.into())),
        ("boolean", Value::Bool(true)),
        ("json", Value::Array(vec![Value::Number(1.into())].into())),
    ];
    for (dt, content) in ok {
        attribute::add(
            &db,
            &e,
            Write { name: dt, datatype: dt, content: Some(content), labels: &[], options: None },
            &Author::operator(),
        )
        .await
        .unwrap_or_else(|err| panic!("{dt}: {err}"));
    }

    // And content that does not.
    let bad: Vec<(&str, Value)> = vec![
        ("number", Value::String("three".into())),
        ("boolean", Value::String("yes".into())),
        ("text", Value::Number(1.into())),
        ("json", Value::String("{\"a\":1}".into())),
    ];
    for (dt, content) in bad {
        let err = attribute::add(
            &db,
            &e,
            Write { name: "x", datatype: dt, content: Some(content), labels: &[], options: None },
            &Author::operator(),
        )
        .await
        .expect_err(&format!("{dt} must refuse content that is not one"));
        assert!(err.to_string().contains(dt), "the refusal names the datatype: {err}");
    }

    // A json string is the case that matters most: it LOOKS like data
    // and the database could never look inside it.
    assert!(
        attribute::check("json", &Value::String("{\"a\":1}".into())).is_err(),
        "a string of json is not json"
    );

    let held = attribute::of(&db, &e, false).await.expect("attrs");
    assert_eq!(held.len(), 5, "the name plus four good ones; nothing bad was written");
}

/// Adding fields is the whole point, and they come back in the order
/// they were added — uuid7 is time-ordered, so nothing stores it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fields_come_back_in_the_order_they_were_added() {
    let db = fresh_db().await;
    let e = entity::create(&db, "DBA", &Author::operator()).await.expect("create");
    for (n, dt, c) in [
        ("description", "text", text("<p>runs the databases</p>")),
        ("comment", "text", text("<p>escalate on failure</p>")),
        ("reviewed", "datetime", Some(Value::Datetime(chrono::Utc::now().into()))),
        ("replicas", "number", Some(Value::Number(3.into()))),
    ] {
        attribute::add(
            &db,
            &e,
            Write { name: n, datatype: dt, content: c, labels: &[], options: None },
            &Author::operator(),
        )
        .await
        .expect("add");
    }
    let names: Vec<String> =
        attribute::of(&db, &e, false).await.expect("attrs").into_iter().map(|a| a.name).collect();
    assert_eq!(
        names,
        vec!["name", "description", "comment", "reviewed", "replicas"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
}

/// TWO TEXT FIELDS ARE THE SAME FIELD. What makes one a description and
/// the other a comment is the label it carries — never a different kind
/// of thing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_distinguishes_two_text_fields_is_the_label() {
    let db = fresh_db().await;
    let description = entity::create(&db, "description", &Author::operator()).await.expect("l1");
    let comment = entity::create(&db, "comment", &Author::operator()).await.expect("l2");
    let dba = entity::create(&db, "DBA", &Author::operator()).await.expect("e");

    for (n, label) in [("about", &description), ("note", &comment)] {
        attribute::add(
            &db,
            &dba,
            Write {
                name: n,
                datatype: "text",
                content: text("<p>prose</p>"),
                labels: std::slice::from_ref(label),
                options: None,
            },
            &Author::operator(),
        )
        .await
        .expect("add");
    }

    let held = attribute::of(&db, &dba, false).await.expect("attrs");
    let about = held.iter().find(|a| a.name == "about").expect("about");
    let note = held.iter().find(|a| a.name == "note").expect("note");
    assert_eq!(about.datatype, note.datatype, "the same kind of field");
    assert_eq!(about.labels, vec![description]);
    assert_eq!(note.labels, vec![comment]);
}

/// A LABEL MUST EXIST. The engine enforces it, and this pins that we
/// have not lost the ASSERT.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_that_does_not_exist_is_refused() {
    let db = fresh_db().await;
    let e = entity::create(&db, "Thing", &Author::operator()).await.expect("create");
    let ghost = superx_mod_entities::new_id("entity");
    assert!(
        attribute::add(
            &db,
            &e,
            Write {
                name: "x",
                datatype: "text",
                content: text("v"),
                labels: &[ghost],
                options: None
            },
            &Author::operator(),
        )
        .await
        .is_err(),
        "a label with nothing behind it reads as meaning something"
    );
}

/// ARCHIVE AND UNARCHIVE. A property of the entity, so it is versioned
/// and attributed like everything else — and nothing is destroyed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entity_can_be_archived_and_brought_back() {
    let db = fresh_db().await;
    let keep = entity::create(&db, "Visible", &Author::operator()).await.expect("a");
    let away = entity::create(&db, "Put away", &Author::operator()).await.expect("b");
    attribute::add(
        &db,
        &away,
        Write { name: "note", datatype: "text", content: text("kept"), labels: &[], options: None },
        &Author::operator(),
    )
    .await
    .expect("a field on it");

    assert!(!entity::is_archived(&db, &away).await.expect("no"));
    assert!(entity::archive(&db, &away, &Author::operator()).await.expect("archive"));
    assert!(
        !entity::archive(&db, &away, &Author::operator()).await.expect("again"),
        "already away — nothing was decided, so nothing is written"
    );

    assert!(entity::is_archived(&db, &away).await.expect("yes"));
    assert_eq!(entity::list(&db, false).await.expect("list"), vec![keep.clone()]);
    assert_eq!(entity::list(&db, true).await.expect("all").len(), 2, "away, not gone");
    assert_eq!(
        entity::name_of(&db, &away).await.expect("name").as_deref(),
        Some("Put away"),
        "and it still knows what it is called"
    );

    assert!(entity::unarchive(&db, &away, &Author::operator()).await.expect("back"));
    assert!(!entity::is_archived(&db, &away).await.expect("no"));
    assert_eq!(entity::list(&db, false).await.expect("list").len(), 2);
    assert_eq!(
        attribute::of(&db, &away, false).await.expect("attrs").len(),
        3,
        "name, note, archived — the field it held survived being put away"
    );
}

/// An attribute can be retired and restored on its own, separately from
/// the entity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attribute_can_be_retired_and_restored() {
    let db = fresh_db().await;
    let e = entity::create(&db, "Thing", &Author::operator()).await.expect("create");
    let uid = attribute::add(
        &db,
        &e,
        Write { name: "old", datatype: "text", content: text("v"), labels: &[], options: None },
        &Author::operator(),
    )
    .await
    .expect("add");

    assert!(attribute::retire(&db, &uid, &Author::operator()).await.expect("retire"));
    assert!(!attribute::retire(&db, &uid, &Author::operator()).await.expect("again"));
    assert_eq!(attribute::of(&db, &e, false).await.expect("held").len(), 1, "just the name");
    assert_eq!(attribute::of(&db, &e, true).await.expect("all").len(), 2, "still on the record");

    assert!(attribute::restore(&db, &uid, &Author::operator()).await.expect("restore"));
    assert_eq!(attribute::of(&db, &e, false).await.expect("held").len(), 2);
    assert_eq!(attribute::history(&db, &uid).await.expect("history").len(), 3);
}

/// WHAT AN ENTITY IS: the labels on attributes that hold nothing. An
/// attribute WITH content describes the value, not the thing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_thing_is_comes_from_its_is_row_and_not_from_its_fields() {
    let db = fresh_db().await;
    let role = entity::create(&db, "role", &Author::operator()).await.expect("l1");
    let mandate = entity::create(&db, "mandate", &Author::operator()).await.expect("l2");
    let dba = entity::create(&db, "DBA", &Author::operator()).await.expect("e");

    entity::declare(&db, &dba, "is", std::slice::from_ref(&role), &Author::operator())
        .await
        .expect("it is a role");
    attribute::add(
        &db,
        &dba,
        Write {
            name: "house_rules",
            datatype: "text",
            content: text("<p>never drop a table</p>"),
            labels: std::slice::from_ref(&mandate),
            options: None,
        },
        &Author::operator(),
    )
    .await
    .expect("a rule that binds");

    assert_eq!(
        entity::labels_of(&db, &dba).await.expect("labels"),
        vec![role.clone()],
        "it IS a role; `mandate` describes the rule, not the DBA"
    );

    // A FIELD BORN WITH A LABEL AND NO VALUE is still a field. Adding
    // `notes` labelled `mandate` and typing nothing into it yet used to
    // make the DBA itself a mandate, because "content-less and labelled"
    // was read as a declaration (operator, 2026-09-03: "entities have
    // labels, fields have labels").
    attribute::add(
        &db,
        &dba,
        Write {
            name: "notes",
            datatype: "text",
            content: None,
            labels: std::slice::from_ref(&mandate),
            options: None,
        },
        &Author::operator(),
    )
    .await
    .expect("an empty, labelled field");
    assert_eq!(
        entity::labels_of(&db, &dba).await.expect("labels"),
        vec![role],
        "an empty field's label is the field's, not the entity's"
    );
}

/// Who did it is on every version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_version_records_who_wrote_it() {
    let db = fresh_db().await;
    let agent = Author::claimed("agent", Some("run-7"), Some("role-3")).expect("kind");
    let e = entity::create(&db, "Written by an agent", &agent).await.expect("create");

    let mut resp = db
        .query("SELECT author_kind, author_uid, via_uid FROM entity_attribute WHERE entity = $e")
        .bind(("e", e))
        .await
        .expect("read");
    let rows: Vec<Value> = resp.take(0).expect("rows");
    let row = format!("{:?}", rows.first().expect("one"));
    assert!(row.contains("agent") && row.contains("run-7") && row.contains("role-3"), "{row}");

    assert!(Author::claimed("wizard", None, None).is_err());
}

// ─── edges ──────────────────────────────────────────────────────────────

/// MANY EDGES BETWEEN THE SAME PAIR. A role can both contain a task and
/// consult it, and those are two connections with two names and two
/// histories — not one connection with two meanings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_entities_can_be_linked_more_than_once() {
    use superx_mod_entities::edge::{self, Direction};

    let db = fresh_db().await;
    let contains = entity::create(&db, "contains", &Author::operator()).await.expect("l1");
    let consults = entity::create(&db, "consults", &Author::operator()).await.expect("l2");
    let dba = entity::create(&db, "DBA", &Author::operator()).await.expect("a");
    let task = entity::create(&db, "Nightly verify", &Author::operator()).await.expect("b");

    let owns = edge::link(&db, &dba, &task, "runs", std::slice::from_ref(&contains), &Author::operator())
        .await
        .expect("link one");
    edge::link(&db, &dba, &task, "reads", std::slice::from_ref(&consults), &Author::operator())
        .await
        .expect("link two");

    let out = edge::of(&db, &dba, Direction::Out).await.expect("out");
    assert_eq!(out.len(), 2, "both connections, kept apart");
    assert_eq!(
        out.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["runs", "reads"]
    );

    // Cutting one leaves the other alone.
    assert!(edge::unlink(&db, &owns, &Author::operator()).await.expect("cut"));
    let out = edge::of(&db, &dba, Direction::Out).await.expect("out");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "reads");
}

/// Both directions are free, because the engine holds the pointers both
/// ways. "What points AT this" is the question a tree needs when you
/// walk back up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edges_are_readable_from_either_end() {
    use superx_mod_entities::edge::{self, Direction};

    let db = fresh_db().await;
    let a = entity::create(&db, "A", &Author::operator()).await.expect("a");
    let b = entity::create(&db, "B", &Author::operator()).await.expect("b");
    edge::link(&db, &a, &b, "contains", &[], &Author::operator()).await.expect("link");

    assert_eq!(edge::of(&db, &a, Direction::Out).await.expect("out").len(), 1);
    assert!(edge::of(&db, &a, Direction::In).await.expect("in").is_empty());
    assert_eq!(edge::of(&db, &b, Direction::In).await.expect("in").len(), 1);
    assert_eq!(edge::of(&db, &b, Direction::Both).await.expect("both").len(), 1);
}

/// A cut connection is still on the record, and can be put back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unlinking_keeps_the_record_and_can_be_undone() {
    use superx_mod_entities::edge::{self, Direction};

    let db = fresh_db().await;
    let a = entity::create(&db, "A", &Author::operator()).await.expect("a");
    let b = entity::create(&db, "B", &Author::operator()).await.expect("b");
    let uid = edge::link(&db, &a, &b, "contains", &[], &Author::operator()).await.expect("link");

    assert!(edge::unlink(&db, &uid, &Author::operator()).await.expect("cut"));
    assert!(!edge::unlink(&db, &uid, &Author::operator()).await.expect("again"));
    assert!(edge::of(&db, &a, Direction::Out).await.expect("out").is_empty());

    assert!(edge::relink(&db, &uid, &Author::operator()).await.expect("back"));
    assert_eq!(edge::of(&db, &a, Direction::Out).await.expect("out").len(), 1);
    assert_eq!(edge::history(&db, &uid).await.expect("history").len(), 3, "link, cut, relink");
}

/// The endpoints must exist, and nothing points at itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edge_needs_two_real_and_different_ends() {
    use superx_mod_entities::edge;

    let db = fresh_db().await;
    let a = entity::create(&db, "A", &Author::operator()).await.expect("a");
    let ghost = superx_mod_entities::new_id("entity");

    assert!(
        edge::link(&db, &a, &ghost, "contains", &[], &Author::operator()).await.is_err(),
        "the schema refuses an edge to an entity that does not exist"
    );
    assert!(
        edge::link(&db, &a, &a, "contains", &[], &Author::operator()).await.is_err(),
        "pointing at itself says nothing"
    );
}

/// THE OPERATOR'S OWN EXAMPLE, end to end.
///
/// A DBA role, four levels deep, with prose, a mandate, a config, the
/// things it consults and the thing it manages — built from nothing but
/// entities, attributes and edges, and read back from ONE walk.
///
/// This is the acceptance test for the whole substrate. If this cannot
/// be expressed and read, nothing downstream matters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_role_is_built_from_entities_attributes_and_edges() {
    use superx_mod_entities::edge::{self, Direction};

    let db = fresh_db().await;
    let op = Author::operator();

    // The vocabulary is entities like everything else.
    let label = entity::create(&db, "label", &op).await.expect("label");
    let role = entity::create(&db, "role", &op).await.expect("role");
    let mandate = entity::create(&db, "mandate", &op).await.expect("mandate");
    let contains = entity::create(&db, "contains", &op).await.expect("contains");
    let consults = entity::create(&db, "consults", &op).await.expect("consults");
    for l in [&role, &mandate, &contains, &consults] {
        entity::declare(&db, l, "is", std::slice::from_ref(&label), &op).await.expect("a label");
    }
    // What `role` MEANS is a field on it.
    attribute::add(
        &db,
        &role,
        Write {
            name: "treatment",
            datatype: "text",
            content: text("This is who you are. Act within it and nothing beyond it."),
            labels: &[],
            options: None,
        },
        &op,
    )
    .await
    .expect("what a role means");

    // The DBA, and what it is.
    let dba = entity::create(&db, "DBA", &op).await.expect("dba");
    entity::declare(&db, &dba, "is", std::slice::from_ref(&role), &op).await.expect("a role");
    attribute::add(
        &db,
        &dba,
        Write {
            name: "house_rules",
            datatype: "text",
            content: text("<p>never drop a table without a snapshot</p>"),
            labels: std::slice::from_ref(&mandate),
            options: None,
        },
        &op,
    )
    .await
    .expect("its mandate");
    attribute::add(
        &db,
        &dba,
        Write {
            name: "replicas",
            datatype: "number",
            content: Some(Value::Number(3.into())),
            labels: &[],
            options: None,
        },
        &op,
    )
    .await
    .expect("a number");

    // Four levels down, plus something it consults off to the side.
    let backups = entity::create(&db, "Backups", &op).await.expect("b");
    let nightly = entity::create(&db, "Nightly verify", &op).await.expect("n");
    let checksum = entity::create(&db, "Checksum step", &op).await.expect("c");
    let runbooks = entity::create(&db, "Runbooks", &op).await.expect("r");
    let c = std::slice::from_ref(&contains);
    edge::link(&db, &dba, &backups, "runs backups", c, &op).await.expect("1");
    edge::link(&db, &backups, &nightly, "its task", c, &op).await.expect("2");
    edge::link(&db, &nightly, &checksum, "then", c, &op).await.expect("3");
    edge::link(&db, &dba, &runbooks, "reads", std::slice::from_ref(&consults), &op)
        .await
        .expect("4");

    // --- and now READ IT BACK ----------------------------------------

    assert_eq!(entity::name_of(&db, &dba).await.expect("n").as_deref(), Some("DBA"));
    assert_eq!(entity::labels_of(&db, &dba).await.expect("l"), vec![role.clone()], "it IS a role");

    // Its fields, in the order they were added.
    let held = attribute::of(&db, &dba, false).await.expect("held");
    assert_eq!(
        held.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["name", "is", "house_rules", "replicas"]
    );
    let rules = held.iter().find(|a| a.name == "house_rules").expect("rules");
    assert_eq!(rules.labels, vec![mandate], "the rule BINDS, and the label says so");

    // ONE WALK, four deep, following only `contains`.
    let sub = edge::walk(&db, &dba, Some(&contains), 6).await.expect("walk");
    let reached: Vec<String> = {
        let mut names = Vec::new();
        for n in &sub.nodes {
            names.push(entity::name_of(&db, &n.entity).await.expect("n").unwrap_or_default());
        }
        names
    };
    for expected in ["DBA", "Backups", "Nightly verify", "Checksum step"] {
        assert!(reached.contains(&expected.to_string()), "{expected} is in the role's world: {reached:?}");
    }
    assert!(
        !reached.contains(&"Runbooks".to_string()),
        "and `consults` was not followed: {reached:?}"
    );
    assert_eq!(sub.edges.len(), 3, "the three `contains` connections");

    // Following everything finds the rag too.
    let all = edge::walk(&db, &dba, None, 6).await.expect("walk all");
    assert_eq!(all.nodes.len(), 5, "everything reachable");

    // What the DBA points at, one hop, with what each connection means.
    let out = edge::of(&db, &dba, Direction::Out).await.expect("out");
    assert_eq!(out.len(), 2);

    // A rename changes nothing about the graph.
    entity::rename(&db, &dba, "Database Administrator", &op).await.expect("rename");
    let after = edge::walk(&db, &dba, Some(&contains), 6).await.expect("walk again");
    assert_eq!(after.nodes.len(), sub.nodes.len(), "the identity never moved");
    assert_eq!(
        entity::name_of(&db, &dba).await.expect("n").as_deref(),
        Some("Database Administrator")
    );
}

/// A CUT CONNECTION MUST NOT BE FOLLOWED. `walk` filters the traversal
/// with a row-level `active = true`, but unlinking APPENDS a row and
/// leaves the original in place — so the old `active = true` row still
/// matches and the walk keeps reaching a node nothing connects to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_walk_does_not_follow_an_unlinked_edge() {
    use superx_mod_entities::edge;

    let db = fresh_db().await;
    let op = Author::operator();
    let a = entity::create(&db, "A", &op).await.expect("a");
    let b = entity::create(&db, "B", &op).await.expect("b");
    let uid = edge::link(&db, &a, &b, "tmp", &[], &op).await.expect("link");

    let before = edge::walk(&db, &a, None, 4).await.expect("walk");
    assert_eq!(before.nodes.len(), 2, "linked: A reaches B");

    edge::unlink(&db, &uid, &op).await.expect("cut");
    let after = edge::walk(&db, &a, None, 4).await.expect("walk");
    assert_eq!(after.edges.len(), 0, "the edge is gone");
    assert_eq!(
        after.nodes.len(),
        1,
        "and so is the node it reached — a cut connection leaves no orphan: {:?}",
        after.nodes
    );
}

/// A DECIMAL SURVIVES BEING READ. `Number::to_int()` answers `Some` for
/// a Float too, so asking it first turned 19.99 into 19 on the way to
/// the browser — and the next write put 19 back. A price destroyed by
/// being looked at.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_decimal_is_not_truncated_on_the_way_out() {


    let db = fresh_db().await;
    let op = Author::operator();
    let e = entity::create(&db, "Thing", &op).await.expect("e");
    attribute::add(
        &db,
        &e,
        Write {
            name: "ratio",
            datatype: "number",
            content: Some(Value::Number(19.99.into())),
            labels: &[],
            options: None,
        },
        &op,
    )
    .await
    .expect("set");

    let view = api::detail(&db, &superx_ops::record_uuid(&e)).await.expect("detail");
    let ratio = view.attributes.iter().find(|a| a.name == "ratio").expect("there");
    assert_eq!(ratio.content, serde_json::json!(19.99), "the value the operator typed");
}

/// EVERY ADVERTISED DATATYPE CAN ACTUALLY BE WRITTEN, through the only
/// write path there is. `datetime` could not: JSON has no instant
/// literal, so it arrived as a string and the gate refused it — one of
/// the five was unusable, and the test table that should have caught it
/// omitted the very type that was broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_datatype_can_be_written_through_the_api() {


    let db = fresh_db().await;
    let op = Author::operator();
    let e = entity::create(&db, "Thing", &op).await.expect("e");
    let frag = superx_ops::record_uuid(&e);

    for (name, datatype, content) in [
        ("a", "text", serde_json::json!("prose")),
        ("b", "number", serde_json::json!(19.99)),
        ("c", "boolean", serde_json::json!(true)),
        ("d", "datetime", serde_json::json!("2026-08-28T12:34:00Z")),
        ("e", "json", serde_json::json!({ "k": [1, 2] })),
    ] {
        api::put_attribute(
            &db,
            &frag,
            &serde_json::from_value(serde_json::json!({
                "name": name, "datatype": datatype, "content": content
            }))
            .expect("req"),
            &op,
        )
        .await
        .unwrap_or_else(|err| panic!("{datatype} could not be written: {err}"));
    }

    let view = api::detail(&db, &frag).await.expect("detail");
    assert_eq!(view.attributes.len(), 6, "the name plus all five datatypes");

    // And a datetime that is not an instant still says so.
    assert!(
        api::put_attribute(
            &db,
            &frag,
            &serde_json::from_value(serde_json::json!({
                "name": "bad", "datatype": "datetime", "content": "yesterday"
            }))
            .expect("req"),
            &op,
        )
        .await
        .is_err(),
        "'yesterday' is not an instant"
    );
}

/// AN ATTRIBUTE BELONGS TO ONE ENTITY. The amend path took a uid from
/// the body and never checked it against the entity in the URL, so a
/// request addressed to one thing could rewrite another's — including
/// renaming an attribute to `archived` to hide it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_entity_cannot_rewrite_another_entitys_attribute() {


    let db = fresh_db().await;
    let op = Author::operator();
    let a = entity::create(&db, "A", &op).await.expect("a");
    let b = entity::create(&db, "B", &op).await.expect("b");
    let b_name = attribute::of(&db, &b, false)
        .await
        .expect("attrs")
        .into_iter()
        .find(|x| x.name == "name")
        .expect("b has a name");

    let err = api::put_attribute(
        &db,
        &superx_ops::record_uuid(&a),
        &serde_json::from_value(serde_json::json!({
            "uid": b_name.uid, "name": "name", "datatype": "text", "content": "HIJACKED"
        }))
        .expect("req"),
        &op,
    )
    .await
    .expect_err("A must not be able to rewrite B");
    assert!(err.to_string().contains("does not belong"), "{err}");
    assert_eq!(entity::name_of(&db, &b).await.expect("n").as_deref(), Some("B"));
}

/// Depth is the SHORTEST way there. A depth-first walk recorded
/// whichever branch arrived first, so a direct neighbour reachable the
/// long way round was painted as though it were far away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn depth_is_the_shortest_path_not_the_first_one_found() {
    use superx_mod_entities::edge;

    let db = fresh_db().await;
    let op = Author::operator();
    let root = entity::create(&db, "root", &op).await.expect("r");
    let mid = entity::create(&db, "mid", &op).await.expect("m");
    let leaf = entity::create(&db, "leaf", &op).await.expect("l");
    edge::link(&db, &root, &mid, "a", &[], &op).await.expect("1");
    edge::link(&db, &mid, &leaf, "b", &[], &op).await.expect("2");
    edge::link(&db, &root, &leaf, "direct", &[], &op).await.expect("3");

    let sub = edge::walk(&db, &root, None, 4).await.expect("walk");
    let depth_of = |e: &superx_kernel::types::RecordId| {
        sub.nodes.iter().find(|n| &n.entity == e).map(|n| n.depth)
    };
    assert_eq!(depth_of(&leaf), Some(1), "leaf is a direct child of root");
    assert_eq!(depth_of(&mid), Some(1));
}

/// ARCHIVING A PARENT MUST NOT TAKE ITS CHILDREN WITH IT.
///
/// The menu's top level is "entities nothing points at", and archived
/// entities are hidden from it. Those two rules together used to delete
/// a thing from the interface without deleting it from the store: the
/// child still had an inbound edge so it was no root, and the only row
/// that led to it was the one just put away. It was in the database and
/// on no screen, with nothing an operator could click to get it back.
///
/// A parent that cannot be seen is not a parent for this purpose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_a_parent_gives_its_children_back_to_the_top_level() {
    let db = fresh_db().await;
    let op = Author::operator();
    let parent = entity::create(&db, "Parent", &op).await.expect("parent");
    let child = entity::create(&db, "Child", &op).await.expect("child");
    edge::link(&db, &parent, &child, "owns", &[], &op).await.expect("link");

    let names = |rows: &[api::TreeNodeView]| {
        rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>()
    };

    // While the parent is visible the child hangs off it, not the top.
    let top = api::roots(&db, false).await.expect("roots");
    assert!(names(&top).contains(&"Parent".to_string()));
    assert!(
        !names(&top).contains(&"Child".to_string()),
        "a child reachable by expanding must not also sit at the root"
    );

    entity::archive(&db, &parent, &op).await.expect("archive");

    let top = api::roots(&db, false).await.expect("roots after archive");
    assert!(
        !names(&top).contains(&"Parent".to_string()),
        "the archived parent is hidden"
    );
    assert!(
        names(&top).contains(&"Child".to_string()),
        "the child is reachable again — archiving the only way in must not \
         leave it in the store and off every screen"
    );
}

/// The mirror of the same rule: an expander must open on something.
///
/// `has_children` counted any live outbound edge, including ones landing
/// on an entity the menu hides — so a row wore a chevron that expanded
/// to an empty list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_only_opens_when_there_is_something_behind_it() {
    let db = fresh_db().await;
    let op = Author::operator();
    let parent = entity::create(&db, "Parent", &op).await.expect("parent");
    let child = entity::create(&db, "Child", &op).await.expect("child");
    edge::link(&db, &parent, &child, "owns", &[], &op).await.expect("link");

    let opens = |rows: &[api::TreeNodeView], who: &str| {
        rows.iter().find(|r| r.name == who).map(|r| r.has_children)
    };

    let top = api::roots(&db, false).await.expect("roots");
    assert_eq!(opens(&top, "Parent"), Some(true), "it has a visible child");

    entity::archive(&db, &child, &op).await.expect("archive child");

    let top = api::roots(&db, false).await.expect("roots after archive");
    assert_eq!(
        opens(&top, "Parent"),
        Some(false),
        "its only child is hidden, so the chevron would open on nothing"
    );
}

/// PROVISIONING REPLACES. IT DOES NOT ACCUMULATE.
///
/// This is the failure that cost a day. The first cut of this schema
/// defined every table `IF NOT EXISTS`, which is not "define" — it is
/// "define unless something is already sitting there". On an instance
/// that had run the shape before it, `entity` kept v1's required
/// `entity_type` column, and the new code's create, which writes only
/// `created_at`, was refused every single time:
///
///     Found NONE for field `entity_type`, but expected a string
///
/// The database was provisioned, the module was running, and nothing
/// could be created. Applying a new schema over an old one is not an
/// upgrade; it is a half-open door. So the schema drops what it owns
/// before it defines it, and this pins that: v1 first, the real thing
/// second, and an entity must be creatable afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_schema_replaces_an_older_shape_instead_of_layering_over_it() {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");

    // The shape that shipped before #353, exactly as it was.
    let v1 = include_str!("fixtures/v1-entities.surql")
        .replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(v1).await.expect("v1 applies").check().expect("v1 is clean");

    // And the current one, straight over the top of it.
    let now = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(now).await.expect("re-provision").check().expect("schema replaces cleanly");

    // The proof is that the module can do its most basic act.
    let id = entity::create(&db, "DBA", &Author::operator())
        .await
        .expect("an upgraded instance must be able to create an entity");
    assert_eq!(
        entity::name_of(&db, &id).await.expect("read"),
        Some("DBA".to_string())
    );
}

/// And provisioning twice is provisioning once — the operator may run it
/// again without hunting for a reason it would be unsafe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_is_safe_to_repeat() {
    let db = fresh_db().await;
    let again = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(again).await.expect("re-apply").check().expect("clean on a live database");
    entity::create(&db, "Still works", &Author::operator())
        .await
        .expect("create after a second provision");
}

/// AN ATTRIBUTE CANNOT HANG OFF A UUID NOBODY ISSUED.
///
/// `record<entity>` is a TYPE, not a foreign key — the engine checks the
/// shape of the value and never that the row exists. Without an ASSERT
/// an attribute could be written against an invented uuid: absent from
/// every listing, reachable by nothing, and permanent, because nothing
/// here deletes. `labels` on the same table had always asserted
/// existence and `entity_edge` is ENFORCED; this was the third
/// reference, and the only one that was open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attribute_needs_an_entity_that_exists() {
    let db = fresh_db().await;
    let ghost = superx_mod_entities::new_id("entity");
    assert!(
        attribute::add(
            &db,
            &ghost,
            Write {
                name: "orphan",
                datatype: "text",
                content: text("nowhere"),
                labels: &[],
                options: None
            },
            &Author::operator(),
        )
        .await
        .is_err(),
        "an attribute on an entity that was never created is unreachable data"
    );
}

/// ONE NAME, ONE ARCHIVED FLAG. The rest may repeat.
///
/// Most attributes are a list and repetition is the point — declaring
/// two labels writes two rows both called `is`, and two fields may
/// legitimately share a name and differ by label. But `name` and
/// `archived` are the two this module reads BY NAME to answer "what is
/// this called" and "is it put away", and it answers with the first
/// match. A second one makes the answer depend on insertion order,
/// which is an implementation detail standing in for a rule. The form
/// refused this; the module did not, so anything talking to the
/// database directly walked straight past it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_name_and_the_archived_flag_cannot_be_duplicated() {
    let db = fresh_db().await;
    let op = Author::operator();
    let e = entity::create(&db, "Real", &op).await.expect("create");

    let second_name = attribute::add(
        &db,
        &e,
        Write { name: "name", datatype: "text", content: text("IMPOSTOR"), labels: &[], options: None },
        &op,
    )
    .await;
    assert!(second_name.is_err(), "a second name makes the real one a matter of ordering");

    entity::archive(&db, &e, &op).await.expect("archive");
    let second_flag = attribute::add(
        &db,
        &e,
        Write { name: "archived", datatype: "boolean", content: Some(Value::Bool(false)), labels: &[], options: None },
        &op,
    )
    .await;
    assert!(second_flag.is_err(), "two archived flags disagree and one of them wins by accident");

    // And the rename path still works, because it amends the chain it
    // already has rather than adding a second row.
    entity::rename(&db, &e, "Renamed", &op).await.expect("rename still works");
    assert_eq!(entity::name_of(&db, &e).await.expect("read"), Some("Renamed".to_string()));

    // Ordinary repetition is untouched: two labels, two `is` rows.
    let role = entity::create(&db, "role", &op).await.expect("role");
    let tag = entity::create(&db, "tag", &op).await.expect("tag");
    entity::declare(&db, &e, "is", &[role], &op).await.expect("first declaration");
    entity::declare(&db, &e, "is", &[tag], &op).await.expect("a second label is not a duplicate");
}

/// A field is removed through the entity it belongs to, and only that
/// one: the URL names the owner, and a request addressed to another
/// entity may not reach in. A link, by contrast, touches two entities
/// and either may cut it — a third may not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_is_retired_by_its_owner_and_a_link_by_either_end() {

    let db = fresh_db().await;
    let op = Author::operator();
    let a = entity::create(&db, "A", &op).await.expect("a");
    let b = entity::create(&db, "B", &op).await.expect("b");
    let c = entity::create(&db, "C", &op).await.expect("c");
    let field = attribute::add(
        &db,
        &a,
        Write { name: "notes", datatype: "text", content: text("<p>x</p>"), labels: &[], options: None },
        &op,
    )
    .await
    .expect("field");
    let frag = |id: &superx_kernel::types::RecordId| superx_ops::record_uuid(id);

    assert!(
        api::retire_attribute(&db, &frag(&b), &field, &op).await.is_err(),
        "B may not retire A's field"
    );
    assert!(api::retire_attribute(&db, &frag(&a), &field, &op).await.expect("owner retires"));
    assert!(
        attribute::of(&db, &a, false).await.expect("live").iter().all(|x| x.uid != field),
        "the field has left the screen"
    );
    assert!(
        attribute::of(&db, &a, true).await.expect("all").iter().any(|x| x.uid == field),
        "and the record still holds it"
    );

    let link = edge::link(&db, &a, &b, "depends on", &[], &op).await.expect("link");
    assert!(api::unlink(&db, &frag(&c), &link, &op).await.is_err(), "C is not on this edge");
    assert!(api::unlink(&db, &frag(&b), &link, &op).await.expect("the far end may cut it"));
    assert!(
        edge::of(&db, &a, superx_mod_entities::edge::Direction::Both).await.expect("edges").is_empty(),
        "the link is cut for both ends"
    );
}
