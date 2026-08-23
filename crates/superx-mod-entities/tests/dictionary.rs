//! The dictionary (issue #266).
//!
//! Types say what a thing IS; labels say what the terminology MEANS.
//! Reading the dictionary is a precondition for interpreting anything
//! else, so these pin the properties a reader depends on — including
//! the one that is a security property rather than a convenience.

use superx_mod_entities::dictionary::{self, LINK, SLOT};
use superx_mod_entities::SCHEMA_DDL;

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

    dictionary::define(&db, "risk_note", SLOT, "Risk note", "context", Some("v1"))
        .await
        .expect("define");
    dictionary::define(&db, "risk_note", SLOT, "Risk note", "guidance", Some("v2"))
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

    let err = dictionary::define(&db, "wishful", SLOT, "Wishful", "vibes", None)
        .await
        .expect_err("unknown semantics is refused");
    assert!(err.to_string().contains("vibes"), "the error names it: {err}");

    dictionary::define(&db, "audits", LINK, "audits", "binding", None)
        .await
        .expect_err("a slot semantic is not a link semantic");

    dictionary::define(&db, "Risk Note", SLOT, "Risk", "context", None)
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
