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
    // §6: a label the DICTIONARY defines may be set on any entity ad
    // hoc, and promoted to the type later — "the label means the same
    // thing either way, which is the point". The type says what is
    // EXPECTED, not what is permitted.
    fields::set(&db, &task, "retries", "3").await.expect("ad hoc on this task");
    let held = fields::of(&db, &task).await.expect("read");
    assert_eq!(
        held.iter().find(|f| f.key == "retries").and_then(|f| f.value.clone()),
        Some("3".to_string())
    );

    // And the VALUE is still checked against what the label declares —
    // dropping the type check did not drop the kind check.
    fields::set(&db, &task, "retries", "three").await.expect_err("integer means integer");
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
        Some("0.7"),
        "the engine's float marker is notation, not the operator's value"
    );
}

/// The rules held at ONE door out of three. `fields::set` checked
/// everything; `entities update --attrs` and the UI's attributes box —
/// which are the same code path — wrote whatever JSON they were handed.
///
/// So a raw credential could be pasted into the graph from a browser,
/// which is the exact thing the secret-reference rule exists to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bag_write_is_checked_exactly_as_a_field_write_is() {
    let db = fresh_db().await;
    let cred = create_entity(&db, "credential", "github", None, None).await.expect("create");

    let bag = |pairs: &[(&str, &str)]| {
        let mut o = Object::new();
        for (k, v) in pairs {
            o.insert((*k).to_string(), Value::String((*v).to_string()));
        }
        o
    };

    // The refusals, through the OTHER door.
    fields::validate_bag(&db, &cred, &bag(&[("secret", "ghp_rawTokenPasted")]))
        .await
        .expect_err("a pasted token is refused wherever it comes from");

    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");
    fields::validate_bag(&db, &repo, &bag(&[("url", "github.com/x")]))
        .await
        .expect_err("not an http(s) url");

    let other = create_entity(&db, "product", "P", None, None).await.expect("p");
    let uuid = superx_ops::record_uuid(&other);
    fields::validate_bag(&db, &repo, &bag(&[("branch", &uuid)]))
        .await
        .expect_err("a bare id of something that exists is a link, not a value");

    // And a legal bag comes back typed.
    let checked = fields::validate_bag(&db, &cred, &bag(&[("secret", "keychain:github-pat")]))
        .await
        .expect("a pointer is fine");
    assert!(matches!(checked.get("secret"), Some(Value::String(s)) if s == "keychain:github-pat"));
}

/// A type that declares nothing accepts anything, and an entity written
/// under older declarations still holds keys nobody declares now — so
/// the bag check must carry what it does not recognise rather than
/// refusing it and making an existing entity uneditable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undeclared_key_passes_through_untouched() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    let mut incoming = Object::new();
    incoming.insert("url".to_string(), Value::String("https://example.com/r".into()));
    incoming.insert("legacy_mirror".to_string(), Value::String("git://old".into()));

    let checked = fields::validate_bag(&db, &repo, &incoming).await.expect("carried");
    assert!(matches!(checked.get("legacy_mirror"), Some(Value::String(s)) if s == "git://old"));
    assert!(checked.contains_key("url"));
}

/// The bag REPLACES what was there, so an update that omits a key
/// deletes it. For an optional field that is how you clear one; for a
/// required field it is a value disappearing because a form did not
/// mention it, which nobody asked for and nobody would notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_cannot_silently_drop_a_required_field() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");
    fields::set(&db, &repo, "url", "https://example.com/r").await.expect("url");
    fields::set(&db, &repo, "branch", "main").await.expect("branch");

    let mut without_url = Object::new();
    without_url.insert("branch".to_string(), Value::String("develop".into()));
    let err = fields::validate_bag(&db, &repo, &without_url)
        .await
        .expect_err("url is required on repo and this write drops it");
    assert!(err.to_string().contains("url"), "the error names it: {err}");

    // Clearing an OPTIONAL field by omission is still how you clear one.
    let mut without_branch = Object::new();
    without_branch.insert("url".to_string(), Value::String("https://example.com/r".into()));
    fields::validate_bag(&db, &repo, &without_branch)
        .await
        .expect("an optional field may be cleared by omission");
}

/// The two doors have to give the same answer, which is the whole point.
/// `set` refuses a key the TYPE does not carry once the type declares
/// anything; the bag door did not, so the same key was refused one way
/// and accepted the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bag_door_refuses_what_the_field_door_refuses() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");
    dictionary::define(&db, Definition {
        key: "max_notional",
        kind: "slot",
        display: "Max notional",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("number"),
        ..Default::default()
    })
    .await
    .expect("define");

    // A defined label may be set ad hoc (§6), and the value is still
    // checked: `max_notional` is a number, so prose in it is refused by
    // BOTH doors — which is what this test is really about.
    fields::set(&db, &repo, "max_notional", "500").await.expect("ad hoc, and 500 is a number");
    fields::set(&db, &repo, "max_notional", "loads")
        .await
        .expect_err("the field door refuses a non-number");

    let mut bag = Object::new();
    bag.insert("url".to_string(), Value::String("https://example.com/r".into()));
    bag.insert("max_notional".to_string(), Value::String("loads".into()));
    fields::validate_bag(&db, &repo, &bag)
        .await
        .expect_err("and so does the bag door");
}

/// Grandfathered, not refused outright: an entity written under older
/// declarations may still hold a key its type no longer carries, and
/// refusing that would make it uneditable (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_already_there_may_stay_even_if_the_type_stopped_carrying_it() {
    let db = fresh_db().await;
    let repo = create_entity(&db, "repo", "superx", None, None).await.expect("create");

    // Written while the type carried it.
    let author = superx_mod_entities::notes::Author::operator();
    dictionary::define(&db, Definition {
        key: "mirror",
        kind: "slot",
        display: "Mirror",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("url"),
        ..Default::default()
    })
    .await
    .expect("define");
    dictionary::bind_slot(&db, "repo", "mirror", false, None, &author).await.expect("bind");
    fields::set(&db, &repo, "mirror", "https://mirror.example.com").await.expect("set");

    // The type stops carrying it.
    dictionary::retire_slot(&db, "repo", "mirror", false, &author).await.expect("retire");

    // The entity can still be saved with the value it already holds.
    let mut bag = Object::new();
    bag.insert("url".to_string(), Value::String("https://example.com/r".into()));
    bag.insert("mirror".to_string(), Value::String("https://mirror.example.com".into()));
    fields::validate_bag(&db, &repo, &bag)
        .await
        .expect("what is already there may stay");
}

/// §6, the whole flow: "Seed, then design. Create the entity and it
/// exists — a uuid7 and a name. Then design it: add fields and label
/// them, from labels designed ahead … Fields may be added AD HOC to a
/// single entity and PROMOTED to the type when every entity of that
/// type should carry the slot — the label means the same thing either
/// way, which is the point."
///
/// Before this, `set` refused any key the TYPE did not declare, so the
/// only way to say something about ONE product was to change what EVERY
/// product carries. There was no ad hoc, so there was nothing to
/// promote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_is_added_to_one_entity_and_then_promoted_to_its_type() {
    use superx_mod_entities::{api, dictionary, fields, nodes};
    use superx_mod_entities::dictionary::Definition;

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    dictionary::seed_type_labels(&db).await.expect("slots");

    // Not in the shipped dictionary, so define it — "you pick one from
    // the dictionary, OR YOU ADD IT to the dictionary" (§6).
    dictionary::define(&db, Definition {
        key: "max_notional",
        kind: "slot",
        display: "Max notional",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("number"),
        ..Default::default()
    })
    .await
    .expect("define");

    let a = nodes::create_entity(&db, "product", "Desk A", None, None).await.expect("a");
    let b = nodes::create_entity(&db, "product", "Desk B", None, None).await.expect("b");
    let a_frag = superx_ops::record_uuid(&a);

    // `max_notional` is in the shipped dictionary but is NOT a product
    // slot — exactly the case that used to be impossible.
    assert!(
        !dictionary::slots_for(&db, "product", false)
            .await
            .expect("slots")
            .iter()
            .any(|s| s.label == "max_notional"),
        "the type does not carry it, which is the point of the test"
    );

    fields::set(&db, &a, "max_notional", "50000").await.expect("ad hoc on THIS desk");

    let held = api::entity_fields(&db, &a_frag).await.expect("fields");
    let f = held.iter().find(|f| f.key == "max_notional").expect("it is there");
    assert_eq!(f.value.as_deref(), Some("50000"));
    assert!(f.ad_hoc, "marked as an exception rather than looking like part of the type");
    assert!(!f.undeclared, "the dictionary defines it — that is not the same as the type carrying it");

    // The OTHER desk is untouched: an ad-hoc field is about one thing.
    let other = api::entity_fields(&db, &superx_ops::record_uuid(&b)).await.expect("b");
    assert!(!other.iter().any(|f| f.key == "max_notional"), "Desk B did not change");

    // PROMOTE: now every product carries the slot.
    api::promote_field(&db, &a_frag, "max_notional").await.expect("promote");
    assert!(
        dictionary::slots_for(&db, "product", false)
            .await
            .expect("slots")
            .iter()
            .any(|s| s.label == "max_notional"),
        "it is on the type now"
    );

    // Desk A's value is unchanged and no longer an exception.
    let held = api::entity_fields(&db, &a_frag).await.expect("fields");
    let f = held.iter().find(|f| f.key == "max_notional").expect("still there");
    assert_eq!(f.value.as_deref(), Some("50000"), "promoting did not touch the value");
    assert!(!f.ad_hoc, "it is part of the type now");

    // §7: "making a field required does not retroactively invalidate
    // existing entities". Desk B has no value and is still writable.
    fields::set(&db, &b, "max_notional", "10000").await.expect("Desk B still writes fine");
}

/// "You never invent a label inline. You pick one from the dictionary,
/// or you add it to the dictionary." A key the dictionary does not
/// define is still refused — that check does the typo-catching the
/// type check was doing, and does it by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_the_dictionary_does_not_define_is_still_refused() {
    use superx_mod_entities::{fields, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let err = fields::set(&db, &product, "max_notionl", "50000")
        .await
        .expect_err("a typo is not a label");
    assert!(err.to_string().contains("declares no slot"), "{err}");
}

/// The offer is the dictionary's, minus what is already held, minus
/// prose — a description belongs in the note store, and offering it
/// here would put it in the attributes bag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_offer_is_the_dictionary_minus_what_is_held_and_minus_prose() {
    use superx_mod_entities::dictionary::Definition;
    use superx_mod_entities::{api, dictionary, fields, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");

    dictionary::define(&db, Definition {
        key: "max_notional",
        kind: "slot",
        display: "Max notional",
        semantics: "data",
        cardinality: Some("one"),
        value_kind: Some("number"),
        ..Default::default()
    })
    .await
    .expect("define");

    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    let offered = api::addable_fields(&db, &frag).await.expect("offers");
    assert!(offered.iter().any(|o| o.key == "max_notional"), "a value label is offered");
    assert!(
        !offered.iter().any(|o| o.key == "description"),
        "prose is not: it is a note, not a value in the bag"
    );

    fields::set(&db, &product, "max_notional", "1").await.expect("set");
    let offered = api::addable_fields(&db, &frag).await.expect("offers again");
    assert!(
        !offered.iter().any(|o| o.key == "max_notional"),
        "what it already holds is not offered twice"
    );
}

/// §6's sentence has TWO halves and only the first was built: "You pick
/// one from the dictionary, OR YOU ADD IT TO THE DICTIONARY."
///
/// So naming a field and giving it a datatype IS adding it — the
/// operator names the field, says what kind of value it holds, and puts
/// something in it. Semantics start at `data`, which is what a field
/// for your own reference is; making it actionable is a separate,
/// deliberate change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naming_a_field_with_a_datatype_adds_it_to_the_dictionary() {
    use superx_mod_entities::{api, dictionary, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    assert!(
        dictionary::current(&db, "owner", dictionary::SLOT).await.expect("read").is_none(),
        "nothing defines it yet"
    );

    api::add_field(&db, &frag, &api::FieldReq {
        key: "owner".into(),
        value: "calexander".into(),
        value_kind: Some("string".into()),
        label: None,
    })
    .await
    .expect("name it, type it, fill it");

    let defined = dictionary::current(&db, "owner", dictionary::SLOT)
        .await
        .expect("read")
        .expect("it is in the dictionary now");
    assert_eq!(defined.value_kind.as_deref(), Some("string"));
    assert_eq!(defined.semantics, "data", "for your reference until you say otherwise");

    let held = api::entity_fields(&db, &frag).await.expect("fields");
    let f = held.iter().find(|f| f.key == "owner").expect("on the entity");
    assert_eq!(f.value.as_deref(), Some("calexander"));
    assert!(f.ad_hoc, "on this entity alone until promoted");
}

/// A datatype is only honoured when the field is NEW. Naming one that
/// already exists must not redeclare what the term means — §5.6 puts
/// the tightest gate in the system on that, because it retroactively
/// changes every entity that used it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naming_an_existing_field_cannot_redeclare_its_kind() {
    use superx_mod_entities::{api, dictionary, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    api::add_field(&db, &frag, &api::FieldReq {
        key: "port".into(),
        value: "8080".into(),
        value_kind: Some("integer".into()),
        label: None,
    })
    .await
    .expect("new");

    // Same name, a different datatype, and a value that only fits the
    // NEW one. It must be checked against `integer`, which is what the
    // label still declares.
    api::add_field(&db, &frag, &api::FieldReq {
        key: "port".into(),
        value: "not a port".into(),
        value_kind: Some("string".into()),
        label: None,
    })
    .await
    .expect_err("the label still says integer");

    assert_eq!(
        dictionary::current(&db, "port", dictionary::SLOT)
            .await
            .expect("read")
            .and_then(|d| d.value_kind)
            .as_deref(),
        Some("integer"),
        "and the declaration is untouched"
    );
}

/// Without a datatype it is still pick-an-existing, so a typo is a typo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naming_a_field_with_no_datatype_is_still_refused() {
    use superx_mod_entities::{api, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    let err = api::add_field(&db, &frag, &api::FieldReq {
        key: "onwer".into(),
        value: "x".into(),
        value_kind: None,
        label: None,
    })
    .await
    .expect_err("a typo with no datatype is a typo");
    assert!(err.to_string().contains("declares no slot"), "{err}");
}

/// A datatype outside the closed set is refused by name rather than
/// quietly defining something nothing can read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_datatype_outside_the_closed_set_is_refused() {
    use superx_mod_entities::{api, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    let err = api::add_field(&db, &frag, &api::FieldReq {
        key: "weight".into(),
        value: "3".into(),
        value_kind: Some("float".into()),
        label: None,
    })
    .await
    .expect_err("float is not one of them");
    assert!(err.to_string().contains("not a datatype"), "{err}");
}

/// A FIELD IS THREE THINGS, and the third is optional (operator,
/// 2026-08-25): "1. name 2. datatype 3. optional label … a custom field
/// may or may not have the label — adding a label makes it ACTIONABLE."
///
/// Without one the field is theirs: named, typed, and an agent does
/// nothing with it — semantics `data`. With one it borrows that term's
/// semantics, and semantics are what an agent acts on (§5.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_without_a_label_is_yours_and_with_one_is_actionable() {
    use superx_mod_entities::{api, dictionary, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    // No label: for your own reference.
    api::add_field(&db, &frag, &api::FieldReq {
        key: "ticket".into(),
        value: "OPS-4417".into(),
        value_kind: Some("string".into()),
        label: None,
    })
    .await
    .expect("named and typed is enough");

    let d = dictionary::current(&db, "ticket", dictionary::SLOT).await.expect("r").expect("t");
    assert_eq!(d.semantics, "data", "an agent does nothing with it");

    // WITH a label: it takes that term's meaning.
    api::add_field(&db, &frag, &api::FieldReq {
        key: "house_rules".into(),
        value: "never trade after 4pm".into(),
        value_kind: Some("string".into()),
        label: Some("mandate".into()),
    })
    .await
    .expect("labelled");

    let d = dictionary::current(&db, "house_rules", dictionary::SLOT).await.expect("r").expect("t");
    assert_eq!(d.semantics, "binding", "a mandate BINDS — that is what makes it actionable");
    assert!(
        d.agent_note.is_some(),
        "and the prose the model is given comes with it"
    );

    let held = api::entity_fields(&db, &frag).await.expect("fields");
    let plain = held.iter().find(|f| f.key == "ticket").expect("there");
    assert_eq!(plain.label, None, "no label, and the page says so");
    let bound = held.iter().find(|f| f.key == "house_rules").expect("there");
    assert_eq!(bound.label.as_deref(), Some("mandate"), "and this one names what it borrowed");
    assert_eq!(bound.semantics, "binding");
}

/// A label nobody declared cannot make anything actionable, so it is
/// refused BY NAME — a typo would otherwise produce a field the
/// operator believed an agent would act on and it never would.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_the_dictionary_does_not_define_is_refused() {
    use superx_mod_entities::{api, nodes};

    let db = fresh_db().await;
    registry::seed_types(&db).await.expect("types");
    let product = nodes::create_entity(&db, "product", "Desk", None, None).await.expect("p");
    let frag = superx_ops::record_uuid(&product);

    let err = api::add_field(&db, &frag, &api::FieldReq {
        key: "rules".into(),
        value: "x".into(),
        value_kind: Some("string".into()),
        label: Some("mandat".into()),
    })
    .await
    .expect_err("a typo cannot silently mean nothing");
    assert!(err.to_string().contains("defines no label"), "{err}");
}
