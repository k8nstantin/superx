//! What the repositories' and the hosts' tests share (#538): a kernel and
//! the module's own database on the in-memory engine, repositories made with
//! git in a scratch folder, sessions, tool calls and git events written as
//! the lines source writes them, a host answered by a server of the test's
//! own on the loopback that records every request, a token the test makes
//! up, and every log line a pass writes.

#![allow(dead_code)] // skill-allow: allow — each test file uses the part it needs

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, REQUIRED_METAMODEL_TYPES};
use superx_mod_recon::crawler::hosts::Endpoints;
use superx_mod_recon::crawler::{pass_on, Crawler, Pass, Places, Stop};
use superx_mod_recon::params::{from_values, Params};
use superx_mod_recon::{MODULE_NAME, SCHEMA_DDL};
use tokio::sync::watch;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// A token of the test's own, never a real one.
pub const TOKEN: &str = "recon-made-up-token-c0ffee";

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

/// A scratch folder, by its real path: git names the real one.
pub fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-recon-{what}-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::canonicalize(&dir).expect("real path")
}

/// git in `dir`, the test's own identity and a fixed main branch; what it
/// printed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    git_env(dir, args, &[])
}

pub fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test Person", "-c", "user.email=test@example.test", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(env.iter().copied())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with one commit on `main`.
pub fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("dir");
    git(dir, &["init", "-q"]);
    write(dir, "README.md", "first\n");
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "first"]);
}

pub fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

/// Commit everything in `dir` with `message`; its full hash.
pub fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

pub fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

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

pub async fn pass(kernel: &Kernel, db: &superx_kernel::Db, params: &Params, crawler: &mut Crawler) -> Pass {
    let (_asked, mut stop) = never_stop();
    pass_on(kernel, db, params, crawler, &mut stop).await.expect("pass")
}

pub async fn rows(db: &superx_kernel::Db, sql: &str) -> Vec<Value> {
    db.query(sql).await.expect("query").take(0).expect("rows")
}

pub async fn rows_with(db: &superx_kernel::Db, sql: &str, name: &str, value: Value) -> Vec<Value> {
    db.query(sql).bind((name.to_string(), value)).await.expect("query").take(0).expect("rows")
}

pub async fn count(db: &superx_kernel::Db, table: &str) -> i64 {
    let n: Option<i64> =
        db.query(format!("RETURN (SELECT count() AS n FROM {table} GROUP ALL)[0].n")).await.expect("count").take(0).expect("n");
    n.unwrap_or(0)
}

pub fn text(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub fn int(v: &Value, key: &str) -> Option<i64> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int(),
            _ => None,
        },
        _ => None,
    }
}

pub fn flag(v: &Value, key: &str) -> Option<bool> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        },
        _ => None,
    }
}

pub fn texts(v: &Value, key: &str) -> Option<Vec<String>> {
    match v {
        Value::Object(o) => match o.get(key) {
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
        },
        _ => None,
    }
}

pub fn time(v: &Value, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Datetime(d)) => Some(chrono::DateTime::<chrono::Utc>::from(*d)),
            _ => None,
        },
        _ => None,
    }
}

pub fn link(v: &Value, key: &str) -> Option<RecordId> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::RecordId(id)) => Some(id.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// What the `repos` source counted in a pass, by name.
pub fn repos_count(p: &Pass, name: &str) -> i64 {
    p.source("repos").and_then(|s| s.counts.get(name).copied()).unwrap_or(0)
}

/// A session and its reply, as the lines source writes them; the session.
pub async fn session(db: &superx_kernel::Db, key: &str) -> RecordId {
    let id: Vec<Value> = db
        .query(
            "LET $s = (CREATE $sid CONTENT { kernel_session: rand::uuid::v7(), agent: 'claude_code', agent_id: $key, \
             first_at: time::now(), valid_from: time::now() }).id; \
             CREATE $rid CONTENT { session: $s[0], agent_id: $key, kernel_message: rand::uuid::v7(), at: time::now(), \
             model: 'claude-opus-5-5', subagent: false, thinking_dropped: 0, valid_from: time::now() }; \
             RETURN $s[0];",
        )
        .bind(("sid", RecordId::new("session", superx_kernel::types::Uuid::new_v7())))
        .bind(("rid", RecordId::new("reply", superx_kernel::types::Uuid::new_v7())))
        .bind(("key", key.to_string()))
        .await
        .expect("session")
        .take(2)
        .expect("its id");
    match id.first() {
        Some(Value::RecordId(id)) => id.clone(),
        other => unreachable!("a session id: {other:?}"),
    }
}

/// A tool call of the session's reply and one git event of it, as the lines
/// source writes them; the tool call.
pub async fn git_event(db: &superx_kernel::Db, session: &RecordId, action: &str, dir: Option<&Path>, hash: Option<&str>, url: Option<&str>, pr_number: Option<i64>) -> RecordId {
    let reply: Vec<Value> = rows_with(db, "SELECT VALUE id FROM reply WHERE session = $s", "s", Value::RecordId(session.clone())).await;
    let Some(Value::RecordId(reply)) = reply.first() else { unreachable!("a reply") };
    let call = superx_kernel::types::Uuid::new_v7();
    let call = RecordId::new("tool_call", call);
    let mut vars = superx_kernel::types::Object::new();
    vars.insert("call", Value::RecordId(call.clone()));
    vars.insert("call_id", Value::String(format!("toolu_{}", superx_kernel::types::Uuid::new_v7())));
    vars.insert("reply", Value::RecordId(reply.clone()));
    vars.insert("action", Value::String(action.to_string()));
    vars.insert("dir", dir.map_or(Value::None, |d| Value::String(d.to_string_lossy().into_owned())));
    vars.insert("hash", hash.map_or(Value::None, |h| Value::String(h.to_string())));
    vars.insert("url", url.map_or(Value::None, |u| Value::String(u.to_string())));
    vars.insert("pr", pr_number.map_or(Value::None, number));
    db.query(
        "CREATE $call CONTENT { reply: $reply, call_id: $call_id, kernel_message: rand::uuid::v7(), at: time::now(), \
         tool: 'Bash', command: 'git', background: false, input_bytes: 3, sandbox_off: false, valid_from: time::now() }; \
         CREATE git_event CONTENT { tool_call: $call, at: time::now(), action: $action, dir: $dir, hash: $hash, \
         branch: NONE, subject: NONE, pr_number: $pr, url: $url, valid_from: time::now() };",
    )
    .bind(vars)
    .await
    .expect("git event")
    .check()
    .expect("taken");
    call
}

/// A session working in `dir`: the place the lines source wrote for it.
pub async fn worked_in(db: &superx_kernel::Db, session: &RecordId, dir: &Path) {
    db.query("CREATE session_place CONTENT { session: $s, cwd: $cwd, branch: NONE, at: time::now(), valid_from: time::now() }")
        .bind(("s", session.clone()))
        .bind(("cwd", dir.to_string_lossy().into_owned()))
        .await
        .expect("place")
        .check()
        .expect("taken");
}

// ── a host of the test's own ───────────────────────────────────────────────

/// One answer the host gives a path.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// How long the host takes to answer.
    pub after: std::time::Duration,
}

impl Answer {
    pub fn json(body: serde_json::Value) -> Self {
        Self { status: 200, headers: Vec::new(), body: body.to_string(), after: std::time::Duration::ZERO }
    }

    pub fn status(status: u16, body: &str) -> Self {
        Self { status, headers: Vec::new(), body: body.to_string(), after: std::time::Duration::ZERO }
    }

    /// Answered only after `after`.
    pub fn late(mut self, after: std::time::Duration) -> Self {
        self.after = after;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// A prefix of paths and its answers.
pub type Prefixed = (String, VecDeque<Answer>);

/// A request the host was sent.
#[derive(Debug, Clone)]
pub struct Asked {
    pub method: String,
    /// Its path and query.
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Asked {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A host of the test's own: each path's answers, given in turn, the last
/// given again; a path with none of its own takes the answers of the
/// longest prefix it begins with; every request it is sent, recorded. An
/// answer with an etag is HTTP 304 to a request that sends that etag back.
/// A path it holds no answer for is HTTP 404.
#[derive(Debug, Clone, Default)]
pub struct Server {
    pub answers: Arc<Mutex<HashMap<String, VecDeque<Answer>>>>,
    pub prefixes: Arc<Mutex<Vec<Prefixed>>>,
    pub asked: Arc<Mutex<Vec<Asked>>>,
    pub base: String,
}

impl Server {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("its address"));
        let server = Self { base, ..Self::default() };
        let state = server.clone();
        let app = axum::Router::new().fallback(answer).with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serves");
        });
        server
    }

    /// Answer `path` (with its query) with `answers`, in turn.
    pub fn on(&self, path: &str, answers: Vec<Answer>) {
        self.answers.lock().expect("answers").insert(path.to_string(), answers.into());
    }

    /// Answer every path that begins with `prefix` and has no answers of
    /// its own with `answers`, in turn.
    pub fn on_prefix(&self, prefix: &str, answers: Vec<Answer>) {
        self.prefixes.lock().expect("prefixes").push((prefix.to_string(), answers.into()));
    }

    pub fn asked(&self) -> Vec<Asked> {
        self.asked.lock().expect("asked").clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.asked().into_iter().map(|a| a.path).collect()
    }
}

async fn answer(axum::extract::State(server): axum::extract::State<Server>, request: axum::extract::Request) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let path = request.uri().path_and_query().map_or_else(|| request.uri().path().to_string(), |p| p.as_str().to_string());
    let headers: Vec<(String, String)> =
        request.headers().iter().map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap_or_default().to_string())).collect();
    let if_none_match = request.headers().get("if-none-match").and_then(|v| v.to_str().ok()).map(str::to_string);
    server.asked.lock().expect("asked").push(Asked { method: request.method().to_string(), path: path.clone(), headers });
    let take = |queue: &mut VecDeque<Answer>| if queue.len() > 1 { queue.pop_front() } else { queue.front().cloned() };
    let given = {
        let mut answers = server.answers.lock().expect("answers");
        answers.get_mut(&path).and_then(take)
    };
    let given = given.or_else(|| {
        let mut prefixes = server.prefixes.lock().expect("prefixes");
        prefixes.iter_mut().filter(|(p, _)| path.starts_with(p.as_str())).max_by_key(|(p, _)| p.len()).and_then(|(_, queue)| take(queue))
    });
    let Some(given) = given else {
        return (axum::http::StatusCode::NOT_FOUND, "{\"message\":\"Not Found\"}").into_response();
    };
    tokio::time::sleep(given.after).await;
    let etag = given.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("etag")).map(|(_, v)| v.clone());
    let unchanged = etag.is_some() && etag == if_none_match;
    let status = if unchanged { 304 } else { given.status };
    let mut response = (axum::http::StatusCode::from_u16(status).expect("a status"), if unchanged { String::new() } else { given.body })
        .into_response();
    for (name, value) in given.headers {
        let name = axum::http::HeaderName::from_bytes(name.as_bytes()).expect("a header name");
        response.headers_mut().insert(name, axum::http::HeaderValue::from_str(&value).expect("a header value"));
    }
    response
}

/// Places for a pass: a repos folder, a credentials file with the test's
/// token for the hosts named, superx-recon as git's askpass, and the hosts
/// at `endpoints`.
pub fn places(home: &Path, endpoints: Endpoints, tokens: &[&str]) -> Places {
    let module = home.join("modules/recon");
    std::fs::create_dir_all(module.join("repos")).expect("repos folder");
    let mut entries = serde_json::Map::new();
    for host in tokens {
        entries.insert((*host).to_string(), serde_json::json!({ "token": TOKEN }));
    }
    let credentials = module.join("credentials.json");
    std::fs::write(&credentials, serde_json::Value::Object(entries).to_string()).expect("credentials");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    Places {
        home: Some(home.to_path_buf()),
        repos: Some(module.join("repos")),
        credentials: Some(credentials),
        askpass: Some(PathBuf::from(env!("CARGO_BIN_EXE_superx-recon"))),
        hosts: endpoints,
    }
}

/// The hosts at the test's server, and their git at `git_base`'s folders.
pub fn endpoints(server: &Server, git_base: &Path) -> Endpoints {
    Endpoints {
        github_api: server.base.clone(),
        github_git: git_base.join("github.com").to_string_lossy().into_owned(),
        bitbucket_api: format!("{}/2.0", server.base),
        bitbucket_git: git_base.join("bitbucket.org").to_string_lossy().into_owned(),
    }
}

/// A Bitbucket page of `values`, as its lists answer.
pub fn page(values: serde_json::Value) -> Answer {
    let size = values.as_array().map_or(0, Vec::len);
    Answer::json(serde_json::json!({ "values": values, "pagelen": 50, "size": size, "page": 1 }))
}

/// GitHub answering every path step 17 names for `path` with nothing but
/// `repository`, its default branch unprotected.
pub fn github_quiet(server: &Server, path: &str, repository: serde_json::Value) {
    let base = format!("/repos/{path}");
    let default = repository["default_branch"].as_str().unwrap_or("main").to_string();
    server.on(&base, vec![Answer::json(repository)]);
    for list in ["pulls?state=all&sort=created&direction=asc", "issues?state=all&sort=created&direction=asc", "issues/comments?sort=created&direction=asc", "pulls/comments?sort=created&direction=asc"] {
        server.on(&format!("{base}/{list}&per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    for list in ["issues/events", "deployments", "releases", "rulesets"] {
        server.on(&format!("{base}/{list}?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }
    server.on(&format!("{base}/actions/runs?per_page=100"), vec![Answer::json(serde_json::json!({ "total_count": 0, "workflow_runs": [] }))]);
    server.on(&format!("{base}/environments?per_page=100"), vec![Answer::json(serde_json::json!({ "total_count": 0, "environments": [] }))]);
    server.on(&format!("{base}/branches/{default}/protection"), vec![Answer::status(404, "{\"message\":\"Branch not protected\"}")]);
    server.on_prefix(&format!("{base}/commits/"), vec![Answer::json(serde_json::json!([]))]);
}

/// Bitbucket answering every path step 18 names for `path` with nothing but
/// `repository`.
pub fn bitbucket_quiet(server: &Server, path: &str, repository: serde_json::Value) {
    let base = format!("/2.0/repositories/{path}");
    server.on(&base, vec![Answer::json(repository)]);
    server.on(&format!("{base}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&sort=created_on&pagelen=50"), vec![page(serde_json::json!([]))]);
    server.on(&format!("{base}/pipelines?sort=-created_on&pagelen=50"), vec![page(serde_json::json!([]))]);
    for list in ["deployments", "environments", "branch-restrictions", "default-reviewers"] {
        server.on(&format!("{base}/{list}?pagelen=50"), vec![page(serde_json::json!([]))]);
    }
    server.on_prefix(&format!("{base}/commit/"), vec![page(serde_json::json!([]))]);
}

/// The host's own copy of `from`, a bare repository at `<git_base>/<host>/<path>.git`.
pub fn hosted(from: &Path, git_base: &Path, host: &str, path: &str) -> PathBuf {
    let to = git_base.join(host).join(format!("{path}.git"));
    std::fs::create_dir_all(to.parent().expect("a parent")).expect("host folder");
    git(from, &["clone", "-q", "--bare", &from.to_string_lossy(), &to.to_string_lossy()]);
    to
}

// ── every log line ─────────────────────────────────────────────────────────

/// Every event logged while it is the default subscriber, as text.
#[derive(Debug, Clone, Default)]
pub struct Logged(pub Arc<Mutex<Vec<String>>>);

/// Every event logged on any thread of this test binary from its first call
/// on: the process's one subscriber, set once. A subscriber set for one
/// thread misses what tests running beside it leave the callsites caching.
pub fn logged() -> Logged {
    static LOGGED: std::sync::OnceLock<Logged> = std::sync::OnceLock::new();
    LOGGED
        .get_or_init(|| {
            let logged = Logged::default();
            tracing::subscriber::set_global_default(logged.clone()).expect("the one subscriber of this test binary");
            logged
        })
        .clone()
}

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Logged {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        self.0.lock().expect("logged").push(fields.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Every row of every table the module's database holds, written out.
pub async fn every_row(db: &superx_kernel::Db) -> String {
    let info: Value = db.query("INFO FOR DB").await.expect("info").take(0).expect("decodes");
    let Value::Object(info) = info else { unreachable!("an object") };
    let Some(Value::Object(tables)) = info.get("tables") else { unreachable!("tables") };
    let mut out = String::new();
    for table in tables.keys() {
        let all: Vec<Value> = rows(db, &format!("SELECT * FROM {table}")).await;
        out.push_str(&format!("{all:?}"));
    }
    out
}

// ── a session's lines, as capture keeps them ───────────────────────────────

/// An agent and one of its sessions, as capture registers them, and the
/// lines it captures from now on, a tenth of a second apart.
pub struct Lines<'k> {
    pub kernel: &'k Kernel,
    pub agent: RecordId,
    pub session: RecordId,
    pub key: String,
    pub at: chrono::DateTime<chrono::Utc>,
    calls: u32,
}

impl<'k> Lines<'k> {
    pub async fn new(kernel: &'k Kernel, key: &str) -> Lines<'k> {
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let mut d = superx_kernel::types::Object::new();
        d.insert("name".to_string(), Value::String("claude_code".to_string()));
        kernel.supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("agent descriptor");
        let session = kernel.create_entity("node_session").await.expect("session");
        let mut d = superx_kernel::types::Object::new();
        d.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
        d.insert("session".to_string(), Value::String(key.to_string()));
        kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(d)).await.expect("session descriptor");
        Lines { kernel, agent, session, key: key.to_string(), at: chrono::Utc::now() - chrono::TimeDelta::hours(1), calls: 0 }
    }

    /// One line, captured now as a message row.
    pub async fn line(&mut self, json: serde_json::Value) {
        self.at = (self.at + chrono::TimeDelta::milliseconds(100)).max(chrono::Utc::now() - chrono::TimeDelta::seconds(30));
        let at = self.at;
        let mut json = json;
        json["timestamp"] = serde_json::Value::String(at.to_rfc3339());
        json["sessionId"] = serde_json::Value::String(self.key.clone());
        let stamp = uuid::Timestamp::from_unix(uuid::NoContext, u64::try_from(at.timestamp()).expect("after 1970"), at.timestamp_subsec_nanos());
        let id = RecordId::new("message", superx_kernel::types::Uuid::from(uuid::Uuid::new_v7(stamp)));
        self.kernel
            .db()
            .query(
                "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw: $raw, raw_text: $text, \
                 emitted_at: $at, valid_from: $at }",
            )
            .bind(("id", id))
            .bind(("sess", self.session.clone()))
            .bind(("agent", self.agent.clone()))
            .bind(("raw", Value::Object(superx_kernel::message::line_to_object(&json))))
            .bind(("text", serde_json::to_string(&json).expect("json")))
            .bind(("at", at))
            .await
            .expect("line")
            .check()
            .expect("line taken");
    }

    /// The operator working in `dir`.
    pub async fn worked_in(&mut self, dir: &Path) {
        self.line(serde_json::json!({ "type": "user", "cwd": dir, "origin": { "kind": "human" }, "message": { "role": "user", "content": "work here" } }))
            .await;
    }

    /// A shell command the agent ran in `dir`, and what it printed.
    pub async fn ran(&mut self, dir: &Path, command: &str, printed: &str) {
        self.calls += 1;
        let id = format!("call_{}_{}", self.key, self.calls);
        let message = format!("msg_{}_{}", self.key, self.calls);
        self.line(serde_json::json!({
            "type": "assistant", "cwd": dir, "gitBranch": "main",
            "message": { "id": message, "model": "claude-opus-5-5", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "id": id, "name": "Bash", "input": { "command": command } }],
                "usage": { "input_tokens": 1, "output_tokens": 1 } }
        }))
        .await;
        self.line(serde_json::json!({
            "type": "user", "cwd": dir,
            "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": id, "content": printed, "is_error": false }] },
            "toolUseResult": { "stdout": printed, "stderr": "", "interrupted": false }
        }))
        .await;
    }

    /// The agent pushing a new branch from `dir`: the crawler goes back to
    /// its repository, and no commit is printed.
    pub async fn pushed(&mut self, dir: &Path) {
        self.ran(dir, "git push origin topic", "To github.com:acme/widgets.git\n * [new branch]      topic -> topic\n").await;
    }

    /// The agent committing in `dir`: git printed the commit's short hash.
    pub async fn committed(&mut self, dir: &Path, hash: &str, subject: &str) {
        self.ran(dir, &format!("git commit -m '{subject}'"), &format!("[main {}] {subject}\n 1 file changed", &hash[..7])).await;
    }
}

/// git in `dir` where it may fail, as a conflict does: whether it ended well.
pub fn git_may_fail(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test Person", "-c", "user.email=test@example.test", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_EDITOR", "true")
        .envs(env.iter().copied())
        .output()
        .expect("git runs")
        .status
        .success()
}
