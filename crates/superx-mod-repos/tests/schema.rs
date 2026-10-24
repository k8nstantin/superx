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

/// What a crawl counts beyond the first approval (#501): what it scanned,
/// what changed since the previous crawl, and what it cost.
const CRAWL_COUNTS: &[&str] = &[
    "checkouts_read", "refs_read", "files_read", "reflog_read", "unreachable_read", "blame_lines_read", "bytes_read",
    "commits_new", "commit_files_new", "lines_added_new", "lines_removed_new", "refs_new", "refs_moved", "refs_gone",
    "refs_rewritten", "head_moves_new", "placements_changed", "landed_new", "survival_changed", "survival_delta",
    "git_timeouts", "git_ms", "rows_written",
];

/// Every table and its fields. An array field's elements are a field of
/// their own the engine defines (`crawls.*`, `args.*`).
const CONTRACT: &[(&str, &[&str])] = &[
    ("repository", &["common_dir", "name", "found_at", "valid_from"]),
    ("checkout", &["repository", "toplevel", "found_at", "valid_from"]),
    ("checkout_state", &["checkout", "branch", "first_branch", "present", "valid_from"]),
    ("directory", &["path", "source", "checkout", "exact", "valid_from"]),
    ("head_move", &["checkout", "at", "branch", "valid_from"]),
    (
        "commit",
        &[
            "repository", "hash", "authored_at", "committed_at", "author", "committer", "subject", "body", "parents",
            "added", "removed", "valid_from",
        ],
    ),
    ("commit_file", &["commit", "path", "change", "added", "removed", "binary", "valid_from"]),
    ("placement", &["commit", "crawl", "place", "first_parent", "valid_from"]),
    ("landed_via", &["commit", "path", "via", "crawl", "valid_from"]),
    ("survival", &["commit", "crawl", "alive", "valid_from"]),
    ("survival_file", &["commit", "crawl", "path", "alive", "valid_from"]),
    // The first approval's fields; `CRAWL_COUNTS` are added in `contract`.
    (
        "crawl",
        &[
            "repository", "started_at", "finished_at", "mainline", "mainline_from", "identity", "tip", "since", "outcome",
            "commits_read", "files_blamed", "files_left", "git_calls", "git_failed", "note", "valid_from",
        ],
    ),
    ("ref_state", &["repository", "crawl", "name", "tip", "rewritten", "valid_from"]),
    (
        "worktree_file",
        &["checkout", "crawl", "path", "from_path", "state", "staged", "added", "removed", "binary", "valid_from"],
    ),
    (
        "crawl_step",
        &[
            "crawl", "step", "started_at", "ms", "git_calls", "git_failed", "git_timeouts", "objects_read", "bytes_read",
            "rows_written", "note", "valid_from",
        ],
    ),
    (
        "git_call",
        &[
            "crawl", "step", "dir", "args", "args.*", "started_at", "ms", "exit", "timed_out", "bytes_out", "lines_out",
            "stderr", "valid_from",
        ],
    ),
    (
        "sweep",
        &[
            "started_at", "finished_at", "interval_secs", "sessions_read", "directories_read", "directories_new",
            "directories_moved", "repositories_known", "repositories_new", "repositories_stale", "checkouts_known",
            "checkouts_new", "checkouts_gone", "checkouts_back", "excluded", "crawls", "crawls.*", "crawls_complete",
            "crawls_partial", "crawls_unreadable", "commits_new", "rows_written", "git_calls", "git_failed",
            "git_timeouts", "outcome", "note", "valid_from",
        ],
    ),
];

/// A table's fields as the contract has them.
fn contract(table: &str, fields: &[&str]) -> BTreeSet<String> {
    let extra: &[&str] = if table == "crawl" { CRAWL_COUNTS } else { &[] };
    fields.iter().chain(extra).map(|f| (*f).to_string()).collect()
}

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
        let want = contract(table, fields);
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

/// A whole row of `table`, its fields as given with `set` laid over them:
/// so a row the engine must refuse differs from a valid one in the one
/// field under test, and is refused for that alone.
fn row(table: &str, id: &str, base: &[(&str, &str)], set: &[(&str, &str)]) -> String {
    let mut fields: Vec<(String, String)> = base.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    for (k, v) in set {
        match fields.iter_mut().find(|(name, _)| name == k) {
            Some(field) => field.1 = (*v).to_string(),
            None => fields.push(((*k).to_string(), (*v).to_string())),
        }
    }
    let body = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ");
    format!("CREATE {table}{id} CONTENT {{ {body} }}")
}

fn commit(id: &str, repository: &str, hash: &str, set: &[(&str, &str)]) -> String {
    let (repository, hash) = (repository.to_string(), format!("'{hash}'"));
    let base = [
        ("repository", repository.as_str()),
        ("hash", hash.as_str()),
        ("authored_at", "time::now()"),
        ("committed_at", "time::now()"),
        ("author", "'me@x'"),
        ("committer", "'noreply@github.com'"),
        ("subject", "'s'"),
        ("body", "'Why.\\n\\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'"),
        ("parents", "1"),
        ("added", "3"),
        ("removed", "0"),
        ("valid_from", "time::now()"),
    ];
    row("commit", &format!(":{id}"), &base, set)
}

fn crawl(id: &str, set: &[(&str, &str)]) -> String {
    let mut base = vec![
        ("repository", "repository:r1"),
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("mainline", "'origin/main'"),
        ("mainline_from", "'origin_head'"),
        ("identity", "'me@x'"),
        ("tip", "NONE"),
        ("since", "NONE"),
        ("outcome", "'complete'"),
        ("commits_read", "2"),
        ("files_blamed", "1"),
        ("files_left", "0"),
        ("git_calls", "3"),
        ("git_failed", "0"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    base.extend(CRAWL_COUNTS.iter().map(|c| (*c, "0")));
    row("crawl", &format!(":{id}"), &base, set)
}

fn sweep(set: &[(&str, &str)]) -> String {
    let mut base = vec![
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("crawls", "[crawl:c1]"),
        ("outcome", "'complete'"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    for count in [
        "interval_secs", "sessions_read", "directories_read", "directories_new", "directories_moved",
        "repositories_known", "repositories_new", "repositories_stale", "checkouts_known", "checkouts_new",
        "checkouts_gone", "checkouts_back", "excluded", "crawls_complete", "crawls_partial", "crawls_unreadable",
        "commits_new", "rows_written", "git_calls", "git_failed", "git_timeouts",
    ] {
        base.push((count, "1"));
    }
    row("sweep", "", &base, set)
}

fn commit_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("commit", "commit:m1"),
        ("path", "'a.rs'"),
        ("change", "'added'"),
        ("added", "3"),
        ("removed", "0"),
        ("binary", "false"),
        ("valid_from", "time::now()"),
    ];
    row("commit_file", "", &base, set)
}

fn ref_state(set: &[(&str, &str)]) -> String {
    let tip = format!("'{HASH_A}'");
    let base = [
        ("repository", "repository:r1"),
        ("crawl", "crawl:c1"),
        ("name", "'refs/heads/feat/x'"),
        ("tip", tip.as_str()),
        ("rewritten", "false"),
        ("valid_from", "time::now()"),
    ];
    row("ref_state", "", &base, set)
}

fn worktree_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("checkout", "checkout:k1"),
        ("crawl", "crawl:c1"),
        ("path", "'src/new.rs'"),
        ("from_path", "NONE"),
        ("state", "'untracked'"),
        ("staged", "false"),
        ("added", "120"),
        ("removed", "0"),
        ("binary", "false"),
        ("valid_from", "time::now()"),
    ];
    row("worktree_file", "", &base, set)
}

fn crawl_step(set: &[(&str, &str)]) -> String {
    let base = [
        ("crawl", "crawl:c1"),
        ("step", "'blame'"),
        ("started_at", "time::now()"),
        ("ms", "2100"),
        ("git_calls", "40"),
        ("git_failed", "0"),
        ("git_timeouts", "0"),
        ("objects_read", "88000"),
        ("bytes_read", "7000000"),
        ("rows_written", "6"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("crawl_step", "", &base, set)
}

fn git_call(set: &[(&str, &str)]) -> String {
    let base = [
        ("crawl", "crawl:c1"),
        ("step", "'log'"),
        ("dir", "'/w/a'"),
        ("args", "['log', 'origin/main', '--numstat']"),
        ("started_at", "time::now()"),
        ("ms", "120000"),
        ("exit", "NONE"),
        ("timed_out", "true"),
        ("bytes_out", "0"),
        ("lines_out", "0"),
        ("stderr", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("git_call", "", &base, set)
}

fn survival_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("commit", "commit:m2"),
        ("crawl", "crawl:c1"),
        ("path", "'a.rs'"),
        ("alive", "3"),
        ("valid_from", "time::now()"),
    ];
    row("survival_file", "", &base, set)
}

/// The engine holds the constraints the code relies on: one row per
/// repository, checkout, commit, HEAD move and landing; links that point
/// at rows that exist; values from the closed lists; counts never below
/// zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_refuses_what_the_schema_forbids() {
    let db = connect().await;
    provision(&db).await;

    // A whole, valid chain is taken.
    for sql in [
        "CREATE repository:r1 CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout:k1 CONTENT { repository: repository:r1, toplevel: '/w/a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout_state CONTENT { checkout: checkout:k1, branch: 'main', first_branch: NONE, present: true, valid_from: time::now() }".to_string(),
        "CREATE directory CONTENT { path: '/w/a/src', source: 'session', checkout: checkout:k1, exact: true, valid_from: time::now() }".to_string(),
        "CREATE directory CONTENT { path: '/elsewhere', source: 'include', checkout: NONE, exact: false, valid_from: time::now() }".to_string(),
        "CREATE head_move CONTENT { checkout: checkout:k1, at: d'2026-10-20T10:00:00Z', branch: 'feat', valid_from: time::now() }".to_string(),
        crawl("c1", &[("refs_read", "25"), ("survival_delta", "-42")]),
        commit("m1", "repository:r1", HASH_A, &[]),
        commit("m2", "repository:r1", HASH_B, &[("body", "NONE")]),
        commit_file(&[]),
        commit_file(&[("path", "'gone.rs'"), ("change", "'deleted'"), ("added", "0"), ("removed", "40")]),
        "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'branch', first_parent: false, valid_from: time::now() }".to_string(),
        "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }".to_string(),
        "CREATE survival CONTENT { commit: commit:m2, crawl: crawl:c1, alive: 3, valid_from: time::now() }".to_string(),
        survival_file(&[]),
        ref_state(&[]),
        ref_state(&[("name", "'refs/heads/thrown-away'"), ("tip", "NONE")]),
        worktree_file(&[]),
        worktree_file(&[("path", "'src/b.rs'"), ("from_path", "'src/a.rs'"), ("state", "'renamed'"), ("staged", "true")]),
        crawl_step(&[]),
        git_call(&[]),
        git_call(&[("exit", "0"), ("timed_out", "false"), ("bytes_out", "48211"), ("lines_out", "1290")]),
        sweep(&[]),
    ] {
        assert!(takes(&db, &sql).await, "a valid row must be taken: {sql}");
    }
    let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert!(takes(&db, &commit("m3", "repository:r1", sha256, &[])).await, "a SHA-256 repository's hash is taken");

    let refused = [
        (
            "CREATE repository CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }".to_string(),
            "a second row for one repository",
        ),
        (commit("d1", "repository:r1", HASH_A, &[]), "a second row for one commit"),
        (commit("d2", "repository:r1", &HASH_A[..39], &[]), "a hash of 39 characters"),
        (commit("d3", "repository:nothing", "2123456789abcdef0123456789abcdef01234567", &[]), "a commit of a repository that does not exist"),
        (commit("d4", "repository:r1", "3123456789abcdef0123456789abcdef01234567", &[("committer", "NONE")]), "a commit with no committer"),
        (
            "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'merged', first_parent: false, valid_from: time::now() }".to_string(),
            "a place outside the list",
        ),
        ("CREATE survival CONTENT { commit: commit:m1, crawl: crawl:c1, alive: -1, valid_from: time::now() }".to_string(), "a negative survival"),
        (
            "CREATE head_move CONTENT { checkout: checkout:k1, at: d'2026-10-20T10:00:00Z', branch: 'feat', valid_from: time::now() }".to_string(),
            "a second row for one HEAD move",
        ),
        (
            "CREATE directory CONTENT { path: '/w/b', source: 'guess', checkout: NONE, exact: false, valid_from: time::now() }".to_string(),
            "a directory from a source outside the list",
        ),
        (
            "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }".to_string(),
            "a second row for one landing",
        ),
        (crawl("g1", &[("mainline_from", "'guessed'")]), "a main line chosen some way outside the list"),
        (crawl("g2", &[("refs_read", "-1")]), "a crawl count below zero"),
        (commit_file(&[("path", "'b.rs'"), ("change", "'renamed'")]), "a change outside the list: renames read as a delete and an add"),
        (survival_file(&[("alive", "-1")]), "a negative survival in a file"),
        (ref_state(&[("tip", "'abc123'")]), "a ref tip that is not a whole commit id"),
        (worktree_file(&[("state", "'stashed'")]), "an uncommitted state outside the list"),
        (crawl_step(&[("step", "'guess'")]), "a step outside the list"),
        (git_call(&[("ms", "-5")]), "a git call that took less than no time"),
        (git_call(&[("step", "'write'")]), "a git call in the step that writes rows"),
        (sweep(&[("crawls", "[crawl:c1, crawl:nothing]")]), "a pass listing a crawl that does not exist"),
        (sweep(&[("outcome", "'aborted'")]), "a pass that ended some way outside the list"),
    ];
    for (sql, what) in refused {
        assert!(!takes(&db, &sql).await, "{what} must be refused: {sql}");
    }
}
