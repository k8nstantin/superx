//! Claude Code's transcript lines. The kernel keeps its user and assistant
//! lines as `message` rows and every other line whole as a
//! `transcript_event` (#471), the line itself in each.
//!
//! Shapes, mapped from the 194 transcripts on this machine (2026-10-25):
//!
//! - An assistant line carries one content block of one API response:
//!   `message.id` names the response, `apiBlockIndex` the block's place in
//!   it, and every line of the response repeats its `usage` and
//!   `stop_reason`. A `thinking` block keeps only its signature unless the
//!   text was kept; the line carries `thinkingDurationMs` when Claude Code
//!   timed it. `effort`, `perTurnEffort`, `advisorModel`, `requestedModel`,
//!   `requestId` and `isSidechain` ride on the line. Before Claude Code
//!   wrote `requestedModel`, a `fallback` block named the model asked for
//!   (`from`) when another answered (`to`). A response whose
//!   `model` is `<synthetic>` is a notice Claude Code wrote itself (a spend
//!   limit, an API error), not a model's reply.
//! - A user line is the operator's only when `origin.kind` is `human`;
//!   task notifications, peers, coordinators and reminders arrive on the
//!   same kind of line. A `!` command (`<bash-input>`), a slash command
//!   (`<command-name>`) and an interrupt (`[Request interrupted by user…`)
//!   carry no origin. Tool results come back on user lines, one
//!   `tool_result` block each, with what the tool reported in
//!   `toolUseResult`.
//! - `system` lines: `turn_duration` (`durationMs`, `messageCount`),
//!   `compact_boundary` (`compactMetadata`), `away_summary` (`content`).
//! - An `attachment` line of type `queued_command` is a prompt typed while
//!   the agent worked; `origin.kind` is `human` when it is the operator's.
//! - Every line carries `cwd`, `gitBranch`, `entrypoint` and `version`.

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;

use crate::reader::facts::{
    Away, Call, Compaction, Counts, CountedFrom, Edit, EditKind, Fact, GitAction, GitEvent, Header, Outcome, Place,
    Prompt, PromptKind, Read, ReplyLine, Text, Thinking, ToolResult, Turn,
};
use crate::reader::value::{at, chars, flag, float, int, json_len, lines, list, object, some_text, text};

/// The agent, as the kernel names it.
pub const AGENT: &str = superx_kernel::adapters::claude_code::AGENT_NAME;

/// The tool that runs shell commands.
pub const SHELL_TOOL: &str = "Bash";

/// The `system` lines the reader reads.
pub const SYSTEM_SUBTYPES: [&str; 3] = ["turn_duration", "compact_boundary", "away_summary"];

/// The `attachment` lines the reader reads.
pub const ATTACHMENT_TYPES: [&str; 1] = ["queued_command"];

/// The model Claude Code names on a notice it wrote itself.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// What the operator's refusal of a tool call returns.
const REFUSALS: [&str; 2] = ["The user doesn't want to proceed with this tool use", "User rejected tool use"];

/// How an interrupt begins, and how one that stopped tool calls does.
const INTERRUPT: &str = "[Request interrupted by user";
const INTERRUPT_FOR_TOOLS: &str = "[Request interrupted by user for tool use";

/// One line, read. `at_time` is when the line was written.
#[must_use]
pub fn read(line: &Value, at_time: DateTime<Utc>) -> Read {
    let mut read = match text(line, &["type"]) {
        Some("assistant") => assistant(line, at_time),
        Some("user") => user(line, at_time),
        Some("system") => system(line),
        Some("attachment") => attachment(line),
        _ => Read::unknown(),
    };
    read.header =
        Header { entrypoint: some_text(line, &["entrypoint"]), client_version: some_text(line, &["version"]) };
    read.place = some_text(line, &["cwd"]).map(|cwd| Place {
        cwd,
        branch: some_text(line, &["gitBranch"]),
        says_branch: at(line, &["gitBranch"]).is_some(),
    });
    read
}

fn assistant(line: &Value, at_time: DateTime<Utc>) -> Read {
    let Some(agent_id) = some_text(line, &["message", "id"]) else {
        return Read::unknown();
    };
    let model = some_text(line, &["message", "model"]);
    if model.as_deref() == Some(SYNTHETIC_MODEL) {
        return Read::nothing();
    }
    let mut known = model.is_some();
    let first = int(line, &["apiBlockIndex"]);
    let blocks = list(line, &["message", "content"]);
    let mut added = 0;
    let mut thinking = Vec::new();
    let mut calls = Vec::new();
    let mut fell_back_from = None;
    for (i, block) in blocks.iter().enumerate() {
        let on_line = i64::try_from(i).unwrap_or(i64::MAX);
        match text(block, &["type"]) {
            Some("text") => added += chars(text(block, &["text"]).unwrap_or_default()),
            Some(kind @ ("thinking" | "redacted_thinking")) => thinking.push(Thinking {
                position: first.map(|f| f + on_line),
                on_line,
                at: at_time,
                ms: int(line, &["thinkingDurationMs"]).filter(|ms| *ms >= 0),
                chars: if kind == "thinking" { chars(text(block, &["thinking"]).unwrap_or_default()) } else { 0 },
                display: some_text(line, &["thinkingDisplay"]),
            }),
            Some("tool_use") => match call(block, at_time) {
                Some(c) => calls.push(c),
                None => known = false,
            },
            Some("fallback") => fell_back_from = some_text(block, &["from", "model"]),
            // Server tools (web search and fetch) are counted in `usage`.
            Some("server_tool_use" | "web_search_tool_result" | "web_fetch_tool_result") => {}
            _ => known = false,
        }
    }
    let counts = counts(line);
    if counts.is_none() {
        known = false;
    }
    Read {
        known,
        facts: vec![Fact::Reply(Box::new(ReplyLine {
            agent_id,
            request_id: some_text(line, &["requestId"]),
            model,
            requested_model: some_text(line, &["requestedModel"]).or(fell_back_from),
            advisor_model: some_text(line, &["advisorModel"]),
            effort: some_text(line, &["effort"]),
            turn_effort: some_text(line, &["perTurnEffort"]),
            stop_reason: some_text(line, &["message", "stop_reason"]),
            subagent: flag(line, &["isSidechain"]).unwrap_or(false),
            counts,
            text: Text::Added(added),
            thinking,
            calls,
            results: Vec::new(),
            blocks: i64::try_from(blocks.len()).unwrap_or(i64::MAX),
            cumulative: false,
        }))],
        ..Read::default()
    }
}

/// The reply's counts, from its `usage`; none when a count is missing or
/// negative, which no reply has.
fn counts(line: &Value) -> Option<Counts> {
    let usage = at(line, &["message", "usage"])?;
    let count = |path: &[&str]| int(usage, path).filter(|n| *n >= 0);
    let optional = |path: &[&str]| match int(usage, path) {
        Some(n) if n < 0 => Err(()),
        n => Ok(n),
    };
    Some(Counts {
        input_tokens: count(&["input_tokens"])?,
        cache_read_tokens: count(&["cache_read_input_tokens"]).unwrap_or(0),
        cache_write_tokens: count(&["cache_creation_input_tokens"]).unwrap_or(0),
        output_tokens: count(&["output_tokens"])?,
        thinking_tokens: optional(&["output_tokens_details", "thinking_tokens"]).ok()?.unwrap_or(0),
        tool_tokens: None,
        web_searches: optional(&["server_tool_use", "web_search_requests"]).ok()?,
        web_fetches: optional(&["server_tool_use", "web_fetch_requests"]).ok()?,
        service_tier: some_text(usage, &["service_tier"]),
        usage: object(line, &["message", "usage"])?.clone(),
    })
}

/// A `tool_use` block. Its input is read by what it names, whatever the
/// tool: a file, a command, a search, a page, a subagent.
fn call(block: &Value, at_time: DateTime<Utc>) -> Option<Call> {
    let input = at(block, &["input"]);
    let field = |key: &str| input.and_then(|i| some_text(i, &[key]));
    Some(Call {
        call_id: some_text(block, &["id"])?,
        at: at_time,
        tool: some_text(block, &["name"])?,
        path: field("file_path").or_else(|| field("notebook_path")).or_else(|| field("path")),
        command: field("command"),
        description: field("description"),
        pattern: field("pattern"),
        url: field("url"),
        query: field("query"),
        subagent_type: field("subagent_type"),
        background: input.and_then(|i| flag(i, &["run_in_background"])).unwrap_or(false),
        input_bytes: input.map_or(0, json_len),
        dir: None,
    })
}

fn user(line: &Value, at_time: DateTime<Utc>) -> Read {
    match at(line, &["message", "content"]) {
        Some(Value::String(s)) => prompt(line, s.clone(), 0).map_or_else(Read::nothing, |p| Read {
            known: true,
            facts: vec![Fact::Prompt(p)],
            ..Read::default()
        }),
        Some(Value::Array(blocks)) => {
            if blocks.iter().any(|b| text(b, &["type"]) == Some("tool_result")) {
                return results(line, blocks, at_time);
            }
            let mut known = true;
            let mut texts = Vec::new();
            let mut images = 0;
            for b in blocks.iter() {
                match text(b, &["type"]) {
                    Some("text") => texts.push(text(b, &["text"]).unwrap_or_default()),
                    Some("image") => images += 1,
                    Some("document") => {}
                    _ => known = false,
                }
            }
            let facts = prompt(line, texts.join("\n"), images).map(Fact::Prompt).into_iter().collect();
            Read { known, facts, ..Read::default() }
        }
        _ => Read::unknown(),
    }
}

/// Whose a user line is, and what kind of message: the operator's only.
fn prompt(line: &Value, message: String, images: i64) -> Option<Prompt> {
    if ["isSidechain", "isMeta", "isCompactSummary"].iter().any(|key| flag(line, &[key]) == Some(true)) {
        return None;
    }
    let kind = if message.starts_with(INTERRUPT) {
        PromptKind::Interrupt
    } else if message.starts_with("<bash-input>") {
        PromptKind::Shell
    } else if message.starts_with("<command-name>") {
        PromptKind::Command
    } else {
        match at(line, &["origin"]) {
            Some(_) if text(line, &["origin", "kind"]) == Some("human") => PromptKind::Typed,
            Some(_) => return None,
            // Before Claude Code marked who wrote a line, a plain message
            // was the operator's; tags were Claude Code's own.
            None if !message.starts_with('<') => PromptKind::Typed,
            None => return None,
        }
    };
    if kind != PromptKind::Typed && text(line, &["origin", "kind"]).is_some_and(|k| k != "human") {
        return None;
    }
    Some(Prompt { kind, stops_tools: message.starts_with(INTERRUPT_FOR_TOOLS), images, text: message })
}

/// A user line of tool results: one block each, with what the tool
/// reported beside it when the line holds one.
fn results(line: &Value, blocks: &[Value], at_time: DateTime<Utc>) -> Read {
    let reported = at(line, &["toolUseResult"]);
    let only_one = blocks.iter().filter(|b| text(b, &["type"]) == Some("tool_result")).count() == 1;
    let mut known = true;
    let mut facts = Vec::new();
    for b in blocks {
        match text(b, &["type"]) {
            Some("tool_result") => {
                let Some(call_id) = some_text(b, &["tool_use_id"]) else {
                    known = false;
                    continue;
                };
                facts.push(Fact::Result(result(call_id, b, if only_one { reported } else { None }, at_time)));
            }
            Some("text" | "image") => {}
            _ => known = false,
        }
    }
    Read { known, facts, ..Read::default() }
}

fn result(call_id: String, block: &Value, reported: Option<&Value>, at_time: DateTime<Utc>) -> ToolResult {
    let content = at(block, &["content"]);
    let said = content.map(content_text).unwrap_or_default();
    let outcome = if flag(block, &["is_error"]) == Some(true) {
        if REFUSALS.iter().any(|r| said.starts_with(r)) {
            Outcome::Denied
        } else {
            Outcome::Error
        }
    } else if reported.and_then(|r| int(r, &["timedOutAfterMs"])).is_some() {
        Outcome::TimedOut
    } else if reported.and_then(|r| flag(r, &["interrupted"])) == Some(true) {
        Outcome::Interrupted
    } else {
        Outcome::Ok
    };
    let ms = reported.and_then(|r| {
        int(r, &["durationMs"])
            .or_else(|| {
                let secs = std::time::Duration::try_from_secs_f64(float(r, &["durationSeconds"])?).ok()?;
                i64::try_from(secs.as_millis()).ok()
            })
            .filter(|ms| *ms >= 0)
    });
    let bytes = match content {
        Some(Value::String(s)) => i64::try_from(s.len()).unwrap_or(i64::MAX),
        Some(other) => json_len(other),
        None => 0,
    };
    let output = match reported {
        Some(r) if at(r, &["stdout"]).is_some() || at(r, &["stderr"]).is_some() => {
            Some(format!("{}\n{}", text(r, &["stdout"]).unwrap_or_default(), text(r, &["stderr"]).unwrap_or_default()))
        }
        _ if !said.is_empty() => Some(said),
        _ => None,
    };
    ToolResult {
        call_id,
        at: at_time,
        outcome,
        ms,
        bytes,
        edits: if outcome == Outcome::Ok { reported.and_then(edit).into_iter().collect() } else { Vec::new() },
        output,
        noted: reported.map(noted).unwrap_or_default(),
    }
}

/// A tool result's content as text: a string, or its text blocks.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            blocks.iter().filter_map(|b| text(b, &["text"])).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

/// The file a file tool changed, from what it reported.
fn edit(reported: &Value) -> Option<Edit> {
    let path = some_text(reported, &["filePath"])?;
    let operator_changed = flag(reported, &["userModified"]).unwrap_or(false);
    let created = text(reported, &["type"]) == Some("create");
    let kind = if created { EditKind::Created } else { EditKind::Modified };
    let hunks = list(reported, &["structuredPatch"]);
    if !hunks.is_empty() {
        let (mut added, mut removed) = (0, 0);
        for hunk in hunks {
            for l in list(hunk, &["lines"]) {
                match l {
                    Value::String(s) if s.starts_with('+') => added += 1,
                    Value::String(s) if s.starts_with('-') => removed += 1,
                    _ => {}
                }
            }
        }
        return Some(Edit { path, kind, added, removed, counted_from: CountedFrom::Patch, operator_changed });
    }
    if created {
        let content = text(reported, &["content"])?;
        return Some(Edit {
            path,
            kind,
            added: lines(content),
            removed: 0,
            counted_from: CountedFrom::Content,
            operator_changed,
        });
    }
    let (old, new) = (text(reported, &["oldString"])?, text(reported, &["newString"])?);
    (old != new).then(|| Edit {
        path,
        kind,
        added: lines(new),
        removed: lines(old),
        counted_from: CountedFrom::Strings,
        operator_changed,
    })
}

/// The git and GitHub actions Claude Code noted on a result
/// (`gitOperation`): a pull request created or merged, a push, a merge.
fn noted(reported: &Value) -> Vec<GitEvent> {
    let mut out = Vec::new();
    if let Some(pr) = at(reported, &["gitOperation", "pr"]) {
        let action = match text(pr, &["action"]) {
            Some("created") => Some(GitAction::PrCreated),
            Some("merged") => Some(GitAction::PrMerged),
            _ => None,
        };
        if let Some(action) = action {
            out.push(GitEvent {
                pr_number: int(pr, &["number"]).filter(|n| *n > 0),
                url: some_text(pr, &["url"]),
                ..GitEvent::of(action)
            });
        }
    }
    if let Some(push) = at(reported, &["gitOperation", "push"]) {
        out.push(GitEvent { branch: some_text(push, &["branch"]), ..GitEvent::of(GitAction::Push) });
    }
    if let Some(branch) = at(reported, &["gitOperation", "branch"]) {
        if text(branch, &["action"]) == Some("merged") {
            // The ref is what Claude Code saw on the command line: a commit,
            // a branch, or a shell variable it could not see the value of.
            let target = some_text(branch, &["ref"]).filter(|r| !r.starts_with('$'));
            let is_hash = target.as_deref().is_some_and(super::is_hash);
            out.push(GitEvent {
                hash: target.clone().filter(|_| is_hash),
                branch: target.filter(|_| !is_hash),
                ..GitEvent::of(GitAction::Merge)
            });
        }
    }
    if let Some(commit) = at(reported, &["gitOperation", "commit"]) {
        let hash = some_text(commit, &["sha"]).or_else(|| some_text(commit, &["hash"])).filter(|h| super::is_hash(h));
        out.push(GitEvent { hash, ..GitEvent::of(GitAction::Commit) });
    }
    out
}

fn system(line: &Value) -> Read {
    let fact = match text(line, &["subtype"]) {
        Some("turn_duration") => {
            match (int(line, &["durationMs"]).filter(|n| *n >= 0), int(line, &["messageCount"]).filter(|n| *n >= 0)) {
                (Some(ms), Some(lines)) => Fact::Turn(Turn { ms, lines }),
                _ => return Read::unknown(),
            }
        }
        Some("compact_boundary") => {
            let meta = |key: &str| int(line, &["compactMetadata", key]).filter(|n| *n >= 0);
            let (Some(trigger), Some(tokens_before)) =
                (text(line, &["compactMetadata", "trigger"]).filter(|t| matches!(*t, "auto" | "manual")), meta("preTokens"))
            else {
                return Read::unknown();
            };
            Fact::Compaction(Compaction {
                trigger: trigger.to_string(),
                tokens_before,
                tokens_after: meta("postTokens"),
                tokens_dropped: meta("cumulativeDroppedTokens"),
                ms: meta("durationMs"),
            })
        }
        Some("away_summary") => match text(line, &["content"]) {
            Some(t) => Fact::Away(Away { text: t.to_string() }),
            None => return Read::unknown(),
        },
        _ => return Read::nothing(),
    };
    Read { known: true, facts: vec![fact], ..Read::default() }
}

fn attachment(line: &Value) -> Read {
    if text(line, &["attachment", "type"]) != Some("queued_command") {
        return Read::nothing();
    }
    let Some(a) = at(line, &["attachment"]) else {
        return Read::unknown();
    };
    let (message, images) = match at(a, &["prompt"]) {
        Some(Value::String(s)) => (s.clone(), i64::try_from(list(a, &["imagePasteIds"]).len()).unwrap_or(0)),
        Some(Value::Array(parts)) => (
            parts.iter().filter_map(|p| text(p, &["text"])).collect::<Vec<_>>().join("\n"),
            i64::try_from(parts.iter().filter(|p| text(p, &["type"]) == Some("image")).count()).unwrap_or(0),
        ),
        _ => return Read::unknown(),
    };
    let kind = match text(a, &["origin", "kind"]) {
        Some("human") if message.starts_with("<bash-input>") => PromptKind::Shell,
        Some("human") if message.starts_with("<command-name>") => PromptKind::Command,
        Some("human") => PromptKind::Queued,
        // Claude Code 2.1.295 left a queued `!` command unmarked.
        None if message.starts_with("<bash-input>") => PromptKind::Shell,
        _ => return Read::nothing(),
    };
    Read {
        known: true,
        facts: vec![Fact::Prompt(Prompt { kind, text: message, images, stops_tools: false })],
        ..Read::default()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use superx_kernel::message::json_to_object;

    use super::*;
    use crate::reader::facts::{GitAction, PromptKind};

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

    fn only_prompt(read: &Read) -> Option<&Prompt> {
        assert!(read.facts.len() <= 1, "one prompt or none: {read:?}");
        read.facts.iter().find_map(|f| match f {
            Fact::Prompt(p) => Some(p),
            _ => None,
        })
    }

    fn only_result(read: &Read) -> &ToolResult {
        assert_eq!(read.facts.len(), 1, "one fact: {read:?}");
        read.facts
            .iter()
            .find_map(|f| match f {
                Fact::Result(r) => Some(r),
                _ => None,
            })
            .expect("a result")
    }

    fn assistant(block: serde_json::Value, extra: serde_json::Value) -> Value {
        let mut json = serde_json::json!({
            "type": "assistant", "sessionId": "s1", "cwd": "/repo", "gitBranch": "main",
            "entrypoint": "cli", "version": "2.1.304", "isSidechain": false, "requestId": "req_1",
            "effort": "max", "perTurnEffort": "max", "advisorModel": "claude-opus-5-5", "apiBlockIndex": 1,
            "message": {
                "id": "msg_1", "model": "claude-opus-5-5", "stop_reason": "tool_use", "content": [block],
                "usage": {
                    "input_tokens": 6, "cache_creation_input_tokens": 1200, "cache_read_input_tokens": 90000,
                    "output_tokens": 800, "service_tier": "standard",
                    "output_tokens_details": { "thinking_tokens": 500 },
                    "server_tool_use": { "web_search_requests": 1, "web_fetch_requests": 0 }
                }
            }
        });
        if let (Some(o), serde_json::Value::Object(e)) = (json.as_object_mut(), extra) {
            o.extend(e);
        }
        line(json)
    }

    #[test]
    fn a_thinking_line_says_where_it_sits_how_long_it_thought_and_what_the_reply_used() {
        let read = read(
            &assistant(
                serde_json::json!({ "type": "thinking", "thinking": "", "signature": "sig" }),
                serde_json::json!({ "thinkingDurationMs": 4200, "thinkingDisplay": "summarized" }),
            ),
            when(),
        );
        assert!(read.known);
        assert_eq!(read.header, Header { entrypoint: Some("cli".into()), client_version: Some("2.1.304".into()) });
        assert_eq!(read.place, Some(Place { cwd: "/repo".into(), branch: Some("main".into()), says_branch: true }));
        let r = only_reply(&read);
        assert_eq!((r.agent_id.as_str(), r.model.as_deref(), r.stop_reason.as_deref()), ("msg_1", Some("claude-opus-5-5"), Some("tool_use")));
        assert_eq!((r.effort.as_deref(), r.turn_effort.as_deref(), r.advisor_model.as_deref()), (Some("max"), Some("max"), Some("claude-opus-5-5")));
        assert_eq!(
            r.thinking,
            vec![Thinking { position: Some(1), on_line: 0, at: when(), ms: Some(4200), chars: 0, display: Some("summarized".into()) }]
        );
        let c = r.counts.as_ref().expect("counts");
        assert_eq!(
            (c.input_tokens, c.cache_read_tokens, c.cache_write_tokens, c.output_tokens, c.thinking_tokens),
            (6, 90_000, 1_200, 800, 500)
        );
        assert_eq!((c.web_searches, c.web_fetches, c.tool_tokens), (Some(1), Some(0), None));
        assert_eq!(r.text, Text::Added(0));
        assert!(!r.cumulative);
    }

    #[test]
    fn a_tool_use_line_is_a_call_read_by_what_its_input_names() {
        let input = serde_json::json!({ "command": "cargo test", "description": "Run the tests", "run_in_background": true });
        let bash = read(
            &assistant(serde_json::json!({ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": input, "caller": {} }), serde_json::json!({})),
            when(),
        );
        let call = &only_reply(&bash).calls[0];
        assert_eq!((call.call_id.as_str(), call.tool.as_str()), ("toolu_1", "Bash"));
        assert_eq!((call.command.as_deref(), call.description.as_deref(), call.background), (Some("cargo test"), Some("Run the tests"), true));
        assert_eq!(call.input_bytes, i64::try_from(serde_json::to_string(&input).expect("json").len()).expect("fits"));
        let edit = read(
            &assistant(serde_json::json!({ "type": "tool_use", "id": "toolu_2", "name": "Edit", "input": { "file_path": "/repo/a.rs", "old_string": "a", "new_string": "b" } }), serde_json::json!({})),
            when(),
        );
        assert_eq!(only_reply(&edit).calls[0].path.as_deref(), Some("/repo/a.rs"));
    }

    #[test]
    fn a_text_line_adds_its_characters_to_the_answer() {
        let read = read(&assistant(serde_json::json!({ "type": "text", "text": "naïve answer" }), serde_json::json!({})), when());
        assert_eq!(only_reply(&read).text, Text::Added(12));
    }

    #[test]
    fn a_notice_claude_code_wrote_itself_is_no_reply_and_an_unknown_block_is_counted() {
        let notice = line(serde_json::json!({ "type": "assistant", "message": { "id": "x", "model": "<synthetic>", "content": [{ "type": "text", "text": "You've hit your limit" }] } }));
        let read_notice = read(&notice, when());
        assert!(read_notice.known && read_notice.facts.is_empty());
        let fell_back = read(
            &assistant(serde_json::json!({ "type": "fallback", "from": { "model": "claude-fable-5" }, "to": { "model": "claude-opus-5" } }), serde_json::json!({})),
            when(),
        );
        assert!(fell_back.known);
        assert_eq!(only_reply(&fell_back).requested_model.as_deref(), Some("claude-fable-5"), "the model asked for");
        let strange = read(&assistant(serde_json::json!({ "type": "hologram" }), serde_json::json!({})), when());
        assert!(!strange.known, "a block it does not know is counted");
        assert_eq!(only_reply(&strange).agent_id, "msg_1", "what it knows is still taken");
    }

    fn user(content: serde_json::Value, extra: serde_json::Value) -> Value {
        let mut json = serde_json::json!({ "type": "user", "sessionId": "s1", "cwd": "/repo", "message": { "role": "user", "content": content } });
        if let (Some(o), serde_json::Value::Object(e)) = (json.as_object_mut(), extra) {
            o.extend(e);
        }
        line(json)
    }

    #[test]
    fn only_the_operators_lines_are_prompts() {
        let typed = read(&user(serde_json::json!("ship it"), serde_json::json!({ "origin": { "kind": "human" } })), when());
        assert_eq!(only_prompt(&typed).map(|p| (p.kind, p.text.as_str(), p.images)), Some((PromptKind::Typed, "ship it", 0)));
        let with_image = read(
            &user(serde_json::json!([{ "type": "image", "source": {} }, { "type": "text", "text": "look" }]), serde_json::json!({ "origin": { "kind": "human" } })),
            when(),
        );
        assert_eq!(only_prompt(&with_image).map(|p| (p.text.as_str(), p.images)), Some(("look", 1)));
        let shell = read(&user(serde_json::json!("<bash-input>ls</bash-input>"), serde_json::json!({})), when());
        assert_eq!(only_prompt(&shell).map(|p| p.kind), Some(PromptKind::Shell));
        let command = read(&user(serde_json::json!("<command-name>/clear</command-name>"), serde_json::json!({})), when());
        assert_eq!(only_prompt(&command).map(|p| p.kind), Some(PromptKind::Command));
        let stop = read(&user(serde_json::json!([{ "type": "text", "text": "[Request interrupted by user for tool use]" }]), serde_json::json!({})), when());
        assert_eq!(only_prompt(&stop).map(|p| (p.kind, p.stops_tools)), Some((PromptKind::Interrupt, true)));
        let plain_stop = read(&user(serde_json::json!([{ "type": "text", "text": "[Request interrupted by user]" }]), serde_json::json!({})), when());
        assert_eq!(only_prompt(&plain_stop).map(|p| (p.kind, p.stops_tools)), Some((PromptKind::Interrupt, false)));
        let older = read(&user(serde_json::json!("fix the build"), serde_json::json!({})), when());
        assert_eq!(only_prompt(&older).map(|p| p.kind), Some(PromptKind::Typed), "before origin was written");
        for (content, extra) in [
            (serde_json::json!("<task-notification>done</task-notification>"), serde_json::json!({ "origin": { "kind": "task-notification" } })),
            (serde_json::json!("review this"), serde_json::json!({ "origin": { "kind": "peer" }, "isMeta": true })),
            (serde_json::json!("summary of the conversation"), serde_json::json!({ "isCompactSummary": true })),
            (serde_json::json!("do the subtask"), serde_json::json!({ "isSidechain": true })),
            (serde_json::json!("<bash-stdout>a</bash-stdout>"), serde_json::json!({})),
            (serde_json::json!("<local-command-stdout>x</local-command-stdout>"), serde_json::json!({})),
            (serde_json::json!("not the operator"), serde_json::json!({ "origin": null })),
        ] {
            let r = read(&user(content.clone(), extra), when());
            assert!(r.known && r.facts.is_empty(), "{content} is not the operator's: {r:?}");
        }
    }

    fn result_line(block: serde_json::Value, reported: serde_json::Value) -> Value {
        user(serde_json::json!([block]), serde_json::json!({ "toolUseResult": reported }))
    }

    #[test]
    fn a_result_says_how_the_call_ended() {
        let ok = read(
            &result_line(
                serde_json::json!({ "type": "tool_result", "tool_use_id": "toolu_1", "content": "out" }),
                serde_json::json!({ "stdout": "[main 4b9bba8] x", "stderr": "warn", "interrupted": false }),
            ),
            when(),
        );
        let r = only_result(&ok);
        assert_eq!((r.call_id.as_str(), r.outcome, r.bytes, r.output.as_deref()), ("toolu_1", Outcome::Ok, 3, Some("[main 4b9bba8] x\nwarn")));
        let failed = read(
            &result_line(serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "Exit code 1\nboom", "is_error": true }), serde_json::json!("Error: Exit code 1\nboom")),
            when(),
        );
        assert_eq!((only_result(&failed).outcome, only_result(&failed).output.as_deref()), (Outcome::Error, Some("Exit code 1\nboom")));
        let denied = read(
            &result_line(
                serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "The user doesn't want to proceed with this tool use. The tool use was rejected", "is_error": true }),
                serde_json::json!("User rejected tool use"),
            ),
            when(),
        );
        assert_eq!(only_result(&denied).outcome, Outcome::Denied);
        let timed_out = read(
            &result_line(serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "" }), serde_json::json!({ "stdout": "", "timedOutAfterMs": 120_000, "backgroundTaskId": "b1" })),
            when(),
        );
        assert_eq!(only_result(&timed_out).outcome, Outcome::TimedOut);
        let fetched = read(&result_line(serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "page" }), serde_json::json!({ "durationMs": 812, "code": 200 })), when());
        assert_eq!(only_result(&fetched).ms, Some(812));
        let searched = read(&result_line(serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "hits" }), serde_json::json!({ "durationSeconds": 1.5 })), when());
        assert_eq!(only_result(&searched).ms, Some(1_500));
    }

    #[test]
    fn a_file_tool_result_says_which_file_changed_and_by_how_many_lines() {
        let patched = read(
            &result_line(
                serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "ok" }),
                serde_json::json!({ "filePath": "/repo/a.rs", "oldString": "a", "newString": "b", "userModified": true,
                    "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 3, "lines": [" keep", "-gone", "+new", "+added"] }] }),
            ),
            when(),
        );
        assert_eq!(
            only_result(&patched).edits,
            vec![Edit { path: "/repo/a.rs".into(), kind: EditKind::Modified, added: 2, removed: 1, counted_from: CountedFrom::Patch, operator_changed: true }]
        );
        let created = read(
            &result_line(
                serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "ok" }),
                serde_json::json!({ "type": "create", "filePath": "/repo/new.md", "content": "one\ntwo\n", "structuredPatch": [], "userModified": false }),
            ),
            when(),
        );
        assert_eq!(
            only_result(&created).edits,
            vec![Edit { path: "/repo/new.md".into(), kind: EditKind::Created, added: 2, removed: 0, counted_from: CountedFrom::Content, operator_changed: false }]
        );
        let refused = read(
            &result_line(
                serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "File has not been read yet", "is_error": true }),
                serde_json::json!({ "filePath": "/repo/a.rs", "structuredPatch": [{ "lines": ["+x"] }] }),
            ),
            when(),
        );
        assert!(only_result(&refused).edits.is_empty(), "a call that failed changed nothing");
    }

    #[test]
    fn the_git_actions_claude_code_noted_on_a_result() {
        let r = read(
            &result_line(
                serde_json::json!({ "type": "tool_result", "tool_use_id": "t", "content": "ok" }),
                serde_json::json!({ "stdout": "", "stderr": "", "gitOperation": {
                    "pr": { "number": 507, "url": "https://github.com/k8nstantin/superx/pull/507", "action": "created" },
                    "push": { "branch": "feat/x" },
                    "branch": { "ref": "$NEW", "action": "merged" } } }),
            ),
            when(),
        );
        let noted = &only_result(&r).noted;
        assert_eq!(
            noted.iter().map(|e| (e.action, e.pr_number, e.branch.as_deref())).collect::<Vec<_>>(),
            vec![(GitAction::PrCreated, Some(507), None), (GitAction::Push, None, Some("feat/x")), (GitAction::Merge, None, None)]
        );
    }

    fn system(extra: serde_json::Value) -> Value {
        let mut json = serde_json::json!({ "type": "system", "sessionId": "s1", "cwd": "/repo" });
        if let (Some(o), serde_json::Value::Object(e)) = (json.as_object_mut(), extra) {
            o.extend(e);
        }
        line(json)
    }

    #[test]
    fn turns_compactions_and_recaps() {
        let turn = read(&system(serde_json::json!({ "subtype": "turn_duration", "durationMs": 149_008, "messageCount": 73 })), when());
        assert_eq!(turn.facts, vec![Fact::Turn(Turn { ms: 149_008, lines: 73 })]);
        let compacted = read(
            &system(serde_json::json!({ "subtype": "compact_boundary", "compactMetadata": { "trigger": "auto", "preTokens": 972_688, "postTokens": 20_698, "cumulativeDroppedTokens": 951_990, "durationMs": 60_838 } })),
            when(),
        );
        assert_eq!(
            compacted.facts,
            vec![Fact::Compaction(Compaction { trigger: "auto".into(), tokens_before: 972_688, tokens_after: Some(20_698), tokens_dropped: Some(951_990), ms: Some(60_838) })]
        );
        let away = read(&system(serde_json::json!({ "subtype": "away_summary", "content": "While you were away" })), when());
        assert_eq!(away.facts, vec![Fact::Away(Away { text: "While you were away".into() })]);
        assert!(!read(&system(serde_json::json!({ "subtype": "turn_duration" })), when()).known);
    }

    fn queued(attachment: serde_json::Value) -> Value {
        line(serde_json::json!({ "type": "attachment", "sessionId": "s1", "cwd": "/repo", "attachment": attachment }))
    }

    #[test]
    fn a_prompt_typed_while_the_agent_worked() {
        let typed = read(&queued(serde_json::json!({ "type": "queued_command", "prompt": "also this", "commandMode": "prompt", "origin": { "kind": "human" } })), when());
        assert_eq!(only_prompt(&typed).map(|p| (p.kind, p.text.as_str())), Some((PromptKind::Queued, "also this")));
        let pasted = read(
            &queued(serde_json::json!({ "type": "queued_command", "prompt": [{ "type": "image" }, { "type": "text", "text": "see" }], "origin": { "kind": "human" } })),
            when(),
        );
        assert_eq!(only_prompt(&pasted).map(|p| (p.text.as_str(), p.images)), Some(("see", 1)));
        let notice = read(&queued(serde_json::json!({ "type": "queued_command", "prompt": "<task-notification>x", "origin": { "kind": "task-notification" } })), when());
        assert!(only_prompt(&notice).is_none());
        let unmarked = read(&queued(serde_json::json!({ "type": "queued_command", "prompt": "<bash-input>gcloud auth login</bash-input>", "commandMode": "prompt" })), when());
        assert_eq!(only_prompt(&unmarked).map(|p| p.kind), Some(PromptKind::Shell));
    }
}
