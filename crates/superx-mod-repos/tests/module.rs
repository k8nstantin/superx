//! The module's contract: what it registers as and how it keeps its
//! registry entry, what its URL answers, what `status` reads from its own
//! database, and how its executable starts and stops. The registry and the
//! page run on the in-memory engine; the own-database sign-in is QA'd on a
//! real instance: a test handle has no endpoint behind `module_db`, and the
//! page must say so, never fail.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use surrealdb::opt::auth::Database;
use superx_kernel::types::Value;
use superx_kernel::{Kernel, NodeKind, REQUIRED_METAMODEL_TYPES};
use superx_mod_repos::instance::Instance;
use superx_mod_repos::registry::{self, MODULE_UI_URL_PARAM};
use superx_mod_repos::status::{holdings, Holdings};
use superx_mod_repos::{daemon, descriptor, server, MODULE_NAME, SCHEMA_DDL};
use tower::ServiceExt as _;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// The executable this package builds.
const EXE: &str = env!("CARGO_BIN_EXE_superx-repos");

/// A kernel as a booted instance has it: the schema applied, the
/// metamodel seeded, signed in as the service account. The handle beside
/// it reads what the kernel wrote.
async fn booted_kernel() -> (Kernel, superx_kernel::Db) {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = superx_kernel::SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", KERNEL_PASSWORD);
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    db.signin(Database {
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        username: "superx_kernel".to_string(),
        password: KERNEL_PASSWORD.to_string(),
    })
    .await
    .expect("service sign-in");
    let kernel = Kernel::from_db(db.clone());
    for t in REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await.expect("metamodel");
    }
    (kernel, db)
}

async fn own_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db(MODULE_NAME).await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("provision").check().expect("provision is clean");
    db
}

/// The module's lifecycle, as `superx status` and the Status page read it.
async fn lifecycle(kernel: &Kernel) -> String {
    let list = kernel.list_with_status(NodeKind::KernelModule).await.expect("registry reads");
    list.iter().find(|s| s.name == MODULE_NAME).map(|s| s.lifecycle.short_tag().to_string()).unwrap_or_default()
}

/// Rows in the module ledger for this module.
async fn ledger_rows(db: &superx_kernel::Db) -> i64 {
    let mut res = db.query("RETURN (SELECT count() AS n FROM module WHERE uid = $uid GROUP ALL)[0].n").bind(("uid", MODULE_NAME)).await.expect("ledger reads");
    let n: Option<i64> = res.take(0).expect("a count");
    n.unwrap_or_default()
}

/// A scratch instance home: its params pointing at `endpoint`, and the
/// credentials file beside its datastore.
fn scratch_home(endpoint: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("superx-repos-module-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(home.join("params")).expect("params dir");
    std::fs::create_dir_all(home.join("db")).expect("db dir");
    std::fs::write(home.join("params/superx.json"), format!(r#"{{"endpoint":"{endpoint}"}}"#)).expect("params");
    std::fs::write(home.join("db/superx-credentials"), "pw\n").expect("credentials");
    home
}

#[test]
fn it_registers_as_a_module_with_its_own_schema() {
    let d = descriptor();
    assert_eq!(d.name, "repos");
    assert_eq!(d.kind, NodeKind::KernelModule);
    assert!(d.depends_on.is_empty(), "a module depends on the kernel alone");
    assert!(SCHEMA_DDL.contains("superx_mod_repos"), "the schema defines the module's own service account");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_registry_says_starting_then_active_with_its_url_then_stopped() {
    let (kernel, db) = booted_kernel().await;
    let entity = registry::starting(&kernel).await.expect("registers");
    assert_eq!(lifecycle(&kernel).await, "starting");
    assert_eq!(
        kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await.expect("reads"),
        Some(entity.clone()),
        "`superx param set repos` finds it by its name"
    );

    registry::active(&kernel, entity.clone(), "http://127.0.0.1:5154", 12).await.expect("active");
    assert_eq!(lifecycle(&kernel).await, "active");
    let url = kernel.get_parameter(entity.clone(), MODULE_UI_URL_PARAM).await.expect("reads");
    assert!(
        matches!(&url, Some(Value::String(u)) if u == "http://127.0.0.1:5154"),
        "the core UI links to it through its registry entry: {url:?}"
    );
    let record = kernel.latest_module_record(MODULE_NAME).await.expect("reads").expect("a ledger row");
    assert!(!record.provisioned, "no endpoint behind a test handle: its own database is not there");
    assert_eq!(ledger_rows(&db).await, 1, "a start records its registration once");

    registry::stopped(&kernel).await.expect("stopped");
    assert_eq!(lifecycle(&kernel).await, "disabled", "a stopped module is not counted as down");

    let again = registry::starting(&kernel).await.expect("registers again");
    assert_eq!(again, entity, "one registry entry, however often it starts");
    registry::active(&kernel, again, "http://127.0.0.1:6000", 9).await.expect("active again");
    assert_eq!(ledger_rows(&db).await, 2, "one ledger row per start, never two");
    let url = kernel.get_parameter(entity, MODULE_UI_URL_PARAM).await.expect("reads");
    assert!(matches!(&url, Some(Value::String(u)) if u == "http://127.0.0.1:6000"), "the newest URL: {url:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_that_fails_says_why_in_the_registry() {
    let (kernel, _db) = booted_kernel().await;
    registry::starting(&kernel).await.expect("registers");
    registry::failed(&kernel, "cannot listen on 127.0.0.1:5154: address in use").await.expect("failed");
    assert_eq!(lifecycle(&kernel).await, "failed");
    let list = kernel.list_with_status(NodeKind::KernelModule).await.expect("reads");
    let entry = list.iter().find(|s| s.name == MODULE_NAME).expect("listed");
    assert!(format!("{:?}", entry.lifecycle).contains("address in use"), "why, kept: {:?}", entry.lifecycle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn its_url_answers_with_the_status() {
    let (kernel, _db) = booted_kernel().await;
    for path in ["/", "/status"] {
        let r = server::app(kernel.clone())
            .oneshot(axum::http::Request::get(path).body(axum::body::Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(r.status(), 200, "{path}");
        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.expect("body");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("superx/repos: not available."), "{path}: {text}");
        assert!(text.contains("connected instance"), "the kernel's own reason: {text}");
        for name in superx_mod_repos::params::NAMES {
            assert!(text.contains(name), "{name} is listed:\n{text}");
        }
        assert!(
            text.lines().any(|l| l.trim_start().starts_with("attr_repos_blame_files") && l.trim_end().ends_with(" all")),
            "every file is blamed unless the operator says otherwise:\n{text}"
        );
    }
    let r = server::app(kernel)
        .oneshot(axum::http::Request::get("/nothing").body(axum::body::Body::empty()).expect("request"))
        .await
        .expect("response");
    assert_eq!(r.status(), 404);
}

/// What a crawl counts beyond the first approval (#501), all zero.
const NO_COUNTS: &str = "checkouts_read: 0, refs_read: 0, files_read: 0, reflog_read: 0, unreachable_read: 0, \
    blame_lines_read: 0, bytes_read: 0, commits_new: 0, commit_files_new: 0, lines_added_new: 0, lines_removed_new: 0, \
    refs_new: 0, refs_moved: 0, refs_gone: 0, refs_rewritten: 0, head_moves_new: 0, placements_changed: 0, \
    landed_new: 0, survival_changed: 0, survival_delta: 0, git_timeouts: 0, git_ms: 0, rows_written: 0";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holdings_counts_what_the_database_has() {
    let db = own_db().await;
    assert_eq!(holdings(&db).await.expect("reads"), Holdings::default(), "a fresh database holds nothing");

    for sql in [
        "CREATE repository:r1 CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout CONTENT { repository: repository:r1, toplevel: '/w/a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout CONTENT { repository: repository:r1, toplevel: '/w/a-wt', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE commit CONTENT { repository: repository:r1, hash: '0123456789abcdef0123456789abcdef01234567', \
         authored_at: time::now(), committed_at: time::now(), author: 'me@x', committer: 'me@x', subject: 's', \
         body: NONE, parents: 1, added: 1, removed: 0, valid_from: time::now() }"
            .to_string(),
        format!(
            "CREATE crawl CONTENT {{ repository: repository:r1, started_at: d'2026-10-21T09:00:00Z', \
             finished_at: d'2026-10-21T09:00:05Z', mainline: 'origin/main', mainline_from: 'origin_head', identity: NONE, \
             tip: NONE, since: NONE, outcome: 'partial', commits_read: 1, files_blamed: 0, files_left: 0, git_calls: 2, \
             git_failed: 1, note: 'blame timed out', {NO_COUNTS}, valid_from: d'2026-10-21T09:00:05Z' }}"
        ),
        format!(
            "CREATE crawl CONTENT {{ repository: repository:r1, started_at: d'2026-10-21T10:00:00Z', \
             finished_at: d'2026-10-21T10:00:04Z', mainline: 'origin/main', mainline_from: 'origin_head', identity: NONE, \
             tip: NONE, since: NONE, outcome: 'complete', commits_read: 1, files_blamed: 1, files_left: 0, git_calls: 3, \
             git_failed: 0, note: NONE, {NO_COUNTS}, valid_from: d'2026-10-21T10:00:04Z' }}"
        ),
    ] {
        db.query(sql).await.expect("query").check().expect("row taken");
    }
    let h = holdings(&db).await.expect("reads");
    assert_eq!((h.repositories, h.checkouts, h.commits), (1, 2, 1));
    let (at, outcome) = h.last_crawl.expect("a crawl ran");
    assert_eq!(outcome, "complete", "the newest crawl, not the first");
    assert_eq!(at.to_rfc3339(), "2026-10-21T10:00:04+00:00");
}

#[test]
fn the_executable_names_its_commands_and_refuses_an_unknown_one() {
    let help = Command::new(EXE).arg("--help").output().expect("runs");
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    for word in ["--initialize", "--home", "start", "stop", "status", "run"] {
        assert!(text.contains(word), "{word}:\n{text}");
    }
    let bogus = Command::new(EXE).arg("bogus").output().expect("runs");
    assert!(!bogus.status.success());
    assert!(String::from_utf8_lossy(&bogus.stderr).contains("unrecognized subcommand"), "{bogus:?}");
}

#[test]
fn start_without_a_substrate_says_why_and_leaves_nothing_running() {
    let home = scratch_home("ws://127.0.0.1:1");
    let start = Command::new(EXE).arg("--home").arg(&home).arg("start").output().expect("runs");
    assert!(!start.status.success(), "{start:?}");
    let said = String::from_utf8_lossy(&start.stderr);
    assert!(said.contains("exited before it was running"), "{said}");
    let log = std::fs::read_to_string(home.join("modules/repos/logs/superx-repos.out")).expect("its log");
    assert!(log.contains("cannot reach the substrate at ws://127.0.0.1:1"), "why, in its log: {log}");
    assert!(!home.join("modules/repos/superx-repos.pid").exists(), "no pid file: it is not running");

    let status = Command::new(EXE).arg("--home").arg(&home).arg("status").output().expect("runs");
    assert!(!status.status.success(), "the substrate did not answer");
    assert!(String::from_utf8_lossy(&status.stdout).contains("running     no"), "said first: {status:?}");
    let stop = Command::new(EXE).arg("--home").arg(&home).arg("stop").output().expect("runs");
    assert!(stop.status.success());
    assert_eq!(String::from_utf8_lossy(&stop.stdout), "superx-repos is not running\n");
    std::fs::remove_dir_all(&home).expect("clean up");
}

/// A substrate that takes the connection and never answers: `run` waits at
/// its very first step, as it does when the substrate hangs (#452).
fn silent_substrate() -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let endpoint = format!("ws://{}", listener.local_addr().expect("address"));
    (listener, endpoint)
}

/// The connection `run` opens to the substrate: by then it is listening
/// for the stop signals. Held, so the substrate goes on not answering.
fn connection_from(listener: &std::net::TcpListener) -> std::net::TcpStream {
    listener.set_nonblocking(true).expect("non-blocking");
    let connected = (0..200).find_map(|_| match listener.accept() {
        Ok((conn, _)) => Some(conn),
        Err(e) => {
            assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock, "accept: {e}");
            std::thread::sleep(Duration::from_millis(50));
            None
        }
    });
    connected.expect("superx-repos reached the substrate within 10 s")
}

#[test]
fn stop_ends_superx_repos_even_while_it_starts_and_never_signals_another_process() {
    let (substrate, endpoint) = silent_substrate();
    let home = scratch_home(&endpoint);
    let instance = Instance::read(&home).expect("instance");
    std::fs::create_dir_all(instance.module_dir()).expect("module dir");

    // A pid file left behind, naming a pid another program has now.
    let mut other = Command::new("sleep").arg("30").spawn().expect("sleep");
    std::fs::write(instance.pid_path(), other.id().to_string()).expect("pid file");
    assert_eq!(daemon::live_pid(&instance), None, "not superx-repos");
    assert_eq!(daemon::stop(&instance).expect("stop answers"), "superx-repos is not running\n");
    assert!(other.try_wait().expect("waits").is_none(), "the other program was never signalled");
    other.kill().expect("kill sleep");
    other.wait().expect("reap sleep");

    // superx-repos, still starting: the substrate took its connection and
    // never answered. Not reaped until `stop` returns, so it is a zombie
    // in between, which must not read as running.
    let mut ours = Command::new(EXE)
        .arg("--home")
        .arg(&home)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("superx-repos");
    let _held = connection_from(&substrate);
    std::fs::write(instance.pid_path(), ours.id().to_string()).expect("pid file");
    assert_eq!(daemon::live_pid(&instance), Some(ours.id()));
    let asked = Instant::now();
    let said = daemon::stop(&instance).expect("stops");
    assert_eq!(said, format!("superx-repos stopped (pid {})\n", ours.id()));
    assert!(asked.elapsed() < Duration::from_secs(5), "stopped while starting, not after a wait: {:?}", asked.elapsed());
    assert!(ours.wait().expect("reaped").success(), "a stop while starting is an orderly end");
    std::fs::remove_dir_all(&home).expect("clean up");
}
