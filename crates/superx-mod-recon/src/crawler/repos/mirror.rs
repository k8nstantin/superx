//! The crawler's own mirrors (#538, step 4): a bare repository per
//! repository on a host, in the repos folder, made with no template and
//! fetched with every ref the host advertises taken as it is, nothing
//! pruned or collected, every ref's moves logged and never expired, no
//! hook, no redirect, no submodule, the operator's configuration unread
//! and no credential helper. The password is superx-recon's own answer as
//! git's askpass (step 23): no token is in any argument, variable, file or
//! url.

use std::path::{Path, PathBuf};

use crate::crawler::hosts::{Host, HostPath};
use crate::crawler::repos::git::With;

/// The variable that makes superx-recon git's askpass, naming the host
/// whose password it gives (step 23).
pub const ASKPASS_ENV: &str = "SUPERX_RECON_ASKPASS";

/// The bytes a second a fetch's transfer may fall under, and for how many
/// seconds, before git ends it (step 4).
const LOW_SPEED_LIMIT: &str = "1"; // skill-allow: §9-const — step 4's own value, not a policy tunable
const LOW_SPEED_SECS: &str = "60"; // skill-allow: §9-const — step 4's own value, not a policy tunable

/// Where a mirror's own folders and programs are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    /// The repos folder.
    pub repos: Option<PathBuf>,
    /// The instance's home, which the askpass reads the credentials of.
    pub home: Option<PathBuf>,
    /// superx-recon itself, git's askpass.
    pub askpass: Option<PathBuf>,
}

/// A host repository's mirror: `<repos folder>/<host>/<path>.git`.
#[must_use]
pub fn folder(repos: &Path, hp: &HostPath) -> PathBuf {
    repos.join(hp.host.name()).join(format!("{}.git", hp.path))
}

/// What every git call on a mirror of `host` runs with.
#[must_use]
pub fn with(host: Host, places: &Places) -> With {
    let config = [
        "fetch.prune=false",
        "fetch.pruneTags=false",
        "gc.auto=0",
        "maintenance.auto=false",
        "core.logAllRefUpdates=always",
        "gc.reflogExpire=never",
        "gc.reflogExpireUnreachable=never",
        "core.hooksPath=/dev/null",
        "http.followRedirects=false",
        "credential.helper=",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .chain([
        format!("credential.username={}", host.git_user()),
        format!("http.lowSpeedLimit={LOW_SPEED_LIMIT}"),
        format!("http.lowSpeedTime={LOW_SPEED_SECS}"),
    ])
    .collect();
    let mut env: Vec<(String, String)> = vec![
        ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        (ASKPASS_ENV.to_string(), host.name().to_string()),
    ];
    if let Some(askpass) = &places.askpass {
        env.push(("GIT_ASKPASS".to_string(), askpass.to_string_lossy().into_owned()));
    }
    if let Some(home) = &places.home {
        env.push(("SUPERX_HOME".to_string(), home.to_string_lossy().into_owned()));
    }
    With { config, env, ..With::default() }
}

/// The arguments of a mirror's fetch from `url`: every ref taken as it is,
/// the refspec its only one, no maintenance, no submodule.
#[must_use]
pub fn fetch_args(url: &str) -> Vec<String> {
    ["fetch", "--no-auto-maintenance", "--no-recurse-submodules", url, "+refs/*:refs/*"].iter().map(|s| (*s).to_string()).collect()
}
