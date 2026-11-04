//! The task file a session executes (#566, steps 11 and 12): found in the
//! operator's message that names it, read from the session's checkout as a
//! file, read-only, and held to: what it owns, what it forbids, its version
//! and its words.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// A task file a message names: its path in its checkout, and the checkout
/// that holds it, when one does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The path from the spec folder's entry on, as the message wrote it.
    pub path: String,
    pub checkout: Option<PathBuf>,
}

/// The task file `text` names, the first run of its characters without
/// white space that holds an entry of `spec_paths` followed by a file name
/// ending in `.xml`; its checkout, the run's part before the entry when the
/// run is an absolute path, else `folder` or the nearest of its ancestors
/// that holds a file at that path.
#[must_use]
pub fn named(text: &str, spec_paths: &[String], folder: Option<&str>) -> Option<Named> {
    for run in text.split_whitespace() {
        let run = run
            .trim_start_matches(|c: char| "([{<\"'`".contains(c))
            .trim_end_matches(|c: char| ",.:)]}>\"'`".contains(c));
        for entry in spec_paths.iter().filter(|e| !e.is_empty()) {
            let Some(at) = run.find(entry.as_str()) else { continue };
            let path = &run[at..];
            let file = &path[entry.len()..];
            if file.is_empty() || file.contains('/') || !file.ends_with(".xml") || file.len() == ".xml".len() {
                continue;
            }
            let checkout = if run.starts_with('/') {
                Some(PathBuf::from(&run[..at]))
            } else {
                folder.and_then(|f| Path::new(f).ancestors().find(|dir| dir.join(path).is_file()).map(Path::to_path_buf))
            };
            return Some(Named { path: path.to_string(), checkout });
        }
    }
    None
}

/// What the watcher holds of a task file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFile {
    /// Its root element's version attribute.
    pub version: String,
    /// Its owns and forbid, each an entry between semicolons.
    pub owns: Vec<String>,
    pub forbid: Vec<String>,
    /// Its words: the text of its start, of each step and cmd, and of its
    /// done.
    pub words: String,
}

impl TaskFile {
    /// Read `xml` as XML: its root element's version attribute, the text of
    /// its owns and forbid, and its words.
    ///
    /// # Errors
    ///
    /// Why it cannot be read as a task file, in words.
    pub fn parse(xml: &str) -> Result<Self, String> {
        let xml = without_comments(xml);
        let (_, attrs) = root(&xml).ok_or("no root element")?;
        let version = attribute(attrs, "version").ok_or("its root element holds no version attribute")?;
        let one = |tag: &str| texts(&xml, tag).into_iter().next().unwrap_or_default();
        let entries = |text: String| -> Vec<String> {
            text.split(';').map(|e| e.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|e| !e.is_empty()).collect()
        };
        let mut words = texts(&xml, "start");
        words.extend(texts(&xml, "step"));
        words.extend(texts(&xml, "cmd"));
        words.extend(texts(&xml, "done"));
        Ok(Self { version, owns: entries(one("owns")), forbid: entries(one("forbid")), words: words.join("\n") })
    }

    /// The first forbid entry with a path matching `rel`, when no owns
    /// entry's path matches it.
    #[must_use]
    pub fn forbids(&self, rel: &str) -> Option<&str> {
        if self.owns_it(rel) {
            return None;
        }
        self.forbid.iter().find(|e| paths(e).iter().any(|p| matches(p, rel))).map(String::as_str)
    }

    /// Whether an owns entry's path matches `rel`.
    #[must_use]
    pub fn owns_it(&self, rel: &str) -> bool {
        self.owns.iter().any(|e| paths(e).iter().any(|p| matches(p, rel)))
    }

    /// Whether neither an owns nor a forbid entry's path matches `rel`.
    #[must_use]
    pub fn outside(&self, rel: &str) -> bool {
        !self.owns_it(rel) && !self.forbid.iter().any(|e| paths(e).iter().any(|p| matches(p, rel)))
    }
}

/// An entry's paths: each run of its characters without white space that
/// holds a slash or ends in a file name with an extension, brackets and
/// quotes at its start and commas, periods, colons, brackets and quotes at
/// its end set aside.
#[must_use]
pub fn paths(entry: &str) -> Vec<String> {
    entry
        .split_whitespace()
        .map(|run| {
            run.trim_start_matches(|c: char| "([{<\"'`".contains(c))
                .trim_end_matches(|c: char| ",.:)]}>\"'`".contains(c))
        })
        .filter(|run| run.contains('/') || has_extension(run))
        .map(str::to_string)
        .collect()
}

fn has_extension(run: &str) -> bool {
    run.rsplit_once('.').is_some_and(|(name, ext)| !name.is_empty() && !ext.is_empty() && ext.chars().all(char::is_alphanumeric))
}

/// Whether `pattern`, relative to the checkout, matches the path `rel`: a
/// pattern ending in a slash matches every path under that folder, `**`
/// any run of folders, `*` any characters but a slash, every other
/// character itself.
#[must_use]
pub fn matches(pattern: &str, rel: &str) -> bool {
    if pattern.ends_with('/') {
        return rel.starts_with(pattern);
    }
    glob(pattern.as_bytes(), rel.as_bytes())
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    if let Some(rest) = p.strip_prefix(b"**/") {
        return glob(rest, s) || s.iter().enumerate().any(|(i, &c)| c == b'/' && glob(rest, &s[i + 1..]));
    }
    if let Some(rest) = p.strip_prefix(b"**") {
        return (0..=s.len()).any(|i| glob(rest, &s[i..]));
    }
    if let Some(rest) = p.strip_prefix(b"*") {
        for i in 0..=s.len() {
            if glob(rest, &s[i..]) {
                return true;
            }
            if i < s.len() && s[i] == b'/' {
                return false;
            }
        }
        return false;
    }
    !s.is_empty() && p[0] == s[0] && glob(&p[1..], &s[1..])
}

/// What the session shows of the task file it is held to (#566, step 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpecView {
    pub path: String,
    pub file: Option<String>,
    pub version: Option<String>,
    /// The version it held when the session was first held to it.
    pub first_version: Option<String>,
    pub owns: Vec<String>,
    pub forbid: Vec<String>,
    /// Why it holds the session to nothing, in the error's own words.
    pub why: Option<String>,
}

fn without_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The root element's name and the text of its attributes.
fn root(xml: &str) -> Option<(&str, &str)> {
    let mut rest = xml;
    loop {
        let at = rest.find('<')?;
        rest = &rest[at + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            continue;
        }
        let end = tag_end(rest)?;
        let tag = &rest[..end];
        let name_end = tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len());
        return Some((&tag[..name_end], &tag[name_end..]));
    }
}

/// Where a start tag ends: its first `>` outside a quoted value.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn attribute(attrs: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let key = format!("{name}={quote}");
        let mut rest = attrs;
        while let Some(at) = rest.find(&key) {
            let before = rest[..at].chars().next_back();
            let value = &rest[at + key.len()..];
            if before.is_none_or(char::is_whitespace) {
                return value.find(quote).map(|end| unescape(&value[..end]));
            }
            rest = value;
        }
    }
    None
}

/// The text of every `tag` element, in order, its own tags set aside and
/// its entities read.
fn texts(xml: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            rest = after;
            continue;
        }
        let Some(end) = tag_end(after) else { break };
        if after[..end].ends_with('/') {
            out.push(String::new());
            rest = &after[end + 1..];
            continue;
        }
        let inner = &after[end + 1..];
        let Some(stop) = inner.find(&close) else { break };
        out.push(unescape(&strip_tags(&inner[..stop])).split_whitespace().collect::<Vec<_>>().join(" "));
        rest = &inner[stop + close.len()..];
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        out.push(' ');
        match tag_end(&rest[at + 1..]) {
            Some(end) => rest = &rest[at + 1 + end + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let entity = after.find(';').map(|end| (&after[..end], end));
        let decoded = entity.and_then(|(name, end)| {
            let c = match name {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                n if n.starts_with("#x") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
