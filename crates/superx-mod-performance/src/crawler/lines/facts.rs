//! What a captured line says, as the reader takes it: typed facts, read out
//! of a line by [`super::formats`] and written by [`super::state`]. No
//! agent's format reaches past this point.

use chrono::{DateTime, Utc};
use superx_kernel::types::Object;

/// Everything one line says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    /// False when the line holds something in a shape the reader does not
    /// know. What it does know is still taken; the pass counts the line.
    pub known: bool,
    /// How the agent was started and its version, when the line says.
    pub header: Header,
    /// Where the session was working, when the line says.
    pub place: Option<Place>,
    pub facts: Vec<Fact>,
}

impl Read {
    /// A line in a known shape that says nothing measured.
    #[must_use]
    pub fn nothing() -> Self {
        Self { known: true, ..Self::default() }
    }

    /// A line in a shape the reader does not know.
    #[must_use]
    pub fn unknown() -> Self {
        Self::default()
    }
}

/// How the agent was started, and its version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    pub entrypoint: Option<String>,
    pub client_version: Option<String>,
}

/// A session's working directory and branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub cwd: String,
    pub branch: Option<String>,
    /// The line names its branch, or none: a line that does not say keeps
    /// the branch the session was on.
    pub says_branch: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Prompt(Prompt),
    Reply(Box<ReplyLine>),
    Result(ToolResult),
    Turn(Turn),
    Compaction(Compaction),
    Away(Away),
}

/// What kind of message from the operator a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// Sent while the agent waited for it.
    Typed,
    /// Typed while the agent worked, handed to it at its next step.
    Queued,
    /// A slash command.
    Command,
    /// A command run in the agent's shell mode.
    Shell,
    /// The operator stopping the agent.
    Interrupt,
}

impl PromptKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Typed => "typed",
            Self::Queued => "queued",
            Self::Command => "command",
            Self::Shell => "shell",
            Self::Interrupt => "interrupt",
        }
    }
}

/// A message from the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub kind: PromptKind,
    /// The message, whole.
    pub text: String,
    /// Images pasted with it.
    pub images: i64,
    /// An interrupt that stopped tool calls under way: every call of the
    /// session's newest reply that has no result yet ends `interrupted`.
    pub stops_tools: bool,
}

/// One line of a model's reply: for Claude Code, one content block; for
/// Gemini CLI, the reply as it stood when the line was written.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyLine {
    /// Claude's `message.id`, Gemini's record `id`.
    pub agent_id: String,
    pub request_id: Option<String>,
    /// NONE until the agent names it (a Gemini reply's first lines).
    pub model: Option<String>,
    pub requested_model: Option<String>,
    pub advisor_model: Option<String>,
    pub effort: Option<String>,
    pub turn_effort: Option<String>,
    pub stop_reason: Option<String>,
    pub subagent: bool,
    /// The reply's counts as this line has them.
    pub counts: Option<Counts>,
    pub text: Text,
    pub thinking: Vec<Thinking>,
    pub calls: Vec<Call>,
    /// Gemini: the calls of the reply that have ended, carried in the same
    /// record.
    pub results: Vec<ToolResult>,
    /// Content blocks on this line, for a span the agent gives no index.
    pub blocks: i64,
    /// The line repeats the whole reply so far (Gemini), rather than adding
    /// to it (Claude Code).
    pub cumulative: bool,
    /// Earlier thinking the request left out (Claude's
    /// `input_transformations` of type `thinking_dropped`).
    pub thinking_dropped: i64,
}

/// What a reply had used and produced, as its usage says.
#[derive(Debug, Clone, PartialEq)]
pub struct Counts {
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub thinking_tokens: i64,
    pub tool_tokens: Option<i64>,
    pub web_searches: Option<i64>,
    pub web_fetches: Option<i64>,
    pub service_tier: Option<String>,
    /// The whole usage object, as captured.
    pub usage: Object,
}

/// The answer's text on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    /// Characters this line adds to the answer (Claude Code).
    Added(i64),
    /// The answer so far, in characters (Gemini CLI).
    Total(i64),
}

/// One thinking span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thinking {
    /// The span's place in its reply, when the agent records it.
    pub position: Option<i64>,
    /// Its place among this line's blocks.
    pub on_line: i64,
    pub at: DateTime<Utc>,
    pub ms: Option<i64>,
    pub chars: i64,
    pub display: Option<String>,
    /// The thinking as recorded, whole: Claude's summary where the display
    /// shows one, Gemini's thought; empty when only the signature is kept.
    pub text: String,
    /// A Gemini thought's subject.
    pub subject: Option<String>,
    /// The length of the signature Claude keeps in place of the thinking.
    pub signature_chars: i64,
}

/// One tool call, as the model made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub call_id: String,
    pub at: DateTime<Utc>,
    pub tool: String,
    pub path: Option<String>,
    pub command: Option<String>,
    pub description: Option<String>,
    pub pattern: Option<String>,
    pub url: Option<String>,
    pub query: Option<String>,
    pub subagent_type: Option<String>,
    pub background: bool,
    pub input_bytes: i64,
    /// The call asked to run outside the sandbox.
    pub sandbox_off: bool,
    /// Where a shell command ran, when the call says (Gemini); otherwise
    /// the session's working directory at the call.
    pub dir: Option<String>,
}

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// The tool reported a failure.
    Error,
    /// The operator refused the call.
    Denied,
    /// The operator stopped it.
    Interrupted,
    /// The agent's own cancel (Gemini).
    Cancelled,
    /// A command cut off at its time limit.
    TimedOut,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied => "denied",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

/// How one tool call ended, and what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub at: DateTime<Utc>,
    pub outcome: Outcome,
    /// The agent's own measure of the call.
    pub ms: Option<i64>,
    /// The size of what the tool returned.
    pub bytes: i64,
    /// Files the call changed.
    pub edits: Vec<Edit>,
    /// What a shell command printed, for the commands it ran.
    pub output: Option<String>,
    /// The git or GitHub actions the agent itself noted on the result.
    pub noted: Vec<GitEvent>,
    /// The hosts the sandbox stopped the command reaching.
    pub sandbox_hosts: Vec<String>,
}

/// How a file was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Created,
    Modified,
}

impl EditKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
        }
    }
}

/// How an edit's lines were counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountedFrom {
    /// The structured patch the agent recorded.
    Patch,
    /// The old and new text the call carried.
    Strings,
    /// The whole content it wrote.
    Content,
}

impl CountedFrom {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Patch => "patch",
            Self::Strings => "strings",
            Self::Content => "content",
        }
    }
}

/// A file a file tool changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub path: String,
    pub kind: EditKind,
    pub added: i64,
    pub removed: i64,
    pub counted_from: CountedFrom,
    pub operator_changed: bool,
}

/// Which git or GitHub action an agent's command performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GitAction {
    Commit,
    Push,
    Merge,
    Rebase,
    Revert,
    CherryPick,
    Reset,
    BranchDeleted,
    PrCreated,
    PrMerged,
}

impl GitAction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Push => "push",
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::Revert => "revert",
            Self::CherryPick => "cherry_pick",
            Self::Reset => "reset",
            Self::BranchDeleted => "branch_deleted",
            Self::PrCreated => "pr_created",
            Self::PrMerged => "pr_merged",
        }
    }
}

/// A git or GitHub action an agent performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEvent {
    pub action: GitAction,
    pub dir: Option<String>,
    pub hash: Option<String>,
    pub branch: Option<String>,
    pub subject: Option<String>,
    pub pr_number: Option<i64>,
    pub url: Option<String>,
}

impl GitEvent {
    /// An action, with nothing yet known about it.
    #[must_use]
    pub fn of(action: GitAction) -> Self {
        Self { action, dir: None, hash: None, branch: None, subject: None, pr_number: None, url: None }
    }
}

/// Which tool ran tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunner {
    Cargo,
    Pytest,
    Jest,
    Vitest,
    Go,
    Node,
}

impl TestRunner {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Pytest => "pytest",
            Self::Jest => "jest",
            Self::Vitest => "vitest",
            Self::Go => "go",
            Self::Node => "node",
        }
    }
}

/// What a command that ran tests printed, summed over its suites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub runner: TestRunner,
    pub passed: i64,
    pub failed: i64,
    pub ignored: i64,
}

/// Which tool built code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builder {
    Cargo,
    Tsc,
    Vite,
    Npm,
    Go,
}

impl Builder {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Tsc => "tsc",
            Self::Vite => "vite",
            Self::Npm => "npm",
            Self::Go => "go",
        }
    }
}

/// What a command that built code printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRun {
    pub builder: Builder,
    pub ok: bool,
    pub errors: i64,
    pub warnings: i64,
}

/// A turn Claude Code timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub ms: i64,
    pub lines: i64,
}

/// One compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    /// `auto` or `manual`.
    pub trigger: String,
    pub tokens_before: i64,
    pub tokens_after: Option<i64>,
    pub tokens_dropped: Option<i64>,
    pub ms: Option<i64>,
}

/// A recap written for the operator's return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Away {
    pub text: String,
}
