//! What a Claude Code line says to the watcher (#566, step 4), as the
//! adapter captured it (`crates/superx-kernel/src/adapters/claude_code.rs`):
//! the operator's words and how they came, the agent's words to the
//! operator, every call whole with the paths it writes, how each ended, a
//! background job's end, a turn's end, a compaction and the agent's own
//! recap. Read for the watcher alone; the crawler reads the same lines into
//! facts of its own, for history.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value as Json;

/// The agent whose lines the watcher judges.
pub const CLAUDE_CODE: &str = superx_kernel::adapters::claude_code::AGENT_NAME;

const INTERRUPT: &str = "[Request interrupted by user";
const NOTIFICATION: &str = "task-notification";
const SHELL: &str = "<bash-input>";
const COMMAND: &str = "<command-name>";

/// The tools that write a file by its path.
pub const FILE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// How a message of the operator's came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Came {
    /// Typed while the agent waited.
    Typed,
    /// Typed while the agent worked, handed to it at its next step.
    Queued,
    /// The operator stopping the agent.
    Interrupt,
    /// A `!` command, run in the agent's shell.
    Shell,
    /// A slash command.
    Command,
}

/// One thing a line says happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Happened {
    Prompt { came: Came, text: String },
    /// The agent's words to the operator, in its main thread.
    Said { text: String },
    /// A call: what it was called on, its input whole, whether it runs in
    /// the background, and the paths it writes.
    Call { id: String, tool: String, what: String, input: Json, background: bool, writes: Vec<PathBuf> },
    /// How a call ended: failed or not, its error whole, the lines it wrote.
    Result { call: String, failed: bool, error: Option<String>, lines: i64 },
    /// A background job ended, as Claude Code's task notification says.
    JobEnded { call: Option<String>, status: Option<String> },
    TurnEnded,
    Compacted,
    /// The agent's own account of what it is doing (`away_summary`).
    Recap { text: String },
}

/// One line, as the watcher reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub model: Option<String>,
    /// The cwd the line names.
    pub folder: Option<String>,
    pub happened: Vec<Happened>,
}

/// What `line` says. `home` stands for a `~` that starts a path a command
/// writes.
#[must_use]
pub fn read(line: &Json, home: Option<&Path>) -> Reading {
    let mut r = Reading { folder: text(line, &["cwd"]).map(str::to_string), ..Reading::default() };
    match text(line, &["type"]) {
        Some("user") => user(line, &mut r),
        Some("assistant") => assistant(line, home, &mut r),
        Some("system") => system(line, &mut r),
        Some("attachment") => queued(line, &mut r),
        _ => {}
    }
    r
}

fn flag(line: &Json, key: &str) -> bool {
    at(line, &[key]).and_then(Json::as_bool) == Some(true)
}

fn user(line: &Json, r: &mut Reading) {
    let message = match at(line, &["message", "content"]) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(blocks)) => {
            for b in blocks.iter().filter(|b| text(b, &["type"]) == Some("tool_result")) {
                let failed = at(b, &["is_error"]).and_then(Json::as_bool).unwrap_or(false);
                r.happened.push(Happened::Result {
                    call: text(b, &["tool_use_id"]).unwrap_or_default().to_string(),
                    failed,
                    error: failed.then(|| words_of(at(b, &["content"]))),
                    lines: written(at(line, &["toolUseResult"])),
                });
            }
            blocks
                .iter()
                .filter(|b| text(b, &["type"]) == Some("text"))
                .filter_map(|b| text(b, &["text"]))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => String::new(),
    };
    if message.is_empty() {
        return;
    }
    let origin = text(line, &["origin", "kind"]);
    if origin == Some(NOTIFICATION) {
        r.happened.push(Happened::JobEnded {
            call: between(&message, "<tool-use-id>", "</tool-use-id>"),
            status: between(&message, "<status>", "</status>"),
        });
        return;
    }
    if flag(line, "isSidechain") || flag(line, "isMeta") || flag(line, "isCompactSummary") {
        return;
    }
    let came = if message.starts_with(INTERRUPT) {
        Came::Interrupt
    } else if message.starts_with(SHELL) {
        Came::Shell
    } else if message.starts_with(COMMAND) {
        Came::Command
    } else if origin == Some("human") || (origin.is_none() && !message.starts_with('<')) {
        Came::Typed
    } else {
        return;
    };
    r.happened.push(Happened::Prompt { came, text: message });
}

fn queued(line: &Json, r: &mut Reading) {
    if text(line, &["attachment", "type"]) != Some("queued_command") {
        return;
    }
    let message = match at(line, &["attachment", "prompt"]) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(parts)) => parts.iter().filter_map(|p| text(p, &["text"])).collect::<Vec<_>>().join("\n"),
        _ => return,
    };
    let came = match text(line, &["attachment", "origin", "kind"]) {
        Some("human") if message.starts_with(SHELL) => Came::Shell,
        Some("human") if message.starts_with(COMMAND) => Came::Command,
        Some("human") => Came::Queued,
        None if message.starts_with(SHELL) => Came::Shell,
        _ => return,
    };
    r.happened.push(Happened::Prompt { came, text: message });
}

fn assistant(line: &Json, home: Option<&Path>, r: &mut Reading) {
    let sidechain = flag(line, "isSidechain");
    if !sidechain {
        r.model = text(line, &["message", "model"]).filter(|m| !m.starts_with('<')).map(str::to_string);
    }
    let Some(Json::Array(blocks)) = at(line, &["message", "content"]) else { return };
    for b in blocks {
        match text(b, &["type"]) {
            Some("text") if !sidechain => {
                if let Some(t) = text(b, &["text"]).filter(|t| !t.trim().is_empty()) {
                    r.happened.push(Happened::Said { text: t.to_string() });
                }
            }
            Some("tool_use") => {
                let tool = text(b, &["name"]).unwrap_or_default().to_string();
                let input = at(b, &["input"]).cloned().unwrap_or(Json::Null);
                let what = called_on(&input);
                let background = tool == "Monitor" || input.get("run_in_background").and_then(Json::as_bool) == Some(true);
                let writes = writes(&tool, &input, &what, r.folder.as_deref(), home);
                r.happened.push(Happened::Call {
                    id: text(b, &["id"]).unwrap_or_default().to_string(),
                    tool,
                    what,
                    input,
                    background,
                    writes,
                });
            }
            _ => {}
        }
    }
}

fn system(line: &Json, r: &mut Reading) {
    if flag(line, "isSidechain") {
        return;
    }
    match text(line, &["subtype"]) {
        Some("turn_duration") => r.happened.push(Happened::TurnEnded),
        Some("compact_boundary") => r.happened.push(Happened::Compacted),
        Some("away_summary") => {
            if let Some(t) = text(line, &["content"]) {
                r.happened.push(Happened::Recap { text: t.to_string() });
            }
        }
        _ => {}
    }
}

/// What a call was called on, whole: the first its input holds of these
/// keys, else the input whole as JSON.
fn called_on(input: &Json) -> String {
    for key in ["command", "file_path", "notebook_path", "pattern", "url", "query", "description", "prompt"] {
        if let Some(v) = input.get(key).and_then(Json::as_str) {
            return v.to_string();
        }
    }
    if input.is_null() {
        String::new()
    } else {
        input.to_string()
    }
}

/// The paths a call writes: a file tool's path, and a Bash command's
/// targets.
fn writes(tool: &str, input: &Json, what: &str, folder: Option<&str>, home: Option<&Path>) -> Vec<PathBuf> {
    if FILE_TOOLS.contains(&tool) {
        let path = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Json::as_str);
        return path.and_then(|p| resolve(p, folder, home)).into_iter().collect();
    }
    if tool == "Bash" {
        return targets(what).iter().filter_map(|t| resolve(t, folder, home)).collect();
    }
    Vec::new()
}

/// A word of a command, or one of the operators that matter here.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Op(String),
}

/// A command's words, split at white space, their quotes set aside, and its
/// operators: `|`, `||`, `;`, `&`, `&&`, `>`, `>>` and newlines.
fn tokens(command: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Word(std::mem::take(word)));
        }
    };
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '\\' => {
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            '\n' | '|' | ';' | '&' | '>' => {
                flush(&mut word, &mut out);
                let mut op = c.to_string();
                if matches!(c, '|' | '&' | '>') && chars.peek() == Some(&c) {
                    op.push(c);
                    chars.next();
                }
                out.push(Token::Op(op));
            }
            c if c.is_whitespace() => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Where a Bash command writes (#566, step 4): the word after each `>` or
/// `>>`, one that starts with `&` aside; each word after a `tee` to the end
/// of its command that does not start with `-`; and for a `sed` with an
/// option that starts with `-i`, each word of its command after the first
/// that does not start with `-`, the script, that does not start with `-`.
#[must_use]
pub fn targets(command: &str) -> Vec<String> {
    let all = tokens(command);
    let mut out = Vec::new();
    for command in all.split(|t| matches!(t, Token::Op(op) if matches!(op.as_str(), "|" | "||" | ";" | "&" | "&&" | "\n"))) {
        let mut words: Vec<&str> = Vec::new();
        let mut i = 0;
        while i < command.len() {
            match &command[i] {
                Token::Op(op) if op == ">" || op == ">>" => {
                    if let Some(Token::Word(w)) = command.get(i + 1) {
                        if !w.starts_with('&') {
                            out.push(w.clone());
                        }
                        i += 1;
                    }
                }
                Token::Word(w) => words.push(w),
                Token::Op(_) => {}
            }
            i += 1;
        }
        if let Some(at) = words.iter().position(|w| *w == "tee") {
            out.extend(words[at + 1..].iter().filter(|w| !w.starts_with('-')).map(|w| (*w).to_string()));
        }
        if let Some(at) = words.iter().position(|w| *w == "sed") {
            let after = &words[at + 1..];
            if after.iter().any(|w| w.starts_with("-i")) {
                let mut plain = after.iter().filter(|w| !w.starts_with('-'));
                plain.next();
                out.extend(plain.map(|w| (*w).to_string()));
            }
        }
    }
    out
}

/// A written path as the watcher reads it: absolute, with `home` for a `~`
/// that starts it, or joined to the line's folder; `..` and `.` resolved.
fn resolve(word: &str, folder: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    let path = if word == "~" {
        home?.to_path_buf()
    } else if let Some(rest) = word.strip_prefix("~/") {
        home?.join(rest)
    } else if Path::new(word).is_absolute() {
        PathBuf::from(word)
    } else {
        Path::new(folder?).join(word)
    };
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Some(out)
}

/// The lines an edit added, as Claude Code's `toolUseResult` gives them.
fn written(result: Option<&Json>) -> i64 {
    let Some(r) = result else { return 0 };
    if let Some(Json::Array(hunks)) = r.get("structuredPatch") {
        let added = hunks
            .iter()
            .filter_map(|h| h.get("lines").and_then(Json::as_array))
            .flatten()
            .filter_map(Json::as_str)
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count();
        if added > 0 {
            return i64::try_from(added).unwrap_or(i64::MAX);
        }
    }
    if r.get("type").and_then(Json::as_str) == Some("create") {
        if let Some(content) = r.get("content").and_then(Json::as_str) {
            return i64::try_from(content.lines().count()).unwrap_or(i64::MAX);
        }
    }
    0
}

/// A tool result's content as text, whole.
fn words_of(content: Option<&Json>) -> String {
    match content {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Json::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn between(text: &str, open: &str, close: &str) -> Option<String> {
    let (_, rest) = text.split_once(open)?;
    let (inside, _) = rest.split_once(close)?;
    Some(inside.trim().to_string())
}

fn at<'a>(v: &'a Json, path: &[&str]) -> Option<&'a Json> {
    path.iter().try_fold(v, |v, k| v.get(*k))
}

fn text<'a>(v: &'a Json, path: &[&str]) -> Option<&'a str> {
    at(v, path).and_then(Json::as_str)
}
