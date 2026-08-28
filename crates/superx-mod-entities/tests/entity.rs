//! Contract tests: entities and their attributes (epic #353).
//!
//! An entity is a uuid7. Everything else — its name, what it is, what it
//! holds, whether it is archived — is an attribute. These pin the
//! properties the rest of the module and every reader depend on.

use superx_kernel::types::Value;
use superx_mod_entities::attribute::{self, Write};
use superx_mod_entities::author::Author;
use superx_mod_entities::{entity, SCHEMA_DDL};

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
async fn what_a_thing_is_comes_from_attributes_that_hold_nothing() {
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
        vec![role],
        "it IS a role; `mandate` describes the rule, not the DBA"
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
