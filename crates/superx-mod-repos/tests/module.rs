//! The module's contract on the in-memory engine: what it declares, how
//! its CLI answers, and what `status` reads from its own database. The
//! own-database sign-in is QA'd on a real instance: a test handle has no
//! endpoint behind `module_db`, and `status` must say so, never fail.

use superx_kernel::{Kernel, KernelModule};
use superx_mod_repos::status::{holdings, Holdings};
use superx_mod_repos::{ReposModule, MODULE_NAME, SCHEMA_DDL};

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn fresh_kernel() -> Kernel {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = superx_kernel::SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", KERNEL_PASSWORD);
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    Kernel::from_db(db)
}

async fn own_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db(MODULE_NAME).await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("provision").check().expect("provision is clean");
    db
}

#[test]
fn facilities_are_declared() {
    let m = ReposModule;
    assert_eq!(m.descriptor().name, "repos");
    assert!(m.descriptor().depends_on.is_empty(), "a module depends on the kernel alone");
    assert_eq!(m.schema_ddl(), Some(SCHEMA_DDL), "owns its schema");
    assert!(SCHEMA_DDL.contains("superx_mod_repos"), "the schema defines the module's own service account");
    assert!(!m.needs_dir(), "keeps nothing outside its database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_subcommand_answers_with_the_usage() {
    let kernel = fresh_kernel().await;
    let err = ReposModule.cli(&kernel, &["bogus".to_string()]).await.expect_err("unknown subcommand");
    assert!(err.to_string().contains("usage: superx repos status"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_without_an_instance_says_why_and_still_lists_the_parameters() {
    let kernel = fresh_kernel().await;
    for args in [vec![], vec!["status".to_string()]] {
        let out = ReposModule.cli(&kernel, &args).await.expect("status answers");
        assert!(out.contains("superx/repos: not available."), "{out}");
        assert!(out.contains("connected instance"), "the kernel's own reason: {out}");
        assert!(out.contains("attr_repos_blame_files") && out.contains("4000"), "{out}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holdings_counts_what_the_database_has() {
    let db = own_db().await;
    assert_eq!(holdings(&db).await.expect("reads"), Holdings::default(), "a fresh database holds nothing");

    for sql in [
        "CREATE repository:r1 CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }",
        "CREATE checkout CONTENT { repository: repository:r1, toplevel: '/w/a', found_at: time::now(), valid_from: time::now() }",
        "CREATE checkout CONTENT { repository: repository:r1, toplevel: '/w/a-wt', found_at: time::now(), valid_from: time::now() }",
        "CREATE commit CONTENT { repository: repository:r1, hash: '0123456789abcdef0123456789abcdef01234567', \
         authored_at: time::now(), committed_at: time::now(), author: 'me@x', subject: 's', parents: 1, added: 1, \
         removed: 0, valid_from: time::now() }",
        "CREATE crawl CONTENT { repository: repository:r1, started_at: d'2026-10-21T09:00:00Z', \
         finished_at: d'2026-10-21T09:00:05Z', mainline: 'origin/main', mainline_from: 'origin_head', identity: NONE, \
         tip: NONE, since: NONE, outcome: 'partial', commits_read: 1, files_blamed: 0, files_left: 0, git_calls: 2, \
         git_failed: 1, note: 'blame timed out', valid_from: d'2026-10-21T09:00:05Z' }",
        "CREATE crawl CONTENT { repository: repository:r1, started_at: d'2026-10-21T10:00:00Z', \
         finished_at: d'2026-10-21T10:00:04Z', mainline: 'origin/main', mainline_from: 'origin_head', identity: NONE, \
         tip: NONE, since: NONE, outcome: 'complete', commits_read: 1, files_blamed: 1, files_left: 0, git_calls: 3, \
         git_failed: 0, note: NONE, valid_from: d'2026-10-21T10:00:04Z' }",
    ] {
        db.query(sql).await.expect("query").check().expect("row taken");
    }
    let h = holdings(&db).await.expect("reads");
    assert_eq!((h.repositories, h.checkouts, h.commits), (1, 2, 1));
    let (at, outcome) = h.last_crawl.expect("a crawl ran");
    assert_eq!(outcome, "complete", "the newest crawl, not the first");
    assert_eq!(at.to_rfc3339(), "2026-10-21T10:00:04+00:00");
}
