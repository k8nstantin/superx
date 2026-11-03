//! A repository's configuration (#538, step 3): every remote's url and
//! push url, and where each lives, the host and the path the host knows
//! it by; and the email this machine commits as there.

use std::collections::BTreeMap;

/// A remote, as a `remote_state` row says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remote {
    pub url: Option<String>,
    pub push_url: Option<String>,
    pub host: Option<String>,
    pub path: Option<String>,
}

impl Remote {
    /// A remote with its url and push url, its host and path read from
    /// its url.
    #[must_use]
    pub fn new(url: Option<String>, push_url: Option<String>) -> Self {
        let (host, path) = url.as_deref().and_then(host_path).map_or((None, None), |(h, p)| (Some(h), Some(p)));
        Self { url, push_url, host, path }
    }
}

/// What a repository's configuration says, as `git config --null --list`
/// prints it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Each remote by its name: its first url and first push url.
    pub remotes: BTreeMap<String, Remote>,
    /// `user.email`, its last value, as git takes it.
    pub email: Option<String>,
}

/// The configuration `git config --null --list` printed: each entry its
/// key, a newline and its value, entries ended by NUL.
#[must_use]
pub fn config(listing: &str) -> Config {
    let mut urls: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    let mut email = None;
    for entry in listing.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if key == "user.email" {
            email = Some(value.to_string()).filter(|v| !v.is_empty());
            continue;
        }
        let Some(rest) = key.strip_prefix("remote.") else { continue };
        let Some((name, variable)) = rest.rsplit_once('.') else { continue };
        let slot = urls.entry(name.to_string()).or_default();
        match variable {
            "url" if slot.0.is_none() => slot.0 = Some(value.to_string()),
            "pushurl" if slot.1.is_none() => slot.1 = Some(value.to_string()),
            _ => {}
        }
    }
    // A remote is one that names a url; one that names only other
    // variables fetches from nowhere.
    let remotes = urls
        .into_iter()
        .filter(|(_, (url, _))| url.is_some())
        .map(|(name, (url, push_url))| (name, Remote::new(url, push_url)))
        .collect();
    Config { remotes, email }
}

/// The host and path a url names, in its forms
/// `https://[user@]host[:port]/path`, `ssh://[user@]host[:port]/path` and
/// the scp-like `[user@]host:path`: the host lowercased, the path
/// lowercased without a trailing `.git` or slash. A local path, a
/// `file://` url and a url of no such form name neither.
#[must_use]
pub fn host_path(url: &str) -> Option<(String, String)> {
    let (authority, path) = if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("ssh://")) {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        // Its port aside; an address in brackets keeps its colons.
        let host = match host.find(']') {
            Some(end) if host.starts_with('[') => &host[..=end],
            _ => host.split_once(':').map_or(host, |(h, _)| h),
        };
        (host, path)
    } else if url.contains("://") {
        return None;
    } else {
        // scp-like: a colon before any slash.
        let (before, path) = url.split_once(':')?;
        if before.contains('/') || before.is_empty() {
            return None;
        }
        (before.rsplit_once('@').map_or(before, |(_, h)| h), path.trim_start_matches('/'))
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    if authority.is_empty() || path.is_empty() {
        return None;
    }
    Some((authority.to_lowercase(), path.to_lowercase()))
}

/// The `remote_state` rows a crawl writes, by remote name: each remote one
/// of whose url, push url, host and path differs from its newest row, and
/// each remote of a newest row whose url is not NONE that the
/// configuration no longer holds, its url, push url, host and path NONE.
#[must_use]
pub fn changes(newest: &BTreeMap<String, Remote>, now: &Config) -> Vec<(String, Remote)> {
    let mut out = Vec::new();
    for (name, remote) in &now.remotes {
        if newest.get(name) != Some(remote) {
            out.push((name.clone(), remote.clone()));
        }
    }
    for (name, held) in newest {
        if held.url.is_some() && !now.remotes.contains_key(name) {
            out.push((name.clone(), Remote::default()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
