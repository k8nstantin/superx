//! Does the database end up looking like the schema? Apply the module's
//! schema file, read the result back OUT of the engine, and compare it
//! with the contract below: every table and field, and nothing else (the
//! check the entities module learned it needed, epic #353).
//!
//! The contract is written out here rather than parsed from the file, so
//! this test holds no schema statement of its own (§7): a change to the
//! file that is not made here too fails it.

use std::collections::BTreeSet;

use superx_kernel::types::Value;
use superx_mod_specs::SCHEMA_DDL;

const PW: &str = "test-password";

/// Every table and its fields. An array field also gets an element
/// definition from the engine, `field.*`, which INFO reports.
const CONTRACT: &[(&str, &[&str])] = &[
    ("spec_doc", &["sha256", "kind", "key", "name", "version", "path", "content", "first_read_at", "valid_from"]),
    ("spec_clause", &["doc", "kind", "ident", "position", "text", "valid_from"]),
    (
        "spec_run",
        &[
            "session", "session_key", "kickoff", "task_path", "task_doc", "root_doc", "rules_doc", "key", "version",
            "branch", "label", "model", "reads", "reads.*", "first_write", "started_at", "valid_from",
        ],
    ),
    ("spec_finding", &["run", "clause", "check", "result", "note", "evidence", "evidence.*", "valid_from"]),
    (
        "spec_revision",
        &["session", "session_key", "model", "key", "version", "reviewed", "line", "message", "commit", "at", "call", "valid_from"],
    ),
    ("branch_event", &["session", "session_key", "model", "branch", "event", "command", "ok", "at", "call", "valid_from"]),
    (
        "repo_branch",
        &[
            "repo", "host", "branch", "on_local", "on_remote", "merge_commit", "merged_at", "merged_into", "merged_into.*",
            "pull_request", "revert_commit", "reverted_at", "observed_at", "valid_from",
        ],
    ),
];

fn keys_of(v: &Value, section: &str) -> BTreeSet<String> {
    let Value::Object(top) = v else { return BTreeSet::new() };
    match top.get(section) {
        Some(Value::Object(inner)) => inner.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

async fn connect() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    db
}

async fn provision(db: &superx_kernel::Db) {
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", PW);
    db.query(ddl).await.expect("provision").check().expect("provision is clean");
}

async fn tables(db: &superx_kernel::Db) -> BTreeSet<String> {
    let info: Value = db.query("INFO FOR DB").await.expect("info").take(0).expect("decodes");
    keys_of(&info, "tables")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_leaves_the_database_exactly_as_the_contract_says() {
    let db = connect().await;
    provision(&db).await;

    let want: BTreeSet<String> = CONTRACT.iter().map(|(t, _)| (*t).to_string()).collect();
    assert_eq!(tables(&db).await, want, "the database's tables are not the contract's");

    for (table, fields) in CONTRACT {
        let info: Value = db.query(format!("INFO FOR TABLE {table}")).await.expect("info").take(0).expect("decodes");
        let got = keys_of(&info, "fields");
        let want: BTreeSet<String> = fields.iter().map(|f| (*f).to_string()).collect();
        assert_eq!(
            got, want,
            "table `{table}` is not the contract's.\n  in the contract only: {:?}\n  in the database only: {:?}",
            want.difference(&got).collect::<Vec<_>>(),
            got.difference(&want).collect::<Vec<_>>(),
        );
    }
}

/// Provisioning over a database that already holds something leaves
/// nothing of it behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_that_was_there_survives_provisioning() {
    let db = connect().await;
    // A table the schema never declares, made the way any write makes one.
    db.query("CREATE leftover:1 SET junk = 'x'").await.expect("junk").check().expect("junk written");
    assert!(tables(&db).await.contains("leftover"));

    provision(&db).await;
    let want: BTreeSet<String> = CONTRACT.iter().map(|(t, _)| (*t).to_string()).collect();
    assert_eq!(tables(&db).await, want, "a table from before survived provisioning");
}

/// The engine holds the constraints the code relies on: one row per
/// content, links that point at rows that exist, values from the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_refuses_what_the_schema_forbids() {
    let db = connect().await;
    provision(&db).await;
    let doc = "CREATE spec_doc CONTENT { sha256: 'a', kind: 'task', path: 'p', content: 'c', \
               first_read_at: time::now(), valid_from: time::now() }";
    db.query(doc).await.expect("first").check().expect("the first row is taken");
    let dup = db.query(doc).await.expect("second").check();
    assert!(dup.is_err(), "a second row with the same content hash must be refused");

    let kind = db
        .query("CREATE spec_doc CONTENT { sha256: 'b', kind: 'design', path: 'p', content: 'c', \
                first_read_at: time::now(), valid_from: time::now() }")
        .await
        .expect("query")
        .check();
    assert!(kind.is_err(), "a kind outside root, rules, task must be refused");

    let dangling = db
        .query("CREATE spec_clause CONTENT { doc: spec_doc:nothing, kind: 'rule', position: 0, text: 't', \
                valid_from: time::now() }")
        .await
        .expect("query")
        .check();
    assert!(dangling.is_err(), "a clause of a spec file that does not exist must be refused");

    let event = |kind: &str| {
        format!(
            "CREATE branch_event CONTENT {{ session: entity:x, session_key: 's', branch: 'b', event: '{kind}', \
             command: 'git revert 60d21202', at: time::now(), call: 'toolu_{kind}', valid_from: time::now() }}"
        )
    };
    db.query(event("reverted")).await.expect("query").check().expect("a reverted merge is an event (2026-10-16)");
    let committed = db.query(event("committed")).await.expect("query").check();
    assert!(committed.is_err(), "an event kind outside the list must be refused");
}
