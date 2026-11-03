//! What the tests of the repositories and the hosts share (#538): a
//! kernel and the module's own database on the in-memory engine; sessions'
//! lines as capture keeps them; repositories made with git in a scratch
//! folder, their authors, committers and times set here; hosts answered by
//! a server of the test's own on the loopback, which records every request
//! it is sent; and a token made up here.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, TimeDelta, Utc};
use superx_kernel::message::json_to_object;
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Kernel, REQUIRED_METAMODEL_TYPES};
use superx_mod_recon::crawler::hosts::Endpoints;
use superx_mod_recon::crawler::{pass_on, Crawler, Pass, Places, Stop};
use superx_mod_recon::params::{from_values, Params};
use superx_mod_recon::{MODULE_NAME, SCHEMA_DDL};
use tokio::sync::watch;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// A token of the tests' own, never a real one, searched for everywhere.
pub const TOKEN: &str = "recon-test-token-6f1c2a";
pub const BB_TOKEN: &str = "recon-test-bb-token-93d0e7";

pub async fn kernel() -> Kernel {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = superx_kernel::SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", KERNEL_PASSWORD);
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    db.signin(surrealdb::opt::auth::Database {
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

pub async fn own_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db(MODULE_NAME).await.expect("nsdb");
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password"))
        .await
        .expect("provision")
        .check()
        .expect("provision is clean");
    db
}

// The sessions' lines --------------------------------------------------------

/// An id the kernel would give a line it captured at `at`.
fn id_at(table: &str, at: DateTime<Utc>) -> RecordId {
    let stamp = uuid::Timestamp::from_unix(
        uuid::NoContext,
        u64::try_from(at.timestamp()).expect("after 1970"),
        at.timestamp_subsec_nanos(),
    );
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::new_v7(stamp)))
}

/// A session's lines, captured a tenth of a second apart.
pub struct Capture {
    pub kernel: Kernel,
    pub agent: RecordId,
    pub session: RecordId,
    pub key: String,
    pub at: DateTime<Utc>,
    calls: u32,
}

impl Capture {
    /// An agent and one of its sessions, as capture registers them.
    pub async fn new(kernel: &Kernel, key: &str) -> Self {
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let mut d = Object::new();
        d.insert("name".to_string(), Value::String("claude_code".to_string()));
        kernel.supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("agent descriptor");
        let session = kernel.create_entity("node_session").await.expect("session");
        let mut d = Object::new();
        d.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
        d.insert("session".to_string(), Value::String(key.to_string()));
        kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(d)).await.expect("session descriptor");
        Self { kernel: kernel.clone(), agent, session, key: key.to_string(), at: Utc::now() - TimeDelta::minutes(30), calls: 0 }
    }

    /// A conversation line, as `message` keeps it.
    pub async fn message(&mut self, json: serde_json::Value) {
        self.at += TimeDelta::milliseconds(100);
        let at = self.at;
        let mut json = json;
        json["timestamp"] = serde_json::Value::String(at.to_rfc3339());
        json["sessionId"] = serde_json::Value::String(self.key.clone());
        self.kernel
            .db()
            .query(
                "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw: $raw, \
                 emitted_at: $at, valid_from: $at }",
            )
            .bind(("id", id_at("message", at)))
            .bind(("sess", self.session.clone()))
            .bind(("agent", self.agent.clone()))
            .bind(("raw", Value::Object(json_to_object(&json))))
            .bind(("at", at))
            .await
            .expect("line")
            .check()
            .expect("line taken");
    }

    /// The operator typing in `dir`.
    pub async fn worked_in(&mut self, dir: &Path) {
        self.message(serde_json::json!({
            "type": "user", "cwd": dir, "gitBranch": "main", "origin": { "kind": "human" },
            "message": { "role": "user", "content": "work here" }
        }))
        .await;
    }

    /// The agent running `command` with Bash in `dir`, and what it printed.
    pub async fn bash(&mut self, dir: &Path, command: &str, printed: &str) {
        self.calls += 1;
        let call = format!("call_{}", self.calls);
        let reply = format!("msg_{}", self.calls);
        self.message(serde_json::json!({
            "type": "assistant", "cwd": dir, "gitBranch": "main", "requestId": "req",
            "message": { "id": reply, "model": "claude-opus-5-5", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "id": call, "name": "Bash", "input": { "command": command } }],
                "usage": { "input_tokens": 1, "output_tokens": 1 } }
        }))
        .await;
        self.message(serde_json::json!({
            "type": "user", "cwd": dir, "gitBranch": "main",
            "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": call, "content": printed, "is_error": false }] },
            "toolUseResult": { "stdout": printed, "stderr": "", "interrupted": false }
        }))
        .await;
    }
}

// Repositories made with git ---------------------------------------------------

/// A scratch folder, by its real path: git names the real one.
pub fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-recon-538-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::canonicalize(&dir).expect("real path")
}

/// git in `dir`, as the test writes repositories: the machine's own
/// configuration unread, a fixed identity, its times `when` when given.
pub fn git_at(dir: &Path, args: &[&str], when: Option<&str>) -> String {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test Author", "-c", "user.email=author@example.com", "-c", "init.defaultBranch=main"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "protocol.file.allow=always"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_EDITOR", "true");
    if let Some(when) = when {
        command.env("GIT_AUTHOR_DATE", when).env("GIT_COMMITTER_DATE", when);
    }
    let out = command.output().expect("git runs");
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    git_at(dir, args, None)
}

/// git in `dir` that may fail: its exit and what it printed.
pub fn git_may(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test Author", "-c", "user.email=author@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_EDITOR", "true")
        .output()
        .expect("git runs");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// A repository on `main` with no commit.
pub fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("dir");
    git(dir, &["init", "-q", "-b", "main"]);
}

/// Write `files`, then commit them all with `message` at `when`; the new
/// commit's hash.
pub fn commit_at(dir: &Path, files: &[(&str, &str)], message: &str, when: &str) -> String {
    for (path, content) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        std::fs::write(&path, content).expect("file");
    }
    git(dir, &["add", "-A"]);
    git_at(dir, &["commit", "-q", "--allow-empty", "-m", message], Some(when));
    git(dir, &["rev-parse", "HEAD"])
}

pub fn commit(dir: &Path, files: &[(&str, &str)], message: &str) -> String {
    commit_at(dir, files, message, "2026-10-01T12:00:00+02:00")
}

// The crawler -----------------------------------------------------------------

pub fn params(set: &[(&'static str, Value)]) -> Params {
    let values: BTreeMap<&'static str, Value> = set.iter().cloned().collect();
    from_values(&values).params
}

pub fn strings(paths: &[&Path]) -> Value {
    Value::Array(paths.iter().map(|p| Value::String(p.to_string_lossy().into_owned())).collect::<Vec<_>>().into())
}

pub fn number(n: i64) -> Value {
    Value::Number(superx_kernel::types::Number::Int(n))
}

pub fn never_stop() -> (watch::Sender<bool>, Stop) {
    let (asked, watched) = watch::channel(false);
    (asked, Stop::new(watched))
}

/// One pass.
pub async fn pass(kernel: &Kernel, db: &superx_kernel::Db, params: &Params, crawler: &mut Crawler) -> Pass {
    let (_asked, mut stop) = never_stop();
    pass_on(kernel, db, params, crawler, &mut stop).await.expect("pass")
}

pub async fn rows(db: &superx_kernel::Db, sql: &str) -> Vec<Value> {
    db.query(sql).await.expect("query").take(0).expect("rows")
}

pub async fn count(db: &superx_kernel::Db, table: &str) -> i64 {
    let n: Option<i64> =
        db.query(format!("RETURN (SELECT count() AS n FROM {table} GROUP ALL)[0].n")).await.expect("count").take(0).expect("n");
    n.unwrap_or(0)
}

pub fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key),
        _ => None,
    }
}

pub fn text(v: &Value, key: &str) -> Option<String> {
    match field(v, key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

pub fn int(v: &Value, key: &str) -> Option<i64> {
    match field(v, key) {
        Some(Value::Number(n)) => n.to_int(),
        _ => None,
    }
}

pub fn flag(v: &Value, key: &str) -> Option<bool> {
    match field(v, key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

pub fn texts(v: &Value, key: &str) -> Option<Vec<String>> {
    match field(v, key) {
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|i| match i {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Every value of a row written out, to look for a token in.
pub fn written(v: &Value) -> String {
    format!("{v:?}")
}

/// What a source counted in a pass, by name.
pub fn counted(p: &Pass, source: &str, name: &str) -> i64 {
    p.source(source).and_then(|s| s.counts.get(name).copied()).unwrap_or(0)
}

// The hosts ---------------------------------------------------------------------

/// A request a test's host was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    /// Its path and query.
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// An answer a test's host gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    pub fn json(body: &serde_json::Value) -> Self {
        Self { status: 200, headers: Vec::new(), body: body.to_string() }
    }

    pub fn status(status: u16, body: &str) -> Self {
        Self { status, headers: Vec::new(), body: body.to_string() }
    }

    #[must_use]
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

#[derive(Default)]
struct Book {
    seen: Vec<Seen>,
    /// The answers to each path and query, the last kept once the others
    /// are given.
    answers: HashMap<String, VecDeque<Answer>>,
}

/// A host of the test's own on the loopback: it answers each path and
/// query as the test says, HTTP 404 otherwise, honours an etag sent back,
/// and records every request it is sent.
#[derive(Clone)]
pub struct Fake {
    /// Its address, with the API's base path.
    pub url: String,
    /// The base path its API lives under: empty for GitHub, `/2.0` for
    /// Bitbucket.
    pub base: String,
    book: Arc<Mutex<Book>>,
}

impl Fake {
    pub async fn start(base: &str) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("a port");
        let addr = listener.local_addr().expect("its address");
        let book = Arc::new(Mutex::new(Book::default()));
        let app = axum::Router::new().fallback(answer).with_state(Arc::clone(&book));
        tokio::spawn(async move {
            let _served = axum::serve(listener, app).await;
        });
        Self { url: format!("http://{addr}{base}"), base: base.to_string(), book }
    }

    /// Answer `path` (with its query, under the base path) with `answers`
    /// in turn.
    pub fn on(&self, path: &str, answers: Vec<Answer>) {
        let key = format!("{}{path}", self.base);
        self.book.lock().expect("book").answers.insert(key, answers.into());
    }

    pub fn json(&self, path: &str, body: &serde_json::Value) {
        self.on(path, vec![Answer::json(body)]);
    }

    /// Every request it was sent, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.book.lock().expect("book").seen.clone()
    }

    /// The paths it was sent, in order, its base path taken off.
    pub fn paths(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.path.strip_prefix(&self.base).unwrap_or(&s.path).to_string()).collect()
    }

    pub fn forget(&self) {
        self.book.lock().expect("book").seen.clear();
    }
}

async fn answer(State(book): State<Arc<Mutex<Book>>>, request: Request) -> Response {
    let path = request.uri().path_and_query().map_or_else(|| request.uri().path().to_string(), |p| p.as_str().to_string());
    let headers: Vec<(String, String)> =
        request.headers().iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or_default().to_string())).collect();
    let sent_etag = request.headers().get("if-none-match").and_then(|v| v.to_str().ok()).map(str::to_string);
    let given = {
        let mut book = book.lock().expect("book");
        book.seen.push(Seen { method: request.method().to_string(), path: path.clone(), headers });
        match book.answers.get_mut(&path) {
            Some(queue) if queue.len() > 1 => queue.pop_front(),
            Some(queue) => queue.front().cloned(),
            None => None,
        }
    };
    let given = given.unwrap_or_else(|| Answer::status(404, r#"{"message":"Not Found"}"#));
    let etag = given.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("etag")).map(|(_, v)| v.clone());
    let mut response = Response::builder();
    if given.status == 200 && etag.is_some() && etag == sent_etag {
        response = response.status(StatusCode::NOT_MODIFIED).header("etag", etag.unwrap_or_default());
        return response.body(Body::empty()).expect("an answer");
    }
    response = response.status(StatusCode::from_u16(given.status).expect("a status"));
    for (k, v) in &given.headers {
        // `@now+<n>`: the time n seconds after the answer, in epoch seconds,
        // rounded up so that it is never less than n seconds away.
        let v = match v.strip_prefix("@now+").and_then(|n| n.parse::<i64>().ok()) {
            Some(n) => {
                let at = Utc::now() + TimeDelta::seconds(n);
                (at.timestamp() + i64::from(at.timestamp_subsec_nanos() > 0)).to_string()
            }
            None => v.clone(),
        };
        response = response.header(k.as_str(), v.as_str());
    }
    response.body(Body::from(given.body)).expect("an answer")
}

/// A credentials file in `dir` giving each host its token, mode 0600.
pub fn credentials(dir: &Path, github: Option<&str>, bitbucket: Option<&str>) -> PathBuf {
    let path = dir.join("credentials.json");
    let mut hosts = serde_json::Map::new();
    if let Some(t) = github {
        hosts.insert("github.com".to_string(), serde_json::json!({ "token": t }));
    }
    if let Some(t) = bitbucket {
        hosts.insert("bitbucket.org".to_string(), serde_json::json!({ "token": t }));
    }
    std::fs::write(&path, serde_json::Value::Object(hosts).to_string()).expect("credentials");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    path
}

/// Where the crawler finds its own things in `dir`: its repos folder, the
/// credentials file, the hosts of the test's own, and the bare
/// repositories its mirrors fetch from (`<dir>/hosts/<host>/<path>.git`).
pub fn places(dir: &Path, credentials: Option<PathBuf>, github: &Fake, bitbucket: &Fake) -> Places {
    let repos = dir.join("repos");
    std::fs::create_dir_all(&repos).expect("repos folder");
    Places {
        repos: Some(repos),
        credentials,
        home: Some(dir.to_path_buf()),
        askpass: Some(PathBuf::from(env!("CARGO_BIN_EXE_superx-recon"))),
        endpoints: Endpoints { github: github.url.clone(), bitbucket: bitbucket.url.clone(), git: Some(dir.join("hosts")) },
    }
}

/// A host's repository its mirrors fetch from, made from `from`.
pub fn hosted(dir: &Path, host: &str, path: &str, from: &Path) -> PathBuf {
    let bare = dir.join("hosts").join(host).join(format!("{path}.git"));
    std::fs::create_dir_all(bare.parent().expect("a folder")).expect("folder");
    git(dir, &["clone", "-q", "--bare", &from.to_string_lossy(), &bare.to_string_lossy()]);
    bare
}

/// GitHub's answer for a repository.
pub fn github_repository(id: i64, default_branch: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "full_name": "acme/widget", "default_branch": default_branch,
        "owner": { "login": "acme", "id": 9001, "type": "Organization" }
    })
}

/// A GitHub account.
pub fn github_user(login: &str, id: i64) -> serde_json::Value {
    serde_json::json!({ "login": login, "id": id, "type": "User", "name": format!("{login} name") })
}

/// Write `files` and commit them as `author` at `authored` and
/// `committer` at `committed`, each `Name <email>`; the new commit's hash.
pub fn commit_as(dir: &Path, files: &[(&str, &str)], message: &str, author: (&str, &str, &str), committer: (&str, &str, &str)) -> String {
    for (path, content) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        std::fs::write(&path, content).expect("file");
    }
    git(dir, &["add", "-A"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "-q", "--allow-empty", "--cleanup=verbatim", "-m", message])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", author.0)
        .env("GIT_AUTHOR_EMAIL", author.1)
        .env("GIT_AUTHOR_DATE", author.2)
        .env("GIT_COMMITTER_NAME", committer.0)
        .env("GIT_COMMITTER_EMAIL", committer.1)
        .env("GIT_COMMITTER_DATE", committer.2)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(dir, &["rev-parse", "HEAD"])
}

/// An object written whole with `git hash-object -w`, its hash.
pub fn object(dir: &Path, kind: &str, content: &str) -> String {
    object_as(dir, kind, content, false)
}

/// An object written whole, `literally` past git's checks of its format.
pub fn object_as(dir: &Path, kind: &str, content: &str, literally: bool) -> String {
    use std::io::Write as _;
    let mut args = vec!["hash-object", "-t", kind, "-w", "--stdin"];
    if literally {
        args.push("--literally");
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(&args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("git runs");
    child.stdin.take().expect("its input").write_all(content.as_bytes()).expect("written");
    let out = child.wait_with_output().expect("git ends");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A GitHub pull request: `merged` names its merge commit.
pub fn github_pull(number: i64, head: &str, base: &str, merged: Option<&str>, updated_at: &str) -> serde_json::Value {
    serde_json::json!({
        "id": number * 100, "number": number, "state": if merged.is_some() { "closed" } else { "open" },
        "title": format!("pull {number}"), "draft": false,
        "user": github_user("dev", 11),
        "created_at": "2026-10-01T10:00:00Z", "updated_at": updated_at,
        "head": { "ref": "feat", "sha": head, "repo": { "full_name": "acme/widget" } },
        "base": { "ref": "main", "sha": base, "repo": { "full_name": "acme/widget" } },
        "html_url": format!("https://github.com/acme/widget/pull/{number}"),
        "merged": merged.is_some(), "merge_commit_sha": merged.unwrap_or(head),
        "merged_at": merged.map(|_| "2026-10-02T10:00:00Z"), "merged_by": merged.map(|_| github_user("lead", 12)),
        "closed_at": merged.map(|_| "2026-10-02T10:00:00Z"),
        "additions": 3, "deletions": 1, "changed_files": 2, "commits": 2, "comments": 1, "review_comments": 0
    })
}
