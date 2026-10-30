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
//!   same kind of line, and are triggers. A `!` command (`<bash-input>`), a
//!   slash command (`<command-name>`) and an interrupt (`[Request
//!   interrupted by user…`) carry no origin. Tool results come back on user
//!   lines, one `tool_result` block each, with what the tool reported in
//!   `toolUseResult`.
//! - `system` lines: `turn_duration` (`durationMs`, `messageCount`),
//!   `compact_boundary` (`compactMetadata`), `away_summary` (`content`).
//! - An `attachment` line of type `queued_command` is a prompt typed while
//!   the agent worked; `origin.kind` is `human` when it is the operator's.
//! - Every line carries `cwd`, `gitBranch`, `entrypoint` and `version`.
//! - Every other kind of line it writes ([`NO_TABLE`]), its other system
//!   lines and attachments, and the files beside its transcripts feed no
//!   table: they are read, counted, and their fields listed.

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;

use crate::crawler::lines::census::Shapes;
use crate::crawler::lines::facts::{
    Away, Call, Compaction, Counts, CountedFrom, Edit, EditKind, Fact, GitAction, GitEvent, Header, Outcome, Place,
    Prompt, PromptKind, Read, ReplyLine, ResultLine, Text, Thinking, ToolResult, Trigger, Turn, TurnPosition,
};
use crate::crawler::lines::kernel::Body;
use crate::crawler::value::{at, chars, flag, float, int, json_len, lines, list, object, some_text, text, Columns};

/// The agent, as the kernel names it.
pub const AGENT: &str = superx_kernel::adapters::claude_code::AGENT_NAME;

/// The tool that runs shell commands.
pub const SHELL_TOOL: &str = "Bash";

/// A file beside the transcripts, as the kernel keeps it (#520).
pub const SIDE_FILE: &str = "file";

/// The kinds of line Claude Code writes that feed no table, beside its
/// other system lines and attachments.
pub const NO_TABLE: [&str; 23] = [
    "file-history-snapshot",
    "file-history-delta",
    "last-prompt",
    "atis-latch",
    "agent-name",
    "fork-context-ref",
    "started",
    "launched",
    "result",
    "failed",
    "artifact-autoreact-ledger",
    "artifact-comment-monitor",
    "api-request",
    "api-request-blob",
    "api-request-shape",
    "ai-title",
    "permission-mode",
    "mode",
    "cost-state",
    "pr-link",
    "frame-link",
    "queue-operation",
    SIDE_FILE,
];

/// How Claude Code's lines name their fields: the blocks of a message's
/// content by their type, and the maps it keys by data.
pub const SHAPES: Shapes = Shapes {
    content: Some("message.content"),
    result: Some("toolUseResult"),
    keyed_by_data: &[
        "modelUsage",
        "snapshot.trackedFileBackups",
        "artifacts",
        "wireToolInputs",
        "wireIngestContext",
        "toolUseResult[AskUserQuestion].answers",
        "toolUseResult[AskUserQuestion].annotations",
        "message.content[tool_use:Artifact].input.files",
    ],
};

/// What the reading takes from every line, whatever its kind: where the
/// session works, how the agent was started and its version, and when the
/// line was written.
pub const STORED_ON_EVERY_LINE: &[&str] = &["cwd", "gitBranch", "entrypoint", "version", "timestamp"];

/// What the reading puts in a column, by kind of line; a path ending in
/// `.**` is held whole, and `*` stands for any tool.
pub const STORED: &[(&str, &[&str])] = &[
    (
        "user",
        &[
            "message.content",
            "message.content[text].text",
            "promptId",
            "promptSource",
            "permissionMode",
            "turnPosition.turnIndex",
            "turnPosition.promptIndex",
            "origin.kind",
            "origin.from",
            "origin.producer",
            "origin.name",
            "message.content[tool_result:*].content",
            "message.content[tool_result:*].content[].text",
            "toolUseResult[*].durationMs",
            "toolUseResult[*].durationSeconds",
            "toolUseResult[*].filePath",
            "toolUseResult[*].userModified",
            "toolUseResult[*].gitOperation.pr.number",
            "toolUseResult[*].gitOperation.pr.url",
            "toolUseResult[*].gitOperation.push.branch",
            "toolUseResult[*].gitOperation.branch.ref",
            "toolUseResult[*].gitOperation.commit.sha",
            "toolUseResult[*].gitOperation.commit.hash",
            "toolUseResult[*].persistedOutputSize",
            "toolUseResult[*].returnCodeInterpretation",
            "toolUseResult[*].backgroundTaskId",
            "toolUseResult[*].bashEditDiff.files[].filePath",
            "permissionDecision.decision",
            "permissionDecision.reasonType",
            "permissionDecision.source",
            "permissionDecision.reasonCode",
            "toolDenialKind",
            "userFeedback",
            "sandboxed",
            "sandboxViolations[].host",
        ],
    ),
    (
        "assistant",
        &[
            "message.id",
            "requestId",
            "message.model",
            "requestedModel",
            "message.content[fallback].from.model",
            "advisorModel",
            "effort",
            "perTurnEffort",
            "message.stop_reason",
            "isSidechain",
            "message.usage.**",
            "attributionSkill",
            "attributionPlugin",
            "attributionMcpServer",
            "attributionMcpTool",
            "message.stop_details.category",
            "message.stop_details.explanation",
            "message.diagnostics.cache_miss_reason.type",
            "message.diagnostics.cache_miss_reason.cache_missed_input_tokens",
            "apiBlockIndex",
            "thinkingDurationMs",
            "thinkingDisplay",
            "message.content[thinking].thinking",
            "message.content[tool_use:*].id",
            "message.content[tool_use:*].name",
            "message.content[tool_use:*].input.file_path",
            "message.content[tool_use:*].input.notebook_path",
            "message.content[tool_use:*].input.path",
            "message.content[tool_use:*].input.command",
            "message.content[tool_use:*].input.description",
            "message.content[tool_use:*].input.pattern",
            "message.content[tool_use:*].input.url",
            "message.content[tool_use:*].input.query",
            "message.content[tool_use:*].input.subagent_type",
            "message.content[tool_use:*].input.run_in_background",
            "message.content[tool_use:*].input.dangerouslyDisableSandbox",
            "message.content[tool_use:*].input.timeout",
            "message.content[tool_use:Skill].input.skill",
            "message.content[tool_use:Agent].input.model",
        ],
    ),
    ("system/turn_duration", &["durationMs", "messageCount"]),
    (
        "system/compact_boundary",
        &[
            "compactMetadata.trigger",
            "compactMetadata.preTokens",
            "compactMetadata.postTokens",
            "compactMetadata.cumulativeDroppedTokens",
            "compactMetadata.durationMs",
        ],
    ),
    ("system/away_summary", &["content"]),
    (
        "attachment/queued_command",
        &[
            "attachment.prompt",
            "attachment.prompt[].text",
            "attachment.origin.kind",
            "attachment.origin.from",
            "attachment.origin.producer",
            "attachment.origin.name",
            "attachment.commandMode",
            "promptId",
            "promptSource",
            "permissionMode",
            "turnPosition.turnIndex",
            "turnPosition.promptIndex",
        ],
    ),
];

/// The model Claude Code names on a notice it wrote itself.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// What the operator's refusal of a tool call returns.
const REFUSALS: [&str; 2] = ["The user doesn't want to proceed with this tool use", "User rejected tool use"];

/// How an interrupt begins, and how one that stopped tool calls does.
const INTERRUPT: &str = "[Request interrupted by user";
const INTERRUPT_FOR_TOOLS: &str = "[Request interrupted by user for tool use";

/// The tool that loads a skill, and the one that starts a subagent.
const SKILL_TOOL: &str = "Skill";
const AGENT_TOOL: &str = "Agent";

/// A line as the kernel kept it, read.
#[must_use]
pub fn read_kept(body: &Body, kernel_kind: Option<&str>, at_time: DateTime<Utc>) -> Read {
    match body {
        Body::Json(line) => read(line, at_time),
        // Kept by its kind alone (before #471): its kind says whether the
        // format knows it, and nothing of it is left to read.
        Body::Missing => match kernel_kind {
            Some(kind) if kind == "system" || kind == "attachment" || NO_TABLE.contains(&kind) => Read::nothing(),
            _ => Read::unknown(),
        },
        Body::Text(_) | Body::NotJson => Read::unknown(),
    }
}

/// A line's kind: its type; a system line's type and subtype, and an
/// attachment's type and its attachment's type, joined by a slash; where
/// the line holds no type, the kind the kernel gave it.
#[must_use]
pub fn kind(line: &Value, kernel_kind: Option<&str>) -> String {
    let joined = |kind: &str, sub: Option<&str>| sub.map_or_else(|| kind.to_string(), |sub| format!("{kind}/{sub}"));
    match text(line, &["type"]) {
        Some("system") => joined("system", text(line, &["subtype"])),
        Some("attachment") => joined("attachment", text(line, &["attachment", "type"])),
        Some(kind) => kind.to_string(),
        None => kernel_kind.unwrap_or_default().to_string(),
    }
}

/// One line, read. `at_time` is when the line was written.
#[must_use]
pub fn read(line: &Value, at_time: DateTime<Utc>) -> Read {
    let mut read = match text(line, &["type"]) {
        Some("assistant") => assistant(line, at_time),
        Some("user") => user(line, at_time),
        Some("system") => system(line),
        Some("attachment") => attachment(line),
        Some(kind) if NO_TABLE.contains(&kind) => Read::nothing(),
        _ => Read::unknown(),
    };
    read.header =
        Header { entrypoint: some_text(line, &["entrypoint"]), client_version: some_text(line, &["version"]) };
    read.place = some_text(line, &["cwd"]).map(|cwd| Place {
        cwd,
        branch: some_text(line, &["gitBranch"]),
        says_branch: at(line, &["gitBranch"]).is_some(),
    });
    let mut columns = Columns::default();
    read.prompt_id = columns.text(line, &["promptId"]);
    read.known &= columns.placed();
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
    let mut columns = Columns::default();
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
            Some(kind @ ("thinking" | "redacted_thinking")) => {
                let said = if kind == "thinking" { text(block, &["thinking"]).unwrap_or_default() } else { "" };
                thinking.push(Thinking {
                    position: first.map(|f| f + on_line),
                    on_line,
                    at: at_time,
                    ms: int(line, &["thinkingDurationMs"]).filter(|ms| *ms >= 0),
                    chars: chars(said),
                    display: some_text(line, &["thinkingDisplay"]),
                    text: said.to_string(),
                    subject: None,
                    signature_chars: chars(text(block, &["signature"]).unwrap_or_default()),
                });
            }
            Some("tool_use") => match call(block, at_time, &mut columns) {
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
    let refused = text(line, &["message", "stop_details", "type"]) == Some("refusal");
    let reply = ReplyLine {
        agent_id,
        request_id: some_text(line, &["requestId"]),
        model,
        requested_model: some_text(line, &["requestedModel"]).or(fell_back_from),
        advisor_model: some_text(line, &["advisorModel"]),
        effort: some_text(line, &["effort"]),
        turn_effort: some_text(line, &["perTurnEffort"]),
        stop_reason: some_text(line, &["message", "stop_reason"]),
        subagent: flag(line, &["isSidechain"]).unwrap_or(false),
        client_version: columns.text(line, &["version"]),
        skill: columns.text(line, &["attributionSkill"]),
        plugin: columns.text(line, &["attributionPlugin"]),
        mcp_server: columns.text(line, &["attributionMcpServer"]),
        mcp_tool: columns.text(line, &["attributionMcpTool"]),
        refusal: if refused { columns.text(line, &["message", "stop_details", "category"]) } else { None },
        refusal_text: if refused { columns.text(line, &["message", "stop_details", "explanation"]) } else { None },
        cache_miss: columns.text(line, &["message", "diagnostics", "cache_miss_reason", "type"]),
        cache_missed: columns.count(line, &["message", "diagnostics", "cache_miss_reason", "cache_missed_input_tokens"]),
        counts,
        text: Text::Added(added),
        thinking,
        calls,
        results: Vec::new(),
        blocks: i64::try_from(blocks.len()).unwrap_or(i64::MAX),
        cumulative: false,
        thinking_dropped: thinking_dropped(line),
    };
    Read { known: known && columns.placed(), facts: vec![Fact::Reply(Box::new(reply))], ..Read::default() }
}

/// The earlier thinking a request left out: its `input_transformations` of
/// type `thinking_dropped`.
fn thinking_dropped(line: &Value) -> i64 {
    let dropped = list(line, &["message", "input_transformations"])
        .iter()
        .filter(|t| text(t, &["type"]) == Some("thinking_dropped"))
        .count();
    i64::try_from(dropped).unwrap_or(i64::MAX)
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
        cache_write_1h_tokens: optional(&["cache_creation", "ephemeral_1h_input_tokens"]).ok()?,
        cache_write_5m_tokens: optional(&["cache_creation", "ephemeral_5m_input_tokens"]).ok()?,
        usage: object(line, &["message", "usage"])?.clone(),
    })
}

/// A `tool_use` block. Its input is read by what it names, whatever the
/// tool: a file, a command, a search, a page, a subagent, a skill, a time
/// limit.
fn call(block: &Value, at_time: DateTime<Utc>, columns: &mut Columns) -> Option<Call> {
    let input = at(block, &["input"]);
    let field = |key: &str| input.and_then(|i| some_text(i, &[key]));
    let tool = some_text(block, &["name"])?;
    let none = Value::None;
    let input_or_none = input.unwrap_or(&none);
    Some(Call {
        call_id: some_text(block, &["id"])?,
        at: at_time,
        path: field("file_path").or_else(|| field("notebook_path")).or_else(|| field("path")),
        command: field("command"),
        description: field("description"),
        pattern: field("pattern"),
        url: field("url"),
        query: field("query"),
        subagent_type: field("subagent_type"),
        background: input.and_then(|i| flag(i, &["run_in_background"])).unwrap_or(false),
        input_bytes: input.map_or(0, json_len),
        sandbox_off: input.and_then(|i| flag(i, &["dangerouslyDisableSandbox"])).unwrap_or(false),
        skill: if tool == SKILL_TOOL { columns.text(input_or_none, &["skill"]) } else { None },
        model: if tool == AGENT_TOOL { columns.text(input_or_none, &["model"]) } else { None },
        timeout_ms: columns.count(input_or_none, &["timeout"]),
        tool,
        dir: None,
    })
}

fn user(line: &Value, at_time: DateTime<Utc>) -> Read {
    match at(line, &["message", "content"]) {
        Some(Value::String(s)) => typed(line, s.clone(), 0),
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
            let mut read = typed(line, texts.join("\n"), images);
            read.known &= known;
            read
        }
        _ => Read::unknown(),
    }
}

/// A user line that is no tool result: the operator's prompt, a trigger,
/// or neither, which feeds no table.
fn typed(line: &Value, message: String, images: i64) -> Read {
    let mut columns = Columns::default();
    let fact = match prompt(line, &message, images, &mut columns) {
        Some(p) => Some(Fact::Prompt(p)),
        None => trigger(line, line, message, &mut columns).map(Fact::Trigger),
    };
    Read { known: columns.placed(), facts: fact.into_iter().collect(), ..Read::default() }
}

/// Whose a user line is, and what kind of message: the operator's only.
fn prompt(line: &Value, message: &str, images: i64, columns: &mut Columns) -> Option<Prompt> {
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
    Some(Prompt {
        kind,
        stops_tools: message.starts_with(INTERRUPT_FOR_TOOLS),
        images,
        text: message.to_string(),
        turn: turn_position(line, columns),
    })
}

/// What a line says of the turn it starts.
fn turn_position(line: &Value, columns: &mut Columns) -> TurnPosition {
    TurnPosition {
        prompt_id: columns.text(line, &["promptId"]),
        source: columns.text(line, &["promptSource"]),
        permission_mode: columns.text(line, &["permissionMode"]),
        turn_index: columns.count(line, &["turnPosition", "turnIndex"]),
        prompt_index: columns.count(line, &["turnPosition", "promptIndex"]),
    }
}

/// A user line that is no prompt and set the agent working: marked by who
/// wrote it (`origin.kind`, any but `human`), a compaction's summary, or
/// the instruction an agent gave its subagent, its first line. `line` is
/// the whole line, `marked` the object that carries the marks.
fn trigger(line: &Value, marked: &Value, message: String, columns: &mut Columns) -> Option<Trigger> {
    let kind = match text(marked, &["origin", "kind"]) {
        Some(kind) if kind != "human" => kind.to_string(),
        _ if flag(marked, &["isCompactSummary"]) == Some(true) => "compact_summary".to_string(),
        _ if flag(marked, &["isSidechain"]) == Some(true)
            && absent(marked, &["parentUuid"])
            && absent(marked, &["origin"]) =>
        {
            "subagent".to_string()
        }
        _ => return None,
    };
    Some(Trigger {
        kind,
        source: source(marked, columns),
        prompt_id: columns.text(line, &["promptId"]),
        text: message,
    })
}

/// Who sent a trigger, as its origin names it: `from`, else `producer`,
/// else `name`.
fn source(marked: &Value, columns: &mut Columns) -> Option<String> {
    ["from", "producer", "name"]
        .iter()
        .find(|key| !absent(marked, &["origin", key]))
        .and_then(|key| columns.text(marked, &["origin", key]))
}

/// No value at `path`, or a null one.
fn absent(v: &Value, path: &[&str]) -> bool {
    matches!(at(v, path), None | Some(Value::None | Value::Null))
}

/// A user line of tool results: one block each, with what the tool
/// reported beside it, and what the line says of the result when it
/// carries that result alone.
fn results(line: &Value, blocks: &[Value], at_time: DateTime<Utc>) -> Read {
    let only_one = blocks.iter().filter(|b| text(b, &["type"]) == Some("tool_result")).count() == 1;
    let reported = if only_one { at(line, &["toolUseResult"]) } else { None };
    let mut columns = Columns::default();
    let says = if only_one { result_line(line, &mut columns) } else { ResultLine::default() };
    let mut known = true;
    let mut facts = Vec::new();
    for b in blocks {
        match text(b, &["type"]) {
            Some("tool_result") => {
                let Some(call_id) = some_text(b, &["tool_use_id"]) else {
                    known = false;
                    continue;
                };
                let (r, shape_known) = result(call_id, b, reported, at_time, &mut columns);
                known &= shape_known;
                facts.push(Fact::Result(Box::new(ToolResult { line: says.clone(), ..r })));
            }
            Some("text" | "image") => {}
            _ => known = false,
        }
    }
    Read { known: known && columns.placed(), facts, ..Read::default() }
}

/// What a line that carries one result alone says of it: who let the call
/// run (`permissionDecision`), how a refused call was refused and what the
/// operator wrote, and the sandbox.
fn result_line(line: &Value, columns: &mut Columns) -> ResultLine {
    ResultLine {
        decision: columns.text(line, &["permissionDecision", "decision"]),
        decided_by: columns.text(line, &["permissionDecision", "reasonType"]),
        decision_source: columns.text(line, &["permissionDecision", "source"]),
        decision_reason: columns.text(line, &["permissionDecision", "reasonCode"]),
        denial: columns.text(line, &["toolDenialKind"]),
        operator_feedback: columns.text(line, &["userFeedback"]),
        sandboxed: columns.flag(line, &["sandboxed"]),
        sandbox_hosts: sandbox_hosts(line),
    }
}

/// One result, and whether its shape was one the format reads.
fn result(
    call_id: String,
    block: &Value,
    reported: Option<&Value>,
    at_time: DateTime<Utc>,
    columns: &mut Columns,
) -> (ToolResult, bool) {
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
        _ if !said.is_empty() => Some(said.clone()),
        _ => None,
    };
    let (edits, shape_known) = match (outcome, reported) {
        (Outcome::Ok, Some(r)) => match at(r, &["bashEditDiff"]) {
            Some(diff) => shell_edits(diff).map_or((Vec::new(), false), |edits| (edits, true)),
            None => (edit(r).into_iter().collect(), true),
        },
        _ => (Vec::new(), true),
    };
    let none = Value::None;
    let reported_or_none = reported.unwrap_or(&none);
    let r = ToolResult {
        call_id,
        at: at_time,
        outcome,
        ms,
        bytes,
        edits,
        output,
        text: (outcome != Outcome::Ok).then_some(said),
        noted: reported.map(noted).unwrap_or_default(),
        line: ResultLine::default(),
        persisted_bytes: columns.count(reported_or_none, &["persistedOutputSize"]),
        return_note: columns.text(reported_or_none, &["returnCodeInterpretation"]),
        background_task: columns.text(reported_or_none, &["backgroundTaskId"]),
    };
    (r, shape_known)
}

/// The hosts a sandbox stopped a command reaching (`sandboxViolations`),
/// each once, in the order the line gives them.
fn sandbox_hosts(line: &Value) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for host in list(line, &["sandboxViolations"]).iter().filter_map(|v| some_text(v, &["host"])) {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
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

/// The files Claude Code saw a shell command change (`bashEditDiff`): an
/// edit per entry of its files, counted from its hunks; none when it is
/// marked shared, since other commands ran meanwhile and the call alone did
/// not change them. `None` when its shape is not one the format reads.
fn shell_edits(diff: &Value) -> Option<Vec<Edit>> {
    if flag(diff, &["shared"]) == Some(true) {
        return Some(Vec::new());
    }
    let Some(Value::Array(files)) = at(diff, &["files"]) else { return None };
    let mut edits = Vec::new();
    for file in files.iter() {
        let path = some_text(file, &["filePath"])?;
        let kind = if flag(file, &["created"]) == Some(true) {
            EditKind::Created
        } else if flag(file, &["deleted"]) == Some(true) {
            EditKind::Deleted
        } else {
            EditKind::Modified
        };
        let (mut added, mut removed) = (0, 0);
        let hunks: &[Value] = match at(file, &["hunks"]) {
            None | Some(Value::None | Value::Null) => &[],
            Some(Value::Array(hunks)) => hunks.as_slice(),
            Some(_) => return None,
        };
        for hunk in hunks {
            let hunk_lines: Vec<&str> = match hunk {
                Value::String(s) => s.lines().collect(),
                Value::Object(_) => list(hunk, &["lines"])
                    .iter()
                    .map(|l| match l {
                        Value::String(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?,
                _ => return None,
            };
            for l in hunk_lines {
                if l.starts_with('+') {
                    added += 1;
                } else if l.starts_with('-') {
                    removed += 1;
                }
            }
        }
        edits.push(Edit { path, kind, added, removed, counted_from: CountedFrom::Shell, operator_changed: false });
    }
    Some(edits)
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

/// A queued command: the operator's prompt typed while the agent worked, a
/// trigger marked by its origin or, with none, by its command mode, or
/// neither.
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
    let mut columns = Columns::default();
    let kind = match text(a, &["origin", "kind"]) {
        Some("human") if message.starts_with("<bash-input>") => Some(PromptKind::Shell),
        Some("human") if message.starts_with("<command-name>") => Some(PromptKind::Command),
        Some("human") => Some(PromptKind::Queued),
        // Claude Code 2.1.295 left a queued `!` command unmarked.
        None if message.starts_with("<bash-input>") => Some(PromptKind::Shell),
        _ => None,
    };
    let fact = match kind {
        Some(kind) => Some(Fact::Prompt(Prompt {
            kind,
            text: message,
            images,
            stops_tools: false,
            turn: turn_position(line, &mut columns),
        })),
        None => queued_trigger(line, a, message, &mut columns).map(Fact::Trigger),
    };
    Read { known: columns.placed(), facts: fact.into_iter().collect(), ..Read::default() }
}

/// A queued command that set the agent working without the operator:
/// marked by its origin, any but `human`; with no origin, by its command
/// mode, any but `prompt`.
fn queued_trigger(line: &Value, a: &Value, message: String, columns: &mut Columns) -> Option<Trigger> {
    if text(a, &["origin", "kind"]).is_some() {
        return trigger(line, a, message, columns);
    }
    if !absent(a, &["origin"]) {
        return None;
    }
    let mode = text(a, &["commandMode"]).filter(|m| *m != "prompt")?;
    Some(Trigger { kind: mode.to_string(), source: None, prompt_id: columns.text(line, &["promptId"]), text: message })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use superx_kernel::message::json_to_object;

    use super::*;
    use crate::crawler::lines::facts::{GitAction, PromptKind};

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
                Fact::Result(r) => Some(r.as_ref()),
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
            vec![Thinking {
                position: Some(1),
                on_line: 0,
                at: when(),
                ms: Some(4200),
                chars: 0,
                display: Some("summarized".into()),
                text: String::new(),
                subject: None,
                signature_chars: 3,
            }]
        );
        assert_eq!(r.thinking_dropped, 0);
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
    fn thinking_keeps_its_summary_and_the_request_says_what_thinking_it_left_out() {
        let read = read(
            &line(serde_json::json!({
                "type": "assistant", "sessionId": "s1", "cwd": "/repo", "apiBlockIndex": 0,
                "message": {
                    "id": "msg_2", "model": "claude-opus-5-5", "stop_reason": "tool_use",
                    "content": [{ "type": "thinking", "thinking": "Weighing the spec against the tests", "signature": "abcdef" }],
                    "input_transformations": [
                        { "type": "thinking_dropped", "path": "messages.2.content.0", "reason": "prefix_binding_mismatch" },
                        { "type": "thinking_dropped", "path": "messages.2.content.1", "reason": "prefix_binding_mismatch" },
                        { "type": "something_else" }
                    ],
                    "usage": { "input_tokens": 1, "output_tokens": 9 }
                }
            })),
            when(),
        );
        let r = only_reply(&read);
        let t = &r.thinking[0];
        assert_eq!((t.text.as_str(), t.chars, t.signature_chars, t.subject.as_deref()), ("Weighing the spec against the tests", 35, 6, None));
        assert_eq!(r.thinking_dropped, 2, "two thinking_dropped transformations, the third is another kind");
    }

    #[test]
    fn a_call_says_when_it_asked_to_leave_the_sandbox_and_its_result_what_the_sandbox_stopped() {
        let off = read(
            &assistant(
                serde_json::json!({ "type": "tool_use", "id": "toolu_9", "name": "Bash", "input": { "command": "git push", "dangerouslyDisableSandbox": true } }),
                serde_json::json!({}),
            ),
            when(),
        );
        assert!(only_reply(&off).calls[0].sandbox_off);
        let on = read(
            &assistant(serde_json::json!({ "type": "tool_use", "id": "toolu_8", "name": "Bash", "input": { "command": "ls" } }), serde_json::json!({})),
            when(),
        );
        assert!(!only_reply(&on).calls[0].sandbox_off);

        let stopped = read(
            &user(
                serde_json::json!([{ "type": "tool_result", "tool_use_id": "toolu_9", "content": "ssh: connect to host bitbucket.org port 22: blocked", "is_error": true }]),
                serde_json::json!({
                    "toolUseResult": "Error: blocked",
                    "sandboxed": true,
                    "sandboxViolations": [
                        { "host": "bitbucket.org", "kind": "network", "port": 22 },
                        { "host": "bitbucket.org", "kind": "network", "port": 443 },
                        { "host": "github.com", "kind": "network", "port": 22 }
                    ]
                }),
            ),
            when(),
        );
        assert_eq!(only_result(&stopped).line.sandbox_hosts, vec!["bitbucket.org".to_string(), "github.com".to_string()]);
        let clean = read(
            &result_line(serde_json::json!({ "type": "tool_result", "tool_use_id": "toolu_8", "content": "a.rs" }), serde_json::json!({ "stdout": "a.rs", "stderr": "" })),
            when(),
        );
        assert!(only_result(&clean).line.sandbox_hosts.is_empty());
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
            assert!(r.known && only_prompt(&r).is_none(), "{content} is not the operator's: {r:?}");
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
