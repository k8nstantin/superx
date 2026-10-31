//! Gemini CLI's chat lines. The kernel keeps its `user` and `gemini`
//! records as `message` rows, each the record as it stood when captured, and
//! every earlier write of a record within one read as a `transcript_event`
//! of kind `superseded` (#518), read as the record is; headers, header
//! changes and `info`, `error` and `warning` lines are `transcript_event`s
//! too, and feed no table.
//!
//! Shapes, mapped from the 26 chats on this machine (2026-10-25):
//!
//! - A record is re-emitted as it streams, under the same `id`, each time
//!   fuller: the kernel keeps a new line whenever it grew. A line is the
//!   whole reply so far, never a part of it.
//! - `gemini`: `content` (the answer so far), `model`, `tokens` (`input`,
//!   `output`, `cached`, `thoughts`, `tool`, `total`), `thoughts`
//!   (`subject`, `description`, `timestamp`) and `toolCalls` (`id`, `name`,
//!   `args`, `result`, `status`, `timestamp`): a call has ended once its
//!   status is `success`, `error` or `cancelled`, and its result is a list
//!   of `functionResponse`s.
//! - `user`: `content` is a list of parts; text parts are the operator's
//!   message, `functionResponse` parts hand results back.
//! - A header (`sessionId`, no `type`) and a header change (`$set`) carry
//!   no type: their kind is the one the kernel gave them.
//! - Lines carry no working directory, branch or version.

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;

use crate::crawler::lines::facts::{
    Call, Counts, CountedFrom, Edit, EditKind, Fact, Outcome, Prompt, PromptKind, Read, ReplyLine, ResultLine, Text,
    Thinking, ToolResult, TurnPosition,
};
use crate::crawler::lines::kernel::Body;
use crate::crawler::value::{at, chars, flag, int, json_len, lines, list, object, some_text, text, time, Columns};

/// The agent, as the kernel names it.
pub const AGENT: &str = superx_kernel::adapters::gemini_cli::AGENT_NAME;

/// The tool that runs shell commands.
pub const SHELL_TOOL: &str = "run_shell_command";

/// The tool that loads a skill.
const SKILL_TOOL: &str = "activate_skill";

/// The kinds of line Gemini CLI writes that feed no table.
pub const NO_TABLE: [&str; 5] = ["header", "$set", "info", "error", "warning"];

/// What the reading takes from every line, whatever its kind.
pub const STORED_ON_EVERY_LINE: &[&str] = &["timestamp"];

/// What the reading puts in a column, by kind of line; a path ending in
/// `.**` is held whole.
pub const STORED: &[(&str, &[&str])] = &[
    ("user", &["content", "content[].text"]),
    (
        "gemini",
        &[
            "id",
            "model",
            "tokens.**",
            "thoughts[].description",
            "thoughts[].subject",
            "thoughts[].timestamp",
            "toolCalls[].id",
            "toolCalls[].name",
            "toolCalls[].timestamp",
            "toolCalls[].args.file_path",
            "toolCalls[].args.path",
            "toolCalls[].args.dir_path",
            "toolCalls[].args.command",
            "toolCalls[].args.description",
            "toolCalls[].args.pattern",
            "toolCalls[].args.url",
            "toolCalls[].args.query",
            "toolCalls[].args.agent_name",
            "toolCalls[].args.is_background",
            "toolCalls[].args.name",
            "toolCalls[].result[].functionResponse.response.output",
            "toolCalls[].result[].functionResponse.response.error",
        ],
    ),
];

/// What `write_file` says when the file did not exist.
const CREATED: &str = "Successfully created";

/// How a shell command's output names its exit code.
const EXIT_CODE: &str = "Exit Code:";

/// A line as the kernel kept it, read: a line with no type by the kind the
/// kernel gave it.
#[must_use]
pub fn read_kept(body: &Body, kernel_kind: Option<&str>, at_time: DateTime<Utc>) -> Read {
    let known_kind = kernel_kind.is_some_and(|kind| NO_TABLE.contains(&kind));
    match body {
        Body::Json(line) if text(line, &["type"]).is_some() => read(line, at_time),
        Body::Json(_) | Body::Missing if known_kind => Read::nothing(),
        Body::Json(_) | Body::Missing | Body::Text(_) | Body::NotJson => Read::unknown(),
    }
}

/// One line, read. `at` is when the line was written.
#[must_use]
pub fn read(line: &Value, at_time: DateTime<Utc>) -> Read {
    match text(line, &["type"]) {
        Some("user") => user(line),
        Some("gemini") => gemini(line, at_time),
        Some(kind) if NO_TABLE.contains(&kind) => Read::nothing(),
        _ => Read::unknown(),
    }
}

fn user(line: &Value) -> Read {
    let parts = list(line, &["content"]);
    let mut known = true;
    let mut texts = Vec::new();
    let mut images = 0;
    let mut responses = 0;
    for p in parts {
        if let Some(t) = text(p, &["text"]) {
            texts.push(t);
        } else if at(p, &["inlineData"]).is_some() || at(p, &["fileData"]).is_some() {
            images += 1;
        } else if at(p, &["functionResponse"]).is_some() {
            responses += 1;
        } else {
            known = false;
        }
    }
    if parts.is_empty() {
        return match text(line, &["content"]) {
            Some(t) => typed(t.to_string(), 0),
            None => Read::unknown(),
        };
    }
    if texts.is_empty() && responses > 0 {
        return Read { known, ..Read::nothing() };
    }
    Read { known, ..typed(texts.join("\n"), images) }
}

fn typed(message: String, images: i64) -> Read {
    Read {
        known: true,
        facts: vec![Fact::Prompt(Prompt {
            kind: PromptKind::Typed,
            text: message,
            images,
            stops_tools: false,
            turn: TurnPosition::default(),
        })],
        ..Read::default()
    }
}

fn gemini(line: &Value, at_time: DateTime<Utc>) -> Read {
    let Some(agent_id) = some_text(line, &["id"]) else {
        return Read::unknown();
    };
    let mut known = true;
    let mut columns = Columns::default();
    let counts = match at(line, &["tokens"]) {
        Some(_) => {
            let c = counts(line);
            known &= c.is_some();
            c
        }
        None => None,
    };
    let thinking = list(line, &["thoughts"])
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let position = i64::try_from(i).unwrap_or(i64::MAX);
            let said = text(t, &["description"]).unwrap_or_default();
            let subject = some_text(t, &["subject"]);
            Thinking {
                position: Some(position),
                on_line: position,
                at: time(t, &["timestamp"]).unwrap_or(at_time),
                ms: None,
                chars: subject.as_deref().map_or(0, chars) + chars(said),
                display: None,
                text: said.to_string(),
                subject,
                signature_chars: 0,
            }
        })
        .collect();
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for c in list(line, &["toolCalls"]) {
        let Some(call) = call(c, at_time, &mut columns) else {
            known = false;
            continue;
        };
        if let Some(r) = result(c, &call, at_time) {
            results.push(r);
        }
        calls.push(call);
    }
    let reply = ReplyLine {
        agent_id,
        request_id: None,
        model: some_text(line, &["model"]),
        requested_model: None,
        advisor_model: None,
        effort: None,
        turn_effort: None,
        stop_reason: None,
        subagent: false,
        client_version: None,
        skill: None,
        plugin: None,
        mcp_server: None,
        mcp_tool: None,
        refusal: None,
        refusal_text: None,
        cache_miss: None,
        cache_missed: None,
        counts,
        text: Text::Total(chars(text(line, &["content"]).unwrap_or_default())),
        thinking,
        calls,
        results,
        blocks: 0,
        cumulative: true,
        thinking_dropped: 0,
    };
    Read { known: known && columns.placed(), facts: vec![Fact::Reply(Box::new(reply))], ..Read::default() }
}

/// The reply's counts, from its `tokens`: the input not served from cache,
/// and everything generated, thinking included.
fn counts(line: &Value) -> Option<Counts> {
    let tokens = at(line, &["tokens"])?;
    let count = |key: &str| int(tokens, &[key]).filter(|n| *n >= 0);
    let (input, cached, output, thoughts) = (count("input")?, count("cached")?, count("output")?, count("thoughts")?);
    Some(Counts {
        input_tokens: (input - cached).max(0),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        output_tokens: output + thoughts,
        thinking_tokens: thoughts,
        tool_tokens: Some(count("tool")?),
        web_searches: None,
        web_fetches: None,
        service_tier: None,
        cache_write_1h_tokens: None,
        cache_write_5m_tokens: None,
        usage: object(line, &["tokens"])?.clone(),
    })
}

fn call(c: &Value, at_time: DateTime<Utc>, columns: &mut Columns) -> Option<Call> {
    let args = at(c, &["args"]);
    let field = |key: &str| args.and_then(|a| some_text(a, &[key]));
    let tool = some_text(c, &["name"])?;
    let shell = tool == SHELL_TOOL;
    let none = Value::None;
    Some(Call {
        call_id: some_text(c, &["id"])?,
        at: time(c, &["timestamp"]).unwrap_or(at_time),
        path: field("file_path").or_else(|| field("path")).or_else(|| if shell { None } else { field("dir_path") }),
        dir: if shell { field("dir_path") } else { None },
        command: field("command"),
        description: field("description"),
        pattern: field("pattern"),
        url: field("url"),
        query: field("query"),
        subagent_type: field("agent_name"),
        background: args.and_then(|a| flag(a, &["is_background"])).unwrap_or(false),
        input_bytes: args.map_or(0, json_len),
        sandbox_off: false,
        skill: if tool == SKILL_TOOL { columns.text(args.unwrap_or(&none), &["name"]) } else { None },
        model: None,
        timeout_ms: None,
        tool,
    })
}

/// A call that has ended, with what it did; none while it runs.
fn result(c: &Value, call: &Call, at_time: DateTime<Utc>) -> Option<ToolResult> {
    let said: Vec<&str> = list(c, &["result"])
        .iter()
        .filter_map(|r| {
            text(r, &["functionResponse", "response", "output"])
                .or_else(|| text(r, &["functionResponse", "response", "error"]))
        })
        .collect();
    let said = said.join("\n");
    let outcome = match text(c, &["status"]) {
        // A shell command that exited with a failure is reported a success:
        // its exit code says otherwise.
        Some("success") if call.tool == SHELL_TOOL && failed_exit(&said) => Outcome::Error,
        Some("success") => Outcome::Ok,
        Some("error") => Outcome::Error,
        Some("cancelled") => Outcome::Cancelled,
        _ => return None,
    };
    let args = at(c, &["args"]);
    let edit = match (outcome, call.tool.as_str(), args) {
        (Outcome::Ok, "replace", Some(a)) => match (some_text(a, &["file_path"]), text(a, &["old_string"]), text(a, &["new_string"])) {
            (Some(path), Some(old), Some(new)) if old != new => Some(Edit {
                path,
                kind: EditKind::Modified,
                added: lines(new),
                removed: lines(old),
                counted_from: CountedFrom::Strings,
                operator_changed: false,
            }),
            _ => None,
        },
        (Outcome::Ok, "write_file", Some(a)) => match (some_text(a, &["file_path"]), text(a, &["content"])) {
            (Some(path), Some(content)) => Some(Edit {
                path,
                kind: if said.starts_with(CREATED) { EditKind::Created } else { EditKind::Modified },
                added: lines(content),
                removed: 0,
                counted_from: CountedFrom::Content,
                operator_changed: false,
            }),
            _ => None,
        },
        _ => None,
    };
    Some(ToolResult {
        call_id: call.call_id.clone(),
        // Gemini records when a call started, not when it ended.
        at: call.at.max(at_time),
        outcome,
        ms: None,
        bytes: at(c, &["result"]).map_or(0, json_len),
        edits: edit.into_iter().collect(),
        output: (!said.is_empty()).then(|| said.clone()),
        text: (outcome != Outcome::Ok).then_some(said),
        noted: Vec::new(),
        line: ResultLine::default(),
        persisted_bytes: None,
        return_note: None,
        background_task: None,
    })
}

/// A shell command's output says it exited with a failure: `Exit Code: N`,
/// N not 0.
fn failed_exit(output: &str) -> bool {
    output.lines().any(|l| l.strip_prefix(EXIT_CODE).is_some_and(|code| code.trim() != "0"))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use superx_kernel::message::json_to_object;

    use super::*;

    fn line(json: serde_json::Value) -> Value {
        Value::Object(json_to_object(&json))
    }

    fn when() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 25, 7, 13, 40).single().expect("time")
    }

    fn only_reply(read: &Read) -> &ReplyLine {
        assert_eq!(read.facts.len(), 1, "one fact: {read:?}");
        read.facts
            .iter()
            .find_map(|f| match f {
                Fact::Reply(r) => Some(r.as_ref()),
                _ => None,
            })
            .expect("a reply")
    }

    #[test]
    fn the_operators_text_is_a_prompt_and_handed_back_results_are_not() {
        let typed = read(&line(serde_json::json!({ "id": "u1", "type": "user", "content": [{ "text": "fix it" }, { "inlineData": {} }] })), when());
        assert_eq!(
            typed.facts,
            vec![Fact::Prompt(Prompt { kind: PromptKind::Typed, text: "fix it".into(), images: 1, stops_tools: false, turn: TurnPosition::default() })]
        );
        let handed = read(&line(serde_json::json!({ "id": "u2", "type": "user", "content": [{ "functionResponse": { "id": "c1" } }] })), when());
        assert!(handed.known && handed.facts.is_empty());
    }

    #[test]
    fn a_record_is_the_whole_reply_so_far() {
        let record = line(serde_json::json!({
            "id": "g1", "type": "gemini", "model": "gemini-3.1-pro-preview", "content": "Done.",
            "timestamp": "2026-10-25T07:13:30Z",
            "tokens": { "input": 1000, "output": 50, "cached": 600, "thoughts": 200, "tool": 30, "total": 1280 },
            "thoughts": [{ "subject": "Plan", "description": "Read the file", "timestamp": "2026-10-25T07:13:31Z" }],
            "toolCalls": [
                { "id": "c1", "name": "replace", "status": "success", "timestamp": "2026-10-25T07:13:32Z",
                  "args": { "file_path": "/repo/a.rs", "old_string": "a\nb", "new_string": "c" },
                  "result": [{ "functionResponse": { "id": "c1", "name": "replace", "response": { "output": "Successfully modified file" } } }] },
                { "id": "c2", "name": "run_shell_command", "status": "success", "timestamp": "2026-10-25T07:13:33Z",
                  "args": { "command": "cargo test", "dir_path": "/repo" },
                  "result": [{ "functionResponse": { "id": "c2", "name": "run_shell_command", "response": { "output": "Output: test result: FAILED. 1 passed; 1 failed;\nExit Code: 101\n" } } }] },
                { "id": "c3", "name": "write_file", "status": "executing", "args": { "file_path": "/repo/b.md", "content": "x" }, "result": [] }
            ]
        }));
        let read = read(&record, when());
        assert!(read.known, "{read:?}");
        let r = only_reply(&read);
        assert!(r.cumulative);
        assert_eq!(r.text, Text::Total(5));
        let c = r.counts.as_ref().expect("counts");
        assert_eq!(
            (c.input_tokens, c.cache_read_tokens, c.output_tokens, c.thinking_tokens, c.tool_tokens),
            (400, 600, 250, 200, Some(30)),
            "input less cached; output with thinking"
        );
        assert_eq!(r.thinking.len(), 1);
        assert_eq!((r.thinking[0].position, r.thinking[0].chars, r.thinking[0].ms), (Some(0), 17, None));
        assert_eq!(
            (r.thinking[0].text.as_str(), r.thinking[0].subject.as_deref(), r.thinking[0].signature_chars),
            ("Read the file", Some("Plan"), 0),
            "a thought keeps its description whole, its subject apart, and no signature"
        );
        assert_eq!(r.thinking_dropped, 0);
        assert_eq!(r.calls.iter().map(|c| c.call_id.as_str()).collect::<Vec<_>>(), ["c1", "c2", "c3"]);
        assert_eq!((r.calls[1].command.as_deref(), r.calls[1].dir.as_deref(), r.calls[1].path.as_deref()), (Some("cargo test"), Some("/repo"), None));
        assert_eq!(r.results.len(), 2, "a call still running has no result");
        assert_eq!(
            r.results[0].edits,
            vec![Edit { path: "/repo/a.rs".into(), kind: EditKind::Modified, added: 1, removed: 2, counted_from: CountedFrom::Strings, operator_changed: false }]
        );
        assert_eq!(r.results[1].outcome, Outcome::Error, "the exit code says it failed");
        assert!(r.results[1].output.as_deref().is_some_and(|o| o.contains("test result")));
    }

    #[test]
    fn a_reply_whose_model_is_not_named_yet_waits_for_it() {
        let first = read(&line(serde_json::json!({ "id": "g2", "type": "gemini", "content": "" })), when());
        let r = only_reply(&first);
        assert_eq!(r.model, None);
        assert_eq!(r.counts, None);
    }
}
