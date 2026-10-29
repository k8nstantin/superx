//! The module's contract: what it registers as and how it keeps its
//! registry entry, what its URL answers and to whom, what `status` reads
//! from its own database, and how its executable starts and stops. The
//! registry and the page run on the in-memory engine; the own-database
//! sign-in is QA'd on a real instance: a test handle has no endpoint behind
//! `module_db`, and the page must say so, never fail.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use surrealdb::opt::auth::Database;
use superx_kernel::types::Value;
use superx_kernel::{Kernel, NodeKind, REQUIRED_METAMODEL_TYPES};
use superx_mod_recon::credentials::{self, Credentials};
use superx_mod_recon::folder;
use superx_mod_recon::instance::Instance;
use superx_mod_recon::registry::{self, MODULE_UI_URL_PARAM};
use superx_mod_recon::status::{fact_tables, holdings};
use superx_mod_recon::{daemon, descriptor, server, MODULE_NAME, SCHEMA_DDL};
use tower::ServiceExt as _;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// The executable this package builds.
const EXE: &str = env!("CARGO_BIN_EXE_superx-recon");

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
    let home = std::env::temp_dir().join(format!("superx-recon-module-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(home.join("params")).expect("params dir");
    std::fs::create_dir_all(home.join("db")).expect("db dir");
    std::fs::write(home.join("params/superx.json"), format!(r#"{{"endpoint":"{endpoint}"}}"#)).expect("params");
    std::fs::write(home.join("db/superx-credentials"), "pw\n").expect("credentials");
    home
}

/// A GET of `path` on the module's URL, addressed to `host`, or to no one.
async fn get(kernel: &Kernel, instance: &Instance, path: &str, host: Option<&str>) -> (u16, String) {
    let mut request = axum::http::Request::get(path);
    if let Some(host) = host {
        request = request.header(axum::http::header::HOST, host);
    }
    let r = server::app(kernel.clone(), instance.clone(), PORT)
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
    assert_eq!(d.name, "recon");
    assert_eq!(d.kind, NodeKind::KernelModule);
    assert!(d.depends_on.is_empty(), "a module depends on the kernel alone");
    assert!(SCHEMA_DDL.contains("superx_mod_recon"), "the schema defines the module's own service account");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_module_registers_as_recon_starting_then_active_with_its_url_then_stopped() {
    assert_eq!(MODULE_NAME, "recon");
    let (kernel, db) = booted_kernel().await;
    let entity = registry::starting(&kernel).await.expect("registers");
    assert_eq!(lifecycle(&kernel).await, "starting");
    assert_eq!(
        kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await.expect("reads"),
        Some(entity.clone()),
        "`superx param set recon` finds it by its name"
    );

    registry::active(&kernel, entity.clone(), "http://127.0.0.1:5153", 12).await.expect("active");
    assert_eq!(lifecycle(&kernel).await, "active");
    let url = kernel.get_parameter(entity.clone(), MODULE_UI_URL_PARAM).await.expect("reads");
    assert!(
        matches!(&url, Some(Value::String(u)) if u == "http://127.0.0.1:5153"),
        "it publishes its URL on its registry entry: {url:?}"
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
    let home = scratch_home("ws://127.0.0.1:1");
    let instance = Instance::read(&home).expect("instance");
    for host in ["127.0.0.1:5153", "localhost:5153"] {
        for path in ["/", "/status"] {
            let (status, text) = get(&kernel, &instance, path, Some(host)).await;
            assert_eq!(status, 200, "{host}{path}: {text}");
            assert!(text.contains("superx/recon: not available."), "{path}: {text}");
            assert!(text.contains("connected instance"), "the kernel's own reason: {text}");
            for name in superx_mod_recon::params::NAMES {
                assert!(text.contains(name), "{name} is listed:\n{text}");
            }
            assert!(
                text.lines().any(|l| l.trim_start().starts_with("attr_recon_port") && l.trim_end().ends_with(" 5153")),
                "it runs on 5153 until the operator says otherwise:\n{text}"
            );
        }
    }
    assert_eq!(get(&kernel, &instance, "/nothing", Some("127.0.0.1:5153")).await.0, 404);
    std::fs::remove_dir_all(&home).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_addressed_to_another_name_is_refused_403_naming_superx_recon() {
    let (kernel, _db) = booted_kernel().await;
    let home = scratch_home("ws://127.0.0.1:1");
    let instance = Instance::read(&home).expect("instance");
    // A page elsewhere that points a name of its own at 127.0.0.1 (DNS
    // rebinding) sends that name; a request with none names no one.
    for host in [Some("evil.example:5153"), Some("127.0.0.1.evil.example:5153"), Some("127.0.0.1:5154"), None] {
        for path in ["/", "/status", "/nothing"] {
            let (status, text) = get(&kernel, &instance, path, host).await;
            assert_eq!(status, 403, "{host:?}{path}: {text}");
            assert_eq!(text, "superx-recon answers only at http://127.0.0.1:5153\n");
        }
    }
    std::fs::remove_dir_all(&home).expect("clean up");
}

/// `status` counts every table the provisioned database has, but the
/// crawler's own record, which it says apart: a table added to the schema
/// and left out of `status` fails here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_counts_every_table_the_provisioned_database_has_but_pass_and_pass_source_each_once() {
    let db = own_db().await;
    let info: Value = db.query("INFO FOR DB").await.expect("info").take(0).expect("decodes");
    let Value::Object(info) = info else { unreachable!("INFO FOR DB is an object") };
    let Some(Value::Object(tables)) = info.get("tables") else { unreachable!("INFO FOR DB lists its tables") };
    let in_database: std::collections::BTreeSet<&str> =
        tables.keys().map(String::as_str).filter(|t| !["pass", "pass_source"].contains(t)).collect();
    let counted: std::collections::BTreeSet<&str> = fact_tables().collect();
    assert_eq!(fact_tables().count(), counted.len(), "each table once");
    assert_eq!(counted, in_database, "status counts exactly the database's tables of facts");
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
        CREATE trigger CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), \
            kind: 'task-notification', source: NONE, prompt_id: NONE, text: 'done', chars: 4, valid_from: time::now() };\
        CREATE reply:r1 CONTENT { session: session:s1, agent_id: 'msg_01', request_id: NONE, kernel_message: rand::uuid::v7(), \
            at: time::now(), model: 'claude-opus-5-5', requested_model: NONE, advisor_model: NONE, effort: NONE, \
            turn_effort: NONE, stop_reason: NONE, subagent: false, thinking_dropped: 0, valid_from: time::now() };\
        CREATE reply_state CONTENT { reply: reply:r1, input_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0, \
            output_tokens: 1, thinking_tokens: 0, tool_tokens: NONE, web_searches: NONE, web_fetches: NONE, \
            service_tier: NONE, text_chars: 0, usage: {}, valid_from: time::now() };\
        CREATE thinking CONTENT { reply: reply:r1, position: 0, kernel_message: rand::uuid::v7(), at: time::now(), ms: 5, \
            chars: 0, display: NONE, text: '', signature_chars: 0, valid_from: time::now() };\
        CREATE tool_call:t1 CONTENT { reply: reply:r1, call_id: 'toolu_01', kernel_message: rand::uuid::v7(), at: time::now(), \
            tool: 'Bash', path: NONE, command: 'cargo test', description: NONE, pattern: NONE, url: NONE, query: NONE, \
            subagent_type: NONE, background: false, input_bytes: 10, sandbox_off: false, valid_from: time::now() };\
        CREATE tool_result CONTENT { tool_call: tool_call:t1, kernel_message: rand::uuid::v7(), at: time::now(), \
            outcome: 'ok', ms: NONE, bytes: 10, sandbox_hosts: [], valid_from: time::now() };\
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
            valid_from: time::now() };\
        CREATE repository:r1 CONTENT { common_dir: '/w/.git', name: 'w', found_at: time::now(), valid_from: time::now() };\
        CREATE checkout:k1 CONTENT { repository: repository:r1, toplevel: '/w', found_at: time::now(), valid_from: time::now() };\
        CREATE directory CONTENT { path: '/w', source: 'session', checkout: checkout:k1, exact: true, valid_from: time::now() };";
    db.query(chain).await.expect("query").check().expect("rows taken");
    // Two passes, each with its sources; the newest is the one `status` says.
    let pass = |id: &str, finished: &str, outcome: &str, read: u32| {
        format!(
            "CREATE pass:{id} CONTENT {{ started_at: d'{finished}', finished_at: d'{finished}', rows_written: 17, \
             outcome: '{outcome}', note: NONE, valid_from: d'{finished}' }};\
             CREATE pass_source CONTENT {{ pass: pass:{id}, source: 'lines', started_at: d'{finished}', \
             finished_at: d'{finished}', items: {read}, unread: 2, calls: 0, failed: 0, timed_out: 0, \
             rows: {{ prompt: 2, reply: 1 }}, counts: {{}}, from_id: NONE, to_id: NONE, crawls: [], outcome: 'complete', \
             note: NONE, valid_from: d'{finished}' }};\
             CREATE pass_source CONTENT {{ pass: pass:{id}, source: 'repos', started_at: d'{finished}', \
             finished_at: d'{finished}', items: 3, unread: 0, calls: 9, failed: 1, timed_out: 0, rows: {{ directory: 1 }}, \
             counts: {{ repositories_new: 1 }}, from_id: NONE, to_id: NONE, crawls: [], outcome: '{outcome}', \
             note: NONE, valid_from: d'{finished}' }};"
        )
    };
    for sql in [pass("p1", "2026-10-24T09:00:00Z", "partial", 100), pass("p2", "2026-10-24T10:00:00Z", "complete", 500)] {
        db.query(sql).await.expect("query").check().expect("pass taken");
    }

    let h = holdings(&db).await.expect("reads");
    assert_eq!(
        h.rows,
        [
            ("session", 1),
            ("session_place", 1),
            ("prompt", 2),
            ("trigger", 1),
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
            ("repository", 1),
            ("checkout", 1),
            ("checkout_state", 0),
            ("directory", 1),
            ("commit", 0),
            ("commit_file", 0),
            ("placement", 0),
            ("landed_via", 0),
            ("survival", 0),
            ("crawl", 0),
            ("git_call", 0),
            ("remote_state", 0),
            ("ref_move", 0),
            ("commit_session", 0),
            ("host_repository", 0),
            ("host_user", 0),
            ("host_crawl", 0),
            ("host_call", 0),
            ("host_object", 0),
            ("mirror", 0),
            ("pull_request", 0),
            ("pull_request_state", 0),
            ("pull_request_commit", 0),
            ("ci_run", 0),
            ("ci_run_state", 0),
            ("line_field", 0),
        ]
    );
    let last = h.last_pass.expect("a pass ran");
    assert_eq!(last.outcome, "complete", "the newest pass, not the first");
    assert_eq!(last.finished_at.to_rfc3339(), "2026-10-24T10:00:00+00:00");
    assert_eq!(last.rows_written, 17);
    let sources: Vec<(&str, i64, i64, i64, i64)> =
        last.sources.iter().map(|s| (s.source.as_str(), s.items, s.unread, s.calls, s.rows_written)).collect();
    assert_eq!(sources, [("lines", 500, 2, 0, 3), ("repos", 3, 0, 9, 1)], "the newest pass's own sources");
}

#[test]
fn the_executable_names_itself_superx_recon_and_refuses_an_unknown_command_naming_it() {
    let help = Command::new(EXE).arg("--help").output().expect("runs");
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    for word in ["superx-recon", "--initialize", "--home", "start", "stop", "status", "run"] {
        assert!(text.contains(word), "{word}:\n{text}");
    }
    let bogus = Command::new(EXE).arg("bogus").output().expect("runs");
    assert!(!bogus.status.success());
    let said = String::from_utf8_lossy(&bogus.stderr);
    assert!(said.contains("unrecognized subcommand") && said.contains("superx-recon"), "{bogus:?}");
}

#[test]
fn initialize_with_no_root_password_changes_nothing_and_says_so() {
    let home = scratch_home("ws://127.0.0.1:1");
    let out = Command::new(EXE).arg("--home").arg(&home).arg("--initialize").stdin(Stdio::null()).output().expect("runs");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "superx-recon: no root password given\n");
    std::fs::remove_dir_all(&home).expect("clean up");
}

#[test]
fn start_without_a_substrate_says_why_and_leaves_nothing_running() {
    let home = scratch_home("ws://127.0.0.1:1");
    let start = Command::new(EXE).arg("--home").arg(&home).arg("start").output().expect("runs");
    assert!(!start.status.success(), "{start:?}");
    let said = String::from_utf8_lossy(&start.stderr);
    assert!(said.contains("exited before it was running"), "{said}");
    let log = std::fs::read_to_string(home.join("modules/recon/logs/superx-recon.out")).expect("its log");
    assert!(log.contains("cannot reach the substrate at ws://127.0.0.1:1"), "why, in its log: {log}");
    assert!(!home.join("modules/recon/superx-recon.pid").exists(), "no pid file: it is not running");

    let status = Command::new(EXE).arg("--home").arg(&home).arg("status").output().expect("runs");
    assert!(!status.status.success(), "the substrate did not answer");
    assert!(String::from_utf8_lossy(&status.stdout).contains("running     no"), "said first: {status:?}");
    let stop = Command::new(EXE).arg("--home").arg(&home).arg("stop").output().expect("runs");
    assert!(stop.status.success());
    assert_eq!(String::from_utf8_lossy(&stop.stdout), "superx-recon is not running\n");
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
    connected.expect("superx-recon reached the substrate within 10 s")
}

#[test]
fn stop_ends_superx_recon_even_while_it_starts_and_never_signals_another_process() {
    let (substrate, endpoint) = silent_substrate();
    let home = scratch_home(&endpoint);
    let instance = Instance::read(&home).expect("instance");
    std::fs::create_dir_all(instance.module_dir()).expect("module dir");

    // A pid file left behind, naming a pid another program has now.
    let mut other = Command::new("sleep").arg("30").spawn().expect("sleep");
    std::fs::write(instance.pid_path(), other.id().to_string()).expect("pid file");
    assert_eq!(daemon::live_pid(&instance), None, "not superx-recon");
    assert_eq!(daemon::stop(&instance).expect("stop answers"), "superx-recon is not running\n");
    assert!(other.try_wait().expect("waits").is_none(), "the other program was never signalled");
    other.kill().expect("kill sleep");
    other.wait().expect("reap sleep");

    // superx-recon, still starting: the substrate took its connection
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
        .expect("superx-recon");
    let _held = connection_from(&substrate);
    std::fs::write(instance.pid_path(), ours.id().to_string()).expect("pid file");
    assert_eq!(daemon::live_pid(&instance), Some(ours.id()));
    let asked = Instant::now();
    let said = daemon::stop(&instance).expect("stops");
    assert_eq!(said, format!("superx-recon stopped (pid {})\n", ours.id()));
    assert!(asked.elapsed() < Duration::from_secs(5), "stopped while starting, not after a wait: {:?}", asked.elapsed());
    assert!(ours.wait().expect("reaped").success(), "a stop while starting is an orderly end");
    std::fs::remove_dir_all(&home).expect("clean up");
}

/// Every file under `dir`, folders walked through.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return files };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// A token of the test's own, never a real one: every output the test
/// gathers is searched for it.
const MADE_UP_TOKEN: &str = "recon-made-up-token-9c2e";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_never_shows_in_debug_display_status_an_error_or_a_log_line() {
    let home = scratch_home("ws://127.0.0.1:1");
    let instance = Instance::read(&home).expect("instance");
    folder::prepare(&instance).expect("the module folder");
    let path = instance.credentials_path();
    let tokens = format!(r#"{{"github.com": {{"token": "{MADE_UP_TOKEN}"}}, "bitbucket.org": {{"token": ""}}}}"#);
    std::fs::write(&path, &tokens).expect("the operator's tokens");
    let mut outputs: Vec<String> = Vec::new();

    // Debug and Display.
    let read = credentials::read(&path);
    let Credentials::Hosts(hosts) = &read else { panic!("the hosts: {read:?}") };
    let token = hosts.github.clone().expect("a token for github.com");
    assert!(token.secret() == MADE_UP_TOKEN, "the token is read");
    outputs.extend([format!("{read:?}"), format!("{hosts:?}"), format!("{token:?}"), format!("{token}")]);

    // The status text, as the module's URL answers it.
    let (kernel, _db) = booted_kernel().await;
    let (_, page) = get(&kernel, &instance, "/status", Some("127.0.0.1:5153")).await;
    assert!(page.contains("github.com token set · bitbucket.org no token"), "{page}");
    outputs.push(page);

    // An error: the same token beside a host that breaks the file.
    std::fs::write(&path, format!(r#"{{"github.com": {{"token": "{MADE_UP_TOKEN}"}}, "bitbucket.org": 5}}"#)).expect("write");
    let broken = credentials::read(&path);
    assert!(matches!(broken, Credentials::Unreadable(_)), "{broken:?}");
    outputs.push(format!("{broken:?}"));
    outputs.push(get(&kernel, &instance, "/status", Some("127.0.0.1:5153")).await.1);

    // The executable's output and log lines, started on an instance whose
    // substrate does not answer, and asked for its status.
    std::fs::write(&path, &tokens).expect("write");
    for command in ["start", "status", "stop"] {
        let out = Command::new(EXE).arg("--home").arg(&home).arg(command).output().expect("runs");
        outputs.push(String::from_utf8_lossy(&out.stdout).into_owned());
        outputs.push(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let logs = walk(&instance.log_dir());
    assert!(!logs.is_empty(), "the executable wrote its log");
    outputs.extend(logs.iter().map(|log| String::from_utf8_lossy(&std::fs::read(log).expect("a log")).into_owned()));

    for (i, output) in outputs.iter().enumerate() {
        // Never printed: a failure here would show the token it found.
        assert!(!output.contains(MADE_UP_TOKEN), "output {i} of {} holds the token", outputs.len());
    }
    std::fs::remove_dir_all(&home).expect("clean up");
}

/// The word no file of the module uses for anything in it, spelled here in
/// two halves so this file does not hold it either.
fn the_word() -> String {
    ["read", "er"].concat()
}

#[test]
fn no_file_under_src_or_tests_holds_the_banned_word() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let word = the_word();
    let holding: Vec<String> = ["src", "tests"]
        .iter()
        .flat_map(|folder| walk(&here.join(folder)))
        .filter(|file| std::fs::read_to_string(file).is_ok_and(|text| text.to_lowercase().contains(&word)))
        .map(|file| file.display().to_string())
        .collect();
    assert!(holding.is_empty(), "files that hold the word: {holding:?}");
}

#[test]
fn no_crate_depends_on_recon_recon_depends_on_no_module_and_performance_and_repos_left_the_workspace() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline"])
        .current_dir(&workspace)
        .output()
        .expect("cargo metadata runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let metadata: serde_json::Value = serde_json::from_slice(&out.stdout).expect("cargo metadata's JSON");
    let members = metadata["packages"].as_array().expect("the workspace's packages");
    let names: Vec<&str> = members.iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.contains(&"superx-mod-recon"), "{names:?}");
    for gone in ["superx-mod-performance", "superx-mod-repos"] {
        assert!(!names.contains(&gone), "{gone} is still a member of the workspace: {names:?}");
    }
    for member in members {
        let name = member["name"].as_str().unwrap_or_default();
        let depends_on: Vec<&str> =
            member["dependencies"].as_array().into_iter().flatten().filter_map(|d| d["name"].as_str()).collect();
        if name == "superx-mod-recon" {
            let modules: Vec<&&str> = depends_on.iter().filter(|d| d.starts_with("superx-mod-")).collect();
            assert!(modules.is_empty(), "superx-mod-recon depends on another module's crate: {modules:?}");
        } else {
            assert!(!depends_on.contains(&"superx-mod-recon"), "{name} depends on superx-mod-recon");
        }
    }
}
