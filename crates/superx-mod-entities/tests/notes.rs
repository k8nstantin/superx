//! The prose store (issue #268).
//!
//! A description, a spec, a mandate and a comment are the same
//! mechanism with a different label. These pin the properties that
//! makes true — above all the one the whole entity model turns on: a
//! product can carry BOTH a description and a spec, which is
//! impossible while the carrier is an edge's rel_type.

use superx_mod_entities::dictionary;
use superx_mod_entities::nodes::create_entity;
use superx_mod_entities::notes::{self, Author};
use superx_mod_entities::registry;
use superx_mod_entities::texts;
use superx_mod_entities::SCHEMA_DDL;

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema applies clean");
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("dictionary");
    db
}

/// The case the whole entity model turns on. Same entity, same store,
/// same shape — and the label is the entire difference. This is
/// impossible while the carrier is an edge's rel_type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_product_carries_both_a_description_and_a_spec() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");

    notes::write(&db, &product, "description", "a ledger, for humans", &Author::operator())
        .await
        .expect("description");
    notes::write(&db, &product, "spec", "append-only; latest wins", &Author::operator())
        .await
        .expect("spec");

    let all = notes::for_entity(&db, &product, false).await.expect("read");
    assert_eq!(all.len(), 2, "both, side by side, distinguished only by label");
    let by = |l: &str| all.iter().find(|n| n.label == l).expect("present").body.clone();
    assert_eq!(by("description"), "a ledger, for humans");
    assert_eq!(by("spec"), "append-only; latest wins");
}

/// Cardinality is the dictionary's to decide, not the call site's:
/// `description` is `one`, so a second write amends the first rather
/// than leaving two live descriptions and no rule for which counts.
/// `comments` is `many`, so each is its own note.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dictionary_decides_whether_a_write_amends_or_adds() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build it", None, None).await.expect("create");

    let (first, new) = notes::write(&db, &task, "description", "v1", &Author::operator())
        .await
        .expect("write");
    assert!(new);
    let (second, new_again) = notes::write(&db, &task, "description", "v2", &Author::operator())
        .await
        .expect("write");
    assert!(!new_again, "amended, not added");
    assert_eq!(first, second, "same chain");
    assert_eq!(
        notes::for_entity(&db, &task, false).await.expect("read").len(),
        1,
        "one description, two versions"
    );

    notes::write(&db, &task, "comments", "one", &Author::operator()).await.expect("c1");
    notes::write(&db, &task, "comments", "two", &Author::operator()).await.expect("c2");
    let comments: Vec<_> = notes::for_entity(&db, &task, false)
        .await
        .expect("read")
        .into_iter()
        .filter(|n| n.label == "comments")
        .collect();
    assert_eq!(comments.len(), 2, "many means many");
}

/// A slot nobody defined is a slot nobody can interpret — accepting it
/// silently is how a binding field becomes a constraint that vanished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prose_under_an_undefined_label_is_refused() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");

    let err = notes::write(&db, &product, "vibes", "anything", &Author::operator())
        .await
        .expect_err("refused");
    assert!(err.to_string().contains("vibes"), "the error names it: {err}");
    assert!(
        notes::for_entity(&db, &product, true).await.expect("read").is_empty(),
        "a refused write leaves nothing behind"
    );
}

/// Editing prose is versioned like everything else: the earlier wording
/// stays readable, because the work done under it was done under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_wording_stays_readable() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build it", None, None).await.expect("create");

    let (uid, _) = notes::write(&db, &task, "description", "first", &Author::operator())
        .await
        .expect("v1");
    notes::write(&db, &task, "description", "second", &Author::operator())
        .await
        .expect("v2");

    let versions = notes::history(&db, &uid).await.expect("history");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].body, "first", "oldest first");
    assert_eq!(versions[1].body, "second");
    assert!(versions.iter().all(|v| v.valid_from.is_some()), "every version is dated");
    assert_eq!(
        notes::current(&db, &uid).await.expect("read").expect("there").body,
        "second"
    );
}

/// A comment on a comment. Edges have no fields, so this could not
/// exist while prose was an entity on the far end of one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_comment_can_answer_a_comment() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build it", None, None).await.expect("create");

    let (parent, _) = notes::write(&db, &task, "comments", "why this way?", &Author::operator())
        .await
        .expect("comment");
    let child = notes::reply(
        &db,
        &task,
        &parent,
        "comments",
        "because the alternative loses history",
        &Author::operator(),
    )
    .await
    .expect("reply");

    let all = notes::for_entity(&db, &task, false).await.expect("read");
    let reply = all.iter().find(|n| n.uid == child).expect("reply present");
    assert_eq!(reply.parent_uid.as_deref(), Some(parent.as_str()));

    notes::reply(&db, &task, "no-such-note", "comments", "…", &Author::operator())
        .await
        .expect_err("a reply to nothing is a mistake, not a root comment");
}

/// Retracting says "this no longer stands". It must not say "this was
/// never said" — the record of having said it is the point.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retracting_withdraws_without_erasing() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build it", None, None).await.expect("create");

    let (uid, _) = notes::write(&db, &task, "comments", "ship it friday", &Author::operator())
        .await
        .expect("comment");
    notes::retract(&db, &task, &uid, &Author::operator()).await.expect("retract");

    assert!(
        notes::for_entity(&db, &task, false).await.expect("read").is_empty(),
        "withdrawn from the live view"
    );
    let kept = notes::for_entity(&db, &task, true).await.expect("read all");
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].active);
    assert_eq!(kept[0].body, "ship it friday", "still on the record, verbatim");
    assert_eq!(
        notes::history(&db, &uid).await.expect("history").len(),
        2,
        "the retraction is itself a version"
    );
}

/// Authorship is the subject of authorization, not decoration: "a role
/// may not edit another role's mandate" is only enforceable because
/// every write records who wrote it, acting as what.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_note_records_who_wrote_it_and_as_what() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build it", None, None).await.expect("create");

    let agent = Author {
        kind: "agent".to_string(),
        uid: Some("agent-7".to_string()),
        via: Some("role-crypto-trader".to_string()),
    };
    let (uid, _) = notes::write(&db, &task, "comments", "position opened", &agent)
        .await
        .expect("write");

    let note = notes::current(&db, &uid).await.expect("read").expect("there");
    assert_eq!(note.author_kind.as_deref(), Some("agent"));
    assert_eq!(note.author_uid.as_deref(), Some("agent-7"));
    assert_eq!(
        note.via_uid.as_deref(),
        Some("role-crypto-trader"),
        "agent X acting as role Y — the part you need when reconstructing what happened"
    );

    // Co-authorship falls out of the chain rather than needing a field.
    notes::write(&db, &task, "comments", "…", &Author::operator()).await.expect("many");
    let versions = notes::history(&db, &uid).await.expect("history");
    assert_eq!(versions[0].author_kind.as_deref(), Some("agent"));
}

/// The seam (#268): describing an entity writes a note AND the legacy
/// text carrier, so the note store becomes real while every existing
/// reader — the runner above all — keeps working unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_legacy_carrier_and_the_note_store_are_both_written() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build it", None, None).await.expect("create");

    texts::set_role_text(&db, &task, "describes", "what to do").await.expect("describe");
    texts::add_comment(&db, &task, "a remark").await.expect("comment");

    // Legacy: the reader that exists today still sees everything.
    let annotations = texts::annotations(&db, &task).await.expect("annotations");
    assert_eq!(annotations.len(), 2);

    // New: the note store carries the same prose, under dictionary labels.
    let stored = notes::for_entity(&db, &task, false).await.expect("notes");
    assert_eq!(stored.len(), 2);
    assert!(stored.iter().any(|n| n.label == "description" && n.body == "what to do"));
    assert!(stored.iter().any(|n| n.label == "comments" && n.body == "a remark"));

    // Describing again amends both sides rather than accumulating.
    texts::set_role_text(&db, &task, "describes", "what to do, revised")
        .await
        .expect("redescribe");
    let descriptions: Vec<_> = notes::for_entity(&db, &task, false)
        .await
        .expect("notes")
        .into_iter()
        .filter(|n| n.label == "description")
        .collect();
    assert_eq!(descriptions.len(), 1, "one description");
    assert_eq!(descriptions[0].body, "what to do, revised");
}

/// Every legacy prose role maps to a dictionary label. A role with no
/// label would write prose nothing can interpret, so it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_prose_role_maps_to_a_label() {
    for role in texts::TEXT_ROLES {
        let label = texts::label_for_role(role).expect("every prose role has a label");
        assert!(!label.is_empty());
    }
    texts::label_for_role("produced").expect_err("a role with no label is refused");
}

/// Provisioning applies the schema and tells the operator to restart to
/// activate startup seeding — so "schema exists, dictionary does not" is
/// a real state, and module CLI verbs reach the database inside it. A
/// prose write there must work, not lecture the operator about defining
/// `description`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_never_seeded_dictionary_is_seeded_before_the_first_prose_write() {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema");
    registry::seed_types(&db).await.expect("types");
    // Deliberately NOT dictionary::seed — this is the provisioned-but-
    // never-started instance.
    assert_eq!(dictionary::revision(&db).await.expect("rev"), 0);

    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    notes::write(&db, &product, "description", "it works", &Author::operator())
        .await
        .expect("a provisioned instance can be described");

    assert!(dictionary::revision(&db).await.expect("rev") > 0, "seeded on the way past");
    assert_eq!(
        notes::for_entity(&db, &product, false).await.expect("read")[0].body,
        "it works"
    );
}

/// The other half of that judgment: an INITIALIZED dictionary is never
/// touched by a prose write, so the revision readers cache against
/// cannot move because somebody left a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seeded_dictionary_is_never_written_by_prose() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    let before = dictionary::revision(&db).await.expect("rev");

    notes::write(&db, &product, "description", "fine", &Author::operator())
        .await
        .expect("write");
    notes::write(&db, &product, "vibes", "not fine", &Author::operator())
        .await
        .expect_err("an absent label in an initialized dictionary is still refused");

    assert_eq!(
        dictionary::revision(&db).await.expect("rev"),
        before,
        "neither the accepted write nor the refused one moved the cache key"
    );
}
