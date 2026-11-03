//! Every call to a host (#538, step 15): a GET through reqwest with
//! rustls, at most one at a time per host across every worker, cut off at
//! `attr_recon_host_timeout_ms`, with the host's own headers and its token,
//! which goes to that host's address alone. A redirect is followed only
//! within the host. Each answer's rate limit is read; a call refused for
//! the rate waits and is asked again; a call made while the host's newest
//! answer said none of its limit is left waits for its reset first. Every
//! call is a `host_call` row, the path and query asked, never a host or a
//! token.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone as _, Utc};
use reqwest::header::{HeaderMap, ACCEPT, ETAG, IF_NONE_MATCH, LINK, RETRY_AFTER, USER_AGENT};
use reqwest::Url;
use superx_kernel::types::RecordId;
use tokio::sync::Mutex;

use crate::credentials::Token;
use crate::crawler::hosts::{Endpoints, Host};
use crate::crawler::progress::Watch;
use crate::crawler::store::{Batch, Row};
use crate::crawler::Stop;

/// What the crawler says it is to a host.
const USER_AGENT_NAME: &str = "superx-recon"; // skill-allow: §9-const — the crawler's own name, not a policy tunable

/// GitHub's media type, and the API version whose pull requests hold
/// `merge_commit_sha`, served until 2028-03-10 (step 15).
const GITHUB_ACCEPT: &str = "application/vnd.github+json"; // skill-allow: §9-const — the host's own header, not a policy tunable
const GITHUB_VERSION_HEADER: &str = "X-GitHub-Api-Version"; // skill-allow: §9-const — the host's own header, not a policy tunable
const GITHUB_VERSION: &str = "2022-11-28"; // skill-allow: §9-const — the host's own API version, not a policy tunable

/// Bitbucket's media type.
const BITBUCKET_ACCEPT: &str = "application/json"; // skill-allow: §9-const — the host's own header, not a policy tunable

/// How long a refusal for the rate waits when the host says neither how
/// long nor when its limit resets: a minute (step 15).
pub const UNSAID_WAIT: Duration = Duration::from_secs(60); // skill-allow: §9-duration — the hosts' own minute, not a policy tunable

/// Redirects followed at most, within the host.
const REDIRECTS: usize = 10; // skill-allow: §9-const — redirect bound, not a policy tunable

/// One host's gate: one call at a time across every worker, and its
/// newest answer's rate limit.
#[derive(Debug, Default)]
pub struct Gate {
    rate: Mutex<Rate>,
}

/// The rate limit as a host's newest answer said it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rate {
    pub remaining: Option<i64>,
    pub reset: Option<DateTime<Utc>>,
}

/// The client every worker of a pass calls the hosts through.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoints: Endpoints,
    timeout: Duration,
    gates: Arc<HashMap<Host, Arc<Gate>>>,
}

impl Client {
    /// A client cut off at `timeout_ms`, one gate per host.
    ///
    /// # Errors
    ///
    /// reqwest's reason, should it refuse to build one.
    pub fn new(endpoints: Endpoints, timeout_ms: u64) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let within = attempt.previous().last().is_some_and(|from| same_origin(from, attempt.url()));
                if within && attempt.previous().len() <= REDIRECTS {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|e| format!("the hosts' client: {e}"))?;
        let gates = [Host::GitHub, Host::Bitbucket].into_iter().map(|h| (h, Arc::new(Gate::default()))).collect();
        Ok(Self { http, endpoints, timeout: Duration::from_millis(timeout_ms), gates: Arc::new(gates) })
    }

    /// The URL of `path` on `host`'s API.
    #[must_use]
    pub fn url(&self, host: Host, path: &str) -> String {
        format!("{}{path}", self.endpoints.api(host))
    }

    fn gate(&self, host: Host) -> Arc<Gate> {
        self.gates.get(&host).cloned().unwrap_or_default()
    }
}

/// Whether two URLs share a scheme, a host and a port.
#[must_use]
pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default()
}

/// One answer, as the host gave it.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// The path and query asked, never the host or a token.
    pub path: String,
    pub started_at: DateTime<Utc>,
    pub ms: u64,
    /// NONE when no answer came.
    pub status: Option<u16>,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub rate_limit: Option<i64>,
    pub rate_remaining: Option<i64>,
    pub rate_reset: Option<DateTime<Utc>>,
    pub retry_after: Option<Duration>,
    /// The URL GitHub's link header names `rel="next"`.
    pub link_next: Option<String>,
    /// What went wrong before an answer came.
    pub transport: Option<String>,
    pub timed_out: bool,
}

impl Answer {
    /// The body, as text.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn refused(path: String, why: String) -> Self {
        Self { path, started_at: Utc::now(), transport: Some(why), ..Self::default() }
    }
}

/// The path and query of `url`, as a `host_call` row says it.
#[must_use]
pub fn path_of(url: &Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    }
}

/// Whether an answer refuses the call for the rate: HTTP 429, or HTTP 403
/// with none of the limit left, a retry-after, or a message that says a
/// rate limit was exceeded.
#[must_use]
pub fn refused_for_rate(a: &Answer) -> bool {
    match a.status {
        Some(429) => true,
        Some(403) => {
            let said = a.text().to_lowercase();
            a.rate_remaining == Some(0) || a.retry_after.is_some() || (said.contains("rate limit") && said.contains("exceeded"))
        }
        _ => false,
    }
}

/// How long a refusal for the rate waits: retry-after when the answer gives
/// it, else until the reset when none of the limit is left, else a minute.
#[must_use]
pub fn rate_wait(a: &Answer, now: DateTime<Utc>) -> Duration {
    if let Some(after) = a.retry_after {
        return after;
    }
    match (a.rate_remaining, a.rate_reset) {
        (Some(0), Some(reset)) => (reset - now).to_std().unwrap_or(Duration::ZERO),
        _ => UNSAID_WAIT,
    }
}

/// What a host's newest answer makes the next call wait first: until its
/// reset, when it said none of its limit is left.
#[must_use]
pub fn wait_first(rate: Rate, now: DateTime<Utc>) -> Option<Duration> {
    match (rate.remaining, rate.reset) {
        (Some(0), Some(reset)) if reset > now => (reset - now).to_std().ok(),
        _ => None,
    }
}

/// One GET of `url` with `host`'s headers and `token`, no more: the gate
/// and the waits are the caller's.
async fn send(client: &Client, host: Host, url: &Url, token: &Token, etag: Option<&str>) -> Answer {
    let started_at = Utc::now();
    let clock = Instant::now();
    let path = path_of(url);
    let mut request = client
        .http
        .get(url.clone())
        .timeout(client.timeout)
        .header(USER_AGENT, USER_AGENT_NAME)
        .bearer_auth(token.secret());
    request = match host {
        Host::GitHub => request.header(ACCEPT, GITHUB_ACCEPT).header(GITHUB_VERSION_HEADER, GITHUB_VERSION),
        Host::Bitbucket => request.header(ACCEPT, BITBUCKET_ACCEPT),
    };
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let ms = |clock: Instant| u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            let timed_out = e.is_timeout();
            return Answer { path, started_at, ms: ms(clock), transport: Some(e.to_string()), timed_out, ..Answer::default() };
        }
    };
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = match response.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            let timed_out = e.is_timeout();
            return Answer {
                path,
                started_at,
                ms: ms(clock),
                status: Some(status),
                transport: Some(e.to_string()),
                timed_out,
                ..Answer::default()
            };
        }
    };
    Answer {
        path,
        started_at,
        ms: ms(clock),
        status: Some(status),
        body,
        etag: header(&headers, ETAG.as_str()),
        rate_limit: header(&headers, "x-ratelimit-limit").and_then(|v| v.trim().parse().ok()),
        rate_remaining: header(&headers, "x-ratelimit-remaining").and_then(|v| v.trim().parse().ok()),
        rate_reset: header(&headers, "x-ratelimit-reset")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .and_then(|s| Utc.timestamp_opt(s, 0).single()),
        retry_after: header(&headers, RETRY_AFTER.as_str()).and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs),
        link_next: header(&headers, LINK.as_str()).and_then(|l| next_link(&l)),
        transport: None,
        timed_out: false,
    }
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// The URL a link header names `rel="next"`.
#[must_use]
pub fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        params
            .split(';')
            .any(|p| matches!(p.trim(), "rel=\"next\"" | "rel=next"))
            .then(|| url.trim().trim_start_matches('<').trim_end_matches('>').to_string())
    })
}

/// What a host crawl's calls added up to.
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

/// How a call ended, as the host crawl takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// HTTP 200, read.
    Read,
    /// HTTP 304: what the path held is unchanged.
    NotModified,
    /// HTTP 400, 403 or 404 after the repository's own: the host's answer
    /// for that path, no failure (step 15).
    HostsAnswer(u16),
    /// Refused for the rate, waited and asked again: no failure.
    Refused,
    /// Anything else: a transport error, a cut-off, another status, a
    /// redirect off the host, an answer of no shape the call reads.
    Failed,
    /// A stop came first.
    Stopped,
}

/// One call, as the host crawl reads it.
#[derive(Debug, Clone)]
pub struct Got {
    pub ended: Ended,
    pub answer: Answer,
}

impl Got {
    fn stopped() -> Self {
        Self { ended: Ended::Stopped, answer: Answer::default() }
    }
}

/// One host crawl's calls: its host, token and gate, the rows its calls
/// make, and what they added up to.
pub struct Calls {
    pub client: Client,
    pub host: Host,
    pub token: Token,
    /// The host crawl its rows name.
    pub crawl: RecordId,
    pub counts: Counts,
    /// The paths that failed, and those the host gave its answer to, each
    /// with its status, for the host crawl's note.
    pub failures: Vec<String>,
    pub answers: Vec<String>,
    /// Where each call and wait is reported as it happens (#539).
    pub watch: Option<Watch>,
}

impl Calls {
    #[must_use]
    pub fn new(client: Client, host: Host, token: Token, crawl: RecordId) -> Self {
        Self { client, host, token, crawl, counts: Counts::default(), failures: Vec::new(), answers: Vec::new(), watch: None }
    }

    /// GET `url` with `etag` as If-None-Match, waiting first when the
    /// host's newest answer said none of its limit is left, and waiting and
    /// asking again while the host refuses it for the rate, each refused
    /// call a row of its own. `first` marks the host repository's own call,
    /// whose HTTP 401, 403 or 404 is no host's answer. The call that ends
    /// it is the caller's to record ([`Calls::record`]).
    pub async fn get(&mut self, rows: &mut Batch, url: &str, etag: Option<&str>, first: bool, stop: &mut Stop) -> Got {
        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(e) => return Got { ended: Ended::Failed, answer: Answer::refused(url.to_string(), format!("not a URL: {e}")) },
        };
        let own = Url::parse(self.client.endpoints.api(self.host)).ok();
        if !own.as_ref().is_some_and(|api| same_origin(api, &parsed)) {
            // A token goes to its own host's address alone.
            let why = format!("not {}'s address: not asked", self.host.name());
            return Got { ended: Ended::Failed, answer: Answer::refused(path_of(&parsed), why) };
        }
        let gate = self.client.gate(self.host);
        loop {
            let mut rate = gate.rate.lock().await;
            if let Some(wait) = wait_first(*rate, Utc::now()) {
                drop(rate);
                if !self.wait(wait, stop).await {
                    return Got::stopped();
                }
                continue;
            }
            let Some(answer) = stop.unless(send(&self.client, self.host, &parsed, &self.token, etag)).await else {
                return Got::stopped();
            };
            if answer.status.is_some() && answer.rate_remaining.is_some() {
                *rate = Rate { remaining: answer.rate_remaining, reset: answer.rate_reset };
            }
            drop(rate);
            if refused_for_rate(&answer) {
                let wait = rate_wait(&answer, Utc::now());
                let mut refused = Got { ended: Ended::Refused, answer };
                self.record(rows, &mut refused, 0, false, false);
                if !self.wait(wait, stop).await {
                    return Got::stopped();
                }
                continue;
            }
            let ended = match answer.status {
                Some(200) if answer.transport.is_none() => Ended::Read,
                Some(304) => Ended::NotModified,
                Some(s @ (400 | 403 | 404)) if !first => Ended::HostsAnswer(s),
                _ => Ended::Failed,
            };
            return Got { ended, answer };
        }
    }

    /// Wait `wait`, counted; false when a stop ended it.
    async fn wait(&mut self, wait: Duration, stop: &mut Stop) -> bool {
        self.counts.rate_waits += 1;
        self.counts.waited_ms = self.counts.waited_ms.saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX));
        if let Some(w) = &self.watch {
            w.progress.host_wait(&w.common_dir, w.host.as_deref().unwrap_or_default());
        }
        stop.unless(tokio::time::sleep(wait)).await.is_some()
    }

    /// The `host_call` row of a call, and its count: `objects` is what its
    /// answer held, `next` whether a page followed, `unread` an answer HTTP
    /// 200 of no shape the call reads, which fails the call. The row's id.
    pub fn record(&mut self, rows: &mut Batch, got: &mut Got, objects: usize, next: bool, unread: bool) -> RecordId {
        if unread {
            self.counts.unread += 1;
            got.ended = Ended::Failed;
        }
        let a = &got.answer;
        let error = match got.ended {
            Ended::NotModified | Ended::Read | Ended::Stopped => None,
            _ if unread => Some(format!("an answer not of the shape asked: {}", a.text())),
            Ended::HostsAnswer(_) | Ended::Refused | Ended::Failed => match (&a.transport, a.status) {
                (Some(why), _) => Some(why.clone()),
                (None, Some(status)) => Some(format!("HTTP {status}: {}", a.text())),
                (None, None) => Some("no answer".to_string()),
            },
        };
        self.counts.calls += 1;
        self.counts.not_modified += i64::from(got.ended == Ended::NotModified);
        self.counts.timed_out += i64::from(a.timed_out);
        if let Some(w) = &self.watch {
            w.progress.host_call(&w.common_dir, w.host.as_deref().unwrap_or_default(), &a.path, got.ended == Ended::Failed);
        }
        match got.ended {
            Ended::Failed => {
                self.counts.failed += 1;
                let said = match (a.status, &a.transport) {
                    (Some(s), None) if !unread => format!("HTTP {s}"),
                    (_, Some(why)) => why.clone(),
                    _ if unread => "not of the shape asked".to_string(),
                    _ => "no answer".to_string(),
                };
                self.failures.push(format!("{}: {said}", a.path));
            }
            Ended::HostsAnswer(status) => self.answers.push(format!("{}: HTTP {status}", a.path)),
            Ended::Read | Ended::NotModified | Ended::Refused | Ended::Stopped => {}
        }
        rows.create(
            "host_call",
            Row::new()
                .link("host_crawl", &self.crawl)
                .text("method", "GET")
                .text("path", a.path.clone())
                .time("started_at", a.started_at)
                .int("ms", i64::try_from(a.ms).unwrap_or(i64::MAX))
                .some_int("status", a.status.map(i64::from))
                .flag("not_modified", got.ended == Ended::NotModified)
                .int("bytes", i64::try_from(a.body.len()).unwrap_or(i64::MAX))
                .some_text("etag", a.etag.clone())
                .some_int("rate_limit", a.rate_limit)
                .some_int("rate_remaining", a.rate_remaining)
                .some_time("rate_reset", a.rate_reset)
                .some_int("retry_after_ms", a.retry_after.map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)))
                .int("objects", i64::try_from(objects).unwrap_or(i64::MAX))
                .flag("next", next)
                .some_text("error", error),
        )
    }
}
