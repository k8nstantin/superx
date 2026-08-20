//! Typed API surface (epic #141 P5). Every struct derives `ts_rs::TS`
//! and exports to `ui/src/generated/` (written when tests run), so the
//! P6 frontend is type-checked against the actual kernel records —
//! WASM's type-safety benefit without WASM.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct StatusResponse {
    pub os: String,
    pub ui_version: String,
    pub agents: usize,
    pub modules: Vec<ModuleView>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ModuleView {
    pub name: String,
    pub kind: String,
    pub lifecycle: String,
    pub version: String,
    pub module_id: String,
    pub provisioned: Option<bool>,
    /// The module's OWN UI, when it serves one — discovered from the
    /// substrate via the `attr_module_ui_url` parameter on the
    /// module's registry entity (epic #216, D-UI2). The dashboard's
    /// nav grows a button per module UI with zero per-module code.
    pub ui_url: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AgentView {
    pub name: String,
    pub agent_id: String,
    pub sources: usize,
    pub sessions: usize,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SessionView {
    pub identity: String,
    pub session_id: String,
    pub agent: String,
    pub src: String,
    /// TOTAL activity for the session — messages + action events
    /// (issue #187).
    pub actions: i64,
    /// Current context-window footprint in tokens, from the newest
    /// usage-bearing captured message (issue #200); `None` when the
    /// session carries no usage data.
    pub context_tokens: Option<i64>,
    /// `context_tokens` as a percentage of the instance's context
    /// window (`attr_ui_context_window_tokens`), clamped to 100 —
    /// drives the Sessions list's context bar (issue #202).
    pub context_pct: Option<i64>,
    /// Cumulative output tokens across the session (issue #200).
    pub output_tokens: Option<i64>,
    /// RFC3339 timestamp of the session's newest message.
    pub last_active: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ActionView {
    pub event: String,
    pub summary: String,
    pub agent_id: Option<String>,
    pub valid_from: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ChartsSummary {
    pub events_per_minute: Vec<TimeCount>,
    pub per_agent: Vec<NameCount>,
    pub message_roles: Vec<NameCount>,
    pub boot_durations: Vec<TimeCount>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TimeCount {
    pub t: String,
    pub value: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct NameCount {
    pub name: String,
    pub value: i64,
}

/// The Status page's aggregation (issue #228): substrate totals plus
/// a bounded walk over the newest raw messages — what the agents
/// actually DID. Window-scoped figures carry `window_messages` so the
/// UI can label them honestly.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct StatsSummary {
    pub agents: i64,
    pub sessions_total: i64,
    pub sessions_active: i64,
    pub modules_total: i64,
    pub modules_active: i64,
    pub events_total: i64,
    pub messages_total: i64,
    pub output_tokens_total: i64,
    /// Lines of code written by Write/Edit tools across the window.
    pub lines_written: i64,
    /// Total tool invocations across the window.
    pub tools_window: i64,
    /// The raw-message window the walk covered (newest N).
    pub window_messages: u32,
    pub events_per_minute: Vec<TimeCount>,
    pub message_roles: Vec<NameCount>,
    pub boot_durations: Vec<TimeCount>,
    /// Tool invocations by tool name, window-scoped, descending.
    pub tools: Vec<NameCount>,
    /// Did those calls work? Window-scoped, descending by volume.
    pub tool_outcomes: Vec<ToolOutcome>,
    /// Busiest sessions in the window, by message count.
    pub top_sessions: Vec<SessionStat>,
}

/// A tool's outcomes in the window. Claude Code reports per call via
/// `tool_result.is_error` (joined back to the name by `tool_use_id`);
/// Gemini reports a status string per call. `unknown` is a call whose
/// result fell outside the window — counted, never guessed at.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ToolOutcome {
    pub name: String,
    pub ok: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub unknown: i64,
}

/// Deep statistics (issue #237) — engine-side aggregates over ALL
/// history, not a window. Served separately from [`StatsSummary`] so
/// the live tiles keep their fast refresh.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct InsightsSummary {
    /// Events per calendar day, oldest first (`t` = `YYYY-MM-DD`).
    pub events_per_day: Vec<TimeCount>,
    /// Events by hour of day × day of week.
    pub hour_weekday: Vec<HeatCell>,
    pub tokens: TokenTotals,
    /// Messages by model name, descending.
    pub models: Vec<NameCount>,
    /// Messages and output tokens by agent, descending.
    pub per_agent: Vec<AgentSplit>,
    /// Telemetry volume by event kind, descending.
    pub event_kinds: Vec<NameCount>,
    /// Newest startup reading per module, in milliseconds.
    pub module_startup: Vec<NameCount>,
    /// Age of the newest captured event — the capture-alive signal.
    pub last_event_secs: Option<i64>,
    pub events_last_hour: i64,
}

/// One cell of the hour × weekday grid. `weekday` is SurrealDB's
/// `time::wday` (1 = Monday … 7 = Sunday).
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct HeatCell {
    pub hour: i64,
    pub weekday: i64,
    pub value: i64,
}

/// The four token counters, summed across every captured message.
/// Cost is deliberately absent: nothing in the substrate records it,
/// and a computed guess would be a fabrication.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TokenTotals {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AgentSplit {
    pub name: String,
    pub messages: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SessionStat {
    pub identity: String,
    pub session_id: String,
    pub messages: i64,
    pub lines_written: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SseEvent {
    /// The underlying row's UUIDv7 — the event's EXACT identity, used
    /// for client dedupe and stable render keys (issue #187 review).
    pub id: String,
    pub kind: String, // "action" | "message"
    pub rendered: String,
    /// Source-native role for message events (`user`, `assistant`, …);
    /// `None` on actions.
    pub role: Option<String>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    /// The source-session key stamped on per-session action events
    /// (`payload.session`, issue #172) — lets clients attribute an
    /// action to its session (`SessionView.src` matches). `None` on
    /// messages (they carry `session_id`) and on global events.
    pub session_src: Option<String>,
    pub valid_from: String,
}
