//! Content on anything, in two shapes (issue #296, spec §3).
//!
//! `note` and `attachment` are one idea: content that belongs to
//! something and is never a node. These pin the parts that make that
//! true — a file does not become an entity, a label decides what a file
//! MEANS, and a type can hold the argument about itself.

use superx_mod_entities::notes::Author;
use superx_mod_entities::target::Target;
use superx_mod_entities::{attachments, dictionary, nodes, notes, registry, SCHEMA_DDL};

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

fn temp_file(name: &str, body: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-attach-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write");
    path
}

fn module_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-mod-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

/// §5.4: a mandate can be typed inline on one role and uploaded as a PDF
/// on another. The label carries the meaning, the source carries the
/// bytes — and a consumer asks for "the mandate" and gets content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_can_be_the_mandate_rather_than_a_file_next_to_one() {
    let db = fresh_db().await;
    let dir = module_dir();
    let role = nodes::create_entity(&db, "product", "Crypto desk", None, None)
        .await
        .expect("entity");
    let target = Target::Entity(role.clone());

    let file = temp_file("mandate.md", "never exceed 50,000 notional in a day");
    let uid = attachments::attach(&db, &dir, &target, "mandate", &file, &Author::operator())
        .await
        .expect("attach");

    let all = attachments::for_target(&db, &target, false).await.expect("read");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].label, "mandate", "the file IS the mandate");
    assert_eq!(all[0].filename, "mandate.md");
    assert_eq!(all[0].mime, "text/markdown");
    assert_eq!(all[0].uid, uid);

    // The bytes are under the module's own directory, at a RELATIVE path:
    // an instance that moves must not carry a dead absolute path.
    assert!(!all[0].path.starts_with('/'), "path is relative: {}", all[0].path);
    let stored = attachments::absolute_path(&dir, &all[0]);
    assert_eq!(
        std::fs::read_to_string(&stored).expect("bytes"),
        "never exceed 50,000 notional in a day"
    );
}

/// B3's exit criterion, stated as a test: a file must not become a node
/// in the graph or a row in the entity list. That is the whole reason
/// attachments are not `document` entities.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attaching_a_file_creates_no_entity() {
    let db = fresh_db().await;
    let dir = module_dir();
    let product = nodes::create_entity(&db, "product", "Ledger", None, None).await.expect("e");
    let before = nodes::list_entities(&db, None).await.expect("list").len();

    let file = temp_file("spec.pdf", "bytes");
    attachments::attach(
        &db,
        &dir,
        &Target::Entity(product.clone()),
        "spec",
        &file,
        &Author::operator(),
    )
    .await
    .expect("attach");

    assert_eq!(
        nodes::list_entities(&db, None).await.expect("list").len(),
        before,
        "no new node — a file is content, not a fact in the graph"
    );
}

/// A file nobody can interpret is a file nobody reads, so the label is
/// checked exactly as a note's is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_cannot_be_attached_under_an_undefined_label() {
    let db = fresh_db().await;
    let dir = module_dir();
    let product = nodes::create_entity(&db, "product", "Ledger", None, None).await.expect("e");
    let file = temp_file("thing.txt", "x");

    attachments::attach(
        &db,
        &dir,
        &Target::Entity(product),
        "vibes",
        &file,
        &Author::operator(),
    )
    .await
    .expect_err("an undefined label means nothing to whoever reads it");
}

/// Retracting says "this no longer stands". The bytes stay and every
/// version stays: the record of having attached it is the point.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retracting_a_file_keeps_the_bytes_and_the_record() {
    let db = fresh_db().await;
    let dir = module_dir();
    let product = nodes::create_entity(&db, "product", "Ledger", None, None).await.expect("e");
    let target = Target::Entity(product);

    let file = temp_file("old-spec.md", "superseded");
    let uid = attachments::attach(&db, &dir, &target, "spec", &file, &Author::operator())
        .await
        .expect("attach");
    attachments::retract(&db, &uid, &Author::operator()).await.expect("retract");

    assert!(
        attachments::for_target(&db, &target, false).await.expect("read").is_empty(),
        "withdrawn from the live view"
    );
    let kept = attachments::for_target(&db, &target, true).await.expect("all");
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].active);
    let stored = attachments::absolute_path(&dir, &kept[0]);
    assert!(stored.exists(), "the bytes are still there");
}

/// §3: `target_kind` is entity, type or label — "because a type is
/// exactly the thing people argue about and needs a thread of its own".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_and_a_label_can_carry_content() {
    let db = fresh_db().await;
    let dir = module_dir();

    for target in [Target::Type("product".into()), Target::Label("mandate".into())] {
        notes::write_to_target(&db, &target, "description", "what this means", &Author::operator())
            .await
            .expect("prose on a dimension");
        let notes = notes::for_target(&db, &target, false).await.expect("read");
        assert_eq!(notes.len(), 1, "{} carries its own prose", target.kind());
        assert_eq!(notes[0].body, "what this means");

        let file = temp_file("rationale.md", "why we chose this");
        attachments::attach(&db, &dir, &target, "spec", &file, &Author::operator())
            .await
            .expect("a file on a dimension");
        assert_eq!(
            attachments::for_target(&db, &target, false).await.expect("read").len(),
            1
        );
    }

    // And a note on a type is not a note on an entity: the two do not mix.
    let product = nodes::create_entity(&db, "product", "Ledger", None, None).await.expect("e");
    assert!(
        notes::for_target(&db, &Target::Entity(product), false).await.expect("read").is_empty(),
        "an entity of that type carries nothing yet"
    );
}

/// The engine cannot check a polymorphic target, so the verbs must —
/// otherwise content attaches to nothing and is unreachable from both
/// directions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_cannot_belong_to_something_that_does_not_exist() {
    let db = fresh_db().await;

    Target::resolve(&db, "type", "no_such_type").await.expect_err("no such type");
    Target::resolve(&db, "label", "no_such_label").await.expect_err("no such label");
    Target::resolve(&db, "planet", "mars").await.expect_err("not a kind of target");

    Target::resolve(&db, "type", "product").await.expect("a real type");
    Target::resolve(&db, "label", "mandate").await.expect("a real label");
}

/// §3 gives a type a thread "of its own", and a thread you cannot answer
/// is not a thread. Both `reply` and `retract` resolved a note's home
/// through the typed `record<entity>` link alone, so a note on a TYPE
/// reported "attached to nothing" about something plainly attached to a
/// type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_and_a_label_hold_a_thread_that_can_be_answered() {
    let db = fresh_db().await;

    for target in [Target::Type("product".into()), Target::Label("mandate".into())] {
        let (root, _) = notes::write_to_target(
            &db,
            &target,
            "description",
            "what this means",
            &Author::operator(),
        )
        .await
        .expect("write");

        let answer = notes::reply(&db, &root, "but should it?", &Author::operator())
            .await
            .expect("a thread can be answered");

        let live = notes::for_target(&db, &target, false).await.expect("read");
        let reply = live.iter().find(|n| n.uid == answer).expect("the answer is there");
        assert_eq!(reply.parent_uid.as_deref(), Some(root.as_str()));
        assert_eq!(
            reply.label, "comments",
            "answering a singular slot is a comment on it, not a second one"
        );
        assert_eq!(
            reply.target_kind.as_deref(),
            Some(target.kind()),
            "and it belongs to the same {} it answers",
            target.kind()
        );

        notes::retract(&db, &answer, &Author::operator())
            .await
            .expect("and it can be withdrawn");
        assert!(
            !notes::for_target(&db, &target, false)
                .await
                .expect("read")
                .iter()
                .any(|n| n.uid == answer),
            "withdrawn from the live view"
        );
    }
}
