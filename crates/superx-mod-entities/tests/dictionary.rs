//! The dictionary (issue #266).
//!
//! Types say what a thing IS; labels say what the terminology MEANS.
//! Reading the dictionary is a precondition for interpreting anything
//! else, so these pin the properties a reader depends on — including
//! the one that is a security property rather than a convenience.

use superx_mod_entities::dictionary::{self, Definition, LINK, SLOT};
use superx_mod_entities::{registry, SCHEMA_DDL};

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl)
        .await
        .expect("ddl")
        .check()
        .expect("schema applies clean");
    db
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeds_idempotently_and_bumps_the_revision() {
    let db = fresh_db().await;
    assert_eq!(
        dictionary::revision(&db).await.expect("rev"),
        0,
        "nothing defined yet"
    );

    let first = dictionary::seed(&db).await.expect("seed");
    assert!(first > 0, "the shipped vocabulary lands");
    let after_seed = dictionary::revision(&db).await.expect("rev");
    assert_eq!(after_seed, 1, "one write, one revision");

    // Provisioning re-applies the schema and re-runs the seed. It must
    // not resurrect a definition the operator has since changed.
    let second = dictionary::seed(&db).await.expect("reseed");
    assert_eq!(second, 0, "already-defined labels are left alone");
    assert_eq!(
        dictionary::revision(&db).await.expect("rev"),
        after_seed,
        "a no-op seed does not bump the cache key readers revalidate against"
    );
}

/// The case that makes labels matter: identical in kind, cardinality
/// and storage, and the label is the entire difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn description_and_spec_differ_only_by_label() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    let description = dictionary::current(&db, "description", SLOT)
        .await
        .expect("read")
        .expect("seeded");
    let spec = dictionary::current(&db, "spec", SLOT)
        .await
        .expect("read")
        .expect("seeded");

    assert_eq!(description.value_kind, spec.value_kind);
    assert_eq!(description.cardinality, spec.cardinality);
    assert_ne!(description.key, spec.key);
    assert_ne!(description.description, spec.description);
}

/// Only a binding field binds. Comments are the open channel — anyone
/// writes them — so if one could alter behaviour the grant system
/// would be bypassable by writing a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mandate_binds_and_a_comment_cannot() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    let mandate = dictionary::current(&db, "mandate", SLOT)
        .await
        .expect("read")
        .expect("seeded");
    let comments = dictionary::current(&db, "comments", SLOT)
        .await
        .expect("read")
        .expect("seeded");
    let playbook = dictionary::current(&db, "playbook", SLOT)
        .await
        .expect("read")
        .expect("seeded");

    assert_eq!(mandate.semantics, "binding");
    assert_eq!(comments.semantics, "dialogue");
    assert_eq!(
        mandate.writable_by.as_deref(),
        Some("operator"),
        "not the role's to edit — a role that could rewrite its mandate could rewrite away its constraints"
    );
    assert_eq!(playbook.writable_by.as_deref(), Some("owner"), "the role's own");
    assert_eq!(
        playbook.semantics, "guidance",
        "mandate and playbook differ by semantics and writability, nothing else"
    );
    assert!(
        comments
            .agent_note
            .as_deref()
            .unwrap_or_default()
            .contains("never grant"),
        "the agent is told in words that a comment carries no authority"
    );
}

/// `compute_waves` reads the literal string `depends_on` today.
/// Semantics is what lets a new ordering label work with no code change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_label_says_how_a_reader_must_treat_the_edge() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    assert_eq!(
        dictionary::current(&db, "depends_on", LINK)
            .await
            .expect("read")
            .expect("seeded")
            .semantics,
        "ordering"
    );
    assert_eq!(
        dictionary::current(&db, "contains", LINK)
            .await
            .expect("read")
            .expect("seeded")
            .semantics,
        "composition"
    );
}

/// Changing what a label means retroactively changes every entity that
/// used it, so a redefinition appends rather than edits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defining_a_label_appends_a_version_rather_than_editing_one() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");
    let before = dictionary::revision(&db).await.expect("rev");

    dictionary::define(&db, Definition {
        key: "risk_note",
        kind: SLOT,
        display: "Risk note",
        semantics: "context",
        description: Some("v1"),
        ..Default::default()
    })
        .await
        .expect("define");
    dictionary::define(&db, Definition {
        key: "risk_note",
        kind: SLOT,
        display: "Risk note",
        semantics: "guidance",
        description: Some("v2"),
        ..Default::default()
    })
        .await
        .expect("redefine");

    let now = dictionary::current(&db, "risk_note", SLOT)
        .await
        .expect("read")
        .expect("defined");
    assert_eq!(now.semantics, "guidance", "latest wins");
    assert_eq!(now.description.as_deref(), Some("v2"));
    assert_eq!(
        dictionary::revision(&db).await.expect("rev"),
        before + 2,
        "each definition is its own revision, so a cached reader revalidates"
    );
}

/// Semantics is a closed vocabulary because code acts on it, and free
/// text cannot be acted on reliably.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_outside_the_closed_vocabulary_is_refused() {
    let db = fresh_db().await;

    let err = dictionary::define(&db, Definition {
        key: "wishful",
        kind: SLOT,
        display: "Wishful",
        semantics: "vibes",
        ..Default::default()
    })
        .await
        .expect_err("unknown semantics is refused");
    assert!(err.to_string().contains("vibes"), "the error names it: {err}");

    dictionary::define(&db, Definition {
        key: "audits",
        kind: LINK,
        display: "audits",
        semantics: "binding",
        ..Default::default()
    })
        .await
        .expect_err("a slot semantic is not a link semantic");

    dictionary::define(&db, Definition {
        key: "Risk Note",
        kind: SLOT,
        display: "Risk",
        semantics: "context",
        ..Default::default()
    })
        .await
        .expect_err("one spelling per term, or the dictionary defeats itself");
}

/// Nothing is ever deleted, so a superseded label must be hideable or
/// the dictionary becomes a graveyard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_listing_hides_archived_labels_by_default() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    let visible = dictionary::list(&db, false).await.expect("list");
    let all = dictionary::list(&db, true).await.expect("list all");
    assert_eq!(visible.len(), all.len(), "nothing archived yet");
    assert!(
        visible.iter().any(|l| l.key == "spec"),
        "the shipped vocabulary is listed"
    );
    assert!(
        visible.iter().any(|l| l.label_kind == LINK),
        "both kinds of label are one vocabulary"
    );
}

/// Redefining a label must not silently strip what the redefinition did
/// not mention. `mandate` is operator-writable precisely so a role
/// cannot rewrite away its own constraints — editing its wording must
/// not hand that power back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redefining_preserves_what_it_does_not_mention() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    dictionary::define(&db, Definition {
        key: "mandate",
        kind: SLOT,
        display: "Mandate",
        semantics: "binding",
        description: Some("reworded, same meaning"),
        ..Default::default()
    })
    .await
    .expect("redefine");

    let now = dictionary::current(&db, "mandate", SLOT)
        .await
        .expect("read")
        .expect("still defined");
    assert_eq!(now.description.as_deref(), Some("reworded, same meaning"));
    assert_eq!(
        now.writable_by.as_deref(),
        Some("operator"),
        "a reworded mandate is still not the role's to edit"
    );
    assert_eq!(now.cardinality.as_deref(), Some("one"), "shape survives");
    assert!(
        now.agent_note.is_some(),
        "the note telling the agent it cannot change this must survive"
    );
}

/// The dictionary only grows, so a superseded term must be hideable or
/// the vocabulary in use gets buried in the vocabulary that used to be.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_hides_a_label_and_restoring_brings_it_back() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    dictionary::archive(&db, "spec", SLOT, true)
        .await
        .expect("archive");

    let visible = dictionary::list(&db, false).await.expect("list");
    assert!(
        !visible.iter().any(|l| l.key == "spec"),
        "an archived label is out of the way"
    );
    let all = dictionary::list(&db, true).await.expect("list all");
    assert!(
        all.iter().any(|l| l.key == "spec" && l.archived),
        "still there, still readable — nothing is ever deleted"
    );
    assert_eq!(
        dictionary::current(&db, "spec", SLOT)
            .await
            .expect("read")
            .expect("defined")
            .description
            .as_deref(),
        Some("the contract the work is built against and judged by"),
        "archiving hides a definition; it does not erase one"
    );

    dictionary::archive(&db, "spec", SLOT, false)
        .await
        .expect("restore");
    assert!(dictionary::list(&db, false)
        .await
        .expect("list")
        .iter()
        .any(|l| l.key == "spec"));

    dictionary::archive(&db, "nonexistent", SLOT, true)
        .await
        .expect_err("archiving a term the dictionary does not define is a mistake, not a no-op");
}

/// The whole point of versioning a label is being able to read what it
/// used to mean — every entity written under the old meaning was
/// written under the old meaning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_reads_every_version_oldest_first() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    dictionary::define(&db, Definition {
        key: "spec",
        kind: SLOT,
        display: "Spec",
        semantics: "binding",
        description: Some("now binding"),
        ..Default::default()
    })
        .await
        .expect("redefine");

    let versions = dictionary::history(&db, "spec", SLOT).await.expect("history");
    assert_eq!(versions.len(), 2, "the seeded one and the redefinition");
    assert_eq!(versions[0].semantics, "context", "oldest first");
    assert_eq!(versions[1].semantics, "binding");
    assert!(
        versions[0].valid_from <= versions[1].valid_from,
        "a version chain you cannot date is not reviewable"
    );
    assert!(
        versions.iter().all(|v| v.valid_from.is_some()),
        "every version is dated"
    );
}

/// A type that declares no slots is inert: there is nowhere to put
/// anything, so nothing can be said about one of its entities and
/// nothing can act on it. Every shipped entity type therefore carries at
/// least something to say what it is and a channel to discuss it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_shipped_type_carries_at_least_a_description_and_a_channel() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    let bound = dictionary::seed_type_labels(&db).await.expect("slots");
    assert!(bound > 0);

    for entity_type in ["product", "task", "rag", "model", "document", "repo", "credential"] {
        let slots = dictionary::slots_for(&db, entity_type, false).await.expect("slots");
        assert!(
            slots.iter().any(|s| s.label == "description"),
            "{entity_type} can say what it is"
        );
        assert!(
            slots.iter().any(|s| s.label == "comments"),
            "{entity_type} can be discussed"
        );
    }

    // The motivating pair, declared where it belongs: a product carries
    // both, a task carries the assignment instead of the contract.
    let product = dictionary::slots_for(&db, "product", false).await.expect("slots");
    assert!(product.iter().any(|s| s.label == "spec"));
    let task = dictionary::slots_for(&db, "task", false).await.expect("slots");
    assert!(task.iter().any(|s| s.label == "instructions"));

    // Display order is declared, not incidental: description leads.
    assert_eq!(product[0].label, "description");
    assert!(product[0].required, "a product with no description says nothing");

    // Idempotent, like every other seed.
    assert_eq!(
        dictionary::seed_type_labels(&db).await.expect("reseed"),
        0,
        "a binding the operator retired is not resurrected"
    );
}

/// Cardinality decides whether a write amends or adds, so it is a closed
/// choice like every other thing the dictionary declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cardinality_is_one_or_many_and_nothing_else() {
    let db = fresh_db().await;
    dictionary::define(&db, Definition {
        key: "risk_note",
        kind: SLOT,
        display: "Risk",
        semantics: "context",
        cardinality: Some("some"),
        ..Default::default()
    })
        .await
        .expect_err("'some' is not a cardinality");
    dictionary::define(&db, Definition {
        key: "risk_note",
        kind: SLOT,
        display: "Risk",
        semantics: "context",
        cardinality: Some("one"),
        ..Default::default()
    })
        .await
        .expect("one is");
    assert_eq!(
        dictionary::current(&db, "risk_note", SLOT)
            .await
            .expect("read")
            .expect("defined")
            .cardinality
            .as_deref(),
        Some("one")
    );
}

/// Adding a slot to the shipped list mid-way must not leave an upgraded
/// instance ordering its slots differently from a fresh one. Observed
/// live before this was fixed: `comments` kept order 1 from an earlier
/// release while a newly shipped `url` was written at 1 as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slot_added_to_the_shipped_list_does_not_scramble_the_order() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    dictionary::seed_type_labels(&db).await.expect("slots");

    // Every shipped binding sits at its shipped position, and no two
    // share one — which is what "ordered" has to mean.
    for entity_type in ["product", "task", "repo", "credential"] {
        let slots = dictionary::slots_for(&db, entity_type, false).await.expect("slots");
        let orders: Vec<i64> = slots.iter().map(|s| s.display_order).collect();
        let mut unique = orders.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            orders.len(),
            unique.len(),
            "{entity_type} has two slots claiming one position: {slots:?}"
        );
    }

    // A repo reads in the order the shipped list declares it.
    let repo = dictionary::slots_for(&db, "repo", false).await.expect("slots");
    let keys: Vec<String> = repo.iter().map(|s| s.label.clone()).collect();
    assert_eq!(keys, vec!["description", "url", "branch", "host", "comments"]);

    // Re-seeding is still a no-op once everything sits where it belongs.
    assert_eq!(
        dictionary::seed_type_labels(&db).await.expect("reseed"),
        0,
        "nothing left to correct"
    );
}

/// The spec's loop, which was not buildable at all: design a type, define
/// a term, say the type carries it, and only then does an entity of that
/// type mean anything when read.
///
/// Before this, `type_label` had one writer — the startup seed, over a
/// hardcoded list — so a type an operator invented could never carry a
/// field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_invented_at_runtime_can_be_given_slots() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    let author = superx_mod_entities::notes::Author::operator();

    registry::add_type(&db, "desk", "entity", Some("a trading desk")).await.expect("type");
    assert!(
        dictionary::slots_for(&db, "desk", false).await.expect("slots").is_empty(),
        "inert until it declares something"
    );

    dictionary::define(&db, Definition {
        key: "max_notional",
        kind: SLOT,
        display: "Max notional",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("number"),
        ..Default::default()
    })
    .await
    .expect("define");

    dictionary::bind_slot(&db, "desk", "max_notional", true, None, &author)
        .await
        .expect("bind");

    let slots = dictionary::slots_for(&db, "desk", false).await.expect("slots");
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].label, "max_notional");
    assert!(slots[0].required);
}

/// types → labels → entities is a dependency, not a convention: a type
/// cannot adopt a term the dictionary has not defined, or the slot means
/// nothing to whoever reads it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_cannot_adopt_a_term_nobody_defined() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    registry::add_type(&db, "desk", "entity", None).await.expect("type");

    let err = dictionary::bind_slot(
        &db,
        "desk",
        "vibes",
        false,
        None,
        &superx_mod_entities::notes::Author::operator(),
    )
    .await
    .expect_err("undefined term");
    assert!(err.to_string().contains("vibes"), "the error names it: {err}");
}

/// §5.2: `description` is `context` on a product and `directive` on a
/// task — the same label treated differently by what carries it,
/// declared where the type adopts it rather than fixed globally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_can_override_how_its_slot_is_treated() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    let author = superx_mod_entities::notes::Author::operator();
    registry::add_type(&db, "desk", "entity", None).await.expect("type");

    dictionary::bind_slot(&db, "desk", "description", false, Some("directive"), &author)
        .await
        .expect("bind");

    let slots = dictionary::slots_for(&db, "desk", false).await.expect("slots");
    assert_eq!(slots[0].semantics_override.as_deref(), Some("directive"));
    // The label itself is untouched — a product still reads it as context.
    assert_eq!(
        dictionary::current(&db, "description", SLOT).await.expect("read").expect("there").semantics,
        "context"
    );

    dictionary::bind_slot(&db, "desk", "description", false, Some("vibes"), &author)
        .await
        .expect_err("an override is closed like every other semantics");
}

/// Adding a slot must never silently reorder the ones already arranged,
/// and retiring one must never erase it: entities written while it stood
/// still hold values in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slots_keep_their_order_and_retire_without_erasing() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    let author = superx_mod_entities::notes::Author::operator();
    registry::add_type(&db, "desk", "entity", None).await.expect("type");

    for label in ["description", "spec", "comments"] {
        dictionary::bind_slot(&db, "desk", label, false, None, &author).await.expect("bind");
    }
    let keys = |v: Vec<dictionary::TypeSlot>| v.into_iter().map(|s| s.label).collect::<Vec<_>>();
    assert_eq!(
        keys(dictionary::slots_for(&db, "desk", false).await.expect("slots")),
        vec!["description", "spec", "comments"],
        "added in the order they were declared"
    );

    // Editing one leaves the order alone.
    dictionary::bind_slot(&db, "desk", "spec", true, None, &author).await.expect("edit");
    assert_eq!(
        keys(dictionary::slots_for(&db, "desk", false).await.expect("slots")),
        vec!["description", "spec", "comments"],
        "an edit is not a reorder"
    );

    dictionary::order_slot(&db, "desk", "comments", 0, &author).await.expect("move");
    assert_eq!(
        keys(dictionary::slots_for(&db, "desk", false).await.expect("slots"))[0],
        "comments",
        "and a move IS a move"
    );

    dictionary::retire_slot(&db, "desk", "spec", false, &author).await.expect("retire");
    assert!(
        !keys(dictionary::slots_for(&db, "desk", false).await.expect("slots"))
            .contains(&"spec".to_string()),
        "gone from the live declaration"
    );
    assert!(
        keys(dictionary::slots_for(&db, "desk", true).await.expect("all"))
            .contains(&"spec".to_string()),
        "still on the record — an entity may hold a value in it"
    );

    dictionary::retire_slot(&db, "desk", "spec", true, &author).await.expect("restore");
    assert!(keys(dictionary::slots_for(&db, "desk", false).await.expect("slots"))
        .contains(&"spec".to_string()));
}

/// A binding an operator wrote is theirs: the seed corrects its own
/// ordering but must not overrule a person who arranged it deliberately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_seed_does_not_overrule_an_operator() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    dictionary::seed_type_labels(&db).await.expect("slots");
    let author = superx_mod_entities::notes::Author::operator();

    dictionary::order_slot(&db, "product", "comments", 0, &author).await.expect("move");
    assert_eq!(
        dictionary::slots_for(&db, "product", false).await.expect("slots")[0].label,
        "comments"
    );

    dictionary::seed_type_labels(&db).await.expect("reseed");
    assert_eq!(
        dictionary::slots_for(&db, "product", false).await.expect("slots")[0].label,
        "comments",
        "the seed corrects its own ordering, never an operator's"
    );
}

/// Editing a RETIRED slot must not quietly bring it back. `bind_slot`
/// asserted `active: true`, so changing whether a slot was required also
/// un-retired it — two decisions, one of which nobody made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_a_retired_slot_does_not_restore_it() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");
    let author = superx_mod_entities::notes::Author::operator();
    registry::add_type(&db, "desk", "entity", None).await.expect("type");

    dictionary::bind_slot(&db, "desk", "description", false, None, &author).await.expect("bind");
    dictionary::retire_slot(&db, "desk", "description", false, &author).await.expect("retire");

    // An edit that says nothing about retirement.
    dictionary::bind_slot(&db, "desk", "description", true, None, &author).await.expect("edit");

    let all = dictionary::slots_for(&db, "desk", true).await.expect("all");
    let slot = all.iter().find(|s| s.label == "description").expect("still there");
    assert!(!slot.active, "it stays retired: {slot:?}");
    assert!(slot.required, "and the edit still took effect");

    // Restoring is its own act, and it works.
    dictionary::retire_slot(&db, "desk", "description", true, &author).await.expect("restore");
    assert!(
        dictionary::slots_for(&db, "desk", false)
            .await
            .expect("live")
            .iter()
            .any(|s| s.label == "description"),
        "back when somebody actually asks for it"
    );
}

/// §5.5: a mislabelled field is a confusing entity; a mislabelled edge
/// is a WRONG GRAPH, and the graph is what agents execute. The endpoint
/// columns existed since #266 and nothing read them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_label_refuses_endpoints_it_does_not_accept() {
    use superx_mod_entities::{edges, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");

    dictionary::define(&db, Definition {
        key: "audits",
        kind: LINK,
        display: "audits",
        semantics: "governance",
        source_types: Some(&["role".to_string()]),
        target_types: Some(&["task".to_string()]),
        ..Default::default()
    })
    .await
    .expect("define");
    registry::add_type(&db, "role", "entity", None).await.expect("role");
    registry::add_type(&db, "audits", "relation", None).await.expect("relation kind");

    let role = nodes::create_entity(&db, "role", "auditor", None, None).await.expect("role");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("task");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");

    edges::link(&db, &role, &task, "audits").await.expect("role audits task");

    let err = edges::link(&db, &product, &task, "audits")
        .await
        .expect_err("a product does not audit");
    assert!(err.to_string().contains("role"), "the error says what it should be: {err}");

    edges::link(&db, &role, &product, "audits")
        .await
        .expect_err("and it does not audit a product");
}

/// A label nobody has finished describing must not block work — the same
/// rule as a type that declares no slots accepting anything (§7).
/// Enforcement arrives with the declaration, never before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_label_with_no_declared_endpoints_stays_permissive() {
    use superx_mod_entities::{edges, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let task = nodes::create_entity(&db, "task", "Trade", None, None).await.expect("t");

    // `contains` declares no endpoints: any hierarchy is one edge, and
    // privileging one is what the model exists to avoid.
    edges::link(&db, &product, &task, "contains").await.expect("anything contains anything");

    // And `depends_on` stays open too — this instance has products
    // depending on products, so shipping the spec's [task] example as
    // policy would have refused links the operator already makes.
    let other = nodes::create_entity(&db, "product", "Other", None, None).await.expect("p2");
    edges::link(&db, &product, &other, "depends_on")
        .await
        .expect("a product may wait on a product");
}

/// A cycle in `depends_on` does not read oddly: the runner's wave pass
/// drops every task in the loop, so the work never runs and nothing says
/// why. Refused before the edge exists rather than discovered when a
/// plan comes back short.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acyclic_label_refuses_a_link_that_would_close_a_loop() {
    use superx_mod_entities::{edges, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("labels");

    let a = nodes::create_entity(&db, "task", "A", None, None).await.expect("a");
    let b = nodes::create_entity(&db, "task", "B", None, None).await.expect("b");
    let c = nodes::create_entity(&db, "task", "C", None, None).await.expect("c");

    edges::link(&db, &a, &b, "depends_on").await.expect("a waits on b");
    edges::link(&db, &b, &c, "depends_on").await.expect("b waits on c");

    // The loop is two hops away, not adjacent — a check that only looked
    // at the pair would miss it.
    let err = edges::link(&db, &c, &a, "depends_on")
        .await
        .expect_err("c waiting on a closes the loop");
    assert!(err.to_string().contains("acyclic"), "the error says why: {err}");

    // A link that does NOT close one is still fine.
    let d = nodes::create_entity(&db, "task", "D", None, None).await.expect("d");
    edges::link(&db, &c, &d, "depends_on").await.expect("c waits on d");
}

/// A redefinition that does not mention endpoints must not silently
/// widen the label back to accepting anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redefining_a_link_label_keeps_endpoints_it_does_not_mention() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("labels");

    dictionary::define(&db, Definition {
        key: "audits",
        kind: LINK,
        display: "audits",
        semantics: "governance",
        source_types: Some(&["role".to_string()]),
        acyclic: Some(true),
        ..Default::default()
    })
    .await
    .expect("define");

    // Reworded, saying nothing about endpoints.
    dictionary::define(&db, Definition {
        key: "audits",
        kind: LINK,
        display: "audits",
        semantics: "governance",
        description: Some("an independent check"),
        ..Default::default()
    })
    .await
    .expect("redefine");

    let now = dictionary::current(&db, "audits", LINK).await.expect("read").expect("there");
    assert_eq!(now.source_types, vec!["role".to_string()], "endpoints survive a rewording");
    assert!(now.acyclic, "and so does acyclic");
    assert_eq!(now.description.as_deref(), Some("an independent check"));
}

/// The seed fills gaps; it does not overrule. A value somebody set —
/// including a deliberate `false` — is a decision, and re-provisioning
/// must not quietly undo it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeding_does_not_overrule_a_rule_somebody_set() {
    let db = fresh_db().await;
    dictionary::seed(&db).await.expect("seed");

    // The operator decides their `contains` may hold a loop.
    dictionary::define(&db, Definition {
        key: "contains",
        kind: LINK,
        display: "contains",
        semantics: "composition",
        acyclic: Some(false),
        ..Default::default()
    })
    .await
    .expect("their decision");

    dictionary::seed(&db).await.expect("re-provision");

    let now = dictionary::current(&db, "contains", LINK).await.expect("r").expect("t");
    assert!(!now.acyclic, "their decision survives re-provisioning");
}
