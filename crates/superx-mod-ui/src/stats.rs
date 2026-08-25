//! Status-page statistics (issue #228) — the numbers the OS already
//! captures, aggregated for display: totals from the substrate,
//! plus a bounded walk over the newest raw messages that mines what
//! the agents actually DID — tools run, lines of code written,
//! per-session activity. Window-scoped figures are labeled with
//! their window; nothing here pretends to be all-time when it isn't.
//!
//! Pure SELECT throughout — readers must not mutate the stream they
//! observe. All code lives in the ui module; kernel untouched.

use std::collections::HashMap;

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, MessageRecord, NodeKind, Result};

use crate::api::{NameCount, SessionStat, StatsSummary, TimeCount, ToolOutcome};

/// Telemetry window backing the events/min timeline (same bound the
/// charts endpoint has always used).
const EVENT_WINDOW: u32 = 2000; // skill-allow: §9-const — aggregation page bound

/// A session counts as ACTIVE when it has a message newer than this.
/// Matches the Sessions page's liveness threshold.
pub const ACTIVE_SECS_PARAM: &str = "attr_ui_active_session_secs";
const DEFAULT_ACTIVE_SECS: i64 = 300; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// Tools whose input content counts as CODE WRITTEN.
const WRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];
/// Tools that inspect rather than change (issue #308).
const READ_TOOLS: &[&str] = &["Read", "Glob", "Grep", "NotebookRead"];

fn obj(v: &Value) -> Option<&Object> {
    match v {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

fn get_str<'a>(o: &'a Object, key: &str) -> Option<&'a str> {
    match o.get(key) {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn get_int(o: &Object, key: &str) -> i64 {
    match o.get(key) {
        Some(Value::Number(n)) => n.to_int().unwrap_or(0),
        _ => 0,
    }
}

fn line_count(s: &str) -> i64 {
    if s.is_empty() {
        0
    } else {
        s.split('\n').count() as i64
    }
}

/// Lines of code a single tool_use block writes (Write/Edit content;
/// MultiEdit sums its edits).
fn block_lines(name: &str, input: &Object) -> i64 {
    if !WRITE_TOOLS.contains(&name) {
        return 0;
    }
    if let Some(s) = get_str(input, "content").or_else(|| get_str(input, "new_string")) {
        return line_count(s);
    }
    if let Some(Value::Array(edits)) = input.get("edits") {
        return edits
            .iter()
            .filter_map(|e| obj(e))
            .filter_map(|e| get_str(e, "new_string"))
            .map(line_count)
            .sum();
    }
    0
}

/// Classify a shell command into the tool and, where it carries one,
/// its subcommand — `git commit`, `cargo test` — so the mix reads the
/// way a developer thinks about their own day (issue #308).
fn command_label(cmd: &str) -> Option<String> {
    // Take the first pipeline stage and drop leading env assignments.
    let head = cmd
        .split(['|', ';', '&'])
        .next()
        .unwrap_or("")
        .trim();
    let mut parts = head
        .split_whitespace()
        .skip_while(|w| w.contains('=') || *w == "sudo" || *w == "time");
    let prog_path = parts.next()?;
    let prog = prog_path.rsplit('/').next().unwrap_or(prog_path);
    if prog.is_empty() {
        return None;
    }
    // For multiplexers the verb is the information.
    const SUBCOMMANDED: [&str; 8] = ["git", "cargo", "npm", "npx", "docker", "go", "gh", "pnpm"];
    if SUBCOMMANDED.contains(&prog) {
        if let Some(sub) = parts.find(|w| !w.starts_with('-')) {
            return Some(format!("{prog} {sub}"));
        }
    }
    Some(prog.to_string())
}

/// Does this shell call run tests / build / drive git?
fn classify_command(label: &str) -> (bool, bool, bool) {
    let test = label.contains("test") || label.starts_with("pytest") || label.starts_with("jest");
    let build = label.contains("build")
        || label.contains("compile")
        || label == "cargo check"
        || label == "make"
        || label.starts_with("tsc");
    let git = label.starts_with("git") || label.starts_with("gh ");
    (test, build, git)
}

/// The extension of a path, lowercased — the language proxy.
fn extension_of(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let (_, ext) = file.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 12 || ext.contains(' ') {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// The directory a path sits in, shortened to its last two segments.
fn dir_of(path: &str) -> Option<String> {
    let (dir, _) = path.rsplit_once('/')?;
    let segs: Vec<&str> = dir.rsplit('/').take(2).collect();
    if segs.is_empty() {
        return None;
    }
    Some(segs.into_iter().rev().collect::<Vec<_>>().join("/"))
}

/// Top-N of a count map, descending, ties broken by name so the
/// panel does not reshuffle between refreshes.
fn top_n(map: HashMap<String, i64>, n: usize) -> Vec<NameCount> {
    let mut v: Vec<NameCount> = map
        .into_iter()
        .map(|(name, value)| NameCount { name, value })
        .collect();
    v.sort_by(|a, b| b.value.cmp(&a.value).then(a.name.cmp(&b.name)));
    v.truncate(n);
    v
}

/// Everything the walk learns about the code itself.
#[derive(Default)]
struct CodeAgg {
    lines_added: i64,
    lines_removed: i64,
    writes: i64,
    reads: i64,
    tests: i64,
    builds: i64,
    git: i64,
    mcp: i64,
    web: i64,
    subagent: i64,
    thinking: i64,
    files: HashMap<String, i64>,
    languages: HashMap<String, i64>,
    commands: HashMap<String, i64>,
    projects: HashMap<String, i64>,
    /// Newest branch seen per project — the walk is newest-first, so
    /// the first one wins. Kept apart from the counter so a project
    /// is not split into one row per branch.
    project_branch: HashMap<String, String>,
    dirs: HashMap<String, i64>,
}

/// Lines a call REPLACED — an Edit's `old_string`, which the
/// lines-written figure alone cannot see. A Write replaces nothing;
/// it is counted entirely as added.
fn replaced_lines(name: &str, input: &Object) -> i64 {
    if !WRITE_TOOLS.contains(&name) {
        return 0;
    }
    if let Some(s) = get_str(input, "old_string") {
        return line_count(s);
    }
    if let Some(Value::Array(edits)) = input.get("edits") {
        return edits
            .iter()
            .filter_map(|e| obj(e))
            .filter_map(|e| get_str(e, "old_string"))
            .map(line_count)
            .sum();
    }
    0
}

#[derive(Default)]
struct SessAgg {
    messages: i64,
    lines: i64,
    out_tokens: i64,
}

/// Did the calls work? Claude Code answers in a LATER message — the
/// `tool_result` block carries `is_error` and points back at the call
/// by `tool_use_id` — so outcomes are resolved by joining the two
/// across the window. A call whose result fell outside the window
/// stays `unknown` rather than being scored as a success.
#[derive(Default)]
struct Outcome {
    ok: i64,
    failed: i64,
    cancelled: i64,
    calls: i64,
}

/// Fold the Gemini status vocabulary into the shared one.
fn score_status(o: &mut Outcome, status: &str) {
    match status {
        "error" | "failed" => o.failed += 1,
        "cancelled" | "canceled" => o.cancelled += 1,
        _ => o.ok += 1,
    }
}

/// One in-engine `count() GROUP ALL` over a table.
async fn count_rows(kernel: &Kernel, query: &'static str) -> Result<i64> {
    let rows: Vec<Value> = kernel.db().query(query).await?.take(0)?;
    Ok(rows.first().and_then(|r| obj(r).map(|o| get_int(o, "c"))).unwrap_or(0))
}

/// The full status aggregation. `window` bounds the raw-message walk
/// (newest N) — the window-scoped figures carry it back for honest
/// labeling.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn stats_summary(kernel: &Kernel, window: u32) -> Result<StatsSummary> {
    // ── cheap in-engine totals ──────────────────────────────────────
    let events_total =
        count_rows(kernel, "SELECT count() AS c FROM telemetry_stream GROUP ALL").await?;
    let messages_total = count_rows(kernel, "SELECT count() AS c FROM message GROUP ALL").await?;
    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) \
                 AS c FROM message GROUP ALL",
        )
        .await?
        .take(0)?;
    let output_tokens_total = rows
        .first()
        .and_then(|r| obj(r).map(|o| get_int(o, "c")))
        .unwrap_or(0);

    // Active sessions: distinct sessions with a message inside the
    // activity threshold (parameter on the ui entity; default matches
    // the Sessions page's liveness window).
    let active_secs = resolved_active_secs(kernel).await;
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(active_secs);
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT session FROM message WHERE valid_from > $cutoff GROUP BY session")
        .bind(("cutoff", cutoff))
        .await?
        .take(0)?;
    let sessions_active = rows.len() as i64;

    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await?
        .len() as i64;
    let sessions = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await?;
    let sessions_total = sessions.len() as i64;
    // uuid → "agent/uuid8" display identity.
    let mut identity: HashMap<String, String> = HashMap::new();
    for s in &sessions {
        let name = match &s.payload {
            Value::Object(o) => get_str(o, "name").unwrap_or("?").to_string(),
            _ => "?".to_string(),
        };
        let agent = name.split('/').next().unwrap_or("?").to_string();
        let uuid = superx_ops::record_uuid(&s.entity_id);
        identity.insert(uuid.clone(), format!("{agent}/{}", &uuid[..uuid.len().min(8)]));
    }

    let mut modules_total = 0i64;
    let mut modules_active = 0i64;
    for kind in [NodeKind::KernelModule, NodeKind::Adapter] {
        if let Ok(list) = kernel.list_with_status(kind).await {
            for s in &list {
                modules_total += 1;
                if s.lifecycle.short_tag() == "active" {
                    modules_active += 1;
                }
            }
        }
    }

    // ── the raw-message window walk: what the agents actually did ──
    let msgs: Vec<MessageRecord> = kernel
        .db()
        .query("SELECT * FROM message ORDER BY valid_from DESC LIMIT $limit")
        .bind(("limit", window))
        .await?
        .take(0)?;
    let mut tools: HashMap<String, i64> = HashMap::new();
    let mut code = CodeAgg::default();
    let mut lines_written = 0i64;
    let mut per_session: HashMap<String, SessAgg> = HashMap::new();
    let mut outcomes: HashMap<String, Outcome> = HashMap::new();
    // tool_use_id → tool name, so a later tool_result can be scored.
    let mut call_names: HashMap<String, String> = HashMap::new();
    // Results seen before their call (the walk is newest-first).
    let mut pending_results: HashMap<String, bool> = HashMap::new();
    for m in &msgs {
        let sid = superx_ops::record_uuid(&m.session);
        let agg = per_session.entry(sid).or_default();
        agg.messages += 1;
        let Some(raw) = &m.raw else { continue };
        // Which checkout the agent was standing in (#308).
        if let Some(cwd) = get_str(raw, "cwd") {
            let project = cwd.rsplit('/').next().unwrap_or(cwd).to_string();
            if let Some(branch) = get_str(raw, "gitBranch").filter(|b| !b.is_empty()) {
                code.project_branch
                    .entry(project.clone())
                    .or_insert_with(|| branch.to_string());
            }
            *code.projects.entry(project).or_insert(0) += 1;
        }
        // Claude-style usage + blocks: raw.message.{usage, content[]}.
        if let Some(Value::Object(msg)) = raw.get("message") {
            if let Some(Value::Object(usage)) = msg.get("usage") {
                agg.out_tokens += get_int(usage, "output_tokens");
                if let Some(Value::Object(details)) = usage.get("output_tokens_details") {
                    code.thinking += get_int(details, "thinking_tokens");
                }
            }
            if let Some(Value::Array(blocks)) = msg.get("content") {
                for b in blocks.iter() {
                    let Some(block) = obj(b) else { continue };
                    match get_str(block, "type") {
                        Some("tool_use") => {
                            let name = get_str(block, "name").unwrap_or("tool").to_string();
                            *tools.entry(name.clone()).or_insert(0) += 1;
                            let entry = outcomes.entry(name.clone()).or_default();
                            entry.calls += 1;
                            // The result may already have gone by.
                            if let Some(id) = get_str(block, "id") {
                                match pending_results.remove(id) {
                                    Some(true) => entry.failed += 1,
                                    Some(false) => entry.ok += 1,
                                    None => {
                                        call_names.insert(id.to_string(), name.clone());
                                    }
                                }
                            }
                            // Instrument the call itself (#308).
                            if name.starts_with("mcp__") {
                                code.mcp += 1;
                            }
                            if matches!(name.as_str(), "WebFetch" | "WebSearch" | "web_fetch" | "google_web_search") {
                                code.web += 1;
                            }
                            if matches!(name.as_str(), "Task" | "Skill" | "Agent") {
                                code.subagent += 1;
                            }
                            if READ_TOOLS.contains(&name.as_str()) {
                                code.reads += 1;
                            }
                            if WRITE_TOOLS.contains(&name.as_str()) {
                                code.writes += 1;
                            }
                            if let Some(Value::Object(input)) = block.get("input") {
                                let n = block_lines(&name, input);
                                lines_written += n;
                                agg.lines += n;
                                code.lines_added += n;
                                code.lines_removed += replaced_lines(&name, input);

                                // The file this call touched.
                                if let Some(path) = get_str(input, "file_path")
                                    .or_else(|| get_str(input, "path"))
                                    .or_else(|| get_str(input, "notebook_path"))
                                {
                                    *code.files.entry(path.to_string()).or_insert(0) += 1;
                                    if let Some(ext) = extension_of(path) {
                                        *code.languages.entry(ext).or_insert(0) += 1;
                                    }
                                    if let Some(dir) = dir_of(path) {
                                        *code.dirs.entry(dir).or_insert(0) += 1;
                                    }
                                }
                                // The shell command it ran.
                                if let Some(cmd) = get_str(input, "command") {
                                    if let Some(label) = command_label(cmd) {
                                        let (is_test, is_build, is_git) = classify_command(&label);
                                        if is_test {
                                            code.tests += 1;
                                        }
                                        if is_build {
                                            code.builds += 1;
                                        }
                                        if is_git {
                                            code.git += 1;
                                        }
                                        *code.commands.entry(label).or_insert(0) += 1;
                                    }
                                }
                            }
                        }
                        Some("tool_result") => {
                            let Some(id) = get_str(block, "tool_use_id") else { continue };
                            let failed = matches!(block.get("is_error"), Some(Value::Bool(true)));
                            match call_names.remove(id) {
                                Some(name) => {
                                    let entry = outcomes.entry(name).or_default();
                                    if failed {
                                        entry.failed += 1;
                                    } else {
                                        entry.ok += 1;
                                    }
                                }
                                // Newest-first: the call comes later in
                                // the walk. Hold the verdict for it.
                                None => {
                                    pending_results.insert(id.to_string(), failed);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // Gemini-style: raw.tokens.output + raw.toolCalls[].
        if let Some(Value::Object(toks)) = raw.get("tokens") {
            agg.out_tokens += get_int(toks, "output");
        }
        if let Some(Value::Array(calls)) = raw.get("toolCalls") {
            for c in calls.iter() {
                let Some(call) = obj(c) else { continue };
                let name = get_str(call, "name").unwrap_or("tool").to_string();
                *tools.entry(name.clone()).or_insert(0) += 1;
                // Gemini reports the verdict inline, on the call itself.
                let entry = outcomes.entry(name).or_default();
                entry.calls += 1;
                if let Some(status) = get_str(call, "status") {
                    score_status(entry, status);
                }
            }
        }
    }
    let mut tool_outcomes: Vec<ToolOutcome> = outcomes
        .into_iter()
        .map(|(name, o)| ToolOutcome {
            name,
            ok: o.ok,
            failed: o.failed,
            cancelled: o.cancelled,
            unknown: (o.calls - o.ok - o.failed - o.cancelled).max(0),
        })
        .collect();
    tool_outcomes.sort_by_key(|t| std::cmp::Reverse(t.ok + t.failed + t.cancelled + t.unknown));
    let tools_window: i64 = tools.values().sum();
    let mut tools: Vec<NameCount> = tools
        .into_iter()
        .map(|(name, value)| NameCount { name, value })
        .collect();
    tools.sort_by_key(|t| std::cmp::Reverse(t.value));

    let mut top: Vec<(String, SessAgg)> = per_session.into_iter().collect();
    top.sort_by_key(|(_, a)| std::cmp::Reverse(a.messages));
    let top_sessions: Vec<SessionStat> = top
        .into_iter()
        .take(6)
        .map(|(sid, a)| SessionStat {
            identity: identity
                .get(&sid)
                .cloned()
                .unwrap_or_else(|| sid.chars().take(8).collect()),
            session_id: sid,
            messages: a.messages,
            lines_written: a.lines,
            output_tokens: a.out_tokens,
        })
        .collect();

    // ── timeline / roles / boots (the former charts endpoint's data) ─
    let events = kernel.recent_telemetry(EVENT_WINDOW).await?;
    // Keyed by the FULL timestamp: bucketing on "%H:%M" alone sorts
    // 00:03 before 23:59, so any window spanning midnight came out
    // scrambled. The label stays short; only the sort key is whole.
    let mut per_minute: std::collections::BTreeMap<String, i64> = Default::default();
    let mut boots = Vec::new();
    for e in &events {
        *per_minute
            .entry(e.valid_from.format("%Y-%m-%dT%H:%M").to_string())
            .or_insert(0) += 1;
        if e.lifecycle_event == "boot_complete" {
            if let Value::Object(o) = &e.payload {
                boots.push(TimeCount {
                    t: e.valid_from.format("%m-%d %H:%M").to_string(),
                    value: get_int(o, "duration_ms"),
                });
            }
        }
    }
    boots.reverse(); // recent_telemetry is newest-first; charts read left→right
    let mut message_roles = Vec::new();
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT role, count() AS c FROM message GROUP BY role")
        .await?
        .take(0)?;
    for row in rows {
        if let Some(o) = obj(&row) {
            if let Some(role) = get_str(o, "role") {
                message_roles.push(NameCount {
                    name: role.to_string(),
                    value: get_int(o, "c"),
                });
            }
        }
    }

    // ── 24×7 instruments: engine-side, cheap, whole-history ──────
    let messages_last_hour = count_rows(
        kernel,
        "SELECT count() AS c FROM message WHERE valid_from > time::now() - 1h GROUP ALL",
    )
    .await
    .unwrap_or(0);
    let tokens_last_hour = {
        let rows: Vec<Value> = kernel
            .db()
            .query(
                "SELECT math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) \
                 AS c FROM message WHERE valid_from > time::now() - 1h GROUP ALL",
            )
            .await?
            .take(0)?;
        rows.first().and_then(|r| obj(r).map(|o| get_int(o, "c"))).unwrap_or(0)
    };
    // Clock coverage: how many of the last 24 hours saw any activity.
    let active_hours_24h = {
        let rows: Vec<Value> = kernel
            .db()
            .query(
                "SELECT time::hour(valid_from) AS h FROM message \
                 WHERE valid_from > time::now() - 24h GROUP BY h",
            )
            .await?
            .take(0)?;
        rows.len() as i64
    };

    Ok(StatsSummary {
        agents,
        sessions_total,
        sessions_active,
        modules_total,
        modules_active,
        events_total,
        messages_total,
        output_tokens_total,
        lines_written,
        tools_window,
        window_messages: window,
        events_per_minute: per_minute
            .into_iter()
            .map(|(key, value)| TimeCount {
                // "…T14:07" → "14:07"; the sort already happened.
                t: key.split('T').next_back().unwrap_or(&key).to_string(),
                value,
            })
            .collect(),
        message_roles,
        boot_durations: boots,
        tools,
        tool_outcomes,
        top_sessions,
        lines_added: code.lines_added,
        lines_removed: code.lines_removed,
        files_touched: code.files.len() as i64,
        writes_window: code.writes,
        reads_window: code.reads,
        files: top_n(code.files, 12),
        languages: top_n(code.languages, 10),
        commands: top_n(code.commands, 12),
        projects: top_n(code.projects, 8)
            .into_iter()
            .map(|p| match code.project_branch.get(&p.name) {
                Some(b) => NameCount { name: format!("{} · {b}", p.name), value: p.value },
                None => p,
            })
            .collect(),
        messages_last_hour,
        tokens_last_hour,
        active_hours_24h,
        tests_run: code.tests,
        builds_run: code.builds,
        git_ops: code.git,
        mcp_calls: code.mcp,
        web_calls: code.web,
        subagent_calls: code.subagent,
        thinking_tokens: code.thinking,
        dirs: top_n(code.dirs, 8),
    })
}

/// Resolve the active-session threshold from the ui module's
/// parameter, else the default.
async fn resolved_active_secs(kernel: &Kernel) -> i64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, crate::MODULE_NAME)
        .await
    else {
        return DEFAULT_ACTIVE_SECS;
    };
    match kernel.get_parameter(entity, ACTIVE_SECS_PARAM).await {
        Ok(Some(Value::Number(n))) => n.to_int().filter(|&v| v > 0).unwrap_or(DEFAULT_ACTIVE_SECS),
        _ => DEFAULT_ACTIVE_SECS,
    }
}
