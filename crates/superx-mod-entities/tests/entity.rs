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
