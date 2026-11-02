//! The calls to a host (#538, step 15): a GET each, one at a time per host
//! across every worker, cut off at `attr_recon_host_timeout_ms`, with the
//! host's own headers and its token, which goes to its own host alone. A
//! list is paged as the host gives it; an etag a complete host crawl was
//! given is sent back, and an answer HTTP 304 is read as unchanged; a call
//! refused for the rate waits as the host says and is asked again. Every
//! call is a `host_call` row.

use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone as _, Utc};
use reqwest::header::{HeaderMap, ACCEPT, ETAG, IF_NONE_MATCH, LINK, RETRY_AFTER, USER_AGENT};
use reqwest::{redirect, StatusCode, Url};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use crate::credentials::Token;
use crate::crawler::hosts::{Host, Lane, Pass, Rate};
use crate::crawler::store::{Row, Rows};
use crate::crawler::value::{at, text};
use crate::crawler::Stop;

/// What GitHub's API answers in.
const GITHUB_ACCEPT: &str = "application/vnd.github+json"; // skill-allow: §9-const — the host's own header, not a policy tunable
/// The version of GitHub's API asked: its pull requests hold
/// merge_commit_sha, which 2026-03-10 removed; served until 2028-03-10.
const GITHUB_API_VERSION: &str = "2022-11-28"; // skill-allow: §9-const — the host's own header, not a policy tunable
/// What Bitbucket's API answers in.
const BITBUCKET_ACCEPT: &str = "application/json"; // skill-allow: §9-const — the host's own header, not a policy tunable
/// Who calls.
const AGENT: &str = "superx-recon";
/// The most a GitHub list takes a page.
pub const GITHUB_PAGE: u32 = 100; // skill-allow: §9-const — the host's own page size, not a policy tunable
/// The most a Bitbucket list takes a page: its pull requests' list refuses more.
pub const BITBUCKET_PAGE: u32 = 50; // skill-allow: §9-const — the host's own page size, not a policy tunable
/// How long a call refused for the rate waits when the host says neither
/// when to ask again nor when its limit resets.
pub const RATE_WAIT: Duration = Duration::from_secs(60); // skill-allow: §9-duration — the hosts' own guidance, not a policy tunable

/// The client every call goes through: a redirect is followed only when it
/// keeps the scheme, the host and the port, and no proxy sits between the
/// crawler and a host.
///
/// # Errors
///
/// reqwest's, when the client cannot be built.
pub fn client() -> std::result::Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(redirect::Policy::custom(|attempt| {
            let same = attempt.previous().last().is_some_and(|before| {
                before.scheme() == attempt.url().scheme()
                    && before.host_str() == attempt.url().host_str()
                    && before.port_or_known_default() == attempt.url().port_or_known_default()
            });
            if same {
                attempt.follow()
            } else {
                let to = format!("{}://{}", attempt.url().scheme(), attempt.url().host_str().unwrap_or_default());
                attempt.error(format!("a redirect to another host is not followed: {to}"))
            }
        }))
        .no_proxy()
        .build()
}

/// How an answer holds its objects (step 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// The answer is one object.
    Object,
    /// A GitHub array.
    Array,
    /// The one list a GitHub answer wraps beside its total_count.
    Wrapped(&'static str),
    /// A Bitbucket page: its values, its next, its size.
    Page,
}

/// One answer, read.
#[derive(Debug, Clone)]
pub struct Got {
    /// Its `host_call` row.
    pub call: RecordId,
    pub status: Option<u16>,
    pub not_modified: bool,
    /// The objects it held.
    pub objects: Vec<serde_json::Value>,
    /// The total the host states beside a list.
    pub total: Option<i64>,
    /// The next page's url, as the host gave it.
    pub next: Option<String>,
    /// HTTP 200.
    pub ok: bool,
    /// What went wrong; none for HTTP 200 and HTTP 304.
    pub error: Option<String>,
    /// The answer's body, as text.
    pub body: String,
}

/// What one host crawl's calls came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub calls: i64,
    pub not_modified: i64,
    pub failed: i64,
    pub timed_out: i64,
    pub rate_waits: i64,
    pub waited_ms: i64,
    pub unread: i64,
}

/// What the newest earlier answer to a path said.
#[derive(Debug, Clone)]
struct Before {
    etag: String,
    /// The path the call after it asked, when a page followed it.
    next: Option<String>,
}

/// The calls of one host crawl.
pub struct Caller<'p> {
    pass: &'p Pass,
    host: Host,
    token: Token,
    lane: std::sync::Arc<Lane>,
    host_crawl: RecordId,
    pub counts: Counts,
}

impl<'p> Caller<'p> {
    #[must_use]
    pub fn new(pass: &'p Pass, host: Host, token: Token, host_crawl: RecordId) -> Self {
        Self { pass, host, token, lane: pass.memory.lane(host), host_crawl, counts: Counts::default() }
    }

    /// The url of `path` on the host's API.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.pass.endpoints.api(self.host))
    }

    /// The last call answered as its step says is no failure (step 17: an
    /// unprotected default branch).
    pub fn forgive(&mut self) {
        self.counts.failed = (self.counts.failed - 1).max(0);
    }

    /// GET `url` until it is answered, or refused for good: a refusal for
    /// the rate waits and asks again. `None` when a stop came first.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer.
    pub async fn get(&mut self, db: &Db, rows: &mut Rows, url: &str, shape: Shape, stop: &mut Stop) -> Result<Option<Got>> {
        let Ok(parsed) = Url::parse(url) else {
            return Ok(Some(self.record(rows, Answered::refused(url, format!("not a url: {url}")), shape, false)));
        };
        let base = Url::parse(self.pass.endpoints.api(self.host)).ok();
        if base.as_ref().is_none_or(|b| b.origin() != parsed.origin()) {
            // A token goes to its own host's address alone.
            return Ok(Some(self.record(rows, Answered::refused(url, format!("not the host's address: {url}")), shape, false)));
        }
        let path = path_of(&parsed);
        let before = earlier(db, &path).await?;
        loop {
            let Some(answered) = stop.unless(self.ask(&parsed, &path, before.as_ref().map(|b| b.etag.as_str()), stop.clone())).await else {
                return Ok(None);
            };
            let Some(answered) = answered else { return Ok(None) };
            let wait = refused_for_rate(&answered).then(|| rate_wait(&answered, Utc::now()));
            let mut got = self.record(rows, answered, shape, wait.is_some());
            if got.not_modified {
                got.next = before.as_ref().and_then(|b| b.next.as_ref()).map(|p| self.origin_url(p));
            }
            match wait {
                Some(wait) => {
                    if !self.wait(wait, stop).await {
                        return Ok(None);
                    }
                }
                None => return Ok(Some(got)),
            }
        }
    }

    fn origin_url(&self, path: &str) -> String {
        let api = self.pass.endpoints.api(self.host);
        match Url::parse(api) {
            Ok(u) => format!("{}{path}", u.origin().ascii_serialization()),
            Err(_) => format!("{api}{path}"),
        }
    }

    /// Wait out a refusal for the rate, counted; false when a stop ended
    /// it.
    async fn wait(&mut self, wait: Duration, stop: &mut Stop) -> bool {
        self.counts.rate_waits += 1;
        let began = Instant::now();
        let waited = stop.unless(tokio::time::sleep(wait)).await.is_some();
        self.counts.waited_ms += i64::try_from(began.elapsed().as_millis()).unwrap_or(i64::MAX);
        waited
    }

    /// One GET, in the host's lane: a call made while the host's newest
    /// answer said none of its limit is left waits until it resets first.
    async fn ask(&mut self, url: &Url, path: &str, etag: Option<&str>, mut stop: Stop) -> Option<Answered> {
        let lane = std::sync::Arc::clone(&self.lane);
        let mut rate = lane.rate.lock().await;
        if let (Some(0), Some(reset)) = (rate.remaining, rate.reset) {
            let wait = (reset - Utc::now()).to_std().unwrap_or_default();
            if !wait.is_zero() && !self.wait(wait, &mut stop).await {
                return None;
            }
        }
        let started_at = Utc::now();
        let clock = Instant::now();
        let mut request = self
            .pass
            .client
            .get(url.clone())
            .timeout(self.pass.timeout)
            .header(USER_AGENT, AGENT)
            .bearer_auth(self.token.secret());
        request = match self.host {
            Host::GitHub => request.header(ACCEPT, GITHUB_ACCEPT).header("X-GitHub-Api-Version", GITHUB_API_VERSION),
            Host::Bitbucket => request.header(ACCEPT, BITBUCKET_ACCEPT),
        };
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let answered = match request.send().await {
            Ok(response) => {
                let status = response.status();
                let headers = response.headers().clone();
                match response.bytes().await {
                    Ok(body) => Answered::of(path, started_at, clock.elapsed(), status, &headers, body.to_vec()),
                    Err(e) => Answered::failed(path, started_at, clock.elapsed(), Some(status), &e),
                }
            }
            Err(e) => Answered::failed(path, started_at, clock.elapsed(), None, &e),
        };
        if answered.status.is_some() {
            *rate = Rate { remaining: answered.rate_remaining, reset: answered.rate_reset };
        }
        Some(answered)
    }

    /// The call's row, and its objects as `shape` reads them; an answer
    /// that is not JSON, or not of the shape, is unread and its call failed.
    /// A refusal for the rate is waited and asked again: a wait, counted as
    /// one, and no failed call.
    fn record(&mut self, rows: &mut Rows, answered: Answered, shape: Shape, refused_for_rate: bool) -> Got {
        let ok = answered.status == Some(StatusCode::OK.as_u16());
        let not_modified = answered.status == Some(StatusCode::NOT_MODIFIED.as_u16());
        let mut error = answered.error.clone();
        let mut objects = Vec::new();
        let mut total = None;
        let mut next = None;
        if ok {
            match read(&answered.body, shape, &answered.headers) {
                Some(read) => {
                    objects = read.objects;
                    total = read.total;
                    next = read.next;
                }
                None => {
                    self.counts.unread += 1;
                    error = Some(format!("an answer not of the shape asked: {}", String::from_utf8_lossy(&answered.body)));
                }
            }
        }
        let failed = error.is_some();
        self.counts.calls += 1;
        self.counts.failed += i64::from(failed && !refused_for_rate);
        self.counts.timed_out += i64::from(answered.timed_out);
        self.counts.not_modified += i64::from(not_modified);
        let call = rows.push(
            "host_call",
            Row::new()
                .link("host_crawl", &self.host_crawl)
                .text("method", "GET")
                .text("path", answered.path.clone())
                .time("started_at", answered.started_at)
                .int("ms", i64::try_from(answered.ms).unwrap_or(i64::MAX))
                .some_int("status", answered.status.map(i64::from))
                .flag("not_modified", not_modified)
                .int("bytes", i64::try_from(answered.body.len()).unwrap_or(i64::MAX))
                .some_text("etag", answered.etag.clone())
                .some_int("rate_limit", answered.rate_limit)
                .some_int("rate_remaining", answered.rate_remaining)
                .some_time("rate_reset", answered.rate_reset)
                .some_int("retry_after_ms", answered.retry_after.map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)))
                .int("objects", i64::try_from(objects.len()).unwrap_or(i64::MAX))
                .flag("next", next.is_some())
                .some_text("error", error.clone()),
        );
        Got {
            call,
            status: answered.status,
            not_modified,
            objects,
            total,
            next,
            ok: ok && !failed,
            error,
            body: String::from_utf8_lossy(&answered.body).into_owned(),
        }
    }
}

/// The path and query of a url, as a `host_call` row keeps it.
#[must_use]
pub fn path_of(url: &Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    }
}

/// The newest earlier answer to `path`, when it gave an etag in a host
/// crawl that ended complete: the etag, and the path asked after it when a
/// page followed.
async fn earlier(db: &Db, path: &str) -> Result<Option<Before>> {
    let calls: Vec<Value> = db
        .query("SELECT etag, next, started_at, host_crawl, host_crawl.outcome AS outcome, valid_from FROM host_call WHERE path = $path")
        .bind(("path", path.to_string()))
        .await?
        .take(0)?;
    let newest = calls.iter().filter_map(|c| Some((time(c, "valid_from")?, c))).max_by_key(|(t, _)| *t).map(|(_, c)| c);
    let Some(newest) = newest else { return Ok(None) };
    let (Some(etag), Some("complete")) = (text(newest, &["etag"]), text(newest, &["outcome"])) else { return Ok(None) };
    let mut next = None;
    if matches!(at(newest, &["next"]), Some(Value::Bool(true))) {
        if let (Some(Value::RecordId(crawl)), Some(started)) = (at(newest, &["host_crawl"]), time(newest, "started_at")) {
            let after: Vec<Value> = db
                .query("SELECT path, started_at FROM host_call WHERE host_crawl = $crawl")
                .bind(("crawl", crawl.clone()))
                .await?
                .take(0)?;
            next = after
                .iter()
                .filter_map(|c| Some((time(c, "started_at")?, text(c, &["path"])?)))
                .filter(|(t, _)| *t > started)
                .min_by_key(|(t, _)| *t)
                .map(|(_, p)| p.to_string());
        }
    }
    Ok(Some(Before { etag: etag.to_string(), next }))
}

fn time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match at(v, &[key])? {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}

/// One answer as it came, before it is read.
#[derive(Debug, Clone)]
pub struct Answered {
    pub path: String,
    pub started_at: DateTime<Utc>,
    pub ms: u128,
    pub status: Option<u16>,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub rate_limit: Option<i64>,
    pub rate_remaining: Option<i64>,
    pub rate_reset: Option<DateTime<Utc>>,
    pub retry_after: Option<Duration>,
    pub timed_out: bool,
    pub error: Option<String>,
}

impl Answered {
    fn of(path: &str, started_at: DateTime<Utc>, took: Duration, status: StatusCode, headers: &HeaderMap, body: Vec<u8>) -> Self {
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).map(str::to_string);
        let number = |name: &str| header(name).and_then(|v| v.parse::<i64>().ok());
        let code = status.as_u16();
        let error = (code != StatusCode::OK.as_u16() && code != StatusCode::NOT_MODIFIED.as_u16())
            .then(|| format!("HTTP {code}: {}", String::from_utf8_lossy(&body)));
        Self {
            path: path.to_string(),
            started_at,
            ms: took.as_millis(),
            status: Some(code),
            headers: headers.clone(),
            etag: header(ETAG.as_str()),
            rate_limit: number("x-ratelimit-limit"),
            rate_remaining: number("x-ratelimit-remaining"),
            rate_reset: number("x-ratelimit-reset").and_then(|s| Utc.timestamp_opt(s, 0).single()),
            retry_after: header(RETRY_AFTER.as_str()).and_then(|v| v.parse::<u64>().ok()).map(Duration::from_secs),
            body,
            timed_out: false,
            error,
        }
    }

    fn failed(path: &str, started_at: DateTime<Utc>, took: Duration, status: Option<StatusCode>, e: &reqwest::Error) -> Self {
        Self {
            path: path.to_string(),
            started_at,
            ms: took.as_millis(),
            status: status.map(|s| s.as_u16()),
            headers: HeaderMap::new(),
            body: Vec::new(),
            etag: None,
            rate_limit: None,
            rate_remaining: None,
            rate_reset: None,
            retry_after: None,
            timed_out: e.is_timeout(),
            error: Some(error_text(e)),
        }
    }

    fn refused(url: &str, why: String) -> Self {
        Self {
            path: Url::parse(url).map(|u| path_of(&u)).unwrap_or_default(),
            started_at: Utc::now(),
            ms: 0,
            status: None,
            headers: HeaderMap::new(),
            body: Vec::new(),
            etag: None,
            rate_limit: None,
            rate_remaining: None,
            rate_reset: None,
            retry_after: None,
            timed_out: false,
            error: Some(why),
        }
    }
}

/// A transport error, whole, with every cause it carries.
fn error_text(e: &reqwest::Error) -> String {
    let mut said = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        said.push_str(": ");
        said.push_str(&cause.to_string());
        source = cause.source();
    }
    said
}

/// A call refused for the rate: HTTP 429, or HTTP 403 with none of the
/// limit left, a retry-after, or a message that says a rate limit was
/// exceeded.
#[must_use]
pub fn refused_for_rate(a: &Answered) -> bool {
    match a.status {
        Some(429) => true,
        Some(403) => {
            a.rate_remaining == Some(0)
                || a.retry_after.is_some()
                || String::from_utf8_lossy(&a.body).to_lowercase().contains("rate limit")
        }
        _ => false,
    }
}

/// How long a call refused for the rate waits: retry-after when the answer
/// gives it; else until the limit resets when none of it is left; else a
/// minute.
#[must_use]
pub fn rate_wait(a: &Answered, now: DateTime<Utc>) -> Duration {
    if let Some(after) = a.retry_after {
        return after;
    }
    if let (Some(0), Some(reset)) = (a.rate_remaining, a.rate_reset) {
        return (reset - now).to_std().unwrap_or_default();
    }
    RATE_WAIT
}

/// What an answer held, as its shape reads.
struct Read {
    objects: Vec<serde_json::Value>,
    total: Option<i64>,
    next: Option<String>,
}

fn read(body: &[u8], shape: Shape, headers: &HeaderMap) -> Option<Read> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    let link_next = || headers.get(LINK).and_then(|v| v.to_str().ok()).and_then(next_link);
    match shape {
        Shape::Object => json.is_object().then(|| Read { objects: vec![json], total: None, next: None }),
        Shape::Array => match json {
            serde_json::Value::Array(items) => Some(Read { objects: items, total: None, next: link_next() }),
            _ => None,
        },
        Shape::Wrapped(key) => {
            let total = json.get("total_count").and_then(serde_json::Value::as_i64);
            match json.get(key) {
                Some(serde_json::Value::Array(items)) => Some(Read { objects: items.clone(), total, next: link_next() }),
                _ => None,
            }
        }
        Shape::Page => {
            let items = match json.get("values") {
                Some(serde_json::Value::Array(items)) => items.clone(),
                _ => return None,
            };
            Some(Read {
                objects: items,
                total: json.get("size").and_then(serde_json::Value::as_i64),
                next: json.get("next").and_then(serde_json::Value::as_str).map(str::to_string),
            })
        }
    }
}

/// The url a `link` header names `rel="next"`, as given.
#[must_use]
pub fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        let is_next = params.split(';').any(|p| {
            let p = p.trim();
            p == "rel=\"next\"" || p == "rel=next"
        });
        is_next.then(|| url.trim().trim_start_matches('<').trim_end_matches('>').to_string())
    })
}
