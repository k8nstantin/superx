//! Declared, typed values (issue #274).
//!
//! A field is a slot label whose kind is a value kind. These pin the two
//! rules that hold without exception — composition is an edge, a secret
//! is a reference — and the evolution rules that stop a type's first
//! refinement from breaking every entity written before it.

use superx_kernel::types::{Object, Value};
use superx_mod_entities::fields;
use superx_mod_entities::nodes::{create_entity, current_state, update_entity};
use superx_mod_entities::dictionary::{self, Definition};
use superx_mod_entities::{registry, SCHEMA_DDL};

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

/// The registry has always DESCRIBED a repo as carrying url, branch and
/// host "in attributes" while declaring nothing — so the keys were
/// folklore. Now they are declared, typed and readable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repo_declares_the_fields_its_description_always_claimed() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    fields::set(&db, &repo, "url", "https://github.com/k8nstantin/superx")
        .await
        .expect("url");
    fields::set(&db, &repo, "branch", "main").await.expect("branch");

    let all = fields::of(&db, &repo).await.expect("read");
    let by = |k: &str| all.iter().find(|f| f.key == k).expect("declared").clone();
    assert_eq!(by("url").value.as_deref(), Some("https://github.com/k8nstantin/superx"));
    assert_eq!(by("url").value_kind, "url");
    assert_eq!(by("branch").value.as_deref(), Some("main"));
    assert!(by("url").required, "a repo with no url is not a repo");
    assert_eq!(by("host").value, None, "declared, unset — not an error");
}

/// `"yes"` is a word, not a boolean, and a typo is not a new field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_is_checked_against_the_kind_its_label_declares() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    fields::set(&db, &repo, "url", "github.com/x")
        .await
        .expect_err("not an http(s) url");
    fields::set(&db, &repo, "brnach", "main")
        .await
        .expect_err("a typo is not a new field");

    dictionary::define(&db, Definition {
        key: "retries",
        kind: "slot",
        display: "Retries",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("integer"),
        ..Default::default()
    })
        .await
        .expect("define");
    let task = create_entity(&db, "task", "t", None, None).await.expect("create");
    // `task` declares slots, so an undeclared key is refused there too.
    fields::set(&db, &task, "retries", "3")
        .await
        .expect_err("declared in the dictionary is not the same as carried by the type");
}

/// A type that declares nothing accepts anything: a role must be able to
/// invent a type and refine it later. Strictness arrives with the
/// declaration, not before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_that_declares_nothing_accepts_anything() {
    let db = fresh_db().await;
    registry::add_type(&db, "experiment", "entity", Some("invented at runtime"))
        .await
        .expect("type");
    dictionary::define(&db, Definition {
        key: "trials",
        kind: "slot",
        display: "Trials",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("integer"),
        ..Default::default()
    })
        .await
        .expect("label");

    let e = create_entity(&db, "experiment", "e1", None, None).await.expect("create");
    fields::set(&db, &e, "trials", "12").await.expect("nothing declared, so nothing refused");

    assert_eq!(
        fields::of(&db, &e).await.expect("read")[0].value.as_deref(),
        Some("12")
    );
}

/// Composition must never hide inside JSON: the graph cannot traverse a
/// field, and an agent walking the graph would never find it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_that_names_an_entity_is_refused_because_it_is_an_edge() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    let other = create_entity(&db, "product", "Ledger", None, None).await.expect("create");
    let uuid = superx_ops::record_uuid(&other);

    // Prefixed, which is the polite way to write the mistake.
    let err = fields::set(&db, &repo, "branch", &format!("entity:{uuid}"))
        .await
        .expect_err("that is a link, not a value");
    assert!(err.to_string().contains("EDGE"), "the error says why: {err}");

    // And bare, which is the way anyone would actually write it. Matching
    // the prefix alone was not the rule it claimed to be.
    fields::set(&db, &repo, "branch", &uuid)
        .await
        .expect_err("a bare id of something that exists is still a link");

    // A uuid that resolves to nothing is just a string — a field must be
    // able to hold another system's id.
    fields::set(&db, &repo, "branch", "01a00000-0000-7000-8000-000000000000")
        .await
        .expect("an id of nothing in this graph is not a link");
}

/// The secret itself lives in the tool that uses it. What the graph
/// holds is the means of finding it — so it never enters the graph and
/// never leaves in an export.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_field_holds_a_reference_and_refuses_a_value() {
    let db = fresh_db().await;
    let cred = create_entity(&db, "credential", "github", None, None).await.expect("create");

    let err = fields::set(&db, &cred, "secret", "ghp_liveTokenPastedByMistake")
        .await
        .expect_err("a pasted token is refused");
    assert!(err.to_string().contains("REFERENCE"), "the error says why: {err}");

    for reference in ["env:GITHUB_TOKEN", "keychain:github-pat", "vault:prod/github"] {
        fields::set(&db, &cred, "secret", reference).await.expect("a pointer is fine");
    }
    fields::set(&db, &cred, "secret", "env:")
        .await
        .expect_err("a source with nothing after it points at nothing");
}

/// Prose has its own store with its own history. Writing it into the
/// value bag would give it neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prose_cannot_be_written_as_a_value() {
    let db = fresh_db().await;
    let product = create_entity(&db, "product", "Ledger", None, None).await.expect("create");

    let err = fields::set(&db, &product, "description", "a ledger")
        .await
        .expect_err("description is prose");
    assert!(err.to_string().contains("prose"), "the error says where it belongs: {err}");
}

/// An enum with no options declared allows nothing — better than
/// allowing everything and calling it an enum.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_enum_allows_exactly_what_its_label_lists() {
    let db = fresh_db().await;
    registry::add_type(&db, "experiment", "entity", None).await.expect("type");
    let e = create_entity(&db, "experiment", "e1", None, None).await.expect("create");

    dictionary::define(&db, Definition {
        key: "phase",
        kind: "slot",
        display: "Phase",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("enum"),
        ..Default::default()
    })
        .await
        .expect("define");
    fields::set(&db, &e, "phase", "pilot")
        .await
        .expect_err("no options declared, so nothing is valid");

    // Options live in the label's own extensible bag.
    let mut attrs = Object::new();
    attrs.insert(
        "options".to_string(),
        Value::Array(
            vec![Value::String("pilot".into()), Value::String("live".into())].into(),
        ),
    );
    define_with_options(&db, "phase", attrs).await;

    fields::set(&db, &e, "phase", "live").await.expect("listed");
    fields::set(&db, &e, "phase", "someday").await.expect_err("not listed");
}

/// Writing one field must not disturb the others — the bag is replaced
/// wholesale by the state verb, so a read-modify-write that dropped a
/// key would lose data silently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_one_field_leaves_the_rest_alone() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    fields::set(&db, &repo, "url", "https://example.com/r").await.expect("url");
    fields::set(&db, &repo, "branch", "main").await.expect("branch");
    fields::set(&db, &repo, "host", "github").await.expect("host");
    fields::set(&db, &repo, "branch", "develop").await.expect("rewrite branch");

    let all = fields::of(&db, &repo).await.expect("read");
    let by = |k: &str| all.iter().find(|f| f.key == k).expect("there").value.clone();
    assert_eq!(by("branch").as_deref(), Some("develop"));
    assert_eq!(by("url").as_deref(), Some("https://example.com/r"), "untouched");
    assert_eq!(by("host").as_deref(), Some("github"), "untouched");
}

/// Reads never fail. An entity holding a key the type no longer declares
/// still reads in full, flagged — because a removed declaration must not
/// look like deleted data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_no_longer_declared_still_reads() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    // Written directly, as an older release would have left it.
    let mut bag = Object::new();
    bag.insert("legacy_mirror".to_string(), Value::String("git://old".into()));
    update_entity(&db, &repo, None, None, Some(Value::Object(bag)))
        .await
        .expect("write");

    let all = fields::of(&db, &repo).await.expect("read never fails");
    let leftover = all.iter().find(|f| f.key == "legacy_mirror").expect("still visible");
    assert!(leftover.undeclared, "flagged, not hidden");
    assert_eq!(leftover.value.as_deref(), Some("git://old"));
    assert!(current_state(&db, &repo).await.expect("state").is_some());
}

/// The kind decides storage and rendering, so it is closed like every
/// other thing the dictionary declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_kind_outside_the_closed_set_is_refused() {
    let db = fresh_db().await;
    dictionary::define(&db, Definition {
        key: "guess",
        kind: "slot",
        display: "Guess",
        semantics: "data",
        value_kind: Some("vibes"),
        ..Default::default()
    })
        .await
        .expect_err("unknown kind");
    dictionary::define(&db, Definition {
        key: "guess",
        kind: "slot",
        display: "Guess",
        semantics: "data",
        value_kind: Some("integer"),
        ..Default::default()
    })
        .await
        .expect("known kind");
}

/// Define a label carrying an attributes bag, which `define` does not
/// take — the bag is carried forward, so this seeds it directly.
async fn define_with_options(db: &superx_kernel::Db, key: &str, attributes: Object) {
    let mut row = Object::new();
    row.insert("key".to_string(), Value::String(key.to_string()));
    row.insert("label_kind".to_string(), Value::String("slot".to_string()));
    row.insert("display".to_string(), Value::String(key.to_string()));
    row.insert("semantics".to_string(), Value::String("data".to_string()));
    row.insert("value_kind".to_string(), Value::String("enum".to_string()));
    row.insert("cardinality".to_string(), Value::String("one".to_string()));
    row.insert("archived".to_string(), Value::Bool(false));
    row.insert("attributes".to_string(), Value::Object(attributes));
    row.insert(
        "valid_from".to_string(),
        Value::Datetime(chrono::Utc::now().into()),
    );
    db.query("CREATE $id CONTENT $row")
        .bind(("id", registry::new_id("label")))
        .bind(("row", Value::Object(row)))
        .await
        .expect("create")
        .check()
        .expect("ok");
}

/// `"NaN"`, `"inf"` and `1e400` all parse as f64. None survives a JSON
/// round trip, every comparison against one is false, and arithmetic on
/// one poisons whatever it touches.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_number_field_refuses_what_a_number_cannot_mean() {
    let db = fresh_db().await;
    registry::add_type(&db, "experiment", "entity", None).await.expect("type");
    let e = create_entity(&db, "experiment", "e1", None, None).await.expect("create");
    dictionary::define(&db, Definition {
        key: "temperature",
        kind: "slot",
        display: "Temperature",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("number"),
        ..Default::default()
    })
    .await
    .expect("define");

    for refused in ["NaN", "inf", "-inf", "infinity", "1e400"] {
        assert!(
            fields::set(&db, &e, "temperature", refused).await.is_err(),
            "{refused} must be refused"
        );
    }
    fields::set(&db, &e, "temperature", "0.7").await.expect("a real number is fine");
    assert_eq!(
        fields::of(&db, &e).await.expect("read")[0].value.as_deref(),
        Some("0.7f")
    );
}
