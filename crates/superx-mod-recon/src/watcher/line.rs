//! What a Claude Code line says to the watcher (#573), as the adapter
//! captured it (`crates/superx-kernel/src/adapters/claude_code.rs`): the
//! operator's words and how they came, the model thinking and writing and
//! the words of each, every call with what it was called on and what it
//! works on, how each call ended, a background job's end and a turn's end;
//! each request Claude Code sends the API, and its giving up when the API
//! fails it. Lines of a subagent's own thread are set aside: its work shows
//! as the call that started it.

use serde::Serialize;
use serde_json::Value as Json;

/// The agent whose lines the watcher reads.
pub const CLAUDE_CODE: &str = superx_kernel::adapters::claude_code::AGENT_NAME;

const INTERRUPT: &str = "[Request interrupted by user";
const NOTIFICATION: &str = "<task-notification>";
const SHELL: &str = "<bash-input>";
const COMMAND: &str = "<command-name>";
const BACKGROUND: &str = "running in background with ID: ";

/// The longest text kept of a prompt, a thought, the model's words, what a
/// call was called on, or a failed call's error.
const KEPT_CHARS: usize = 2_000; // skill-allow: §9-const — memory bound of the snapshot, not a policy tunable

/// How much of a call's result is read for the background job it names.
const RESULT_HEAD_CHARS: usize = 400; // skill-allow: §9-const — where Claude Code writes the job's id, not a policy tunable

/// The tools that write a file by its path.
pub const FILE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The tools that stop a background job.
pub const STOP_TOOLS: [&str; 3] = ["TaskStop", "KillShell", "KillBash"];

/// The tools whose result is the operator's answer, not a tool's run.
pub const ASK_TOOLS: [&str; 2] = ["AskUserQuestion", "ExitPlanMode"];

/// How a message of the operator's came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Came {
    /// Typed while the agent waited.
    Typed,
    /// Typed while the agent worked, handed to it at its next step.
    Queued,
    /// The operator stopping the agent mid-turn.
    Interrupt,
    /// A `!` command, run in the agent's shell.
    Shell,
    /// A slash command.
    Command,
}

/// One thing a line says happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Happened {
    Prompt { came: Came, text: String },
    /// The model thinking, in its words when the line keeps them.
    Thought { text: String },
    /// The model's words to the operator.
    Said { text: String },
    /// A call: what it was called on, the file it names, what it works on
    /// ([`target`]), whether it runs in the background, and the whole text
    /// a Write writes.
    Call {
        id: String,
        tool: String,
        what: String,
        about: Option<String>,
        path: Option<String>,
        target: String,
        /// The part of its file a Read reads, `offset 10, limit 40`; empty
        /// for the whole.
        part: String,
        background: bool,
        content: Option<String>,
        stops: Option<String>,
    },
    /// How a call ended, the background job it started, if any, and its
    /// output whole, read once for a plan the call read.
    Result { call: String, failed: bool, job: Option<String>, text: String },
    /// A background job ended, as Claude Code's task notification says, and
    /// its summary.
    JobEnded { call: Option<String>, status: Option<String>, summary: Option<String> },
    /// A request sent to the API: sent again unchanged when it adds nothing
    /// to the request it builds on; the model and the effort it asks for.
    Asked { resent: bool, model: Option<String>, effort: Option<String> },
    /// Claude Code giving up on the API, in its own words.
    ApiFailed { error: String },
    TurnEnded,
    /// The context compacted: what the agent held is now a summary of it.
    Compacted,
}

/// One line, as the watcher reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    pub model: Option<String>,
    /// The cwd the line names.
    pub folder: Option<String>,
    pub happened: Vec<Happened>,
}

/// What `line` says.
#[must_use]
pub fn read(line: &Json) -> Reading {
    let mut r = Reading { folder: text(line, &["cwd"]).map(str::to_string), ..Reading::default() };
    if flag(line, "isSidechain") {
        return r;
    }
    match text(line, &["type"]) {
        Some("user") => user(line, &mut r),
        Some("assistant") => assistant(line, &mut r),
        Some("system") if text(line, &["subtype"]) == Some("turn_duration") => r.happened.push(Happened::TurnEnded),
        Some("attachment") => queued(line, &mut r),
        Some("api-request") => asked(line, &mut r),
        _ => {}
    }
    r
}

fn user(line: &Json, r: &mut Reading) {
    let message = match at(line, &["message", "content"]) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(blocks)) => {
            for b in blocks.iter().filter(|b| text(b, &["type"]) == Some("tool_result")) {
                let whole = all_text(at(b, &["content"]));
                let head: String = whole.chars().take(RESULT_HEAD_CHARS).collect();
                r.happened.push(Happened::Result {
                    call: text(b, &["tool_use_id"]).unwrap_or_default().to_string(),
                    failed: at(b, &["is_error"]).and_then(Json::as_bool).unwrap_or(false),
                    job: head.find(BACKGROUND).map(|i| head[i + BACKGROUND.len()..].chars().take_while(char::is_ascii_alphanumeric).collect()),
                    text: whole,
                });
            }
            blocks.iter().filter(|b| text(b, &["type"]) == Some("text")).filter_map(|b| text(b, &["text"])).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    };
    if message.is_empty() {
        return;
    }
    if text(line, &["origin", "kind"]) == Some("task-notification") || message.trim_start().starts_with(NOTIFICATION) {
        r.happened.push(Happened::JobEnded {
            call: between(&message, "<tool-use-id>", "</tool-use-id>"),
            status: between(&message, "<status>", "</status>"),
            summary: between(&message, "<summary>", "</summary>").map(|s| kept(&s)),
        });
        return;
    }
    if flag(line, "isCompactSummary") {
        r.happened.push(Happened::Compacted);
        return;
    }
    if flag(line, "isMeta") {
        return;
    }
    let origin = text(line, &["origin", "kind"]);
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
    r.happened.push(Happened::Prompt { came, text: kept(&message) });
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
    if message.trim_start().starts_with(NOTIFICATION) {
        // The notice also arrives as a line of its own, which is the one read.
        return;
    }
    let came = match text(line, &["attachment", "origin", "kind"]) {
        Some("human") | None if message.starts_with(SHELL) => Came::Shell,
        Some("human") if message.starts_with(COMMAND) => Came::Command,
        Some("human") => Came::Queued,
        _ => return,
    };
    r.happened.push(Happened::Prompt { came, text: kept(&message) });
}

/// A request to the API, as Claude Code writes it before it sends one: the
/// request it builds on (`base`) and the messages it adds (`tail`). One that
/// adds none sends its base again: the API did not answer it.
fn asked(line: &Json, r: &mut Reading) {
    let resent = text(line, &["base"]).is_some() && at(line, &["tail"]).and_then(Json::as_array).is_some_and(Vec::is_empty);
    r.happened.push(Happened::Asked {
        resent,
        model: text(line, &["params", "model"]).map(str::to_string),
        effort: text(line, &["params", "output_config", "effort"]).map(str::to_string),
    });
}

fn assistant(line: &Json, r: &mut Reading) {
    // Claude Code's own line when it gave up on the API: no model wrote it.
    if flag(line, "isApiErrorMessage") {
        r.happened.push(Happened::ApiFailed { error: kept(&all_text(at(line, &["message", "content"]))) });
        return;
    }
    r.model = text(line, &["message", "model"]).filter(|m| !m.starts_with('<')).map(str::to_string);
    let Some(Json::Array(blocks)) = at(line, &["message", "content"]) else { return };
    for b in blocks {
        match text(b, &["type"]) {
            Some("thinking" | "redacted_thinking") => r.happened.push(Happened::Thought { text: kept(text(b, &["thinking"]).unwrap_or_default()) }),
            Some("text") if text(b, &["text"]).is_some_and(|t| !t.trim().is_empty()) => {
                r.happened.push(Happened::Said { text: kept(text(b, &["text"]).unwrap_or_default()) });
            }
            Some("tool_use") => {
                let tool = text(b, &["name"]).unwrap_or_default().to_string();
                let input = at(b, &["input"]).cloned().unwrap_or(Json::Null);
                let path = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Json::as_str).map(str::to_string);
                // A watch reports as it goes and ends at its own time: only a
                // command run in the background is a job that ends at a notice.
                let background = tool == "Bash" && input.get("run_in_background").and_then(Json::as_bool) == Some(true);
                let content = (tool == "Write").then(|| input.get("content").and_then(Json::as_str).map(str::to_string)).flatten();
                // A call that stops a background job names it by its id.
                let stops = STOP_TOOLS
                    .contains(&tool.as_str())
                    .then(|| ["task_id", "shell_id", "bash_id"].iter().find_map(|k| input.get(*k).and_then(Json::as_str)).map(str::to_string))
                    .flatten();
                r.happened.push(Happened::Call {
                    id: text(b, &["id"]).unwrap_or_default().to_string(),
                    what: kept(&called_on(&input)),
                    about: input.get("description").and_then(Json::as_str).map(kept),
                    target: target(&tool, &input),
                    part: ["offset", "limit"].iter().filter_map(|k| input.get(*k).map(|v| format!("{k} {v}"))).collect::<Vec<_>>().join(", "),
                    tool,
                    path,
                    background,
                    content,
                    stops,
                });
            }
            _ => {}
        }
    }
    // The turn's end as the message says it: kept on every message captured,
    // where Claude Code's own line of it is not. Every line of the message
    // carries it, its thinking's too: the line with its words ends the turn.
    if text(line, &["message", "stop_reason"]) == Some("end_turn") && r.happened.iter().any(|h| matches!(h, Happened::Said { .. })) {
        r.happened.push(Happened::TurnEnded);
    }
}

/// What a call was called on: the first its input holds of these keys,
/// else the input whole as JSON.
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

/// What a call works on, as its time is counted: the file a file tool
/// reads or writes, the folder a search looks in, the host a fetch asks, the
/// kind of agent a subagent is, the skill a skill call runs. A command's is
/// read from the command itself, with the operator's jobs
/// ([`super::plan::command`]); every other call's is its tool alone.
fn target(tool: &str, input: &Json) -> String {
    let field = |k: &str| input.get(k).and_then(Json::as_str).unwrap_or_default().to_string();
    match tool {
        "Read" | "Edit" | "Write" | "MultiEdit" => field("file_path"),
        "NotebookEdit" => field("notebook_path"),
        "Grep" | "Glob" => field("path"),
        "WebFetch" => {
            let url = field("url");
            let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
            rest.split(['/', '?', '#']).next().unwrap_or_default().to_string()
        }
        "Task" | "Agent" => field("subagent_type"),
        "Skill" => field("skill"),
        "Artifact" => Some(field("file_path")).filter(|f| !f.is_empty()).unwrap_or_else(|| field("url")),
        _ => String::new(),
    }
}

/// A text cut to the longest the watcher keeps.
pub fn kept(s: &str) -> String {
    s.chars().take(KEPT_CHARS).collect()
}

fn all_text(content: Option<&Json>) -> String {
    match content {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(parts)) => parts.iter().filter_map(|p| text(p, &["text"])).collect(),
        _ => String::new(),
    }
}

fn between(s: &str, open: &str, close: &str) -> Option<String> {
    let from = s.find(open)? + open.len();
    let to = s[from..].find(close)? + from;
    Some(s[from..to].trim().to_string())
}

fn flag(line: &Json, key: &str) -> bool {
    at(line, &[key]).and_then(Json::as_bool) == Some(true)
}

fn at<'a>(v: &'a Json, path: &[&str]) -> Option<&'a Json> {
    path.iter().try_fold(v, |v, k| v.get(k))
}

fn text<'a>(v: &'a Json, path: &[&str]) -> Option<&'a str> {
    at(v, path).and_then(Json::as_str)
}
