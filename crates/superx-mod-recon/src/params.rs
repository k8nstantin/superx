//! What the module can be tuned by (#466). Every tunable is a parameter on
//! this module's registry entity, set with
//! `superx param set recon <name> <json>`, and runs on the fallback
//! below until it is set (§9). A value of the wrong shape is reported by
//! name and the fallback stands in for it: the module never runs on a
//! guess, and never hides one.

use std::collections::BTreeMap;

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::MODULE_NAME;

/// The port the module's own URL listens on, at 127.0.0.1.
pub const PORT_PARAM: &str = "attr_recon_port";
const DEFAULT_PORT: u16 = 5153; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_port

/// How often the crawler reads the lines captured since its last pass, in
/// seconds; 0 pauses it.
pub const READ_SECS_PARAM: &str = "attr_recon_read_secs";
const DEFAULT_READ_SECS: u64 = 15; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_read_secs

/// Directories to crawl besides the ones the sessions worked in.
pub const INCLUDE_PARAM: &str = "attr_recon_include";

/// Repositories never crawled, by name or by a prefix of their path.
pub const EXCLUDE_PARAM: &str = "attr_recon_exclude";

/// Seconds between full crawls of the repositories; 0 pauses them.
pub const CRAWL_SECS_PARAM: &str = "attr_recon_crawl_secs";
const DEFAULT_CRAWL_SECS: u64 = 600; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_crawl_secs

/// How far back a crawl reads, in days; 0 reads all of history.
pub const SINCE_DAYS_PARAM: &str = "attr_recon_since_days";
const DEFAULT_SINCE_DAYS: u64 = 0; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_since_days

/// No limit: a limit's fallback, so nothing is left out unless the
/// operator sets one. Less data leads to wrong conclusions (operator,
/// 2026-10-24).
pub const ALL: u64 = u64::MAX;

/// Files blamed per repository to see what survives; 0 blames none.
pub const BLAME_FILES_PARAM: &str = "attr_recon_blame_files";
const DEFAULT_BLAME_FILES: u64 = ALL;

/// Blames run at once; each is a git process of its own.
pub const BLAMES_AT_ONCE_PARAM: &str = "attr_recon_blames_at_once";
const DEFAULT_BLAMES_AT_ONCE: u64 = 8; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_blames_at_once

/// Milliseconds a git call may take before it is abandoned and counted
/// as unreadable. The crawler runs in its own process, in the background:
/// a large repository's whole history takes seconds to read, and a read
/// cut off at the UI's 4 s dropped gryphon-data-lake from every chart
/// (#498).
pub const GIT_TIMEOUT_MS_PARAM: &str = "attr_recon_git_timeout_ms";
const DEFAULT_GIT_TIMEOUT_MS: u64 = 120_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_git_timeout_ms

/// Unreachable commits read per repository, at most.
pub const UNREACHABLE_COMMITS_PARAM: &str = "attr_recon_unreachable_commits";
const DEFAULT_UNREACHABLE_COMMITS: u64 = ALL;

/// Paths never blamed, because their lines say nothing about whether work
/// survived. An entry that starts and ends with `/` matches that directory
/// anywhere in a path; any other entry matches the end of a path.
pub const SURVIVAL_SKIP_PARAM: &str = "attr_recon_survival_skip";
const DEFAULT_SURVIVAL_SKIP: &[&str] =
    &["/dist/", ".lock", ".tsbuildinfo", ".png", ".jpg", ".jpeg", ".ico", ".woff", ".woff2", ".svg", ".pdf"];

/// The ref a repository's work lands on, where it is not what `origin/HEAD`
/// says: an object `{repository name: ref}`. The operator's decision; no
/// rule in code can know it.
pub const MAINLINE_REFS_PARAM: &str = "attr_recon_mainline_refs";

/// Repositories crawled in depth at once, each by a worker of its own
/// (#538, decision repos-at-once): no limit until the operator sets one.
pub const REPOS_AT_ONCE_PARAM: &str = "attr_recon_repos_at_once";
const DEFAULT_REPOS_AT_ONCE: u64 = ALL;

/// Milliseconds a call to a host may take before it is cut off (#538); its
/// fallback is attr_recon_git_timeout_ms's.
pub const HOST_TIMEOUT_MS_PARAM: &str = "attr_recon_host_timeout_ms";
const DEFAULT_HOST_TIMEOUT_MS: u64 = DEFAULT_GIT_TIMEOUT_MS;

/// The longest a gap between two replies counts as work, in seconds: a
/// model's minutes are the gaps before its replies, each capped here (#539,
/// as mod-ui's attr_ui_active_session_secs).
pub const ACTIVE_SECS_PARAM: &str = "attr_recon_active_secs";
const DEFAULT_ACTIVE_SECS: u64 = 300; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_active_secs

/// How long after a model takes over from another its commits count as
/// "just after the switch", in seconds (#539).
pub const TAKEOVER_SECS_PARAM: &str = "attr_recon_takeover_secs";
const DEFAULT_TAKEOVER_SECS: u64 = 10_800; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_takeover_secs

/// Lines added and removed above which a commit is a bulk drop, a vendored
/// or generated change, and not a model's authored work (#539).
pub const BULK_LINES_PARAM: &str = "attr_recon_bulk_lines";
const DEFAULT_BULK_LINES: u64 = 5_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_bulk_lines

/// Commits by one model to one file at which the file counts as returned to
/// rather than written: thrash (#539).
pub const THRASH_TOUCHES_PARAM: &str = "attr_recon_thrash_touches";
const DEFAULT_THRASH_TOUCHES: u64 = 3; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_thrash_touches

/// The words a commit's subject starts with, or has after a space, when the
/// work is being done again: rework (#539).
pub const REWORK_WORDS_PARAM: &str = "attr_recon_rework_words";
const DEFAULT_REWORK_WORDS: &[&str] = &["fix", "revert", "undo", "redo", "restore", "re-add", "readd", "rework"];

/// The words in an operator's prompt that correct the agent, as whole words
/// (#539, mod-ui's list).
pub const CORRECTION_WORDS_PARAM: &str = "attr_recon_correction_words";
const DEFAULT_CORRECTION_WORDS: &[&str] = &[
    "again", "stop", "wrong", "instead", "follow", "missed", "broke", "redo", "throw", "i said", "i told you", "deviate", "deviated",
    "deviating", "start over", "from scratch", "revert", "undo", "not what", "fix it",
];

/// The words in an operator's prompt that escalate, as the start of a word
/// (#539, mod-ui's list).
pub const ESCALATION_WORDS_PARAM: &str = "attr_recon_escalation_words";
const DEFAULT_ESCALATION_WORDS: &[&str] = &["fuck", "shit", "wtf", "damn", "useless", "garbage"];

/// The words in a commit's subject that say it is a snapshot (a savepoint
/// kept while the work goes on), not work meant to land (#539): such a
/// commit counts nowhere. git's own stash commits are known without them.
pub const SNAPSHOT_WORDS_PARAM: &str = "attr_recon_snapshot_words";
const DEFAULT_SNAPSHOT_WORDS: &[&str] = &["savepoint", "not for merge"];

/// How often the watcher looks at the lines captured since its last look,
/// in seconds; 0 pauses it (#573).
pub const WATCH_SECS_PARAM: &str = "attr_recon_watch_secs";
const DEFAULT_WATCH_SECS: u64 = 5; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_secs

/// How far back the watcher reads when it starts, in seconds: its measures
/// and its alerts cover this window (#573).
pub const WATCH_WINDOW_SECS_PARAM: &str = "attr_recon_watch_window_secs";
const DEFAULT_WATCH_WINDOW_SECS: u64 = 604_800; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_window_secs

/// A launch the same as one that ran to its end in at least this many
/// seconds is finished work run again (#573).
pub const WATCH_REPEAT_SECS_PARAM: &str = "attr_recon_watch_repeat_secs";
const DEFAULT_WATCH_REPEAT_SECS: u64 = 600; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_repeat_secs

/// A background job running past this many seconds is a run going long
/// (#573): long running means problems (operator, 2026-11-05).
pub const WATCH_LONG_SECS_PARAM: &str = "attr_recon_watch_long_secs";
const DEFAULT_WATCH_LONG_SECS: u64 = 3_600; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_long_secs

/// A request the API leaves without an answer this many seconds is an alert
/// (#573): an answer starts within 197 s in 999 of 1,000 (ten days to
/// 2026-11-05), and the outage of that day held three sessions for minutes.
pub const WATCH_API_WAIT_SECS_PARAM: &str = "attr_recon_watch_api_wait_secs";
const DEFAULT_WATCH_API_WAIT_SECS: u64 = 300; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_api_wait_secs

/// The commands that launch jobs: a simple command starting with one of
/// them is a launch, and the words after it name the job and its input
/// (#573).
pub const WATCH_JOBS_PARAM: &str = "attr_recon_watch_jobs";
const DEFAULT_WATCH_JOBS: &[&str] = &[
    "gcloud run jobs execute",
    "gcloud dataproc batches submit",
    "gcloud builds submit",
    "gcloud dataflow jobs run",
    "terraform apply",
    "bq query",
    "kubectl apply",
    "spark-submit",
];

/// The words your message starts with when you stop the agent (#573).
pub const WATCH_STOP_WORDS_PARAM: &str = "attr_recon_watch_stop_words";
const DEFAULT_WATCH_STOP_WORDS: &[&str] = &["stop", "halt", "cancel", "abort", "don't", "dont", "do not"];

/// The files that hold a plan of runs: a path holding one of these is read
/// for jobs launched more than once (#573).
pub const WATCH_PLANS_PARAM: &str = "attr_recon_watch_plans";
const DEFAULT_WATCH_PLANS: &[&str] = &["/tasks/"];

/// A command run in the foreground that sleeps, and runs this many seconds
/// or more, is an agent asleep: deaf to you while it waits (#573).
pub const WATCH_SLEEP_SECS_PARAM: &str = "attr_recon_watch_sleep_secs";
const DEFAULT_WATCH_SLEEP_SECS: u64 = 60; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_sleep_secs

/// A file edited this many times within `attr_recon_watch_thrash_secs` is
/// edited over and over: trial and error (#573).
pub const WATCH_THRASH_EDITS_PARAM: &str = "attr_recon_watch_thrash_edits";
const DEFAULT_WATCH_THRASH_EDITS: u64 = 10; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_thrash_edits

/// The span within which `attr_recon_watch_thrash_edits` edits of one file
/// are edits over and over, in seconds (#573).
pub const WATCH_THRASH_SECS_PARAM: &str = "attr_recon_watch_thrash_secs";
const DEFAULT_WATCH_THRASH_SECS: u64 = 3_600; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_thrash_secs

/// A call's result this many characters long or longer floods the agent's
/// context (#573).
pub const WATCH_FLOOD_CHARS_PARAM: &str = "attr_recon_watch_flood_chars";
const DEFAULT_WATCH_FLOOD_CHARS: u64 = 40_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_recon_watch_flood_chars

/// The words your message starts with when you ask how the work stands
/// (#573): asked while a job runs, the agent had not told you.
pub const WATCH_STATUS_WORDS_PARAM: &str = "attr_recon_watch_status_words";
const DEFAULT_WATCH_STATUS_WORDS: &[&str] = &["status", "update", "eta", "progress", "where are we", "how long", "what's going on", "whats going on", "is it done", "is it ready"];

/// The protected places, each `pattern = words`: a change to a path holding
/// the pattern, made when neither your message nor the agent's words you
/// answered named one of the words, is listed (#573).
pub const WATCH_PROTECTED_PARAM: &str = "attr_recon_watch_protected";
const DEFAULT_WATCH_PROTECTED: &[&str] = &[
    "/schema = schema",
    ".sql = schema table ddl",
    ".surql = schema",
    "superx-kernel/ = kernel",
    "terraform = terraform infrastructure",
    ".tf = terraform infrastructure",
    "/migrations/ = migration",
    ".claude/skills/ = skill",
    "BLUEPRINT.md = blueprint",
    "DDL: = table schema column alter drop",
];

/// The branches others build on: a push that rewrites one undoes what was
/// merged into it (#573).
pub const WATCH_SHARED_BRANCHES_PARAM: &str = "attr_recon_watch_shared_branches";
const DEFAULT_WATCH_SHARED_BRANCHES: &[&str] = &["main", "master", "sandbox", "develop", "dev", "staging", "production", "release"];

/// Words of the agent's that say work is not done (#573): said after it read
/// the merge of a task's branch and before your next message, the task you
/// took for done was not.
pub const WATCH_NOT_DONE_WORDS_PARAM: &str = "attr_recon_watch_not_done_words";
const DEFAULT_WATCH_NOT_DONE_WORDS: &[&str] = &[
    "not done",
    "not run",
    "not finished",
    "not verified",
    "not checked",
    "wasn't checked",
    "isn't done",
    "remains",
    "still needs",
    "still has to",
    "left to do",
    "your steps",
    "before you start",
    "before you hand",
];

/// The line a spec's gate prints when it passes (#573): a task file merged
/// with no such line after its last change was merged unproven.
pub const WATCH_SPEC_PASSED_PARAM: &str = "attr_recon_watch_spec_passed";
const DEFAULT_WATCH_SPEC_PASSED: &[&str] = &["GATE: PASSED"];

/// The parameters in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub port: u16,
    pub read_secs: u64,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub crawl_secs: u64,
    pub since_days: u64,
    pub blame_files: u64,
    pub blames_at_once: u64,
    pub git_timeout_ms: u64,
    pub unreachable_commits: u64,
    pub survival_skip: Vec<String>,
    pub mainline_refs: BTreeMap<String, String>,
    pub repos_at_once: u64,
    pub host_timeout_ms: u64,
    pub active_secs: u64,
    pub takeover_secs: u64,
    pub bulk_lines: u64,
    pub thrash_touches: u64,
    pub rework_words: Vec<String>,
    pub correction_words: Vec<String>,
    pub escalation_words: Vec<String>,
    pub snapshot_words: Vec<String>,
    pub watch_secs: u64,
    pub watch_window_secs: u64,
    pub watch_repeat_secs: u64,
    pub watch_long_secs: u64,
    pub watch_jobs: Vec<String>,
    pub watch_stop_words: Vec<String>,
    pub watch_plans: Vec<String>,
    pub watch_sleep_secs: u64,
    pub watch_thrash_edits: u64,
    pub watch_thrash_secs: u64,
    pub watch_flood_chars: u64,
    pub watch_status_words: Vec<String>,
    pub watch_protected: Vec<String>,
    pub watch_shared_branches: Vec<String>,
    pub watch_not_done_words: Vec<String>,
    pub watch_spec_passed: Vec<String>,
    pub watch_api_wait_secs: u64,
}

impl Params {
    /// Whether a repository is one the operator excluded: by its name, or
    /// by a prefix of one of its paths.
    #[must_use]
    pub fn excludes(&self, name: &str, paths: &[&str]) -> bool {
        self.exclude
            .iter()
            .filter(|e| !e.is_empty())
            .any(|e| e == name || paths.iter().any(|p| p.starts_with(e.as_str())))
    }

    /// Whether `path` is one the crawler never blames.
    #[must_use]
    pub fn skips_survival(&self, path: &str) -> bool {
        self.survival_skip.iter().any(|entry| {
            if entry.len() > 1 && entry.starts_with('/') && entry.ends_with('/') {
                path.contains(entry.as_str()) || path.starts_with(&entry[1..])
            } else {
                path.ends_with(entry.as_str())
            }
        })
    }
}

/// One parameter as it stands: the value in force, as `status` prints it,
/// whether it was set or is the fallback, and the shape a value must take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: &'static str,
    pub value: String,
    pub set: bool,
    pub wanted: &'static str,
}

/// A parameter set to a value of the wrong shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub name: &'static str,
    pub found: String,
    pub wanted: &'static str,
}

/// The parameters in force, how each one stands, and what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub params: Params,
    pub settings: Vec<Setting>,
    pub problems: Vec<Problem>,
}

/// Every parameter's name, in the order `status` lists them.
pub const NAMES: [&str; 39] = [
    PORT_PARAM,
    READ_SECS_PARAM,
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    CRAWL_SECS_PARAM,
    SINCE_DAYS_PARAM,
    BLAME_FILES_PARAM,
    BLAMES_AT_ONCE_PARAM,
    GIT_TIMEOUT_MS_PARAM,
    UNREACHABLE_COMMITS_PARAM,
    SURVIVAL_SKIP_PARAM,
    MAINLINE_REFS_PARAM,
    REPOS_AT_ONCE_PARAM,
    HOST_TIMEOUT_MS_PARAM,
    ACTIVE_SECS_PARAM,
    TAKEOVER_SECS_PARAM,
    BULK_LINES_PARAM,
    THRASH_TOUCHES_PARAM,
    REWORK_WORDS_PARAM,
    CORRECTION_WORDS_PARAM,
    ESCALATION_WORDS_PARAM,
    SNAPSHOT_WORDS_PARAM,
    WATCH_SECS_PARAM,
    WATCH_WINDOW_SECS_PARAM,
    WATCH_REPEAT_SECS_PARAM,
    WATCH_LONG_SECS_PARAM,
    WATCH_JOBS_PARAM,
    WATCH_STOP_WORDS_PARAM,
    WATCH_PLANS_PARAM,
    WATCH_SLEEP_SECS_PARAM,
    WATCH_THRASH_EDITS_PARAM,
    WATCH_THRASH_SECS_PARAM,
    WATCH_FLOOD_CHARS_PARAM,
    WATCH_STATUS_WORDS_PARAM,
    WATCH_PROTECTED_PARAM,
    WATCH_SHARED_BRANCHES_PARAM,
    WATCH_NOT_DONE_WORDS_PARAM,
    WATCH_SPEC_PASSED_PARAM,
    WATCH_API_WAIT_SECS_PARAM,
];

/// What a parameter is for, as the tab shows it (#539): when the crawler
/// runs, where it looks, how much it does at once, and where it listens;
/// and the input a value is typed into: `number`, `number or all`, `list`
/// or `refs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct About {
    pub group: &'static str,
    pub decides: &'static str,
    pub input: &'static str,
}

/// What `name` is for; `None` for a name that is not one of [`NAMES`].
#[must_use]
pub fn about(name: &str) -> Option<About> {
    let (group, input, decides) = match name {
        PORT_PARAM => ("listens", "number", "The port the module's own URL listens on, at 127.0.0.1; a change applies at restart."),
        READ_SECS_PARAM => ("when", "number", "How often the crawler reads the lines captured since its last pass, in seconds; 0 pauses it."),
        CRAWL_SECS_PARAM => ("when", "number", "Seconds between full crawls of the repositories; 0 pauses them."),
        SINCE_DAYS_PARAM => ("when", "number", "How far back a crawl reads, in days; 0 reads all of history."),
        INCLUDE_PARAM => ("where", "list", "Directories to crawl besides the ones the sessions worked in."),
        EXCLUDE_PARAM => ("where", "list", "Repositories never crawled, by name or by a prefix of their path."),
        MAINLINE_REFS_PARAM => ("where", "refs", "The ref a repository's work lands on, where it is not what origin/HEAD says."),
        SURVIVAL_SKIP_PARAM => ("where", "list", "Paths never blamed, because their lines say nothing about whether work survived."),
        REPOS_AT_ONCE_PARAM => ("how much", "number or all", "Repositories crawled in depth at once, each by a worker of its own."),
        BLAMES_AT_ONCE_PARAM => ("how much", "number", "Blames run at once; each is a git process of its own."),
        BLAME_FILES_PARAM => ("how much", "number or all", "Files blamed per repository to see what survives; 0 blames none."),
        UNREACHABLE_COMMITS_PARAM => ("how much", "number or all", "Unreachable commits read per repository, at most."),
        GIT_TIMEOUT_MS_PARAM => ("how much", "number", "Milliseconds a git call may take before it is abandoned."),
        HOST_TIMEOUT_MS_PARAM => ("how much", "number", "Milliseconds a call to a host may take before it is cut off."),
        ACTIVE_SECS_PARAM => ("measures", "number", "The longest gap between two replies that counts as a model's working time, in seconds."),
        TAKEOVER_SECS_PARAM => ("measures", "number", "How long after a model takes over its commits count as just after the switch, in seconds."),
        BULK_LINES_PARAM => ("measures", "number", "Lines a commit adds and removes above which it is a bulk drop, not a model's authored work."),
        THRASH_TOUCHES_PARAM => ("measures", "number", "Commits by one model to one file at which the file is returned to rather than written."),
        REWORK_WORDS_PARAM => ("measures", "list", "Words a commit's subject starts with, or has after a space, when work is done again."),
        CORRECTION_WORDS_PARAM => ("measures", "list", "Whole words in your prompt that correct the agent."),
        ESCALATION_WORDS_PARAM => ("measures", "list", "Words in your prompt, at a word's start, that escalate."),
        SNAPSHOT_WORDS_PARAM => ("measures", "list", "Words in a commit's subject that make it a snapshot, not work: it counts nowhere."),
        WATCH_SECS_PARAM => ("watches", "number", "How often the watcher looks at the newest lines, in seconds; 0 pauses it."),
        WATCH_WINDOW_SECS_PARAM => ("watches", "number", "How far back the watcher reads when it starts, in seconds: its measures and alerts cover this."),
        WATCH_REPEAT_SECS_PARAM => ("watches", "number", "A launch the same as one that ran to its end in at least this many seconds is an alert."),
        WATCH_LONG_SECS_PARAM => ("watches", "number", "A background job running past this many seconds is an alert."),
        WATCH_JOBS_PARAM => ("watches", "list", "Commands that launch jobs: the words after one name the job and its input."),
        WATCH_STOP_WORDS_PARAM => ("watches", "list", "Words your message starts with when you stop the agent."),
        WATCH_PLANS_PARAM => ("watches", "list", "Files holding a plan of runs: a path holding one of these is read for repeated launches."),
        WATCH_SLEEP_SECS_PARAM => ("watches", "number", "A foreground command that sleeps and runs this many seconds or more is the agent asleep, deaf to you."),
        WATCH_THRASH_EDITS_PARAM => ("watches", "number", "A file edited this many times within the span below is edited over and over."),
        WATCH_THRASH_SECS_PARAM => ("watches", "number", "The span, in seconds, within which those edits of one file are edits over and over."),
        WATCH_FLOOD_CHARS_PARAM => ("watches", "number", "A call's result this many characters long or longer floods the agent's context."),
        WATCH_STATUS_WORDS_PARAM => ("watches", "list", "Words your message starts with when you ask how the work stands."),
        WATCH_PROTECTED_PARAM => ("watches", "list", "Protected places, each `pattern = words`: a change there your message did not name is listed."),
        WATCH_SHARED_BRANCHES_PARAM => ("watches", "list", "Branches others build on: a push that rewrites one is listed, and reopens the task."),
        WATCH_NOT_DONE_WORDS_PARAM => ("watches", "list", "Words of the agent's saying work is not done: after it reads a task's merge, before your next message, they reopen the task."),
        WATCH_SPEC_PASSED_PARAM => ("watches", "list", "The line a spec's gate prints when it passes: a task file merged with no such line after its last change was merged unproven."),
        WATCH_API_WAIT_SECS_PARAM => ("watches", "number", "A request the API leaves without an answer this many seconds is an alert: the agent waits on the API, not on its work."),
        _ => return None,
    };
    Some(About { group, decides, input })
}

/// Read every parameter from this module's registry entity. Before the
/// module is registered, every parameter is its fallback.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the substrate cannot answer, or a
/// parameter's row is corrupt.
pub async fn resolve(kernel: &Kernel) -> Result<Resolved> {
    let entity = match kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await {
        Ok(entity) => entity,
        // An instance defines no module type before its first boot, so no
        // module is registered on it yet.
        Err(KernelError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let mut values = BTreeMap::new();
    if let Some(entity) = entity {
        for name in NAMES {
            if let Some(v) = kernel.get_parameter(entity.clone(), name).await? {
                values.insert(name, v);
            }
        }
    }
    Ok(from_values(&values))
}

/// The parameters from the values set, by name; an unset one is its
/// fallback, and so is one of the wrong shape, which is reported.
#[must_use]
pub fn from_values(values: &BTreeMap<&'static str, Value>) -> Resolved {
    let mut r = Resolver { values, settings: Vec::new(), problems: Vec::new() };
    let params = Params {
        port: r.port(PORT_PARAM, DEFAULT_PORT),
        read_secs: r.count(READ_SECS_PARAM, 0, DEFAULT_READ_SECS),
        include: r.strings(INCLUDE_PARAM, &[]),
        exclude: r.strings(EXCLUDE_PARAM, &[]),
        crawl_secs: r.count(CRAWL_SECS_PARAM, 0, DEFAULT_CRAWL_SECS),
        since_days: r.count(SINCE_DAYS_PARAM, 0, DEFAULT_SINCE_DAYS),
        blame_files: r.limit(BLAME_FILES_PARAM, DEFAULT_BLAME_FILES),
        blames_at_once: r.count(BLAMES_AT_ONCE_PARAM, 1, DEFAULT_BLAMES_AT_ONCE),
        git_timeout_ms: r.count(GIT_TIMEOUT_MS_PARAM, 1, DEFAULT_GIT_TIMEOUT_MS),
        unreachable_commits: r.limit(UNREACHABLE_COMMITS_PARAM, DEFAULT_UNREACHABLE_COMMITS),
        survival_skip: r.strings(SURVIVAL_SKIP_PARAM, DEFAULT_SURVIVAL_SKIP),
        mainline_refs: r.refs(MAINLINE_REFS_PARAM),
        repos_at_once: r.at_once(REPOS_AT_ONCE_PARAM, DEFAULT_REPOS_AT_ONCE),
        host_timeout_ms: r.count(HOST_TIMEOUT_MS_PARAM, 1, DEFAULT_HOST_TIMEOUT_MS),
        active_secs: r.count(ACTIVE_SECS_PARAM, 1, DEFAULT_ACTIVE_SECS),
        takeover_secs: r.count(TAKEOVER_SECS_PARAM, 1, DEFAULT_TAKEOVER_SECS),
        bulk_lines: r.count(BULK_LINES_PARAM, 1, DEFAULT_BULK_LINES),
        thrash_touches: r.count(THRASH_TOUCHES_PARAM, 1, DEFAULT_THRASH_TOUCHES),
        rework_words: r.strings(REWORK_WORDS_PARAM, DEFAULT_REWORK_WORDS),
        correction_words: r.strings(CORRECTION_WORDS_PARAM, DEFAULT_CORRECTION_WORDS),
        escalation_words: r.strings(ESCALATION_WORDS_PARAM, DEFAULT_ESCALATION_WORDS),
        snapshot_words: r.strings(SNAPSHOT_WORDS_PARAM, DEFAULT_SNAPSHOT_WORDS),
        watch_secs: r.count(WATCH_SECS_PARAM, 0, DEFAULT_WATCH_SECS),
        watch_window_secs: r.count(WATCH_WINDOW_SECS_PARAM, 1, DEFAULT_WATCH_WINDOW_SECS),
        watch_repeat_secs: r.count(WATCH_REPEAT_SECS_PARAM, 1, DEFAULT_WATCH_REPEAT_SECS),
        watch_long_secs: r.count(WATCH_LONG_SECS_PARAM, 1, DEFAULT_WATCH_LONG_SECS),
        watch_jobs: r.strings(WATCH_JOBS_PARAM, DEFAULT_WATCH_JOBS),
        watch_stop_words: r.strings(WATCH_STOP_WORDS_PARAM, DEFAULT_WATCH_STOP_WORDS),
        watch_plans: r.strings(WATCH_PLANS_PARAM, DEFAULT_WATCH_PLANS),
        watch_sleep_secs: r.count(WATCH_SLEEP_SECS_PARAM, 1, DEFAULT_WATCH_SLEEP_SECS),
        watch_thrash_edits: r.count(WATCH_THRASH_EDITS_PARAM, 1, DEFAULT_WATCH_THRASH_EDITS),
        watch_thrash_secs: r.count(WATCH_THRASH_SECS_PARAM, 1, DEFAULT_WATCH_THRASH_SECS),
        watch_flood_chars: r.count(WATCH_FLOOD_CHARS_PARAM, 1, DEFAULT_WATCH_FLOOD_CHARS),
        watch_status_words: r.strings(WATCH_STATUS_WORDS_PARAM, DEFAULT_WATCH_STATUS_WORDS),
        watch_protected: r.strings(WATCH_PROTECTED_PARAM, DEFAULT_WATCH_PROTECTED),
        watch_shared_branches: r.strings(WATCH_SHARED_BRANCHES_PARAM, DEFAULT_WATCH_SHARED_BRANCHES),
        watch_not_done_words: r.strings(WATCH_NOT_DONE_WORDS_PARAM, DEFAULT_WATCH_NOT_DONE_WORDS),
        watch_spec_passed: r.strings(WATCH_SPEC_PASSED_PARAM, DEFAULT_WATCH_SPEC_PASSED),
        watch_api_wait_secs: r.count(WATCH_API_WAIT_SECS_PARAM, 1, DEFAULT_WATCH_API_WAIT_SECS),
    };
    Resolved { params, settings: r.settings, problems: r.problems }
}

/// Reads typed values out of the set parameters, noting how each stands.
struct Resolver<'a> {
    values: &'a BTreeMap<&'static str, Value>,
    settings: Vec<Setting>,
    problems: Vec<Problem>,
}

impl Resolver<'_> {
    /// The value set under `name`, parsed; the fallback when it is unset or
    /// `parse` refuses it, which is reported as wanting `wanted`.
    fn read<T>(
        &mut self,
        name: &'static str,
        wanted: &'static str,
        fallback: T,
        parse: impl Fn(&Value) -> Option<T>,
        show: impl Fn(&T) -> String,
    ) -> T {
        let parsed = match self.values.get(name) {
            None => None,
            Some(v) => {
                let p = parse(v);
                if p.is_none() {
                    self.problems.push(Problem { name, found: shown(v), wanted });
                }
                p
            }
        };
        let set = parsed.is_some();
        let value = parsed.unwrap_or(fallback);
        self.settings.push(Setting { name, value: show(&value), set, wanted });
        value
    }

    /// A whole number, `min` or more.
    fn count(&mut self, name: &'static str, min: u64, fallback: u64) -> u64 {
        let wanted = if min == 0 { "a whole number, 0 or more" } else { "a whole number, 1 or more" };
        self.read(
            name,
            wanted,
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u64::try_from(i).ok()).filter(|&i| i >= min),
                _ => None,
            },
            u64::to_string,
        )
    }

    /// A limit: a whole number, 0 or more, or `"all"` for none, which is
    /// how it is shown. With no way to clear a parameter, `"all"` is how a
    /// limit once set is lifted again (#539).
    fn limit(&mut self, name: &'static str, fallback: u64) -> u64 {
        self.read(
            name,
            "a whole number, 0 or more, or \"all\"",
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u64::try_from(i).ok()),
                Value::String(s) if s == "all" => Some(ALL),
                _ => None,
            },
            |&v| if v == ALL { "all".to_string() } else { v.to_string() },
        )
    }

    /// How many at once: a whole number, 1 or more, or `"all"` for no
    /// limit, which is how it is shown.
    fn at_once(&mut self, name: &'static str, fallback: u64) -> u64 {
        self.read(
            name,
            "a whole number, 1 or more, or \"all\"",
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u64::try_from(i).ok()).filter(|&i| i >= 1),
                Value::String(s) if s == "all" => Some(ALL),
                _ => None,
            },
            |&v| if v == ALL { "all".to_string() } else { v.to_string() },
        )
    }

    fn port(&mut self, name: &'static str, fallback: u16) -> u16 {
        self.read(
            name,
            "a port, 1 to 65535",
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u16::try_from(i).ok()).filter(|&p| p > 0),
                _ => None,
            },
            u16::to_string,
        )
    }

    fn strings(&mut self, name: &'static str, fallback: &[&str]) -> Vec<String> {
        self.read(
            name,
            "a list of non-empty strings",
            fallback.iter().map(|s| (*s).to_string()).collect(),
            |v| match v {
                Value::Array(items) => items
                    .iter()
                    .map(|i| match i {
                        Value::String(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            },
            |list| if list.is_empty() { "none".to_string() } else { list.join(", ") },
        )
    }

    fn refs(&mut self, name: &'static str) -> BTreeMap<String, String> {
        self.read(
            name,
            "an object of repository names to refs",
            BTreeMap::new(),
            |v| match v {
                Value::Object(o) => o
                    .iter()
                    .map(|(k, r)| match r {
                        Value::String(s) if !s.is_empty() => Some((k.clone(), s.clone())),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            },
            |refs| {
                if refs.is_empty() {
                    "none".to_string()
                } else {
                    refs.iter().map(|(repo, r)| format!("{repo} → {r}")).collect::<Vec<_>>().join(", ")
                }
            },
        )
    }
}

/// A value as the operator would write it back.
fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Number(n) => n.to_int().map_or_else(|| format!("{n:?}"), |i| i.to_string()),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(i: i64) -> Value {
        Value::Number(superx_kernel::types::Number::Int(i))
    }

    fn strings(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| (*s).to_string()).collect::<Vec<String>>().into())
    }

    /// The parameters before the rename, as `superx-performance` named them,
    /// in their order, with the fallback each ran on.
    const TODAY: [(&str, &str); 39] = [
        ("port", "5153"),
        ("read_secs", "15"),
        ("include", "none"),
        ("exclude", "none"),
        ("crawl_secs", "600"),
        ("since_days", "0"),
        ("blame_files", "all"),
        ("blames_at_once", "8"),
        ("git_timeout_ms", "120000"),
        ("unreachable_commits", "all"),
        ("survival_skip", "/dist/, .lock, .tsbuildinfo, .png, .jpg, .jpeg, .ico, .woff, .woff2, .svg, .pdf"),
        ("mainline_refs", "none"),
        ("repos_at_once", "all"),
        ("host_timeout_ms", "120000"),
        ("active_secs", "300"),
        ("takeover_secs", "10800"),
        ("bulk_lines", "5000"),
        ("thrash_touches", "3"),
        ("rework_words", "fix, revert, undo, redo, restore, re-add, readd, rework"),
        (
            "correction_words",
            "again, stop, wrong, instead, follow, missed, broke, redo, throw, i said, i told you, deviate, deviated, deviating, \
             start over, from scratch, revert, undo, not what, fix it",
        ),
        ("escalation_words", "fuck, shit, wtf, damn, useless, garbage"),
        ("snapshot_words", "savepoint, not for merge"),
        ("watch_secs", "5"),
        ("watch_window_secs", "604800"),
        ("watch_repeat_secs", "600"),
        ("watch_long_secs", "3600"),
        (
            "watch_jobs",
            "gcloud run jobs execute, gcloud dataproc batches submit, gcloud builds submit, gcloud dataflow jobs run, terraform apply, \
             bq query, kubectl apply, spark-submit",
        ),
        ("watch_stop_words", "stop, halt, cancel, abort, don't, dont, do not"),
        ("watch_plans", "/tasks/"),
        ("watch_sleep_secs", "60"),
        ("watch_thrash_edits", "10"),
        ("watch_thrash_secs", "3600"),
        ("watch_flood_chars", "40000"),
        ("watch_status_words", "status, update, eta, progress, where are we, how long, what's going on, whats going on, is it done, is it ready"),
        (
            "watch_protected",
            "/schema = schema, .sql = schema table ddl, .surql = schema, superx-kernel/ = kernel, terraform = terraform infrastructure, \
             .tf = terraform infrastructure, /migrations/ = migration, .claude/skills/ = skill, BLUEPRINT.md = blueprint, \
             DDL: = table schema column alter drop",
        ),
        ("watch_shared_branches", "main, master, sandbox, develop, dev, staging, production, release"),
        (
            "watch_not_done_words",
            "not done, not run, not finished, not verified, not checked, wasn't checked, isn't done, remains, still needs, still has to, \
             left to do, your steps, before you start, before you hand",
        ),
        ("watch_spec_passed", "GATE: PASSED"),
        ("watch_api_wait_secs", "300"),
    ];

    #[test]
    fn every_parameter_is_named_attr_recon_with_todays_names_order_and_fallbacks() {
        let r = from_values(&BTreeMap::new());
        let got: Vec<(&str, &str)> = r.settings.iter().map(|s| (s.name, s.value.as_str())).collect();
        let want: Vec<(String, &str)> = TODAY.iter().map(|(name, value)| (format!("attr_recon_{name}"), *value)).collect();
        assert_eq!(got.len(), want.len(), "no parameter added or removed");
        for ((name, value), (want_name, want_value)) in got.iter().zip(&want) {
            assert_eq!((*name, *value), (want_name.as_str(), *want_value));
        }
        assert!(NAMES.iter().all(|n| n.starts_with("attr_recon_")), "{NAMES:?}");
    }

    #[test]
    fn nothing_set_is_every_fallback() {
        let r = from_values(&BTreeMap::new());
        assert!(r.problems.is_empty());
        assert_eq!(r.settings.len(), NAMES.len(), "every parameter is listed");
        assert!(r.settings.iter().all(|s| !s.set));
        assert_eq!((r.params.port, r.params.read_secs, r.params.crawl_secs), (5153, 15, 600));
        assert_eq!((r.params.blame_files, r.params.unreachable_commits), (ALL, ALL), "nothing left out unless asked");
        let blame = r.settings.iter().find(|s| s.name == BLAME_FILES_PARAM).expect("listed");
        assert_eq!(blame.value, "all");
        assert_eq!(r.params.git_timeout_ms, 120_000, "a whole history's read is not cut off (#498)");
        assert_eq!(r.params.since_days, 0, "all of history");
        assert!(r.params.include.is_empty() && r.params.mainline_refs.is_empty());
        let names: Vec<&str> = r.settings.iter().map(|s| s.name).collect();
        assert_eq!(names, NAMES, "listed in order");
    }

    #[test]
    fn a_set_value_is_the_one_in_force() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, number(6_000));
        values.insert(READ_SECS_PARAM, number(0));
        values.insert(INCLUDE_PARAM, strings(&["/work/a", "/work/b"]));
        let mut refs = superx_kernel::types::Object::new();
        refs.insert("gryphon-data-lake".to_string(), Value::String("origin/sandbox".to_string()));
        values.insert(MAINLINE_REFS_PARAM, Value::Object(refs));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.port, 6_000);
        assert_eq!(r.params.read_secs, 0, "0 pauses the crawler");
        assert_eq!(r.params.include, ["/work/a", "/work/b"]);
        assert_eq!(r.params.mainline_refs.get("gryphon-data-lake").map(String::as_str), Some("origin/sandbox"));
        let port = r.settings.iter().find(|s| s.name == PORT_PARAM).expect("listed");
        assert!(port.set && port.value == "6000");
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_reported_and_the_fallback_stands_in() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, number(70_000));
        values.insert(READ_SECS_PARAM, Value::String("15".to_string()));
        values.insert(BLAMES_AT_ONCE_PARAM, number(0));
        values.insert(SINCE_DAYS_PARAM, number(-3));
        values.insert(EXCLUDE_PARAM, strings(&["ok", ""]));
        let r = from_values(&values);
        assert_eq!(r.params.port, 5153, "no port above 65535");
        assert_eq!(r.params.read_secs, 15, "text is not a number of seconds");
        assert_eq!(r.params.blames_at_once, 8, "0 blames at once would never blame");
        assert_eq!(r.params.since_days, 0);
        assert!(r.params.exclude.is_empty(), "a list with an empty entry is refused whole");
        let named: Vec<&str> = r.problems.iter().map(|p| p.name).collect();
        assert_eq!(named, [PORT_PARAM, READ_SECS_PARAM, EXCLUDE_PARAM, SINCE_DAYS_PARAM, BLAMES_AT_ONCE_PARAM]);
        let port = r.problems.iter().find(|p| p.name == PORT_PARAM).expect("reported");
        assert_eq!(port.found, "70000");
        assert!(r.settings.iter().all(|s| !s.set), "a refused value is not in force");
    }

    #[test]
    fn a_repository_is_excluded_by_its_name_or_a_prefix_of_its_path() {
        let mut values = BTreeMap::new();
        values.insert(EXCLUDE_PARAM, strings(&["skeema", "/tmp/builds/"]));
        let p = from_values(&values).params;
        assert!(p.excludes("skeema", &["/w/skeema"]), "by name");
        assert!(p.excludes("x", &["/w/x", "/tmp/builds/x/.git"]), "by a prefix of one of its paths");
        assert!(!p.excludes("superx", &["/w/SuperX"]));

        // An empty entry would be a prefix of every path: the list is the
        // wrong shape, said so, and nothing is excluded.
        values.insert(EXCLUDE_PARAM, strings(&["skeema", ""]));
        let r = from_values(&values);
        assert!(r.problems.iter().any(|p| p.name == EXCLUDE_PARAM), "{:?}", r.problems);
        assert!(!r.params.excludes("superx", &["/w/SuperX"]) && !r.params.excludes("skeema", &["/w/skeema"]));
    }

    #[test]
    fn zero_is_allowed_where_it_means_something() {
        let mut values = BTreeMap::new();
        values.insert(CRAWL_SECS_PARAM, number(0));
        values.insert(BLAME_FILES_PARAM, number(0));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.crawl_secs, 0, "the repositories paused");
        assert_eq!(r.params.blame_files, 0, "blame nothing");
    }

    #[test]
    fn attr_recon_repos_at_once_and_attr_recon_host_timeout_ms_follow_todays_parameters_in_that_order_with_their_fallbacks_and_a_value_of_the_wrong_shape_is_reported_by_name() {
        let names: Vec<&str> = from_values(&BTreeMap::new()).settings.iter().map(|s| s.name).collect();
        assert_eq!(names[12..14], [REPOS_AT_ONCE_PARAM, HOST_TIMEOUT_MS_PARAM], "after today's, in that order");
        assert_eq!(names[..12], NAMES[..12], "today's stay as they were");

        let fallback = from_values(&BTreeMap::new());
        assert_eq!((fallback.params.repos_at_once, fallback.params.host_timeout_ms), (ALL, fallback.params.git_timeout_ms));
        let shown: Vec<(&str, &str)> = fallback.settings[12..14].iter().map(|s| (s.name, s.value.as_str())).collect();
        assert_eq!(shown, [(REPOS_AT_ONCE_PARAM, "all"), (HOST_TIMEOUT_MS_PARAM, "120000")], "no limit is shown all");
        assert!(fallback.settings[12..14].iter().all(|s| !s.set));

        let mut values = BTreeMap::new();
        values.insert(REPOS_AT_ONCE_PARAM, number(3));
        values.insert(HOST_TIMEOUT_MS_PARAM, number(9_000));
        let set = from_values(&values);
        assert!(set.problems.is_empty(), "{:?}", set.problems);
        assert_eq!((set.params.repos_at_once, set.params.host_timeout_ms), (3, 9_000));

        for wrong in [number(0), number(-1), Value::String("4".to_string())] {
            let mut values = BTreeMap::new();
            values.insert(REPOS_AT_ONCE_PARAM, wrong.clone());
            values.insert(HOST_TIMEOUT_MS_PARAM, wrong);
            let r = from_values(&values);
            let named: Vec<&str> = r.problems.iter().map(|p| p.name).collect();
            assert_eq!(named, [REPOS_AT_ONCE_PARAM, HOST_TIMEOUT_MS_PARAM], "each reported by name");
            let wanted: Vec<&str> = r.problems.iter().map(|p| p.wanted).collect();
            assert_eq!(wanted, ["a whole number, 1 or more, or \"all\"", "a whole number, 1 or more"], "{:?}", r.problems);
            assert_eq!((r.params.repos_at_once, r.params.host_timeout_ms), (ALL, 120_000), "the fallbacks stand in");
        }
    }

    #[test]
    fn a_limit_or_an_at_once_set_to_all_has_none_again_and_a_count_takes_no_all() {
        let all = || Value::String("all".to_string());
        let mut values = BTreeMap::new();
        values.insert(BLAME_FILES_PARAM, all());
        values.insert(REPOS_AT_ONCE_PARAM, all());
        values.insert(GIT_TIMEOUT_MS_PARAM, all());
        let r = from_values(&values);
        assert_eq!((r.params.blame_files, r.params.repos_at_once), (ALL, ALL));
        let named: Vec<&str> = r.problems.iter().map(|p| p.name).collect();
        assert_eq!(named, [GIT_TIMEOUT_MS_PARAM], "{:?}", r.problems);
        let blame = r.settings.iter().find(|s| s.name == BLAME_FILES_PARAM).expect("listed");
        assert_eq!((blame.value.as_str(), blame.set, blame.wanted), ("all", true, "a whole number, 0 or more, or \"all\""));
    }

    #[test]
    fn every_parameter_says_what_it_is_for() {
        for name in NAMES {
            let about = about(name).expect(name);
            assert!(["when", "where", "how much", "listens", "measures", "watches"].contains(&about.group), "{name}: {about:?}");
            assert!(["number", "number or all", "list", "refs"].contains(&about.input), "{name}: {about:?}");
            assert!(!about.decides.is_empty(), "{name}");
        }
        assert_eq!(about("attr_recon_none"), None);
    }

    #[test]
    fn survival_skips_built_bundles_lockfiles_and_binaries() {
        let p = from_values(&BTreeMap::new()).params;
        for path in ["ui/dist/assets/index.js", "dist/index.html", "Cargo.lock", "ui/tsconfig.tsbuildinfo", "logo.svg", "a/b.PDF.pdf"] {
            assert!(p.skips_survival(path), "{path} is skipped");
        }
        for path in ["src/lib.rs", "distance.rs", "docs/dist.md", "lockfile.rs"] {
            assert!(!p.skips_survival(path), "{path} is blamed");
        }
    }
}
