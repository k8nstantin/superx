//! Status-page statistics (issue #228) — the numbers the OS already
//! captures, aggregated for display: totals from the substrate,
//! plus a bounded walk over the newest raw messages that mines what
//! the agents actually DID — tools run, lines of code written,
//! per-session activity. Window-scoped figures are labeled with
//! their window; nothing here pretends to be all-time when it isn't.
//!
//! Pure SELECT throughout — readers must not mutate the stream they
//! observe. All code lives in the ui module; kernel untouched.

use std::collections::{HashMap, HashSet};

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, MessageRecord, NodeKind, Result};

use crate::api::{
    ChurnPoint, LiveSession, ModelStat, NameCount, RepoStat, SessionStat, StatsSummary,
    TimeCount, ToolOutcome,
};

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
///
/// Real agent shells are chains: `cd repo && cargo test`. A naive
/// "first stage" read labels that `cd`, which buries the actual work
/// (the review of #311 found the command mix collapsing into one `cd`
/// bar). So every stage is returned, with the pure-navigation ones
/// dropped.
fn command_labels(cmd: &str) -> Vec<String> {
    let cleaned = strip_heredocs(cmd).replace("&&", ";").replace("||", ";");
    cleaned
        .split(SEQUENCERS)
        .map(strip_redirections)
        .filter_map(|stage| stage_label(&stage))
        .collect()
}

/// `|`, `;` and newlines separate stages. **Not** bare `&`: a
/// redirection like `2>&1` contains one, and splitting there produced
/// a stage whose program was `1` — which became the most-repeated
/// "command" on a live instance (issue #334). `&&` and `||` are
/// handled by normalizing them to `;` first.
const SEQUENCERS: [char; 3] = ['|', ';', '\n'];

/// Shell grammar that is not a program: keywords, punctuation, and the
/// fragments of embedded scripts. Live QA found `let`, `t`, `assert`,
/// `old`, `if` and `"""` ranking as top commands (issue #334).
const NOT_A_PROGRAM: [&str; 22] = [
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "let", "local", "return", "break", "continue", "assert", "print", "import", "def", "class",
];

/// Everything a heredoc feeds a program is DATA, not commands. A
/// `python3 - <<'EOF' … EOF` block was being read line by line as if
/// each line were a shell call (issue #334).
fn strip_heredocs(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut lines = cmd.lines();
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        // `<<EOF`, `<<'EOF'`, `<<-"EOF"` — take the delimiter and skip
        // until it appears alone on a line.
        if let Some(pos) = line.find("<<") {
            let raw = line[pos + 2..]
                .trim_start_matches('-')
                .split_whitespace()
                .next()
                .unwrap_or("");
            let delim = raw.trim_matches(|c| c == '\'' || c == '"');
            if !delim.is_empty() && delim.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                for body in lines.by_ref() {
                    if body.trim() == delim {
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Drop redirection tokens so their targets are never mistaken for
/// programs or subcommands.
fn strip_redirections(stage: &str) -> String {
    stage
        .split_whitespace()
        .filter(|w| {
            !(w.contains(">&")
                || w.contains("&>")
                || w.starts_with('>')
                || w.starts_with('<')
                || w.starts_with("2>")
                || w.starts_with("1>")
                || *w == "&")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Shell noise that is not the program: grouping, env prefixes, and
/// wrappers that take the real command as their argument.
const WRAPPERS: [&str; 8] = ["(", ")", "{", "}", "sudo", "time", "env", "nohup"];

/// Flags whose NEXT token is a value, not a subcommand — `git -C dir
/// status` must read `git status`, not `git dir`.
const VALUE_FLAGS: [&str; 6] = ["-C", "-c", "--git-dir", "--work-tree", "-f", "--file"];

/// One stage of a chain, or `None` when the stage is only navigation,
/// shell punctuation, or empty.
fn stage_label(stage: &str) -> Option<String> {
    let mut words = stage
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '(' || c == ')' || c == '{' || c == '}'))
        .filter(|w| !w.is_empty())
        .skip_while(|w| w.contains('=') || WRAPPERS.contains(w));
    let prog_path = words.next()?;
    let prog = prog_path.rsplit('/').next().unwrap_or(prog_path);
    // Navigation is not work; `cd repo && cargo test` is one call
    // about cargo.
    if prog.is_empty() || matches!(prog, "cd" | "pushd" | "popd" | "export" | "source" | ".") {
        return None;
    }
    // A program starts with a letter (or is an explicit path). Anything
    // else — a bare number from a redirection, a quote from a script
    // body, a shell keyword — is not a command (issue #334).
    let first = prog.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '/' || first == '_') {
        return None;
    }
    if NOT_A_PROGRAM.contains(&prog) {
        return None;
    }
    const SUBCOMMANDED: [&str; 9] = [
        "git", "cargo", "npm", "npx", "docker", "go", "gh", "pnpm", "yarn",
    ];
    if SUBCOMMANDED.contains(&prog) {
        // Walk the tokens, stepping over flags AND the values that
        // belong to them, until a bare word appears.
        let mut skip_next = false;
        while let Some(w) = words.next() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if VALUE_FLAGS.contains(&w) {
                skip_next = true;
                continue;
            }
            if w.starts_with('-') {
                continue;
            }
            // A path is an argument, never a subcommand.
            if w.contains('/') || w.starts_with('.') {
                continue;
            }
            // `npm run build` and `npm run dev` are different work;
            // keep the script name rather than collapsing both to
            // `npm run` (which also hid the verb from the build
            // classifier — review of #311).
            if w == "run" {
                if let Some(script) = words.find(|x| !x.starts_with('-')) {
                    return Some(format!("{prog} run {script}"));
                }
            }
            return Some(format!("{prog} {w}"));
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
    // A dotfile has no extension: `.gitignore` is a name, not a
    // language (review of #311).
    let (stem, ext) = file.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 12 || ext.contains(' ') {
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
    /// Per-hour code movement, keyed `YYYY-MM-DDTHH` (issue #324).
    churn: HashMap<String, (i64, i64)>,
    /// Text an edit removed from a file, per file — so a later edit
    /// that puts it back can be recognized as an undo.
    /// Hashes of text earlier-in-time edits removed, per file. Only
    /// equality is needed, and whole function bodies for 20k messages
    /// is a lot of resident memory (review of #330).
    removed_text: HashMap<String, HashSet<u64>>,
    reverts: i64,
    out_tokens: i64,
    /// session uuid seen in each 5-minute bucket, for concurrency.
    concurrency: HashMap<String, HashSet<String>>,
    /// Message instants, for the quiet-stretch measure.
    instants: Vec<chrono::DateTime<chrono::Utc>>,
    // ── quality, from what the commands printed (#327) ───────────
    tests_passed: i64,
    tests_failed: i64,
    compile_errors: i64,
    denials: i64,
    compactions: i64,
    interventions: i64,
    // ── the repo and model dimensions (#325, #328) ───────────────
    repos: HashMap<String, RepoAgg>,
    models: HashMap<String, ModelAgg>,
    /// Per-session live state, keyed by session uuid.
    live: HashMap<String, LiveAgg>,
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

/// A snippet reduced to its shape, so trivial whitespace differences
/// do not hide an undo (issue #324). Empty for snippets too small to
/// be meaningful evidence.
fn snippet_key(s: &str) -> Option<u64> {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() < 12 {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&flat, &mut h);
    Some(std::hash::Hasher::finish(&h))
}

/// Work in one repo. Agents run across many repos at once, so the
/// aggregate alone hides a repo that is thrashing (issue #325).
#[derive(Default)]
struct RepoAgg {
    branch: Option<String>,
    messages: i64,
    lines_added: i64,
    lines_removed: i64,
    files: HashSet<String>,
    tests_run: i64,
    tool_failures: i64,
    out_tokens: i64,
    agents: HashSet<String>,
    last_active: Option<chrono::DateTime<chrono::Utc>>,
}

/// Outcomes attributable to one model (issue #328).
#[derive(Default)]
struct ModelAgg {
    messages: i64,
    lines_added: i64,
    lines_removed: i64,
    out_tokens: i64,
    tool_failures: i64,
    reverts: i64,
}

/// What one session is doing, for the live panel (#325).
#[derive(Default)]
struct LiveAgg {
    agent: String,
    repo: Option<String>,
    branch: Option<String>,
    model: Option<String>,
    last_tool: Option<String>,
    messages: i64,
    lines_added: i64,
    out_tokens: i64,
    tool_failures: i64,
    newest: Option<chrono::DateTime<chrono::Utc>>,
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

/// The ranges the cockpit offers (issue #326). `all` is unbounded;
/// everything else is a rolling window, and every range is capped by
/// rows so a month of history cannot stall a page load.
pub fn range_cutoff(range: &str) -> Option<chrono::Duration> {
    match range {
        "1h" => Some(chrono::Duration::hours(1)),
        "6h" => Some(chrono::Duration::hours(6)),
        "24h" => Some(chrono::Duration::hours(24)),
        "7d" => Some(chrono::Duration::days(7)),
        "30d" => Some(chrono::Duration::days(30)),
        _ => None,
    }
}

/// Rows the range walk will read at most. A month of heavy agent work
/// is far more than a page needs; the payload says when it truncated
/// rather than pretending the sample is the whole range.
pub const RANGE_ROW_CAP: u32 = 20_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Tools whose output is a command's own report. Everything else —
/// above all `Read`, whose payload is a FILE — must never be scored:
/// a source comment mentioning "42 failed" is not a test result
/// (review of #330).
const SHELL_TOOLS: [&str; 4] = ["Bash", "run_shell_command", "Shell", "run_terminal_cmd"];

/// Lines scanned from each end of a command's output. Runners print
/// their tally at the END, so scanning only the head loses it — while
/// diagnostics appear throughout. Both ends, bounded.
const SCAN_EDGE: usize = 300; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A count token: strip any trailing punctuation, so cargo's
/// `passed;` counts exactly like jest's `passed,`. Getting this wrong
/// dropped passes while keeping failures — a one-directional bias
/// that made every pass rate read worse than reality.
fn count_word(w: &str) -> &str {
    w.trim_end_matches([',', ';', '.', ')'])
}

/// What a shell command printed, mined for outcomes (issue #327).
/// Test tallies come from the shapes real runners emit; diagnostics
/// from compiler prefixes.
fn score_output(text: &str, code: &mut CodeAgg) {
    let lines: Vec<&str> = text.lines().collect();
    let scan: Vec<&&str> = if lines.len() <= SCAN_EDGE * 2 {
        lines.iter().collect()
    } else {
        lines
            .iter()
            .take(SCAN_EDGE)
            .chain(lines.iter().skip(lines.len() - SCAN_EDGE))
            .collect()
    };
    for line in scan {
        let l = line.trim();
        // cargo / go: "test result: ok. 42 passed; 0 failed; …"
        if let Some(rest) = l.strip_prefix("test result:") {
            for part in rest.split(';') {
                let p = part
                    .trim()
                    .trim_start_matches("ok.")
                    .trim_start_matches("FAILED.")
                    .trim();
                let mut it = p.split_whitespace();
                if let (Some(n), Some(word)) = (it.next(), it.next()) {
                    if let Ok(v) = n.parse::<i64>() {
                        match count_word(word) {
                            "passed" => code.tests_passed += v,
                            "failed" => code.tests_failed += v,
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }
        // pytest / jest / vitest: "5 passed, 2 failed".
        if l.contains("passed") || l.contains("failed") {
            let mut prev: Option<i64> = None;
            for w in l.split_whitespace() {
                match count_word(w) {
                    "passed" => {
                        if let Some(v) = prev.take() {
                            code.tests_passed += v;
                        }
                    }
                    "failed" => {
                        if let Some(v) = prev.take() {
                            code.tests_failed += v;
                        }
                    }
                    other => prev = other.parse::<i64>().ok(),
                }
            }
        }
        // Diagnostics: rustc, tsc, generic.
        if l.starts_with("error[")
            || l.starts_with("error: could not compile")
            || l.contains(" error TS")
        {
            code.compile_errors += 1;
        }
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
    stats_for_range(kernel, window, "window").await
}

/// The aggregation over a chosen range (issue #326). `window` is the
/// row cap for the legacy fixed-size read; a named range replaces it
/// with a time bound and the wider cap, so every instrument gains
/// history instead of being pinned to the newest N messages.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn stats_for_range(kernel: &Kernel, window: u32, range: &str) -> Result<StatsSummary> {
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
    let cutoff = range_cutoff(range);
    let cap = if range == "window" { window } else { RANGE_ROW_CAP };
    let msgs: Vec<MessageRecord> = match cutoff {
        Some(d) => {
            let since = chrono::Utc::now() - d;
            kernel
                .db()
                .query(
                    "SELECT * FROM message WHERE valid_from > $since \
                     ORDER BY valid_from DESC LIMIT $limit",
                )
                .bind(("since", since))
                .bind(("limit", cap))
                .await?
                .take(0)?
        }
        None => kernel
            .db()
            .query("SELECT * FROM message ORDER BY valid_from DESC LIMIT $limit")
            .bind(("limit", cap))
            .await?
            .take(0)?,
    };
    // Truncation is only meaningful for a time-bounded range: the
    // fixed window is BY DEFINITION the newest N, so reporting it as
    // "sampled" on the default view was just wrong (review of #330).
    let truncated = cutoff.is_some() && msgs.len() as u32 >= cap;
    let mut tools: HashMap<String, i64> = HashMap::new();
    let mut code = CodeAgg::default();
    let mut lines_written = 0i64;
    let mut per_session: HashMap<String, SessAgg> = HashMap::new();
    let mut outcomes: HashMap<String, Outcome> = HashMap::new();
    // tool_use_id → (tool name, model, repo). A `tool_result` message
    // carries NO model of its own, so a failure attributed from the
    // result would land on `unknown`; it belongs to whoever made the
    // call (#328).
    let mut call_names: HashMap<String, (String, Option<String>, Option<String>)> = HashMap::new();
    // Results seen before their call (the walk is newest-first).
    let mut pending_results: HashMap<String, bool> = HashMap::new();
    // Output text held until the call names the tool that produced it.
    let mut pending_output: HashMap<String, String> = HashMap::new();
    // Shell calls seen before their output — the reverse order, which
    // happens with interleaved sidechains. Without this the text is
    // stashed forever and silently dropped.
    let mut shell_calls: HashSet<String> = HashSet::new();
    for m in &msgs {
        let sid = superx_ops::record_uuid(&m.session);
        let agg = per_session.entry(sid).or_default();
        agg.messages += 1;
        // Shape of the working day (#324): when messages landed, and
        // how many sessions were live at once.
        code.instants.push(m.valid_from);
        let minute: u32 = m
            .valid_from
            .format("%M")
            .to_string()
            .parse()
            .unwrap_or(0);
        let bucket5 = format!("{}-{}", m.valid_from.format("%Y-%m-%dT%H"), minute / 5);
        code.concurrency
            .entry(bucket5)
            .or_default()
            .insert(superx_ops::record_uuid(&m.session));

        let Some(raw) = &m.raw else { continue };
        // Live state (#325): newest-first, so the first sighting of a
        // session carries its freshest facts.
        {
            let sid = superx_ops::record_uuid(&m.session);
            let l = code.live.entry(sid).or_default();
            l.messages += 1;
            if l.newest.is_none() {
                l.newest = Some(m.valid_from);
                l.agent = superx_ops::record_uuid(&m.agent);
            }
        }

        // Quality signals carried on the message itself (#327).
        if get_str(raw, "toolDenialKind").is_some() {
            code.denials += 1;
        }
        if matches!(raw.get("isCompactSummary"), Some(Value::Bool(true)))
            || raw.get("compactMetadata").is_some()
        {
            code.compactions += 1;
        }
        if raw.get("interruptedMessageId").is_some() || raw.get("userFeedback").is_some() {
            code.interventions += 1;
        }
        // The model that did this message (#328).
        // A tool_result message carries no model. Attributing it to
        // `unknown` put a meaningless row at the top of the model
        // comparison (review of #330) — so an absent model is simply
        // not attributed.
        let model_opt = raw
            .get("message")
            .and_then(obj)
            .and_then(|m| get_str(m, "model"))
            .map(str::to_string);
        let model = model_opt.clone().unwrap_or_else(|| "unknown".to_string());
        if let Some(known) = &model_opt {
            code.models.entry(known.clone()).or_default().messages += 1;
        }
        {
            let sid = superx_ops::record_uuid(&m.session);
            let l = code.live.entry(sid).or_default();
            if l.model.is_none() && model != "unknown" {
                l.model = Some(model.clone());
            }
        }
        // Which repo the agent was standing in (#308, #325).
        let repo_key = get_str(raw, "cwd").map(|c| c.rsplit('/').next().unwrap_or(c).to_string());
        if let Some(rk) = &repo_key {
            let sid = superx_ops::record_uuid(&m.session);
            let l = code.live.entry(sid).or_default();
            if l.repo.is_none() {
                l.repo = Some(rk.clone());
                l.branch = get_str(raw, "gitBranch").filter(|b| !b.is_empty()).map(str::to_string);
            }
            let r = code.repos.entry(rk.clone()).or_default();
            r.messages += 1;
            r.agents.insert(superx_ops::record_uuid(&m.agent));
            if r.last_active.is_none_or(|prev| m.valid_from > prev) {
                r.last_active = Some(m.valid_from);
            }
            if let Some(b) = get_str(raw, "gitBranch").filter(|b| !b.is_empty()) {
                r.branch.get_or_insert_with(|| b.to_string());
            }
        }
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
                let out = get_int(usage, "output_tokens");
                code.live
                    .entry(superx_ops::record_uuid(&m.session))
                    .or_default()
                    .out_tokens += out;
                agg.out_tokens += out;
                code.out_tokens += out;
                code.models.entry(model.clone()).or_default().out_tokens += out;
                if let Some(rk) = &repo_key {
                    code.repos.entry(rk.clone()).or_default().out_tokens += out;
                }
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
                            {
                                let l = code
                                    .live
                                    .entry(superx_ops::record_uuid(&m.session))
                                    .or_default();
                                if l.last_tool.is_none() {
                                    l.last_tool = Some(name.clone());
                                }
                            }
                            let entry = outcomes.entry(name.clone()).or_default();
                            entry.calls += 1;
                            // The result may already have gone by.
                            if let Some(id) = get_str(block, "id") {
                                match pending_results.remove(id) {
                                    Some(true) => {
                                        entry.failed += 1;
                                        code.models.entry(model.clone()).or_default().tool_failures += 1;
                                        if let Some(rk) = &repo_key {
                                            code.repos.entry(rk.clone()).or_default().tool_failures += 1;
                                        }
                                        code.live
                                            .entry(superx_ops::record_uuid(&m.session))
                                            .or_default()
                                            .tool_failures += 1;
                                    }
                                    Some(false) => entry.ok += 1,
                                    None => {
                                        call_names.insert(
                                            id.to_string(),
                                            (name.clone(), Some(model.clone()), repo_key.clone()),
                                        );
                                    }
                                }
                            }
                            // Now the tool is known: score its output
                            // if — and only if — it was a shell call.
                            if let Some(id) = get_str(block, "id") {
                                match pending_output.remove(id) {
                                    Some(text) if SHELL_TOOLS.contains(&name.as_str()) => {
                                        score_output(&text, &mut code);
                                    }
                                    // Output already seen but the tool
                                    // was not a shell: drop it.
                                    Some(_) => {}
                                    // Output not seen yet — remember
                                    // that this id is worth scoring.
                                    None if SHELL_TOOLS.contains(&name.as_str()) => {
                                        shell_calls.insert(id.to_string());
                                    }
                                    None => {}
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
                                let replaced = replaced_lines(&name, input);
                                if n > 0 || replaced > 0 {
                                    let hour = m.valid_from.format("%Y-%m-%dT%H").to_string();
                                    let slot = code.churn.entry(hour).or_insert((0, 0));
                                    slot.0 += n;
                                    slot.1 += replaced;
                                }
                                // Undo detection (#324). The walk is
                                // newest-first, so `removed_text`
                                // holds what LATER edits took out.
                                // An edit whose new_string is in that
                                // set had its work thrown away by a
                                // later edit — one undo relationship.
                                // A flip-flop therefore scores twice,
                                // which is the honest reading.
                                if let Some(rk) = &repo_key {
                                    let r = code.repos.entry(rk.clone()).or_default();
                                    r.lines_added += n;
                                    r.lines_removed += replaced;
                                    if let Some(pth) = get_str(input, "file_path") {
                                        r.files.insert(pth.to_string());
                                    }
                                }
                                code.live
                                    .entry(superx_ops::record_uuid(&m.session))
                                    .or_default()
                                    .lines_added += n;
                                {
                                    let mm = code.models.entry(model.clone()).or_default();
                                    mm.lines_added += n;
                                    mm.lines_removed += replaced;
                                }
                                if let Some(path) = get_str(input, "file_path") {
                                    let seen = code.removed_text.entry(path.to_string()).or_default();
                                    if let Some(key) =
                                        get_str(input, "new_string").and_then(snippet_key)
                                    {
                                        if seen.contains(&key) {
                                            code.reverts += 1;
                                            code.models.entry(model.clone()).or_default().reverts += 1;
                                        }
                                    }
                                    if let Some(key) =
                                        get_str(input, "old_string").and_then(snippet_key)
                                    {
                                        seen.insert(key);
                                    }
                                }
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
                                    // Every stage of the chain counts —
                                    // `cd repo && cargo test` is a test run.
                                    for label in command_labels(cmd) {
                                        let (is_test, is_build, is_git) = classify_command(&label);
                                        if is_test {
                                            code.tests += 1;
                                            if let Some(rk) = &repo_key {
                                                code.repos.entry(rk.clone()).or_default().tests_run += 1;
                                            }
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
                            // What the command PRINTED is where quality
                            // lives (#327) — but only a SHELL call's
                            // output is a report. The walk is
                            // newest-first, so the tool that produced
                            // this text is not known yet: stash it and
                            // score when the call resolves (review of
                            // #330).
                            if let Some(text) = get_str(block, "content") {
                                if shell_calls.remove(id) {
                                    // The call already went by and it
                                    // was a shell: score immediately.
                                    score_output(text, &mut code);
                                } else {
                                    pending_output.insert(id.to_string(), text.to_string());
                                }
                            }

                            match call_names.remove(id) {
                                Some((name, call_model, call_repo)) => {
                                    let entry = outcomes.entry(name).or_default();
                                    if failed {
                                        entry.failed += 1;
                                        // The call's model and repo, not
                                        // this result message's.
                                        if let Some(cm) = call_model {
                                            code.models.entry(cm).or_default().tool_failures += 1;
                                        }
                                        if let Some(cr) = call_repo {
                                            code.repos.entry(cr).or_default().tool_failures += 1;
                                        }
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
                // (day, hour), not hour alone: in a rolling 24-hour
                // window the same clock hour occurs twice, and
                // collapsing them caps a round-the-clock operator
                // below 24 (review of #311).
                "SELECT time::format(valid_from, '%Y-%m-%dT%H') AS h FROM message \
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
        files: top_n(code.files.clone(), 12),
        languages: top_n(code.languages, 10),
        commands: top_n(code.commands.clone(), 12),
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
        churn: {
            let mut pts: Vec<ChurnPoint> = code
                .churn
                .into_iter()
                .map(|(t, (added, removed))| ChurnPoint { t, added, removed })
                .collect();
            pts.sort_by(|a, b| a.t.cmp(&b.t));
            pts
        },
        reverts: code.reverts,
        thrash_files: code.files.values().filter(|&&n| n >= 3).count() as i64,
        out_tokens_window: code.out_tokens,
        top_repeat: top_n(code.commands.clone(), 1)
            .into_iter()
            .find(|c| c.value >= 3),
        max_concurrent_sessions: code
            .concurrency
            .values()
            .map(|s| s.len() as i64)
            .max()
            .unwrap_or(0),
        range: range.to_string(),
        truncated,
        tests_passed: code.tests_passed,
        tests_failed: code.tests_failed,
        compile_errors: code.compile_errors,
        denials: code.denials,
        compactions: code.compactions,
        interventions: code.interventions,
        repos: {
            let mut v: Vec<RepoStat> = code
                .repos
                .iter()
                .map(|(name, r)| RepoStat {
                    name: name.clone(),
                    branch: r.branch.clone(),
                    messages: r.messages,
                    lines_added: r.lines_added,
                    lines_removed: r.lines_removed,
                    files_touched: r.files.len() as i64,
                    tests_run: r.tests_run,
                    tool_failures: r.tool_failures,
                    out_tokens: r.out_tokens,
                    agents: r.agents.len() as i64,
                    last_active: r.last_active.map(|t| t.to_rfc3339()).unwrap_or_default(),
                })
                .collect();
            v.sort_by(|a, b| b.messages.cmp(&a.messages).then(a.name.cmp(&b.name)));
            v.truncate(12);
            v
        },
        models: {
            let mut v: Vec<ModelStat> = code
                .models
                .iter()
                .map(|(name, m)| ModelStat {
                    name: name.clone(),
                    messages: m.messages,
                    lines_added: m.lines_added,
                    lines_removed: m.lines_removed,
                    out_tokens: m.out_tokens,
                    tool_failures: m.tool_failures,
                    reverts: m.reverts,
                })
                .collect();
            v.sort_by(|a, b| b.messages.cmp(&a.messages).then(a.name.cmp(&b.name)));
            v.truncate(8);
            v
        },
        live: {
            let now = chrono::Utc::now();
            let mut v: Vec<LiveSession> = code
                .live
                .iter()
                .filter_map(|(sid, l)| {
                    let newest = l.newest?;
                    let idle = (now - newest).num_seconds();
                    // Live means a message in the last five minutes —
                    // the same threshold the Sessions page uses.
                    if idle > 300 {
                        return None;
                    }
                    Some(LiveSession {
                        identity: sid.clone(),
                        agent: l.agent.clone(),
                        repo: l.repo.clone(),
                        branch: l.branch.clone(),
                        model: l.model.clone(),
                        last_tool: l.last_tool.clone(),
                        messages: l.messages,
                        lines_added: l.lines_added,
                        out_tokens: l.out_tokens,
                        tool_failures: l.tool_failures,
                        idle_secs: idle,
                    })
                })
                .collect();
            v.sort_by(|a, b| b.messages.cmp(&a.messages).then(a.identity.cmp(&b.identity)));
            v.truncate(8);
            v
        },
        longest_quiet_mins: {
            let mut ts = code.instants.clone();
            ts.sort_unstable();
            ts.windows(2)
                .map(|w| (w[1] - w[0]).num_minutes())
                .max()
                .unwrap_or(0)
        },
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
