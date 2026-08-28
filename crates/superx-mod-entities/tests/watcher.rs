//! The operator's second example, built end to end (epic #353).
//!
//! > "I can have a role that checks the Microsoft Teams UI and sends me
//! >  a WhatsApp text when I get a new message, and all I do is set the
//! >  frequency. Another example of a graph role with various labelled
//! >  entities — I can have an entity labelled `credential` and store
//! >  WhatsApp and Teams credentials."
//!
//! Nothing in the module knows what Teams is, what a credential is, or
//! what a frequency means. It stores entities, attributes and edges; the
//! labels carry the meaning and a reader acts on it. If this shape can
//! be written and read back whole, the substrate is doing its job.

use superx_kernel::types::Value;
use superx_mod_entities::attribute::{self, Write};
use superx_mod_entities::author::Author;
use superx_mod_entities::edge::{self, Direction};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_role_that_watches_teams_and_texts_whatsapp() {
    let db = fresh_db().await;
    let op = Author::operator();

    // ── the vocabulary ────────────────────────────────────────────────
    // Every one of these is an entity. Nothing is a type.
    let label = entity::create(&db, "label", &op).await.expect("label");
    let mut vocab = std::collections::HashMap::new();
    for word in ["role", "credential", "task", "schedule", "instructions", "uses", "then"] {
        let e = entity::create(&db, word, &op).await.expect("word");
        entity::declare(&db, &e, "is", std::slice::from_ref(&label), &op).await.expect("a label");
        vocab.insert(word, e);
    }
    let (role, credential, task, schedule, instructions, uses, then) = (
        vocab["role"].clone(),
        vocab["credential"].clone(),
        vocab["task"].clone(),
        vocab["schedule"].clone(),
        vocab["instructions"].clone(),
        vocab["uses"].clone(),
        vocab["then"].clone(),
    );

    // What each label MEANS is a field on the label itself. This is the
    // only interpretation layer, and a reader is the only thing that
    // reads it.
    for (l, meaning) in [
        (&role, "This is who you are. Act within it and nothing beyond it."),
        (&credential, "A REFERENCE to a secret, never the secret. Fetch it, use it, never log it."),
        (&task, "Do what the instructions say, and nothing else."),
        (&schedule, "How often to run. Read by whatever does the running."),
    ] {
        attribute::add(
            &db,
            l,
            Write { name: "treatment", datatype: "text", content: text(meaning), labels: &[], options: None },
            &op,
        )
        .await
        .expect("what it means");
    }

    // ── the credentials ───────────────────────────────────────────────
    // A REFERENCE, not the secret: where to fetch it from. The store has
    // no opinion, but putting a live secret in a versioned, append-only
    // row would mean it could never be removed.
    let mut creds = Vec::new();
    for (name, item) in [("Teams login", "keychain:superx/teams"), ("WhatsApp API key", "env:WHATSAPP_TOKEN")] {
        let c = entity::create(&db, name, &op).await.expect("cred");
        entity::declare(&db, &c, "is", std::slice::from_ref(&credential), &op).await.expect("it is one");
        attribute::add(
            &db,
            &c,
            Write { name: "reference", datatype: "text", content: text(item), labels: &[], options: None },
            &op,
        )
        .await
        .expect("where to fetch it");
        creds.push(c);
    }

    // ── the role, and the only thing the operator sets ────────────────
    let watcher = entity::create(&db, "Teams watcher", &op).await.expect("role");
    entity::declare(&db, &watcher, "is", std::slice::from_ref(&role), &op).await.expect("a role");
    attribute::add(
        &db,
        &watcher,
        Write {
            name: "every",
            datatype: "number",
            content: Some(Value::Number(300.into())),
            labels: std::slice::from_ref(&schedule),
            options: None,
        },
        &op,
    )
    .await
    .expect("the frequency — the one knob");

    // ── the two tasks ─────────────────────────────────────────────────
    let check = entity::create(&db, "Check Teams for new messages", &op).await.expect("t1");
    let send = entity::create(&db, "Send a WhatsApp text", &op).await.expect("t2");
    for (t, orders) in [
        (&check, "Open the Teams UI and report any message newer than the last run."),
        (&send, "Text the operator on WhatsApp with what the previous step found."),
    ] {
        entity::declare(&db, t, "is", std::slice::from_ref(&task), &op).await.expect("a task");
        attribute::add(
            &db,
            t,
            Write {
                name: "orders",
                datatype: "text",
                content: text(orders),
                labels: std::slice::from_ref(&instructions),
                options: None,
            },
            &op,
        )
        .await
        .expect("what to do");
    }

    // ── the graph ─────────────────────────────────────────────────────
    let contains = entity::create(&db, "contains", &op).await.expect("c");
    entity::declare(&db, &contains, "is", std::slice::from_ref(&label), &op).await.expect("l");
    let c = std::slice::from_ref(&contains);
    edge::link(&db, &watcher, &check, "first", c, &op).await.expect("1");
    edge::link(&db, &watcher, &send, "second", c, &op).await.expect("2");
    edge::link(&db, &check, &send, "then", std::slice::from_ref(&then), &op).await.expect("order");
    edge::link(&db, &check, &creds[0], "signs in with", std::slice::from_ref(&uses), &op).await.expect("3");
    edge::link(&db, &send, &creds[1], "sends with", std::slice::from_ref(&uses), &op).await.expect("4");

    // ── READ IT BACK, the way something that had to run it would ──────

    // What is this thing?
    assert_eq!(entity::labels_of(&db, &watcher).await.expect("l"), vec![role.clone()]);

    // The one knob, and what its label says to do about it.
    let held = attribute::of(&db, &watcher, false).await.expect("held");
    let every = held.iter().find(|a| a.name == "every").expect("frequency");
    assert_eq!(every.labels, vec![schedule.clone()]);
    assert!(matches!(every.content, Some(Value::Number(_))));

    let meaning = attribute::of(&db, &schedule, false)
        .await
        .expect("on the label")
        .into_iter()
        .find(|a| a.name == "treatment")
        .and_then(|a| a.content);
    assert!(
        format!("{meaning:?}").contains("How often to run"),
        "the label tells a reader what the number is for"
    );

    // Everything the role reaches, from ONE walk.
    let world = edge::walk(&db, &watcher, None, 6).await.expect("walk");
    let mut names = Vec::new();
    for n in &world.nodes {
        names.push(entity::name_of(&db, &n.entity).await.expect("n").unwrap_or_default());
    }
    for expected in [
        "Teams watcher",
        "Check Teams for new messages",
        "Send a WhatsApp text",
        "Teams login",
        "WhatsApp API key",
    ] {
        assert!(names.contains(&expected.to_string()), "{expected} missing from {names:?}");
    }

    // The credentials it can reach, found by label rather than by name.
    let mut reachable_creds = Vec::new();
    for n in &world.nodes {
        if entity::labels_of(&db, &n.entity).await.expect("l").contains(&credential) {
            reachable_creds.push(entity::name_of(&db, &n.entity).await.expect("n").unwrap_or_default());
        }
    }
    reachable_creds.sort();
    assert_eq!(reachable_creds, vec!["Teams login", "WhatsApp API key"]);

    // NEVER THE SECRET ITSELF — a reference to where it lives.
    let teams_cred = attribute::of(&db, &creds[0], false).await.expect("attrs");
    let reference = teams_cred.iter().find(|a| a.name == "reference").expect("ref");
    assert!(
        format!("{:?}", reference.content).contains("keychain:"),
        "a pointer to the secret, not the secret"
    );

    // The order of work: which task comes after which.
    let after_check = edge::of(&db, &check, Direction::Out)
        .await
        .expect("out")
        .into_iter()
        .filter(|e| e.labels.contains(&then))
        .count();
    assert_eq!(after_check, 1, "check, then send");

    // Changing the frequency is one amend, and nothing else moves.
    attribute::amend(
        &db,
        &every.uid,
        Write {
            name: "every",
            datatype: "number",
            content: Some(Value::Number(60.into())),
            labels: std::slice::from_ref(&schedule),
            options: None,
        },
        &op,
    )
    .await
    .expect("set the frequency");
    let again = edge::walk(&db, &watcher, None, 6).await.expect("walk again");
    assert_eq!(again.nodes.len(), world.nodes.len(), "the graph did not notice");
    assert_eq!(
        attribute::history(&db, &every.uid).await.expect("history").len(),
        2,
        "and both frequencies are on the record, with who set them"
    );
}
