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
