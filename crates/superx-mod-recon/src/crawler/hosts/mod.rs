//! The `hosts` source (#538): the hosts the repositories live on, GitHub and
//! Bitbucket, read through their APIs with GET only and the operator's
//! read-only tokens (decision http-client: reqwest over rustls). Every
//! answer is kept whole in `host_object`, and beside it the accounts, the
//! pull requests with their states and commits, and the CI runs with their
//! states.
//!
//! A host repository is crawled by the worker of the repository whose
//! remote names it, beside that repository's crawl, at most once a pass:
//! the first worker to reach it takes it ([`Pass::claim`]). Calls go to a
//! host one at a time across every worker ([`Lane`]), so the host's rate
//! limit is read and kept by one caller. What each host crawl did is a
//! `host_crawl` row, and the source's part of the pass a `pass_source` row
//! of its own.

pub mod call;
pub mod crawl;
pub mod objects;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Uuid, Value};
use superx_kernel::{Db, Result};

use crate::credentials::{self, Credentials, Token};
use crate::crawler::hosts::objects::Known;
use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::value::{at, text};

/// The source's name, as `pass_source` records it.
pub const SOURCE: &str = "hosts";

/// The variable that makes superx-recon git's askpass for a mirror's fetch,
/// naming the host whose token it answers with (step 23).
pub const ASKPASS_ENV: &str = "SUPERX_RECON_ASKPASS";

/// The variable that names the instance's home to the askpass, as
/// `superx-recon --home` takes it.
pub const HOME_ENV: &str = "SUPERX_HOME";

/// GitHub's API, as the host gives it.
const GITHUB_API: &str = "https://api.github.com"; // skill-allow: §9-const — the host's own address, not a policy tunable
/// Where GitHub serves a repository to git.
const GITHUB_GIT: &str = "https://github.com"; // skill-allow: §9-const — the host's own address, not a policy tunable
/// Bitbucket Cloud's API, its version 2.0.
const BITBUCKET_API: &str = "https://api.bitbucket.org/2.0"; // skill-allow: §9-const — the host's own address, not a policy tunable
/// Where Bitbucket serves a repository to git.
const BITBUCKET_GIT: &str = "https://bitbucket.org"; // skill-allow: §9-const — the host's own address, not a policy tunable

/// A host the crawler reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Host {
    GitHub,
    Bitbucket,
}

impl Host {
    /// The host by the name a remote's url gives it.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        match name {
            credentials::GITHUB => Some(Self::GitHub),
            credentials::BITBUCKET => Some(Self::Bitbucket),
            _ => None,
        }
    }

    /// Its name, as the structure and the credentials file write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => credentials::GITHUB,
            Self::Bitbucket => credentials::BITBUCKET,
        }
    }

    /// The username git gives a mirror's fetch (step 4): GitHub uses none
    /// to authenticate and needs one that is not empty; Bitbucket's for an
    /// API token.
    #[must_use]
    pub fn git_user(self) -> &'static str {
        match self {
            Self::GitHub => "x-access-token",
            Self::Bitbucket => "x-bitbucket-api-token-auth",
        }
    }

    /// Its token, when the credentials file gives it one.
    #[must_use]
    pub fn token(self, credentials: &Credentials) -> Option<Token> {
        match credentials {
            Credentials::Hosts(hosts) => match self {
                Self::GitHub => hosts.github.clone(),
                Self::Bitbucket => hosts.bitbucket.clone(),
            },
            _ => None,
        }
    }
}

/// Why a host has no token, as credentials.rs answers: the note of a host
/// crawl with no credentials.
#[must_use]
pub fn no_token_why(credentials: &Credentials) -> String {
    match credentials {
        Credentials::None => "no credentials file".to_string(),
        Credentials::Refused { mode } => format!("the credentials file is refused, mode {mode:03o}"),
        Credentials::Unreadable(why) => format!("the credentials file is unreadable: {why}"),
        Credentials::Hosts(_) => "no token".to_string(),
    }
}

/// Where the hosts are reached: their own addresses, or a test's server
/// on the loopback and its own repositories (step 15), given through the
/// crawler's arguments and never through a parameter or the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub github_api: String,
    pub github_git: String,
    pub bitbucket_api: String,
    pub bitbucket_git: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            github_api: GITHUB_API.to_string(),
            github_git: GITHUB_GIT.to_string(),
            bitbucket_api: BITBUCKET_API.to_string(),
            bitbucket_git: BITBUCKET_GIT.to_string(),
        }
    }
}

impl Endpoints {
    /// The host's API.
    #[must_use]
    pub fn api(&self, host: Host) -> &str {
        match host {
            Host::GitHub => &self.github_api,
            Host::Bitbucket => &self.bitbucket_api,
        }
    }

    /// The url a mirror of `path` on `host` is fetched from.
    #[must_use]
    pub fn git_url(&self, host: Host, path: &str) -> String {
        let base = match host {
            Host::GitHub => &self.github_git,
            Host::Bitbucket => &self.bitbucket_git,
        };
        format!("{base}/{path}.git")
    }
}

/// What a host's newest answer said of its rate limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rate {
    pub remaining: Option<i64>,
    pub reset: Option<DateTime<Utc>>,
}

/// One host's lane: its calls go one at a time across every worker, and
/// the lock holds what its newest answer said of its rate limit.
#[derive(Debug, Default)]
pub struct Lane {
    pub rate: tokio::sync::Mutex<Rate>,
}

/// What the source keeps from one pass to the next: each host's lane.
#[derive(Debug, Default, Clone)]
pub struct Memory {
    github: Arc<Lane>,
    bitbucket: Arc<Lane>,
}

impl Memory {
    #[must_use]
    pub fn lane(&self, host: Host) -> Arc<Lane> {
        match host {
            Host::GitHub => Arc::clone(&self.github),
            Host::Bitbucket => Arc::clone(&self.bitbucket),
        }
    }
}

/// A host's own id for an account or a repository: a number on GitHub, a
/// uuid on Bitbucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Int(i64),
    Text(String),
}

impl Key {
    /// The key a JSON value gives: a whole number or a text.
    #[must_use]
    pub fn of(v: &serde_json::Value) -> Option<Self> {
        match v {
            serde_json::Value::Number(n) => n.as_i64().map(Self::Int),
            serde_json::Value::String(s) if !s.is_empty() => Some(Self::Text(s.clone())),
            _ => None,
        }
    }

    /// The key a stored value gives.
    #[must_use]
    pub fn of_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.to_int().map(Self::Int),
            Value::String(s) => Some(Self::Text(s.clone())),
            _ => None,
        }
    }

    /// As the database keeps it.
    #[must_use]
    pub fn value(&self) -> Value {
        match self {
            Self::Int(i) => Value::Number(superx_kernel::types::Number::Int(*i)),
            Self::Text(s) => Value::String(s.clone()),
        }
    }
}

/// A repository on a host, as the database holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRepository {
    pub id: RecordId,
    pub host_id: Option<Key>,
}

/// A mirror, as the database holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirror {
    pub id: RecordId,
    pub folder: String,
}

/// What every worker of one pass shares of the hosts: the client, the
/// credentials as the pass found them, which host repositories are taken,
/// the accounts every host has named, and one write at a time.
#[derive(Debug)]
pub struct Pass {
    pub endpoints: Endpoints,
    pub credentials: Credentials,
    pub client: reqwest::Client,
    pub timeout: Duration,
    pub memory: Memory,
    claimed: Mutex<HashSet<(Host, String)>>,
    repositories: Mutex<HashMap<(Host, String), HostRepository>>,
    /// Mirrors by their host repository's UUID: a map keyed by a record id
    /// is one clippy refuses (`mutable_key_type`).
    mirrors: Mutex<HashMap<Uuid, Mirror>>,
    users: Mutex<HashMap<(Host, Key), RecordId>>,
    written_users: Mutex<HashSet<Uuid>>,
    /// What the crawler holds of each host repository, by its UUID: taken
    /// by the worker that takes it, given back once its rows are written.
    known: Mutex<HashMap<Uuid, Known>>,
    /// Workers write one at a time: an account two workers met is written
    /// by the first and only linked by the second.
    pub write: tokio::sync::Mutex<()>,
}

impl Pass {
    /// The hosts as the pass begins: the client, and what the database
    /// holds of the repositories on hosts, their mirrors and the accounts.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer, or
    /// the client cannot be built.
    pub async fn begin(
        db: &Db,
        endpoints: Endpoints,
        credentials: Credentials,
        timeout_ms: u64,
        memory: Memory,
        known: HashMap<Uuid, Known>,
    ) -> Result<Self> {
        let client = call::client().map_err(|e| superx_kernel::KernelError::Config(format!("the hosts' client: {e}")))?;
        let mut res = db
            .query(
                "SELECT id, host, path, host_id FROM host_repository;\
                 SELECT id, host_repository, folder FROM mirror;\
                 SELECT id, host, host_id FROM host_user;",
            )
            .await?;
        let repositories: Vec<Value> = res.take(0)?;
        let mirrors: Vec<Value> = res.take(1)?;
        let users: Vec<Value> = res.take(2)?;
        let record = |v: &Value, key: &str| match at(v, &[key]) {
            Some(Value::RecordId(id)) => Some(id.clone()),
            _ => None,
        };
        let repositories = repositories
            .iter()
            .filter_map(|r| {
                let host = Host::of(text(r, &["host"])?)?;
                let path = text(r, &["path"])?.to_string();
                let host_id = at(r, &["host_id"]).and_then(Key::of_value);
                Some(((host, path), HostRepository { id: record(r, "id")?, host_id }))
            })
            .collect();
        let mirrors = mirrors
            .iter()
            .filter_map(|m| {
                let hr = record(m, "host_repository")?;
                Some((record_uuid_of(&hr)?, Mirror { id: record(m, "id")?, folder: text(m, &["folder"])?.to_string() }))
            })
            .collect();
        let users: HashMap<(Host, Key), RecordId> = users
            .iter()
            .filter_map(|u| Some(((Host::of(text(u, &["host"])?)?, at(u, &["host_id"]).and_then(Key::of_value)?), record(u, "id")?)))
            .collect();
        let written_users = users.values().filter_map(record_uuid_of).collect();
        Ok(Self {
            endpoints,
            credentials,
            client,
            timeout: Duration::from_millis(timeout_ms),
            memory,
            claimed: Mutex::new(HashSet::new()),
            repositories: Mutex::new(repositories),
            mirrors: Mutex::new(mirrors),
            users: Mutex::new(users),
            written_users: Mutex::new(written_users),
            known: Mutex::new(known),
            write: tokio::sync::Mutex::new(()),
        })
    }

    /// Take a host repository for this pass: true for the first worker to
    /// reach it, false for every other.
    #[must_use]
    pub fn claim(&self, host: Host, path: &str) -> bool {
        self.claimed.lock().is_ok_and(|mut c| c.insert((host, path.to_string())))
    }

    /// What the database holds of a host repository.
    #[must_use]
    pub fn repository(&self, host: Host, path: &str) -> Option<HostRepository> {
        self.repositories.lock().ok()?.get(&(host, path.to_string())).cloned()
    }

    /// A host repository made in this pass, under the id its row will have.
    pub fn made(&self, host: Host, path: &str, id: RecordId) {
        if let Ok(mut r) = self.repositories.lock() {
            r.entry((host, path.to_string())).or_insert(HostRepository { id, host_id: None });
        }
    }

    /// The mirror the database holds of a host repository.
    #[must_use]
    pub fn mirror(&self, host_repository: &RecordId) -> Option<Mirror> {
        self.mirrors.lock().ok()?.get(&record_uuid_of(host_repository)?).cloned()
    }

    /// A mirror whose row is made in this pass.
    pub fn mirror_made(&self, host_repository: &RecordId, mirror: Mirror) {
        if let (Ok(mut m), Some(key)) = (self.mirrors.lock(), record_uuid_of(host_repository)) {
            m.entry(key).or_insert(mirror);
        }
    }

    /// The account a host names by `key`: its row's id, the one every
    /// worker of the pass gives it.
    #[must_use]
    pub fn user(&self, host: Host, key: &Key) -> RecordId {
        let Ok(mut users) = self.users.lock() else {
            return crate::crawler::store::new_id("host_user");
        };
        users.entry((host, key.clone())).or_insert_with(|| crate::crawler::store::new_id("host_user")).clone()
    }

    /// Whether an account's row is written already: a worker that made it
    /// too leaves it out.
    #[must_use]
    pub fn user_written(&self, id: &RecordId) -> bool {
        record_uuid_of(id).is_some_and(|k| self.written_users.lock().is_ok_and(|w| w.contains(&k)))
    }

    /// Accounts a worker's write has written.
    pub fn users_written(&self, ids: impl IntoIterator<Item = RecordId>) {
        if let Ok(mut w) = self.written_users.lock() {
            w.extend(ids.into_iter().filter_map(|id| record_uuid_of(&id)));
        }
    }

    /// What the crawler holds of a host repository, taken by the worker
    /// that crawls it.
    #[must_use]
    pub fn take_known(&self, host_repository: &RecordId) -> Option<Known> {
        let key = record_uuid_of(host_repository)?;
        self.known.lock().ok()?.remove(&key)
    }

    /// What no worker took in the pass, kept for the next.
    #[must_use]
    pub fn leftover_known(&self) -> Vec<(Uuid, Known)> {
        self.known.lock().map(|mut k| k.drain().collect()).unwrap_or_default()
    }
}

/// What one host crawl did, for the source's record (step 21).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Crawled {
    pub host: Option<Host>,
    pub path: String,
    pub outcome: &'static str,
    pub note: Option<String>,
    pub calls: i64,
    pub failed: i64,
    pub timed_out: i64,
    pub unread: i64,
    pub not_modified: i64,
    pub rate_waits: i64,
    pub waited_ms: i64,
    pub objects_new: i64,
    pub objects_changed: i64,
    pub totals_stated: i64,
    pub totals_read: i64,
}
