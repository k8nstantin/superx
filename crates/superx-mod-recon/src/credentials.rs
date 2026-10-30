//! The hosts' credentials (#536): `<home>/modules/recon/credentials.json`,
//! written by the operator, read by the crawler alone: when status renders,
//! and when a part of the crawler asks for a host's token (t03).
//!
//! A file whose mode lets anyone but its owner read or write it is refused,
//! and nothing of it is read. A token is held in a type that never shows it:
//! its Debug and Display print `<token>`, and no log line, status text or
//! error holds one.

use std::fmt;
use std::io::{ErrorKind, Read as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

/// The hosts the crawler reads, as the structure names them.
pub const GITHUB: &str = "github.com";
pub const BITBUCKET: &str = "bitbucket.org";

/// The mode bits that let anyone but the file's owner at it.
const NOT_THE_OWNERS: u32 = 0o077; // skill-allow: §9-const — the spec's test of a file others can read or write, not a policy tunable

/// A file's permission bits, as chmod writes them.
const PERMISSION_BITS: u32 = 0o777; // skill-allow: §9-const — the bits chmod writes, not a policy tunable

/// One host's token. Its Debug and Display print `<token>` and nothing of
/// it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// The token itself, for the call that sends it to its own host (t03).
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<token>")
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<token>")
    }
}

/// A token of a test's own, for the tests of the parts that show what the
/// file holds.
#[cfg(test)]
pub(crate) fn token(secret: &str) -> Token {
    Token(secret.to_string())
}

/// What the credentials file holds, host by host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hosts {
    /// `None` when the file gives the host no token, or an empty one.
    pub github: Option<Token>,
    pub bitbucket: Option<Token>,
    /// Every other key that is not a note: a host the crawler does not read.
    pub unknown: Vec<String>,
}

/// The credentials file, as the crawler finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// The file is absent.
    None,
    /// Its mode lets anyone but its owner read or write it: nothing of it
    /// was read. `mode` is its permission bits.
    Refused { mode: u32 },
    /// It cannot be read, or it is not a JSON object of hosts each with a
    /// string `token`: why, in the system's words, serde_json's, or naming
    /// the key that broke it.
    Unreadable(String),
    /// The hosts it names.
    Hosts(Hosts),
}

/// Read the credentials file at `path`.
#[must_use]
pub fn read(path: &Path) -> Credentials {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Credentials::None,
        Err(e) => return Credentials::Unreadable(e.to_string()),
    };
    let mode = match file.metadata() {
        Ok(metadata) => metadata.permissions().mode(),
        Err(e) => return Credentials::Unreadable(e.to_string()),
    };
    if mode & NOT_THE_OWNERS != 0 {
        return Credentials::Refused { mode: mode & PERMISSION_BITS };
    }
    let mut text = String::new();
    if let Err(e) = file.read_to_string(&mut text) {
        return Credentials::Unreadable(e.to_string());
    }
    hosts(&text)
}

/// The hosts a credentials file's text names. A key that starts with `_` is
/// a note and is skipped; every other key names an object with a string
/// `token`, empty when the operator has none for that host.
fn hosts(text: &str) -> Credentials {
    let entries: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(text) {
        Ok(entries) => entries,
        Err(e) => return Credentials::Unreadable(e.to_string()),
    };
    let mut hosts = Hosts::default();
    for (key, entry) in &entries {
        if key.starts_with('_') {
            continue;
        }
        let Some(token) = entry.get("token").and_then(serde_json::Value::as_str) else {
            return Credentials::Unreadable(format!("{key} is not an object with a string \"token\""));
        };
        let token = (!token.is_empty()).then(|| Token(token.to_string()));
        match key.as_str() {
            GITHUB => hosts.github = token,
            BITBUCKET => hosts.bitbucket = token,
            _ => hosts.unknown.push(key.clone()),
        }
    }
    Credentials::Hosts(hosts)
}

#[cfg(test)]
mod tests {
    use std::fs::Permissions;
    use std::path::PathBuf;

    use super::*;

    /// A token of the test's own, never a real one.
    const MADE_UP: &str = "recon-made-up-token-5d1f";

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("superx-recon-credentials-{}", superx_kernel::types::Uuid::new_v7()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn file(dir: &Path, text: &str, mode: u32) -> PathBuf {
        let path = dir.join("credentials.json");
        std::fs::write(&path, text).expect("write");
        std::fs::set_permissions(&path, Permissions::from_mode(mode)).expect("chmod");
        path
    }

    #[test]
    fn a_file_others_may_read_or_write_is_refused_nothing_of_it_read_its_mode_named() {
        let dir = scratch();
        // Not even JSON: had it been read, it would be unreadable.
        for mode in [0o644, 0o640, 0o620, 0o606, 0o602] {
            let path = file(&dir, "{ not json", mode);
            assert_eq!(read(&path), Credentials::Refused { mode }, "mode {mode:o}");
        }
        let path = file(&dir, &format!(r#"{{"github.com": {{"token": "{MADE_UP}"}}}}"#), 0o644);
        assert_eq!(read(&path), Credentials::Refused { mode: 0o644 });
        assert!(!format!("{:?}", read(&path)).contains(MADE_UP));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_missing_file_is_none_an_unreadable_one_names_why_notes_skipped_other_hosts_unknown_empty_tokens_none() {
        let dir = scratch();
        assert_eq!(read(&dir.join("credentials.json")), Credentials::None, "a missing file");

        // A file that cannot be read: a folder in its place.
        let folder = dir.join("folder.json");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::set_permissions(&folder, Permissions::from_mode(0o700)).expect("chmod");
        let got = read(&folder);
        assert!(matches!(&got, Credentials::Unreadable(why) if !why.is_empty()), "a folder is unreadable, the system's reason: {got:?}");

        // Not an object of hosts with string tokens: serde_json's reason, or
        // the key that broke it.
        for (text, named) in [
            ("{ not json", "line 1"),
            (r#"["github.com"]"#, "expected a map"),
            (r#"{"github.com": "a token"}"#, "github.com"),
            (r#"{"bitbucket.org": {"token": 7}}"#, "bitbucket.org"),
            (r#"{"gitlab.example": {}}"#, "gitlab.example"),
        ] {
            let got = read(&file(&dir, text, 0o600));
            assert!(matches!(&got, Credentials::Unreadable(why) if why.contains(named)), "{text}: {got:?}");
        }

        // A note is skipped, another host is unknown, an empty token is none.
        let text = format!(
            r#"{{"_doc": 5, "_note": {{"token": 1}}, "github.com": {{"token": "{MADE_UP}"}}, "bitbucket.org": {{"token": ""}}, "gitlab.example": {{"token": "x"}}}}"#
        );
        let got = read(&file(&dir, &text, 0o600));
        assert!(matches!(got, Credentials::Hosts(_)), "{got:?}");
        if let Credentials::Hosts(hosts) = got {
            assert!(hosts.github.as_ref().is_some_and(|t| t.secret() == MADE_UP), "the token is read");
            assert_eq!(hosts.bitbucket, None, "an empty token is no token");
            assert_eq!(hosts.unknown, ["gitlab.example"]);
        }
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
