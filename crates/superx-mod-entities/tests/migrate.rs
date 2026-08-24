//! Moving prose out of the graph (issue #276).
//!
//! The operator's instruction was "migrate, we are making changes and
//! want to keep the evolution for our history sake" — so these pin the
//! two things that could quietly betray it: every version moving rather
//! than just the current wording, and nothing being deleted.

use superx_mod_entities::dictionary::{self, Definition};
use superx_mod_entities::nodes::{create_entity, list_entities, state_history};
use superx_mod_entities::notes::Author;
use superx_mod_entities::{edges, migrate, notes, registry, texts, SCHEMA_DDL};

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema applies clean");
    registry::seed_types(&db).await.expect("types");
    dictionary::seed(&db).await.expect("dictionary");
    dictionary::seed_type_labels(&db).await.expect("slots");
    db
}

/// Build a text carrier the way the world before #268 did: a `text`
/// entity with its own version chain, joined by a role edge. The note
/// store is deliberately bypassed — this is legacy data.
async fn legacy_carrier(
    db: &superx_kernel::Db,
    owner: &superx_kernel::types::RecordId,
    role: &str,
    versions: &[&str],
) -> superx_kernel::types::RecordId {
    // `text` is no longer a shipped type — B6 retired it — so the
    // legacy shape registers the legacy type first. That IS the honest
    // statement: a fresh instance would never make one of these.
    // Create-once, and a test may build several carriers: an
    // already-registered type is the normal case, not a failure.
    let _ = superx_mod_entities::registry::add_type(db, "text", "entity", None).await;
    let first = versions.first().copied().unwrap_or_default();
    let node = create_entity(db, "text", first, Some(first.to_string()), None)
        .await
        .expect("text node");
    for later in versions.iter().skip(1) {
        superx_mod_entities::nodes::update_entity(
            db,
            &node,
            Some((*later).to_string()),
            Some((*later).to_string()),
            None,
        )
        .await
        .expect("version");
    }
    edges::link(db, owner, &node, role).await.expect("edge");
    node
}

/// The instruction was to keep the evolution. One description on the
/// live instance has eleven versions; moving only the current wording
/// would keep the text and lose the history of how it got there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_version_moves_not_just_the_current_wording() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    legacy_carrier(&db, &product, "describes", &["first draft", "second", "third"]).await;

    let report = migrate::prose(&db, false).await.expect("migrate");
    assert_eq!(report.carriers, 1);
    assert_eq!(report.versions, 3, "all three, not one");

    let live = notes::for_entity(&db, &product, false).await.expect("read");
    assert_eq!(live.len(), 1, "one description, not three");
    assert_eq!(live[0].body, "third", "the current wording is current");

    let chain = notes::history(&db, &live[0].uid).await.expect("history");
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].body, "first draft", "oldest first");
    assert_eq!(chain[2].body, "third");
}

/// The timestamps are the evolution. Stamping the migration's own clock
/// would collapse every version into the instant it ran and lose the
/// order things actually happened in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn versions_keep_the_time_they_were_written() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    let carrier =
        legacy_carrier(&db, &product, "describes", &["v1", "v2", "v3"]).await;

    let source = state_history(&db, &carrier).await.expect("history");
    migrate::prose(&db, false).await.expect("migrate");

    let uid = notes::for_entity(&db, &product, false).await.expect("read")[0].uid.clone();
    let moved = notes::history(&db, &uid).await.expect("history");

    assert_eq!(moved.len(), source.len());
    for (note, original) in moved.iter().zip(source.iter()) {
        let stamp = note.valid_from.expect("dated").to_rfc3339();
        assert_eq!(
            stamp.get(..19),
            original.valid_from.get(..19),
            "a migrated version keeps the second it was written, not the second it moved"
        );
    }
    assert!(
        moved[0].valid_from < moved[2].valid_from,
        "and they are still in order"
    );
}

/// Running it twice must move nothing the second time — an operator
/// re-running a migration after a scare should not double every comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_it_twice_moves_nothing_the_second_time() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    legacy_carrier(&db, &product, "describes", &["a", "b"]).await;
    legacy_carrier(&db, &product, "comments", &["a remark"]).await;

    let first = migrate::prose(&db, false).await.expect("first");
    assert_eq!(first.versions, 3);

    let second = migrate::prose(&db, false).await.expect("second");
    assert_eq!(second.versions, 0, "nothing moved twice");
    // B4 retracted the role edges on the first run, so the second run
    // finds carriers nothing active points at. They are REPORTED as
    // unclaimed rather than silently skipped — and reported is how the
    // operator can tell "already done" from "never seen".
    assert_eq!(second.orphans.len(), 2, "their role edges are retracted, so nothing claims them");
    assert_eq!(second.edges_retracted, 0, "an edge already retracted is not retracted again");
    assert_eq!(second.anchors_archived, 0, "an anchor already archived is left alone");

    assert_eq!(
        notes::for_entity(&db, &product, false).await.expect("read").len(),
        2,
        "one description, one comment — not four"
    );
}

/// Nothing is deleted — §13: the anchors are "archived rather than
/// deleted, so it reads correctly in both directions", and the
/// retraction is "reversible by un-retracting".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_deleted() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    let carrier = legacy_carrier(&db, &product, "describes", &["still here"]).await;

    migrate::prose(&db, false).await.expect("migrate");

    assert_eq!(
        list_entities(&db, Some("text")).await.expect("list").len(),
        1,
        "the carrier row still exists"
    );

    // Its history GREW by the archiving version rather than losing the
    // original: archiving is an append like everything else.
    let history = state_history(&db, &carrier).await.expect("history");
    assert_eq!(history.len(), 2, "original wording + the archiving version");
    assert_eq!(history[0].content.as_deref(), Some("still here"), "the words are untouched");
    assert!(!history[0].archived, "the original version does not retroactively become archived");
    assert!(history[1].archived, "the new version is what hides it");

    // The role edge is retracted, not removed: the row is still there
    // with active = false, which is what makes un-retracting possible.
    let inbound = superx_mod_entities::edges::expand(
        &db,
        std::slice::from_ref(&carrier),
        true,
    )
    .await
    .expect("expand");
    assert!(!inbound.is_empty(), "the edge history survives");
    assert!(inbound.iter().all(|e| !e.active), "and every version of it is retracted");
}

/// A description written since #268 already has a note. Its older
/// wordings belong on THAT chain, not beside it as a second live
/// description with no rule for which one counts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_joins_the_chain_the_dual_write_already_started() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");

    // Legacy versions, written before the note store existed.
    legacy_carrier(&db, &product, "describes", &["ancient", "old"]).await;
    // Then the dual-write era: describing it makes both a note and a
    // new version on the carrier.
    texts::set_role_text(&db, &product, "describes", "current")
        .await
        .expect("describe");

    migrate::prose(&db, false).await.expect("migrate");

    let live = notes::for_entity(&db, &product, false).await.expect("read");
    assert_eq!(live.len(), 1, "still ONE description");
    assert_eq!(live[0].body, "current", "and the newest wording still wins");
    assert!(
        notes::history(&db, &live[0].uid).await.expect("history").len() >= 3,
        "with the older wordings behind it on the same chain"
    );
}

/// A dry run is what an operator reads before letting anything touch
/// their data, so it must walk exactly the same path and write nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dry_run_reports_the_same_work_and_writes_nothing() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    legacy_carrier(&db, &product, "describes", &["a", "b", "c"]).await;

    let dry = migrate::prose(&db, true).await.expect("dry");
    assert_eq!(dry.carriers, 1);
    assert_eq!(dry.versions, 3);
    assert!(
        notes::for_entity(&db, &product, true).await.expect("read").is_empty(),
        "a dry run writes nothing"
    );

    let wet = migrate::prose(&db, false).await.expect("wet");
    assert_eq!(wet.versions, dry.versions, "and it told the truth about what it would do");
}

/// Text nodes reached by a non-prose edge — chiefly the runner's
/// `produced` — are out of scope, and text nodes nothing points at are
/// unclaimed. Both are reported rather than guessed about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_it_will_not_guess_about_is_reported() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build", None, None).await.expect("create");

    let _ = superx_mod_entities::registry::add_type(&db, "text", "entity", None).await;

    // The runner's output: a text node on a `produced` edge.
    let produced = create_entity(&db, "text", "result", Some("output".into()), None)
        .await
        .expect("text");
    edges::link(&db, &task, &produced, "produced").await.expect("edge");

    // And one nothing points at.
    create_entity(&db, "text", "orphan", Some("nobody".into()), None)
        .await
        .expect("text");

    let report = migrate::prose(&db, false).await.expect("migrate");
    assert_eq!(report.carriers, 0, "neither is prose");
    assert_eq!(report.other_roles.len(), 1, "the runner's output is named");
    assert_eq!(report.orphans.len(), 1, "and so is the unclaimed one");
    assert!(
        notes::for_entity(&db, &task, true).await.expect("read").is_empty(),
        "nothing was invented for them"
    );
}

/// Authorship is the subject of authorization, so it must never be
/// fabricated. Text nodes predate the author columns: claiming the
/// operator wrote them would be a lie in the one column that exists to
/// be trusted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_migrated_version_does_not_claim_an_author_it_never_had() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    legacy_carrier(&db, &product, "describes", &["written by nobody recorded"]).await;

    migrate::prose(&db, false).await.expect("migrate");

    let note = &notes::for_entity(&db, &product, false).await.expect("read")[0];
    assert_eq!(note.author_kind.as_deref(), Some("system"));
    assert_eq!(note.author_uid, None, "no person is invented");
    assert_eq!(note.via_uid, None, "and no role either");
}

/// Every prose role maps to a dictionary label, so a carrier on a role
/// with no label would be prose nothing can interpret.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instructions_move_under_their_own_label() {
    let db = fresh_db().await;
    let task = create_entity(&db, "task", "Build", None, None).await.expect("create");
    legacy_carrier(&db, &task, "instructs", &["do the thing"]).await;
    legacy_carrier(&db, &task, "describes", &["a unit of work"]).await;

    migrate::prose(&db, false).await.expect("migrate");

    let live = notes::for_entity(&db, &task, false).await.expect("read");
    assert!(live.iter().any(|n| n.label == "instructions" && n.body == "do the thing"));
    assert!(live.iter().any(|n| n.label == "description"));

    // And the labels they moved under are the ones the type declares.
    let slots = dictionary::slots_for(&db, "task", false).await.expect("slots");
    for note in &live {
        assert!(
            slots.iter().any(|s| s.label == note.label),
            "'{}' moved under a label the task does not carry",
            note.label
        );
    }
    let _ = Definition::default();
}

/// Every comment is its own note. Joining carriers by label — which is
/// right for a singular slot — would collapse every comment an entity
/// has ever had into one chain, destroying the distinction between
/// separate remarks made months apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comments_stay_separate_and_do_not_collapse_into_one_chain() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");

    // Three legacy remarks, and one written in the dual-write era so a
    // note already exists to wrongly join onto.
    legacy_carrier(&db, &product, "comments", &["first remark"]).await;
    legacy_carrier(&db, &product, "comments", &["second remark"]).await;
    legacy_carrier(&db, &product, "comments", &["third remark"]).await;
    texts::add_comment(&db, &product, "a recent remark", &Author::operator()).await.expect("comment");

    migrate::prose(&db, false).await.expect("migrate");

    let live = notes::for_entity(&db, &product, false).await.expect("read");
    let comments: Vec<_> = live.iter().filter(|n| n.label == "comments").collect();
    assert_eq!(comments.len(), 4, "four remarks, four notes — not one chain of four");

    let bodies: Vec<&str> = comments.iter().map(|n| n.body.as_str()).collect();
    for expected in ["first remark", "second remark", "third remark", "a recent remark"] {
        assert!(bodies.contains(&expected), "{expected} survived as its own note");
    }

    // And each is a single-version chain, not a version of another.
    for c in &comments {
        assert_eq!(
            notes::history(&db, &c.uid).await.expect("history").len(),
            1,
            "'{}' is its own note, not a version of someone else's",
            c.body
        );
    }
}
