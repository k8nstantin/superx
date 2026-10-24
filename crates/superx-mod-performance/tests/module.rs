//! The module's contract: what it registers as and how it keeps its
//! registry entry, what its URL answers and to whom, what `status` reads
//! from its own database, and how its executable starts and stops. The
//! registry and the page run on the in-memory engine; the own-database
//! sign-in is QA'd on a real instance: a test handle has no endpoint behind
//! `module_db`, and the page must say so, never fail.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use surrealdb::opt::auth::Database;
use superx_kernel::types::Value;
use superx_kernel::{Kernel, NodeKind, REQUIRED_METAMODEL_TYPES};
use superx_mod_performance::instance::Instance;
use superx_mod_performance::registry::{self, MODULE_UI_URL_PARAM};
use superx_mod_performance::status::{fact_tables, holdings};
use superx_mod_performance::{daemon, descriptor, server, MODULE_NAME, SCHEMA_DDL};
use tower::ServiceExt as _;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// The executable this package builds.
const EXE: &str = env!("CARGO_BIN_EXE_superx-performance");

/// The port the tests' server listens on.
const PORT: u16 = 5153;

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
    let mut res = db
        .query("RETURN (SELECT count() AS n FROM module WHERE uid = $uid GROUP ALL)[0].n")
        .bind(("uid", MODULE_NAME))
        .await
        .expect("ledger reads");
    let n: Option<i64> = res.take(0).expect("a count");
    n.unwrap_or_default()
}

/// A scratch instance home: its params pointing at `endpoint`, and the
/// credentials file beside its datastore.
fn scratch_home(endpoint: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("superx-performance-module-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(home.join("params")).expect("params dir");
    std::fs::create_dir_all(home.join("db")).expect("db dir");
    std::fs::write(home.join("params/superx.json"), format!(r#"{{"endpoint":"{endpoint}"}}"#)).expect("params");
    std::fs::write(home.join("db/superx-credentials"), "pw\n").expect("credentials");
    home
}

/// A GET of `path` on the module's URL, addressed to `host`, or to no one.
async fn get(kernel: &Kernel, path: &str, host: Option<&str>) -> (u16, String) {
    let mut request = axum::http::Request::get(path);
    if let Some(host) = host {
        request = request.header(axum::http::header::HOST, host);
    }
    let r = server::app(kernel.clone(), PORT)
        .oneshot(request.body(axum::body::Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = r.status().as_u16();
    let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.expect("body");
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[test]
fn it_registers_as_a_module_with_its_own_schema() {
    let d = descriptor();
    assert_eq!(d.name, "performance");
    assert_eq!(d.kind, NodeKind::KernelModule);
    assert!(d.depends_on.is_empty(), "a module depends on the kernel alone");
    assert!(SCHEMA_DDL.contains("superx_mod_performance"), "the schema defines the module's own service account");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_registry_says_starting_then_active_with_its_url_then_stopped() {
    let (kernel, db) = booted_kernel().await;
    let entity = registry::starting(&kernel).await.expect("registers");
    assert_eq!(lifecycle(&kernel).await, "starting");
    assert_eq!(
        kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await.expect("reads"),
        Some(entity.clone()),
        "`superx param set performance` finds it by its name"
    );

    registry::active(&kernel, entity.clone(), "http://127.0.0.1:5153", 12).await.expect("active");
    assert_eq!(lifecycle(&kernel).await, "active");
    let url = kernel.get_parameter(entity.clone(), MODULE_UI_URL_PARAM).await.expect("reads");
    assert!(
        matches!(&url, Some(Value::String(u)) if u == "http://127.0.0.1:5153"),
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
    registry::failed(&kernel, "cannot listen on 127.0.0.1:5153: address in use").await.expect("failed");
    assert_eq!(lifecycle(&kernel).await, "failed");
    let list = kernel.list_with_status(NodeKind::KernelModule).await.expect("reads");
    let entry = list.iter().find(|s| s.name == MODULE_NAME).expect("listed");
    assert!(format!("{:?}", entry.lifecycle).contains("address in use"), "why, kept: {:?}", entry.lifecycle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn its_url_answers_with_the_status() {
    let (kernel, _db) = booted_kernel().await;
    for host in ["127.0.0.1:5153", "localhost:5153"] {
        for path in ["/", "/status"] {
            let (status, text) = get(&kernel, path, Some(host)).await;
            assert_eq!(status, 200, "{host}{path}: {text}");
            assert!(text.contains("superx/performance: not available."), "{path}: {text}");
            assert!(text.contains("connected instance"), "the kernel's own reason: {text}");
            for name in superx_mod_performance::params::NAMES {
                assert!(text.contains(name), "{name} is listed:\n{text}");
            }
            assert!(
                text.lines().any(|l| l.trim_start().starts_with("attr_performance_port") && l.trim_end().ends_with(" 5153")),
                "it runs on 5153 until the operator says otherwise:\n{text}"
            );
        }
    }
    assert_eq!(get(&kernel, "/nothing", Some("127.0.0.1:5153")).await.0, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn its_url_answers_no_request_addressed_to_another_name() {
    let (kernel, _db) = booted_kernel().await;
    // A page elsewhere that points a name of its own at 127.0.0.1 (DNS
    // rebinding) sends that name; a request with none names no one.
    for host in [Some("evil.example:5153"), Some("127.0.0.1.evil.example:5153"), Some("127.0.0.1:5154"), None] {
        for path in ["/", "/status", "/nothing"] {
            let (status, text) = get(&kernel, path, host).await;
            assert_eq!(status, 403, "{host:?}{path}: {text}");
            assert_eq!(text, "superx-performance answers only at http://127.0.0.1:5153\n");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holdings_counts_what_the_database_has() {
    let db = own_db().await;
    let fresh = holdings(&db).await.expect("reads");
    assert_eq!(fresh.rows, fact_tables().map(|table| (table, 0)).collect::<Vec<_>>(), "a fresh database holds nothing");
    assert_eq!(fresh.last_pass, None);

    let chain = "\
        CREATE session:s1 CONTENT { kernel_session: rand::uuid::v7(), agent: 'claude_code', agent_id: NONE, \
            entrypoint: NONE, client_version: NONE, first_at: time::now(), valid_from: time::now() };\
        CREATE session_place CONTENT { session: session:s1, cwd: '/w', branch: NONE, at: time::now(), valid_from: time::now() };\
        CREATE prompt CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), kind: 'typed', \
            text: 'x', chars: 1, images: 0, valid_from: time::now() };\
        CREATE prompt CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), kind: 'queued', \
            text: 'y', chars: 1, images: 0, valid_from: time::now() };\
        CREATE reply:r1 CONTENT { session: session:s1, agent_id: 'msg_01', request_id: NONE, kernel_message: rand::uuid::v7(), \
            at: time::now(), model: 'claude-opus-5-5', requested_model: NONE, advisor_model: NONE, effort: NONE, \
            turn_effort: NONE, stop_reason: NONE, subagent: false, valid_from: time::now() };\
        CREATE reply_state CONTENT { reply: reply:r1, input_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0, \
            output_tokens: 1, thinking_tokens: 0, tool_tokens: NONE, web_searches: NONE, web_fetches: NONE, \
            service_tier: NONE, text_chars: 0, usage: {}, valid_from: time::now() };\
        CREATE thinking CONTENT { reply: reply:r1, position: 0, kernel_message: rand::uuid::v7(), at: time::now(), ms: 5, \
            chars: 0, display: NONE, valid_from: time::now() };\
        CREATE tool_call:t1 CONTENT { reply: reply:r1, call_id: 'toolu_01', kernel_message: rand::uuid::v7(), at: time::now(), \
            tool: 'Bash', path: NONE, command: 'cargo test', description: NONE, pattern: NONE, url: NONE, query: NONE, \
            subagent_type: NONE, background: false, input_bytes: 10, valid_from: time::now() };\
        CREATE tool_result CONTENT { tool_call: tool_call:t1, kernel_message: rand::uuid::v7(), at: time::now(), \
            outcome: 'ok', ms: NONE, bytes: 10, valid_from: time::now() };\
        CREATE edit CONTENT { tool_call: tool_call:t1, path: 'a.rs', kind: 'created', added: 1, removed: 0, \
            counted_from: 'content', operator_changed: false, valid_from: time::now() };\
        CREATE git_event CONTENT { tool_call: tool_call:t1, at: time::now(), action: 'commit', dir: NONE, hash: '5574e81', \
            branch: NONE, subject: NONE, pr_number: NONE, url: NONE, valid_from: time::now() };\
        CREATE test_run CONTENT { tool_call: tool_call:t1, at: time::now(), runner: 'cargo', passed: 1, failed: 0, \
            ignored: 0, valid_from: time::now() };\
        CREATE build_run CONTENT { tool_call: tool_call:t1, at: time::now(), builder: 'cargo', ok: true, errors: 0, \
            warnings: 0, valid_from: time::now() };\
        CREATE turn CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), ms: 1, lines: 1, \
            valid_from: time::now() };\
        CREATE compaction CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), trigger: 'auto', \
            tokens_before: 1, tokens_after: NONE, tokens_dropped: NONE, ms: NONE, valid_from: time::now() };\
        CREATE away CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), text: 'recap', \
            valid_from: time::now() };";
    db.query(chain).await.expect("query").check().expect("rows taken");
    let pass = |finished: &str, outcome: &str, read: u32| {
        format!(
            "CREATE pass CONTENT {{ started_at: d'{finished}', finished_at: d'{finished}', from_line: NONE, to_line: NONE, \
             lines_read: {read}, lines_unread: 2, sessions_new: 0, places_new: 0, prompts_new: 0, replies_new: 0, \
             reply_states_new: 0, thinking_new: 0, tool_calls_new: 0, tool_results_new: 0, edits_new: 0, git_events_new: 0, \
             test_runs_new: 0, build_runs_new: 0, turns_new: 0, compactions_new: 0, aways_new: 0, rows_written: 17, \
             outcome: '{outcome}', note: NONE, valid_from: d'{finished}' }}"
        )
    };
    for sql in [pass("2026-10-24T09:00:00Z", "partial", 100), pass("2026-10-24T10:00:00Z", "complete", 500)] {
        db.query(sql).await.expect("query").check().expect("pass taken");
    }

    let h = holdings(&db).await.expect("reads");
    assert_eq!(
        h.rows,
        [
            ("session", 1),
            ("session_place", 1),
            ("prompt", 2),
            ("turn", 1),
            ("compaction", 1),
            ("away", 1),
            ("reply", 1),
            ("reply_state", 1),
            ("thinking", 1),
            ("tool_call", 1),
            ("tool_result", 1),
            ("edit", 1),
            ("git_event", 1),
            ("test_run", 1),
            ("build_run", 1),
        ]
    );
    let last = h.last_pass.expect("a pass ran");
    assert_eq!(last.outcome, "complete", "the newest pass, not the first");
    assert_eq!(last.finished_at.to_rfc3339(), "2026-10-24T10:00:00+00:00");
    assert_eq!((last.lines_read, last.lines_unread, last.rows_written), (500, 2, 17));
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
fn initialize_with_no_root_password_changes_nothing_and_says_so() {
    let home = scratch_home("ws://127.0.0.1:1");
    let out = Command::new(EXE).arg("--home").arg(&home).arg("--initialize").stdin(Stdio::null()).output().expect("runs");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "superx-performance: no root password given\n");
    std::fs::remove_dir_all(&home).expect("clean up");
}

#[test]
fn start_without_a_substrate_says_why_and_leaves_nothing_running() {
    let home = scratch_home("ws://127.0.0.1:1");
    let start = Command::new(EXE).arg("--home").arg(&home).arg("start").output().expect("runs");
    assert!(!start.status.success(), "{start:?}");
    let said = String::from_utf8_lossy(&start.stderr);
    assert!(said.contains("exited before it was running"), "{said}");
    let log = std::fs::read_to_string(home.join("modules/performance/logs/superx-performance.out")).expect("its log");
    assert!(log.contains("cannot reach the substrate at ws://127.0.0.1:1"), "why, in its log: {log}");
    assert!(!home.join("modules/performance/superx-performance.pid").exists(), "no pid file: it is not running");

    let status = Command::new(EXE).arg("--home").arg(&home).arg("status").output().expect("runs");
    assert!(!status.status.success(), "the substrate did not answer");
    assert!(String::from_utf8_lossy(&status.stdout).contains("running     no"), "said first: {status:?}");
    let stop = Command::new(EXE).arg("--home").arg(&home).arg("stop").output().expect("runs");
    assert!(stop.status.success());
    assert_eq!(String::from_utf8_lossy(&stop.stdout), "superx-performance is not running\n");
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
    connected.expect("superx-performance reached the substrate within 10 s")
}

#[test]
fn stop_ends_superx_performance_even_while_it_starts_and_never_signals_another_process() {
    let (substrate, endpoint) = silent_substrate();
    let home = scratch_home(&endpoint);
    let instance = Instance::read(&home).expect("instance");
    std::fs::create_dir_all(instance.module_dir()).expect("module dir");

    // A pid file left behind, naming a pid another program has now.
    let mut other = Command::new("sleep").arg("30").spawn().expect("sleep");
    std::fs::write(instance.pid_path(), other.id().to_string()).expect("pid file");
    assert_eq!(daemon::live_pid(&instance), None, "not superx-performance");
    assert_eq!(daemon::stop(&instance).expect("stop answers"), "superx-performance is not running\n");
    assert!(other.try_wait().expect("waits").is_none(), "the other program was never signalled");
    other.kill().expect("kill sleep");
    other.wait().expect("reap sleep");

    // superx-performance, still starting: the substrate took its connection
    // and never answered. Not reaped until `stop` returns, so it is a
    // zombie in between, which must not read as running.
    let mut ours = Command::new(EXE)
        .arg("--home")
        .arg(&home)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("superx-performance");
    let _held = connection_from(&substrate);
    std::fs::write(instance.pid_path(), ours.id().to_string()).expect("pid file");
    assert_eq!(daemon::live_pid(&instance), Some(ours.id()));
    let asked = Instant::now();
    let said = daemon::stop(&instance).expect("stops");
    assert_eq!(said, format!("superx-performance stopped (pid {})\n", ours.id()));
    assert!(asked.elapsed() < Duration::from_secs(5), "stopped while starting, not after a wait: {:?}", asked.elapsed());
    assert!(ours.wait().expect("reaped").success(), "a stop while starting is an orderly end");
    std::fs::remove_dir_all(&home).expect("clean up");
}
