//! Does the database end up looking like the schema? Apply the module's
//! schema file, read the result back OUT of the engine, and compare it
//! with the contract below: every table and field, and nothing else.
//!
//! The contract is written out here rather than parsed from the file, so
//! this test holds no schema statement of its own (§7): a change to the
//! file that is not made here too fails it.

use std::collections::BTreeSet;

use superx_kernel::types::Value;
use superx_mod_repos::SCHEMA_DDL;

const PW: &str = "test-password";

/// Every table and its fields.
const CONTRACT: &[(&str, &[&str])] = &[
    ("repository", &["common_dir", "name", "found_at", "valid_from"]),
    ("checkout", &["repository", "toplevel", "found_at", "valid_from"]),
    ("checkout_state", &["checkout", "branch", "first_branch", "present", "valid_from"]),
    ("directory", &["path", "source", "checkout", "exact", "valid_from"]),
    ("head_move", &["checkout", "at", "branch", "valid_from"]),
    (
        "commit",
        &["repository", "hash", "authored_at", "committed_at", "author", "subject", "parents", "added", "removed", "valid_from"],
    ),
    ("commit_file", &["commit", "path", "added", "removed", "binary", "valid_from"]),
    ("placement", &["commit", "crawl", "place", "first_parent", "valid_from"]),
    ("landed_via", &["commit", "path", "via", "crawl", "valid_from"]),
    ("survival", &["commit", "crawl", "alive", "valid_from"]),
    (
        "crawl",
        &[
            "repository", "started_at", "finished_at", "mainline", "mainline_from", "identity", "tip", "since", "outcome",
            "commits_read", "files_blamed", "files_left", "git_calls", "git_failed", "note", "valid_from",
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
    db.use_ns("superx").use_db("repos").await.expect("nsdb");
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

fn contract_tables() -> BTreeSet<String> {
    CONTRACT.iter().map(|(t, _)| (*t).to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_leaves_the_database_exactly_as_the_contract_says() {
    let db = connect().await;
    provision(&db).await;
    assert_eq!(tables(&db).await, contract_tables(), "the database's tables are not the contract's");

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
    db.query("CREATE leftover:1 SET junk = 'x'").await.expect("junk").check().expect("junk written");
    assert!(tables(&db).await.contains("leftover"));

    provision(&db).await;
    assert_eq!(tables(&db).await, contract_tables(), "a table from before survived provisioning");
}

/// Run one statement; whether the engine took it.
async fn takes(db: &superx_kernel::Db, sql: &str) -> bool {
    db.query(sql).await.expect("query").check().is_ok()
}

const HASH_A: &str = "0123456789abcdef0123456789abcdef01234567";
const HASH_B: &str = "1123456789abcdef0123456789abcdef01234567";

fn commit(id: &str, repository: &str, hash: &str) -> String {
    format!(
        "CREATE commit:{id} CONTENT {{ repository: {repository}, hash: '{hash}', authored_at: time::now(), \
         committed_at: time::now(), author: 'me@x', subject: 's', parents: 1, added: 3, removed: 0, valid_from: time::now() }}"
    )
}

/// The engine holds the constraints the code relies on: one row per
/// repository, checkout, commit, HEAD move and landing; links that point
/// at rows that exist; values from the closed lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_refuses_what_the_schema_forbids() {
    let db = connect().await;
    provision(&db).await;

    // A whole, valid chain is taken.
    for sql in [
        "CREATE repository:r1 CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }",
        "CREATE checkout:k1 CONTENT { repository: repository:r1, toplevel: '/w/a', found_at: time::now(), valid_from: time::now() }",
        "CREATE checkout_state CONTENT { checkout: checkout:k1, branch: 'main', first_branch: NONE, present: true, valid_from: time::now() }",
        "CREATE directory CONTENT { path: '/w/a/src', source: 'session', checkout: checkout:k1, exact: true, valid_from: time::now() }",
        "CREATE directory CONTENT { path: '/elsewhere', source: 'include', checkout: NONE, exact: false, valid_from: time::now() }",
        "CREATE head_move CONTENT { checkout: checkout:k1, at: d'2026-10-20T10:00:00Z', branch: 'feat', valid_from: time::now() }",
        "CREATE crawl:c1 CONTENT { repository: repository:r1, started_at: time::now(), finished_at: time::now(), \
         mainline: 'origin/main', mainline_from: 'origin_head', identity: 'me@x', tip: NONE, since: NONE, \
         outcome: 'complete', commits_read: 2, files_blamed: 1, files_left: 0, git_calls: 3, git_failed: 0, \
         note: NONE, valid_from: time::now() }",
        &commit("m1", "repository:r1", HASH_A),
        &commit("m2", "repository:r1", HASH_B),
        "CREATE commit_file CONTENT { commit: commit:m1, path: 'a.rs', added: 3, removed: 0, binary: false, valid_from: time::now() }",
        "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'branch', first_parent: false, valid_from: time::now() }",
        "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }",
        "CREATE survival CONTENT { commit: commit:m2, crawl: crawl:c1, alive: 3, valid_from: time::now() }",
    ] {
        assert!(takes(&db, sql).await, "a valid row must be taken: {sql}");
    }
    let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert!(takes(&db, &commit("m3", "repository:r1", sha256)).await, "a SHA-256 repository's hash is taken");

    let refused = [
        (
            "CREATE repository CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }",
            "a second row for one repository",
        ),
        (&*commit("d1", "repository:r1", HASH_A), "a second row for one commit"),
        (&*commit("d2", "repository:r1", &HASH_A[..39]), "a hash of 39 characters"),
        (&*commit("d3", "repository:nothing", "2123456789abcdef0123456789abcdef01234567"), "a commit of a repository that does not exist"),
        (
            "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'merged', first_parent: false, valid_from: time::now() }",
            "a place outside the list",
        ),
        ("CREATE survival CONTENT { commit: commit:m1, crawl: crawl:c1, alive: -1, valid_from: time::now() }", "a negative survival"),
        (
            "CREATE head_move CONTENT { checkout: checkout:k1, at: d'2026-10-20T10:00:00Z', branch: 'feat', valid_from: time::now() }",
            "a second row for one HEAD move",
        ),
        (
            "CREATE directory CONTENT { path: '/w/b', source: 'guess', checkout: NONE, exact: false, valid_from: time::now() }",
            "a directory from a source outside the list",
        ),
        (
            "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }",
            "a second row for one landing",
        ),
        (
            "CREATE crawl CONTENT { repository: repository:r1, started_at: time::now(), finished_at: time::now(), \
             mainline: 'main', mainline_from: 'guessed', identity: NONE, tip: NONE, since: NONE, outcome: 'complete', \
             commits_read: 0, files_blamed: 0, files_left: 0, git_calls: 0, git_failed: 0, note: NONE, valid_from: time::now() }",
            "a main line chosen some way outside the list",
        ),
    ];
    for (sql, what) in refused {
        assert!(!takes(&db, sql).await, "{what} must be refused");
    }
}
