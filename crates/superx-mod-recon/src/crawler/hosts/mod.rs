//! The `hosts` source (#538): the repositories on GitHub and Bitbucket the
//! repositories' remotes name, read through the hosts' read APIs, GET only,
//! with the operator's tokens; every answer kept whole (`host_object`), and
//! beside it the accounts, the pull requests and the CI runs.
//!
//! A host repository is crawled by the worker of the repository that
//! reaches it first in a pass, beside that repository's crawl: its first
//! call and its mirror at step 4, its other calls once the repository's
//! crawl is done ([`crawl`]). Every call goes through [`call`], one at a
//! time per host across every worker.

pub mod bitbucket;
pub mod call;
pub mod ci;
pub mod crawl;
pub mod github;
pub mod held;
pub mod objects;
pub mod pulls;

use std::path::PathBuf;

/// The source's name, as `pass_source` records it.
pub const SOURCE: &str = "hosts";

/// GitHub's API, as the crawler calls it (step 15).
const GITHUB_API: &str = "https://api.github.com"; // skill-allow: §9-const — the host's own address, not a policy tunable

/// Bitbucket's API, as the crawler calls it (step 15).
const BITBUCKET_API: &str = "https://api.bitbucket.org/2.0"; // skill-allow: §9-const — the host's own address, not a policy tunable

/// A host the crawler reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Host {
    GitHub,
    Bitbucket,
}

impl Host {
    /// The host by the name the structure gives it.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            crate::credentials::GITHUB => Some(Self::GitHub),
            crate::credentials::BITBUCKET => Some(Self::Bitbucket),
            _ => None,
        }
    }

    /// Its name, as the structure and the credentials file write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => crate::credentials::GITHUB,
            Self::Bitbucket => crate::credentials::BITBUCKET,
        }
    }

    /// The username a mirror's fetch gives git (step 4): GitHub uses none
    /// to authenticate and needs one that is not empty; Bitbucket's API
    /// tokens take this one.
    #[must_use]
    pub fn git_user(self) -> &'static str {
        match self {
            Self::GitHub => "x-access-token",
            Self::Bitbucket => "x-bitbucket-api-token-auth",
        }
    }
}

/// Where the hosts are reached: their APIs, and where a mirror fetches
/// from. The hosts' own, unless a test points the crawler at its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub github: String,
    pub bitbucket: String,
    /// A folder whose `<host>/<path>.git` repositories a mirror fetches from
    /// in place of the host's URL.
    pub git: Option<PathBuf>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { github: GITHUB_API.to_string(), bitbucket: BITBUCKET_API.to_string(), git: None }
    }
}

impl Endpoints {
    /// The API a host's calls go to.
    #[must_use]
    pub fn api(&self, host: Host) -> &str {
        match host {
            Host::GitHub => &self.github,
            Host::Bitbucket => &self.bitbucket,
        }
    }

    /// The URL a mirror fetches from.
    #[must_use]
    pub fn git_url(&self, host: Host, path: &str) -> String {
        match &self.git {
            Some(folder) => folder.join(host.name()).join(format!("{path}.git")).to_string_lossy().into_owned(),
            None => format!("https://{}/{path}.git", host.name()),
        }
    }
}

/// A repository on a host, as the remotes name it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostPath {
    pub host: Host,
    /// The path the host knows it by, lowercased.
    pub path: String,
}

impl std::fmt::Display for HostPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.host.name(), self.path)
    }
}
