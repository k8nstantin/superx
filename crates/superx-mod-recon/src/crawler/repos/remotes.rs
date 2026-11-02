//! A repository's remotes (#538, step 3): every remote's url and push url
//! as the repository's configuration gives them, the email it commits as,
//! and the host and path a url names.
//!
//! A url names a host and a path in three forms, `https://[user@]host[:port]/path`,
//! `ssh://[user@]host[:port]/path` and the scp-like `[user@]host:path`: the
//! host lowercased, the path lowercased without a trailing `.git` or slash.
//! A local path, a `file://` url and a url of no such form name neither.

use std::collections::BTreeMap;

/// One remote, as the configuration gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remote {
    pub url: Option<String>,
    pub push_url: Option<String>,
}

/// What `git config --null --list` says of a repository: its remotes, by
/// name, and the email it commits as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub remotes: BTreeMap<String, Remote>,
    pub email: Option<String>,
}

/// The configuration `git config --null --list` printed: entries of `key`,
/// a newline and `value`, each ended by a NUL. Of a key given twice, the
/// last is the one git uses.
#[must_use]
pub fn config(listing: &str) -> Config {
    let mut out = Config::default();
    for entry in listing.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        // Section and variable names are case-insensitive; a remote's name
        // between them is not.
        if key.eq_ignore_ascii_case("user.email") {
            out.email = (!value.is_empty()).then(|| value.to_string());
            continue;
        }
        let Some(rest) = key.strip_prefix("remote.").or_else(|| key.strip_prefix("REMOTE.")) else { continue };
        let Some((name, variable)) = rest.rsplit_once('.') else { continue };
        let remote = out.remotes.entry(name.to_string()).or_default();
        if variable.eq_ignore_ascii_case("url") {
            remote.url = Some(value.to_string());
        } else if variable.eq_ignore_ascii_case("pushurl") {
            remote.push_url = Some(value.to_string());
        }
    }
    // A remote with neither is no remote: a section of other settings.
    out.remotes.retain(|_, r| r.url.is_some() || r.push_url.is_some());
    out
}

/// The host and path a url names, when it is of one of the three forms.
#[must_use]
pub fn host_path(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("ssh://")) {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = host.split_once(':').map_or(host, |(h, _)| h);
        (host, path)
    } else if url.contains("://") {
        return None;
    } else {
        // The scp-like form: a colon before any slash, and something before it.
        let (before, path) = url.split_once(':')?;
        if before.contains('/') || before.is_empty() {
            return None;
        }
        let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
        (host, path)
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/').trim_start_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host.to_lowercase(), path.to_lowercase()))
}
