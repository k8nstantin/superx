//! A pass over the list (#501), end to end: a kernel holding a session
//! whose lines carry working directories, real git repositories in a
//! scratch directory, and the module's own database. What a pass writes is
//! read back from the database.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, TimeDelta, Utc};
use surrealdb::opt::auth::Database;
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Kernel, REQUIRED_METAMODEL_TYPES};
use superx_mod_repos::params::{from_values, Params, EXCLUDE_PARAM, INCLUDE_PARAM};
use superx_mod_repos::store::Sweep;
use superx_mod_repos::sweep::{pass, Stop};
use superx_mod_repos::{MODULE_NAME, SCHEMA_DDL};
use tokio::sync::watch;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn booted_kernel() -> Kernel {
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
    let kernel = Kernel::from_db(db);
    for t in REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await.expect("metamodel");
    }
    kernel
}

async fn own_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db(MODULE_NAME).await.expect("nsdb");
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password"))
        .await
        .expect("provision")
        .check()
        .expect("provision is clean");
    db
}

/// An agent and one of its sessions, as capture registers them.
async fn agent_and_session(kernel: &Kernel) -> (RecordId, RecordId) {
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let mut d = Object::new();
    d.insert("name".to_string(), Value::String("claude_code".to_string()));
    kernel.supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("agent descriptor");
    let session = kernel.create_entity("node_session").await.expect("session");
    let mut d = Object::new();
    d.insert("name".to_string(), Value::String("claude_code/s1".to_string()));
    kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(d)).await.expect("session descriptor");
    (agent, session)
}

/// A line of the session, captured at `at`, carrying `cwd`, as the Claude
/// Code adapter keeps it: its id a UUIDv7 of the moment it was captured, as
/// the kernel sets it.
async fn line(kernel: &Kernel, agent: &RecordId, session: &RecordId, cwd: &Path, at: DateTime<Utc>) {
    let raw = superx_kernel::message::json_to_object(&serde_json::json!({ "type": "user", "cwd": cwd }));
    let stamp = uuid::Timestamp::from_unix(
        uuid::NoContext,
        u64::try_from(at.timestamp()).expect("after 1970"),
        at.timestamp_subsec_nanos(),
    );
    let id = RecordId::new("message", superx_kernel::types::Uuid::from(uuid::Uuid::new_v7(stamp)));
    kernel
        .db()
        .query("CREATE $id CONTENT { session: $sess, agent: $agent, role: 'user', content: '', raw: $raw, valid_from: $at }")
        .bind(("id", id))
        .bind(("sess", session.clone()))
        .bind(("agent", agent.clone()))
        .bind(("raw", Value::Object(raw)))
        .bind(("at", at))
        .await
        .expect("line")
        .check()
        .expect("line taken");
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@x", "-c", "init.defaultBranch=main"])
        .args(args)
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?} in {}", dir.display());
}

/// A repository with one commit on `main`.
fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("dir");
    git(dir, &["init", "-q"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
}

/// A scratch directory, by its real path: git names the real one.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-repos-sweep-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::canonicalize(&dir).expect("real path")
}

fn params(set: &[(&'static str, Value)]) -> Params {
    let values: BTreeMap<&'static str, Value> = set.iter().cloned().collect();
    from_values(&values).params
}

fn strings(paths: &[&Path]) -> Value {
    Value::Array(paths.iter().map(|p| Value::String(p.to_string_lossy().into_owned())).collect::<Vec<_>>().into())
}

/// Every row of a table, as values.
async fn rows(db: &superx_kernel::Db, sql: &str) -> Vec<Value> {
    db.query(sql).await.expect("query").take(0).expect("rows")
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The newest row's `key` for a path, read back as `(checkout's toplevel or
/// "none", exact, source)`.
async fn directory(db: &superx_kernel::Db, path: &Path) -> (String, bool, String) {
    let rows = rows(
        db,
        &format!(
            "SELECT checkout.toplevel ?? 'none' AS top, exact, source, valid_from FROM directory \
             WHERE path = '{}' ORDER BY valid_from DESC",
            path.display()
        ),
    )
    .await;
    let newest = rows.first().expect("a row for the directory");
    let exact = matches!(newest, Value::Object(o) if matches!(o.get("exact"), Some(Value::Bool(true))));
    (text(newest, "top").expect("top"), exact, text(newest, "source").expect("source"))
}

/// A checkout's newest state, as `(present, branch)`.
async fn state(db: &superx_kernel::Db, toplevel: &Path) -> (bool, Option<String>) {
    let rows = rows(
        db,
        &format!(
            "SELECT present, branch, valid_from FROM checkout_state WHERE checkout.toplevel = '{}' ORDER BY valid_from DESC",
            toplevel.display()
        ),
    )
    .await;
    let newest = rows.first().expect("a state for the checkout");
    let present = matches!(newest, Value::Object(o) if matches!(o.get("present"), Some(Value::Bool(true))));
    (present, text(newest, "branch"))
}

fn counts(s: &Sweep) -> [u64; 9] {
    [
        s.directories_new,
        s.directories_moved,
        s.repositories_new,
        s.repositories_known,
        s.repositories_stale,
        s.checkouts_new,
        s.checkouts_known,
        s.checkouts_gone,
        s.checkouts_back,
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_follows_the_sessions_directories_and_keeps_the_list() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel).await;
    let root = scratch();
    let (a, wt, b, c, d) = (root.join("a"), root.join("a-wt"), root.join("b"), root.join("c"), root.join("d"));
    repo(&a);
    std::fs::create_dir_all(a.join("src")).expect("src");
    git(&a, &["worktree", "add", "-q", "-b", "feat", &wt.to_string_lossy()]);
    repo(&b);
    repo(&c);
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).expect("plain");
    let vanished = a.join("gone").join("deep");
    let earlier = Utc::now() - TimeDelta::hours(2);
    for (n, dir) in [a.join("src"), wt.clone(), b.clone(), plain.clone(), vanished.clone()].iter().enumerate() {
        line(&kernel, &agent, &session, dir, earlier + TimeDelta::seconds(n as i64)).await;
    }
    let p = params(&[(INCLUDE_PARAM, strings(&[&c]))]);
    let (asked, watched) = watch::channel(false);
    let mut stop = Stop::new(watched);

    // The first pass: everything is new.
    let first = pass(&kernel, &db, &p, &mut stop).await.expect("first pass");
    assert_eq!(first.outcome, "complete", "{first:?}");
    assert_eq!((first.sessions_read, first.directories_read), (1, 6), "five directories in the lines, one root named");
    assert_eq!(counts(&first), [6, 0, 3, 3, 0, 4, 4, 0, 0], "{first:?}");
    assert_eq!(first.rows_written, 3 + 4 + 6 + 4, "repositories, checkouts, directories and states");
    let names: Vec<String> = rows(&db, "SELECT name FROM repository ORDER BY name").await.iter().filter_map(|r| text(r, "name")).collect();
    assert_eq!(names, ["a", "b", "c"], "a worktree is its repository's");
    assert_eq!(directory(&db, &a.join("src")).await, (a.display().to_string(), true, "session".to_string()));
    assert_eq!(directory(&db, &wt).await, (wt.display().to_string(), true, "session".to_string()));
    assert_eq!(directory(&db, &plain).await, ("none".to_string(), true, "session".to_string()), "no repository holds it");
    assert_eq!(
        directory(&db, &vanished).await,
        (a.display().to_string(), false, "session".to_string()),
        "gone: its repository is known through its nearest folder, not exactly"
    );
    assert_eq!(directory(&db, &c).await, (c.display().to_string(), true, "include".to_string()));
    assert_eq!(state(&db, &a).await, (true, Some("main".to_string())));
    assert_eq!(state(&db, &wt).await, (true, Some("feat".to_string())));

    // Nothing changed: nothing but the pass is written. It reads only the
    // lines captured since the last pass began, a minute before it: none,
    // so the only directory it reads is the root the operator names.
    let second = pass(&kernel, &db, &p, &mut stop).await.expect("second pass");
    assert_eq!((second.rows_written, second.directories_read, second.sessions_read), (0, 1, 0), "{second:?}");
    assert_eq!(counts(&second), [0, 0, 0, 3, 0, 0, 4, 0, 0]);

    // A repository's folder is gone: its checkout goes stale, and so does
    // the repository; the directory no repository holds now says so.
    std::fs::remove_dir_all(&b).expect("remove b");
    let third = pass(&kernel, &db, &p, &mut stop).await.expect("third pass");
    assert_eq!(counts(&third), [0, 1, 0, 3, 1, 0, 4, 1, 0], "{third:?}");
    assert_eq!(state(&db, &b).await, (false, None));
    assert_eq!(directory(&db, &b).await, ("none".to_string(), false, "session".to_string()));

    // It is back: the same repository and checkout, present again.
    repo(&b);
    let fourth = pass(&kernel, &db, &p, &mut stop).await.expect("fourth pass");
    assert_eq!(counts(&fourth), [0, 1, 0, 3, 0, 0, 4, 0, 1], "{fourth:?}");
    assert_eq!(state(&db, &b).await, (true, Some("main".to_string())));

    // A line written after the last pass names a new repository: read.
    repo(&d);
    line(&kernel, &agent, &session, &d, Utc::now()).await;
    let fifth = pass(&kernel, &db, &p, &mut stop).await.expect("fifth pass");
    assert_eq!(counts(&fifth), [1, 0, 1, 4, 0, 1, 5, 0, 0], "{fifth:?}");
    assert_eq!(fifth.sessions_read, 1);

    let passes = rows(&db, "SELECT outcome FROM sweep").await;
    assert_eq!(passes.len(), 5, "every pass is recorded");
    drop(asked);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_excluded_repository_is_never_listed() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel).await;
    let root = scratch();
    let (a, b) = (root.join("a"), root.join("b"));
    repo(&a);
    repo(&b);
    let earlier = Utc::now() - TimeDelta::hours(1);
    line(&kernel, &agent, &session, &a, earlier).await;
    line(&kernel, &agent, &session, &b, earlier + TimeDelta::seconds(1)).await;
    let p = params(&[(EXCLUDE_PARAM, Value::Array(vec![Value::String("b".to_string())].into()))]);
    let (_asked, watched) = watch::channel(false);
    let s = pass(&kernel, &db, &p, &mut Stop::new(watched)).await.expect("pass");
    assert_eq!((s.excluded, s.repositories_known, s.checkouts_known), (1, 1, 1), "{s:?}");
    let names: Vec<String> = rows(&db, "SELECT name FROM repository").await.iter().filter_map(|r| text(r, "name")).collect();
    assert_eq!(names, ["a"]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_asked_to_stop_is_recorded_as_stopped() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel).await;
    let root = scratch();
    repo(&root.join("a"));
    line(&kernel, &agent, &session, &root.join("a"), Utc::now() - TimeDelta::hours(1)).await;
    let (asked, watched) = watch::channel(false);
    asked.send_replace(true);
    let s = pass(&kernel, &db, &params(&[]), &mut Stop::new(watched)).await.expect("pass");
    assert_eq!((s.outcome, s.rows_written, s.git_calls), ("stopped", 0, 0), "{s:?}");
    let outcomes: Vec<String> = rows(&db, "SELECT outcome FROM sweep").await.iter().filter_map(|r| text(r, "outcome")).collect();
    assert_eq!(outcomes, ["stopped"]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// How long the test gives the crawler to start waiting, and then to end.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(200); // skill-allow: §9-duration — test fixture, not a policy
const LIMIT: std::time::Duration = std::time::Duration::from_secs(5); // skill-allow: §9-duration — test fixture, not a policy

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_crawler_ends_when_asked_even_while_it_waits() {
    // A test handle has no database of its own behind it, so the crawler
    // finds nothing to pass over and waits a minute before it looks again.
    let kernel = booted_kernel().await;
    let (asked, watched) = watch::channel(false);
    let crawler = tokio::spawn(superx_mod_repos::sweep::run(kernel, Stop::new(watched)));
    tokio::time::sleep(SETTLE).await;
    asked.send_replace(true);
    let ended = tokio::time::timeout(LIMIT, crawler).await;
    assert!(matches!(ended, Ok(Ok(()))), "it ended when asked: {ended:?}");
}
