//! Status-page statistics (issue #228) — the numbers the OS already
//! captures, aggregated for display: totals from the substrate,
//! plus a bounded walk over the newest raw messages that mines what
//! the agents actually DID — tools run, lines of code written,
//! per-session activity. Window-scoped figures are labeled with
//! their window; nothing here pretends to be all-time when it isn't.
//!
//! Pure SELECT throughout — readers must not mutate the stream they
//! observe. All code lives in the ui module; kernel untouched.

use std::collections::{BTreeMap, HashMap, HashSet};

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, MessageRecord, NodeKind, Result};

use crate::api::{
    BurnPoint, DuplicateWrite, FocusStat, IntensityPoint, ModelEffortStat,
    AgentStat, BranchStat, ChurnPoint, CompactionStat, EffortStat, Exposure, HourRate, LiveSession, ModelStat, NameCount, QualityPoint, RepoStat, SessionSpan, SessionStat, SlowOp, StatsSummary, TimeCount, ToolOutcome, WorkCell,
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

/// How the operator actually redirects work that has gone off course.
///
/// These are not guesses. They were counted over 1,533 of this
/// operator's own turns in the captured transcripts, and they are the
/// phrasings that actually occur: `again` in 8.0% of turns, `stop` in
/// 6.7%, `wrong` in 6.5%, `instead` in 6.2%, `follow` in 4.2%. The
/// phrases a previous version looked for — "second attempt", "rewriting
/// it" — are what an AGENT writes about itself, and matched almost
/// nothing in the operator's half of the record.
///
/// The turn is the operator's; the CAUSE is the agent leaving the
/// instruction. This is the only place that leaving is written down,
/// which is what makes it worth counting (#406).
pub(crate) const CORRECTIONS: [&str; 20] = [
    "again",
    "stop",
    "wrong",
    "instead",
    "follow",
    "missed",
    "broke",
    "redo",
    "throw",
    "i said",
    "i told you",
    // Whole words since #414: the stem `deviat` matched "deviation", and
    // every one of its hits was the operator talking about the Deviations
    // band, not about the agent deviating.
    "deviate",
    "deviated",
    "deviating",
    "start over",
    "from scratch",
    "revert",
    "undo",
    "not what",
    "fix it",
];

/// What plain contempt looks like in the operator's turns. One person
/// writes them all, so their style is a constant and a difference
/// between models is the models (#406). `fuck` alone lands in 20.4% of
/// this operator's turns, so it is the loudest signal in the corpus.
/// Matched as the START of a word, so "fucking" counts and "rashit" does
/// not.
pub(crate) const ESCALATIONS: [&str; 6] = ["fuck", "shit", "wtf", "damn", "useless", "garbage"];

/// Does `text` carry one of `markers` as a word (#414)? A marker must
/// start where a word starts, and — unless `stem` — end where it ends.
/// A plain substring test read "against" as "again" and "following" as
/// "follow": 24 of 66 and 8 of 14 of those hits on the operator's own
/// turns were not corrections at all.
pub(crate) fn says_word(text: &str, markers: &[&str], stem: bool) -> bool {
    let low = text.to_lowercase();
    let bounded = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    markers.iter().any(|m| {
        low.match_indices(m).any(|(i, _)| {
            bounded(low[..i].chars().next_back())
                && (stem || bounded(low[i + m.len()..].chars().next()))
        })
    })
}

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

/// The reply a row belongs to (#409). Claude Code writes one line per
/// content block of a reply — thinking, text, each tool call — and every
/// one of them repeats the reply's `usage` under the same `message.id`.
/// Gemini re-emits a record as it streams, fuller each time, under the
/// same `id`. Either way, a reply is counted once, not once per row.
fn reply_key(raw: &Object) -> Option<String> {
    raw.get("message")
        .and_then(obj)
        .and_then(|m| get_str(m, "id"))
        .or_else(|| get_str(raw, "id"))
        .map(str::to_string)
}

/// The key a reply's rows share, in SurrealQL — the engine-side twin of
/// [`reply_key`] (#409). A row with neither id is its own reply.
pub(crate) const REPLY_KEY_SQL: &str = "(raw.message.id ?? raw.id ?? id)";

/// A reply's output tokens, in SurrealQL, the way [`reply_usage`] reads
/// them: Claude's `output_tokens`, else Gemini's `output` plus its
/// `thoughts`. Every term is parenthesised — `??` binds tighter than `+`.
pub(crate) const OUT_TOKENS_SQL: &str =
    "(raw.message.usage.output_tokens ?? ((raw.tokens.output ?? 0) + (raw.tokens.thoughts ?? 0)))";

/// One reply's token counters, in one shape for both agents (#409).
///
/// Claude Code's `output_tokens` already includes the reasoning it
/// reports under `output_tokens_details.thinking_tokens`; Gemini
/// reports its `thoughts` beside `output`, so the two are added to read
/// the same way. Gemini's `input` includes what it served from cache,
/// so fresh input is the difference.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct ReplyUsage {
    pub(crate) out: i64,
    pub(crate) thinking: i64,
    pub(crate) input: i64,
    pub(crate) cache_write: i64,
    pub(crate) cache_read: i64,
    /// The whole prompt this reply was answering.
    pub(crate) context: i64,
}

fn reply_usage(raw: &Object) -> Option<ReplyUsage> {
    usage_of(
        raw.get("message").and_then(obj).and_then(|m| m.get("usage")).and_then(obj),
        raw.get("tokens").and_then(obj),
    )
}

/// [`ReplyUsage`] from Claude's `message.usage` or Gemini's `tokens`,
/// whichever the row carries.
pub(crate) fn usage_of(claude: Option<&Object>, gemini: Option<&Object>) -> Option<ReplyUsage> {
    if let Some(u) = claude {
        let input = get_int(u, "input_tokens");
        let cache_write = get_int(u, "cache_creation_input_tokens");
        let cache_read = get_int(u, "cache_read_input_tokens");
        return Some(ReplyUsage {
            out: get_int(u, "output_tokens"),
            thinking: u
                .get("output_tokens_details")
                .and_then(obj)
                .map_or(0, |d| get_int(d, "thinking_tokens")),
            input,
            cache_write,
            cache_read,
            context: input + cache_write + cache_read,
        });
    }
    let t = gemini?;
    let cached = get_int(t, "cached");
    let prompt = get_int(t, "input");
    Some(ReplyUsage {
        out: get_int(t, "output") + get_int(t, "thoughts"),
        thinking: get_int(t, "thoughts"),
        input: (prompt - cached).max(0),
        cache_write: 0,
        cache_read: cached,
        context: prompt + get_int(t, "tool"),
    })
}

/// The file a tool call touched: `file_path` for the file tools,
/// `notebook_path` for NotebookRead and NotebookEdit — which carried
/// their path under a name nothing read, so notebook work never
/// reached the files, languages, exposure or created/modified
/// instruments (#346).
fn touched_path(input: &Object) -> Option<&str> {
    get_str(input, "file_path").or_else(|| get_str(input, "notebook_path"))
}

fn line_count(s: &str) -> i64 {
    if s.is_empty() {
        0
    } else {
        s.split('\n').count() as i64
    }
}

/// Lines of code a single tool_use block writes (Write/Edit content;
/// MultiEdit sums its edits; NotebookEdit's `new_source`).
fn block_lines(name: &str, input: &Object) -> i64 {
    if !WRITE_TOOLS.contains(&name) {
        return 0;
    }
    // A notebook edit carries the cell's text as `new_source`, and a
    // `delete` writes nothing (#346).
    if name == "NotebookEdit" {
        if get_str(input, "edit_mode") == Some("delete") {
            return 0;
        }
        return get_str(input, "new_source").map(line_count).unwrap_or(0);
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
    labelled_stages(cmd).into_iter().map(|(label, _)| label).collect()
}

/// Every labelled stage of a chain, with the stage text it was read
/// from — the text is what says whether `sed` was `-n` or `-i`.
fn labelled_stages(cmd: &str) -> Vec<(String, String)> {
    split_stages(&strip_heredocs(cmd))
        .iter()
        .map(|stage| strip_redirections(stage))
        .filter_map(|stage| stage_label(&stage).map(|label| (label, stage)))
        .collect()
}

/// Programs that inspect and change nothing. A shell call whose every
/// stage is one of these is the agent READING — the same act as a
/// `Read` tool call, which was all `reads` counted. An instance whose
/// agents read through the shell showed *make ↔ inspect · 100%
/// writing* under a Commands list led by `head`, `grep`, `cat` and
/// `sed` (#367).
const INSPECT_PROGRAMS: [&str; 22] = [
    "cat", "head", "tail", "less", "more", "sed", "awk", "grep", "egrep", "fgrep", "rg", "ag",
    "find", "fd", "ls", "tree", "wc", "stat", "file", "jq", "yq", "diff",
];

/// git's and gh's read-only verbs, as `stage_label` writes them.
const INSPECT_VERBS: [&str; 15] = [
    "git log", "git status", "git diff", "git show", "git blame", "git rev-parse",
    "git ls-files", "git describe", "git shortlog", "gh pr view", "gh pr list", "gh pr diff",
    "gh pr checks", "gh issue view", "gh issue list",
];

/// Stages that say nothing about the work: prompts, separators and
/// shell bookkeeping. Ignored when deciding what a call IS, and left
/// out of the repeat key — `echo ---` a hundred times a day is
/// scaffolding, not a fight (#367).
const NOISE_PROGRAMS: [&str; 8] = ["echo", "printf", "true", "false", "pwd", "env", "which", "type"];

fn is_noise(label: &str) -> bool {
    NOISE_PROGRAMS.contains(&label)
}

/// Does this stage only look? `sed -i` and `awk -i inplace` write, so
/// the stage text decides for them.
fn stage_inspects(label: &str, stage: &str) -> bool {
    if INSPECT_VERBS.contains(&label) {
        return true;
    }
    if !INSPECT_PROGRAMS.contains(&label) {
        return false;
    }
    if label == "find" {
        // `find -delete` and `find -exec sed -i …` change what they find.
        if stage.split_whitespace().any(|w| w == "-delete") {
            return false;
        }
        if let Some(inner) = handed_command(label, stage) {
            return stage_label(&inner).is_some_and(|l| stage_inspects(&l, &inner));
        }
    }
    !edits_in_place(label, stage)
}

/// Does this stage edit a file where it lies (#412)? Only the programs
/// that can: `sed -i` (any group of switches carrying `i`, since GNU sed
/// reads whatever follows it as the backup suffix), `sed --in-place`,
/// gawk's `-i inplace`, and `perl -i`. The test used to apply to every
/// inspect program, so `grep -i` — case-insensitive — counted as an
/// edit: 81 of 5,014 shell calls in 30 days.
fn edits_in_place(label: &str, stage: &str) -> bool {
    let words: Vec<&str> = stage.split_whitespace().skip(1).collect();
    match label {
        "sed" => words.iter().any(|w| switches_carry_i(w, "efl") || w.starts_with("--in-place")),
        "awk" => words.windows(2).any(|p| p[0] == "-i" && p[1] == "inplace")
            || words.contains(&"--inplace"),
        "perl" => words.iter().any(|w| switches_carry_i(w, "MmIeExdDC")),
        _ => false,
    }
}

/// Does a group of single-letter switches — `-pi.bak`, `-ni`, `-Ei` —
/// carry `i`? Reading stops at the first switch that takes the rest of
/// the word as its value: `-MList::Util` loads a module, `-Ilib` adds a
/// path and `-fscript.sed` names a script, and none of them edits in
/// place (#415 review).
fn switches_carry_i(word: &str, takes_value: &str) -> bool {
    let Some(switches) = word.strip_prefix('-').filter(|s| !s.starts_with('-')) else {
        return false;
    };
    for c in switches.chars() {
        if c == 'i' {
            return true;
        }
        if takes_value.contains(c) {
            return false;
        }
    }
    false
}

/// The command `find -exec` or `xargs` runs on the files it is handed:
/// `find . -name '*.rs' -exec sed -i 's/a/b/' {} +` edits files no word
/// of the line names (#415 review).
fn handed_command(label: &str, stage: &str) -> Option<String> {
    let words: Vec<&str> = stage.split_whitespace().collect();
    let start = match label {
        "find" => words.iter().position(|w| matches!(*w, "-exec" | "-execdir" | "-ok" | "-okdir"))? + 1,
        "xargs" => {
            let mut i = words.iter().position(|w| w.rsplit('/').next() == Some("xargs"))? + 1;
            while let Some(w) = words.get(i) {
                if !w.starts_with('-') {
                    break;
                }
                // xargs's own switches, and the values of those that take one.
                i += if matches!(*w, "-n" | "-I" | "-L" | "-P" | "-s" | "-d" | "-E" | "-a") { 2 } else { 1 };
            }
            i
        }
        _ => return None,
    };
    let inner: Vec<&str> = words
        .get(start..)?
        .iter()
        .take_while(|w| !matches!(**w, ";" | "\\;" | "';'" | "+"))
        .copied()
        .collect();
    (!inner.is_empty()).then(|| inner.join(" "))
}

/// A shell call that is all inspection, once the noise is set aside.
/// `cat x && cargo test` is a test run; `echo; echo` is nothing.
fn shell_inspects(cmd: &str) -> bool {
    let real: Vec<_> = labelled_stages(cmd)
        .into_iter()
        .filter(|(label, _)| !is_noise(label))
        .collect();
    !real.is_empty() && real.iter().all(|(label, stage)| stage_inspects(label, stage))
}

/// Paths an inspecting call read, so a `cat` counts as exposure the
/// way a `Read` does. Path-shaped operands only — something with a slash
/// that is not a flag, a URL, a glob or a pattern — resolved against the
/// directory the stage runs in when relative.
fn inspected_paths(cmd: &str, cwd: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for (raw, here) in stages_in_place(cmd, cwd) {
        let stage = strip_redirections(&raw);
        let Some(label) = stage_label(&stage) else { continue };
        if is_noise(&label) || !stage_inspects(&label, &stage) || output_sent_elsewhere(&raw) {
            continue;
        }
        for w in operands(&raw, &label) {
            let w = w.trim_matches(|c| c == ',' || c == ';' || c == '(' || c == ')');
            if !w.contains('/') || w.contains("://") || w.chars().any(|c| NOT_IN_A_PATH.contains(&c)) {
                continue;
            }
            let path = if w.starts_with('/') || w.starts_with('~') {
                w.to_string()
            } else if let Some(c) = here.as_deref() {
                format!("{}/{}", c.trim_end_matches('/'), w)
            } else {
                continue;
            };
            let path = normalize(&path);
            if !out.contains(&path) {
                out.push(path);
            }
            if out.len() >= INSPECT_PATHS {
                return out;
            }
        }
    }
    out
}

/// Does this stage send its output somewhere other than the call's
/// output? `cat a > b` put nothing of `a` in front of the model; `>&2`
/// only moves it to stderr, which the call's output carries too.
fn output_sent_elsewhere(stage: &str) -> bool {
    shell_words(stage).iter().any(|(w, quoted)| {
        !quoted
            && (w.starts_with('>') || w.starts_with("1>") || w.starts_with("&>"))
            && !w.starts_with(">&")
            && !w.starts_with("1>&")
    })
}

/// Characters a pattern, a glob or an unexpanded word carries and the
/// paths these agents read never do: `sed -n '/^fn x/,/^}/p'` is no file.
const NOT_IN_A_PATH: [char; 12] = ['*', '?', '^', '$', '|', '\\', '[', ']', '{', '}', '`', '\n'];

/// Programs whose first operand is a pattern or a program, not a file.
/// Read as a path, `grep -v '/generated/'` was a read outside the
/// repository: 37 false outside reads in a week of this machine's shells.
const PATTERN_FIRST: [&str; 8] = ["grep", "egrep", "fgrep", "rg", "ag", "sed", "awk", "jq"];

/// How many words after `flag` are its value, for a program whose first
/// operand is its pattern — and whether the flag carried the pattern (or
/// a file of them), which leaves every operand a file. `grep -v` takes no
/// value; `awk -v` does.
fn pattern_flag(label: &str, flag: &str) -> (usize, bool) {
    match (label, flag) {
        (_, "-e" | "--regexp" | "--expression" | "-f" | "--file" | "--from-file") => (1, true),
        ("rg", "--files") => (0, true),
        ("grep" | "egrep" | "fgrep" | "rg" | "ag",
         "-A" | "-B" | "-C" | "-m" | "--max-count" | "--context" | "--after-context" | "--before-context") => (1, false),
        ("grep" | "egrep" | "fgrep", "-d" | "-D" | "--include" | "--exclude" | "--exclude-dir") => (1, false),
        ("rg", "-g" | "--glob" | "--iglob" | "-t" | "--type" | "-T" | "--type-not" | "-j" | "-M" | "-d"
            | "--max-depth") => (1, false),
        ("ag", "-G" | "-g" | "--ignore" | "--depth") => (1, false),
        ("awk", "-F" | "-v") => (1, false),
        ("sed", "-l") => (1, false),
        ("jq", "--arg" | "--argjson" | "--slurpfile" | "--rawfile") => (2, false),
        ("jq", "--indent") => (1, false),
        _ => {
            // `--regexp=…`: the pattern, attached.
            let carried = ["--regexp=", "--expression=", "--file=", "--from-file="]
                .iter()
                .any(|f| flag.starts_with(f));
            (0, carried)
        }
    }
}

/// The operands of an inspecting stage: its words as the shell splits
/// them, with redirections, the program and whatever stood before it set
/// aside, and flags dropped. git and gh take refs and ranges —
/// `origin/main...HEAD` is no file — so theirs are the words after `--`.
/// A search or an editor takes its pattern first, unless a flag carried
/// it.
fn operands(raw: &str, label: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut target_next = false;
    for (w, quoted) in shell_words(raw) {
        if target_next {
            target_next = false;
            continue;
        }
        if !quoted && is_redirection(&w) {
            target_next = bare_redirection(&w);
            continue;
        }
        words.push(w);
    }
    let Some(program) = words.iter().position(|w| {
        let w = w.trim_matches(|c| c == '(' || c == ')' || c == '{' || c == '}');
        !w.is_empty() && !w.contains('=') && !WRAPPERS.contains(&w)
    }) else {
        return Vec::new();
    };
    let args = &words[program + 1..];
    if label.starts_with("git ") || label.starts_with("gh ") {
        return args.iter().position(|w| w == "--").map(|i| args[i + 1..].to_vec()).unwrap_or_default();
    }
    let pattern_first = PATTERN_FIRST.contains(&label);
    let mut pattern_taken = !pattern_first
        || args.iter().any(|w| w.starts_with('-') && pattern_flag(label, w).1);
    let mut out = Vec::new();
    let mut skip = 0usize;
    for w in args {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if w.starts_with('-') && w.len() > 1 {
            if pattern_first {
                skip = pattern_flag(label, w).0;
            }
            continue;
        }
        if !pattern_taken {
            pattern_taken = true;
            continue;
        }
        out.push(w.clone());
    }
    out
}

/// A stage's words as the shell splits them: quotes group and are
/// removed, a backslash escapes outside single quotes. Each word says
/// whether any of it was quoted — a quoted `>` is text, not a redirection.
fn shell_words(stage: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut single, mut double) = (false, false);
    let (mut started, mut quoted) = (false, false);
    let mut chars = stage.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !double => {
                single = !single;
                started = true;
                quoted = true;
            }
            '"' if !single => {
                double = !double;
                started = true;
                quoted = true;
            }
            '\\' if !single => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                    started = true;
                }
            }
            c if c.is_whitespace() && !single && !double => {
                if started {
                    out.push((std::mem::take(&mut cur), quoted));
                }
                started = false;
                quoted = false;
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push((cur, quoted));
    }
    out
}

/// A path as the filesystem would resolve its `.` and `..`, without
/// touching the filesystem: `repo/src/../README.md` is `repo/README.md`.
fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|s| *s != "..") {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Where a `cd` (or `pushd`) stage moves the command, if this stage is
/// one: `Some(Some(dir))` for a place it can name, `Some(None)` for one it
/// cannot — a variable, `-`, home — after which relative paths are
/// unknown rather than resolved against the wrong directory. `None` when
/// the stage is not a move at all.
fn cd_to(stage: &str, here: Option<&str>) -> Option<Option<String>> {
    let words: Vec<&str> = stage
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '(' || c == ')' || c == '{' || c == '}'))
        .filter(|w| !w.is_empty())
        .collect();
    match words.first().copied() {
        Some("cd" | "pushd") => {}
        Some("popd") => return Some(None),
        _ => return None,
    }
    let Some(arg) = words[1..].iter().find(|w| !w.starts_with('-') || **w == "-") else {
        return Some(None);
    };
    let arg = arg.trim_matches(|c| c == '\'' || c == '"');
    if arg == "-" || arg.starts_with('~') || arg.contains('$') || arg.contains('`') {
        return Some(None);
    }
    let path = if arg.starts_with('/') {
        arg.to_string()
    } else if let Some(h) = here {
        format!("{}/{}", h.trim_end_matches('/'), arg)
    } else {
        return Some(None);
    };
    Some(Some(normalize(&path)))
}

/// Each stage of a command with the directory it runs in (#412). A `cd`
/// moves every stage after it; one inside `( … )` moves only the rest of
/// its subshell, so `(cd ui && npm run build) && cat > src/x.rs` writes
/// beside the session again. Reading every path against the transcript's
/// `cwd` filed `cd /tmp/x && cat > f` as a file of the repository. The
/// `cd` stages themselves are not returned.
fn stages_in_place(cmd: &str, cwd: Option<&str>) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut here: Option<String> = cwd.map(str::to_string);
    let mut outer: Vec<Option<String>> = Vec::new();
    for stage in split_stages(&strip_heredocs(cmd)) {
        let t = stage.trim();
        for _ in 0..t.chars().take_while(|&c| c == '(').count() {
            outer.push(here.clone());
        }
        let closes = t.chars().rev().take_while(|&c| c == ')').count();
        match cd_to(&stage, here.as_deref()) {
            Some(next) => here = next,
            None => out.push((stage, here.clone())),
        }
        for _ in 0..closes {
            if let Some(prev) = outer.pop() {
                here = prev;
            }
        }
    }
    out
}

/// Does this stage open a heredoc — `<<EOF`, `<<'EOF'`, `<<-"EOF"`? The
/// same reading [`heredocs`] and [`strip_heredocs`] make of a line.
fn opens_heredoc(stage: &str) -> bool {
    stage.find("<<").is_some_and(|pos| {
        let raw = stage[pos + 2..].trim_start_matches('-').split_whitespace().next().unwrap_or("");
        let delim = raw.trim_matches(|c| c == '\'' || c == '"');
        !delim.is_empty() && delim.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Sessions and duplicate artifacts named on the page, at most (#406).
const FOCUS_ROWS: usize = 12; // skill-allow: §9-const — render-layer cap

/// Paths remembered per written shape, at most (#406).
const DUP_PATHS: usize = 8; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Bright-line paths named on the page, at most (#392).
const BRIGHT_LINE_SHOWN: usize = 6; // skill-allow: §9-const — render-layer cap

/// Paths taken from one inspecting call, at most.
const INSPECT_PATHS: usize = 8; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Programs whose path argument is a file they change.
const WRITE_PROGRAMS: [&str; 5] = ["cp", "mv", "install", "touch", "tee"];
/// Interpreters that take their program on stdin (`python3 -`): the
/// heredoc is a script, and a script that writes is a write (#374).
const STDIN_INTERPRETERS: [&str; 5] = ["python3", "python", "perl", "node", "ruby"];
/// What a script looks like when it writes a file.
const SCRIPT_WRITE_MARKERS: [&str; 8] = [
    "write_text(", "write_bytes(", "writeFileSync(", "appendFileSync(", ".write(", ", 'w'",
    ", \"w\"", ", 'a'",
];
/// Where a script names the file: the string literal right after one
/// of these.
const SCRIPT_PATH_OPENERS: [&str; 4] = ["Path(", "open(", "writeFileSync(", "appendFileSync("];
/// Verbs that change files without naming them on the line.
const WRITE_VERBS: [&str; 2] = ["git apply", "patch"];

/// A shell call that changed files (#374): what it touched, and the
/// lines it can be SEEN to have written — the body of a heredoc that
/// went straight into a file. Everything else a shell writes is of
/// unknown size and is reported as none, not as zero by omission.
struct ShellWrite {
    /// Each file it changed, with what it wrote there. One call often
    /// writes the scratchpad and the repository at once, so what it wrote
    /// is kept per file: setting the scratch file aside must not leave its
    /// lines, its shape or its "created" on the repository's (#412).
    files: Vec<Written>,
    /// It changed something it did not name: `git apply`, `patch`, a
    /// script writing through a variable, `find -exec sed -i`.
    pathless: bool,
}

/// One file a shell call wrote.
struct Written {
    path: String,
    /// The lines written into it, when the text is on the line: a heredoc.
    lines: Option<i64>,
    /// The shape of that text, so the same content landing in two files
    /// is visible (#406).
    shape: Option<u64>,
    /// Written end to end — `cat > f <<EOF` — rather than edited in place
    /// or appended to. The created/modified split reads it the way it
    /// reads a whole-file `Write` (#388).
    whole: bool,
}

impl ShellWrite {
    /// Keep only the files that are work, and say whether anything is
    /// left to count (#412). A heredoc into the scratchpad is neither a
    /// write to the work nor a read of it.
    fn into_work(mut self, checkouts: &crate::checkout::Checkouts) -> Option<Self> {
        self.files.retain(|f| is_work_path(&f.path, checkouts));
        (self.pathless || !self.files.is_empty()).then_some(self)
    }

    /// The lines it can be seen to have written.
    fn added(&self) -> i64 {
        self.files.iter().filter_map(|f| f.lines).sum()
    }

    /// Did it change something where it lay, rather than only write files
    /// whole? That is one rewrite (#388).
    fn rewrote(&self) -> bool {
        self.pathless || self.files.iter().any(|f| !f.whole)
    }
}

/// Note a file the call wrote, once: a file written whole anywhere in the
/// call was created by it, and its heredocs' lines add up.
fn note_written(files: &mut Vec<Written>, w: Written) {
    match files.iter_mut().find(|f| f.path == w.path) {
        Some(f) => {
            f.whole |= w.whole;
            if let Some(n) = w.lines {
                f.lines = Some(f.lines.unwrap_or(0) + n);
            }
            if f.shape.is_none() {
                f.shape = w.shape;
            }
        }
        None => files.push(w),
    }
}

/// A file the call changed and of which nothing more is known.
fn touched(path: String) -> Written {
    Written { path, lines: None, shape: None, whole: false }
}

/// The heredocs in a command: the line that opened each, with its body.
fn heredocs(cmd: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = cmd.lines();
    while let Some(line) = lines.next() {
        if let Some(pos) = line.find("<<") {
            let raw = line[pos + 2..]
                .trim_start_matches('-')
                .split_whitespace()
                .next()
                .unwrap_or("");
            let delim = raw.trim_matches(|c| c == '\'' || c == '"');
            if !delim.is_empty() && delim.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let mut body = String::new();
                for b in lines.by_ref() {
                    if b.trim() == delim {
                        break;
                    }
                    body.push_str(b);
                    body.push('\n');
                }
                out.push((line.to_string(), body));
            }
        }
    }
    out
}

/// A token that names a file the agent wrote, absolute against `cwd`.
/// Scratch and device paths are not work; an unexpanded `$VAR` is not
/// a path we can name.
fn written_path(w: &str, cwd: Option<&str>) -> Option<String> {
    let w = w.trim_matches(|c| c == '\'' || c == '"' || c == ',' || c == ';');
    if w.is_empty()
        || w.starts_with('-')
        || w.starts_with('&')
        || w.contains('$')
        || w.contains('`')
        || w.contains("://")
        || w.contains('*')
    {
        return None;
    }
    let path = if w.starts_with('/') || w.starts_with('~') {
        w.to_string()
    } else if w.contains('/') || w.contains('.') {
        format!("{}/{}", cwd?.trim_end_matches('/'), w)
    } else {
        return None;
    };
    // A device is never a file anyone wrote. Whether a real path is WORK
    // — anything but the agent's own scratch; a file in a checkout the
    // range worked in always is — is the caller's call (#412).
    if path.starts_with("/dev/") {
        return None;
    }
    Some(normalize(&path))
}

/// The files a stage's `>` / `>>` / `&>` redirections write, and whether
/// each only appends (`>>`). `2>` is a log of the run, not work; `>&2` is
/// plumbing.
fn redirection_targets(stage: &str, cwd: Option<&str>) -> Vec<(String, bool)> {
    let words: Vec<&str> = stage.split_whitespace().collect();
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let core = words[i].trim_start_matches(['1', '&']);
        if core.starts_with('>') {
            let appends = core.starts_with(">>");
            let rest = core.trim_start_matches('>');
            let target = if rest.is_empty() {
                i += 1;
                words.get(i).copied()
            } else if rest.starts_with('&') {
                None
            } else {
                Some(rest)
            };
            if let Some(p) = target.and_then(|t| written_path(t, cwd)) {
                if !out.iter().any(|(q, _)| *q == p) {
                    out.push((p, appends));
                }
            }
        }
        i += 1;
    }
    out
}

fn script_writes(body: &str) -> bool {
    SCRIPT_WRITE_MARKERS.iter().any(|m| body.contains(m))
}

/// The files a script names in its openers — `Path('…')`, `open("…"`.
fn script_paths(body: &str, cwd: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for opener in SCRIPT_PATH_OPENERS {
        let mut rest = body;
        while let Some(i) = rest.find(opener) {
            rest = &rest[i + opener.len()..];
            let Some(q) = rest.chars().next() else { break };
            if q == '\'' || q == '"' {
                if let Some(end) = rest[1..].find(q) {
                    if let Some(p) = written_path(&rest[1..1 + end], cwd) {
                        if !out.contains(&p) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Did this shell call WRITE, and what? Under an operating mode that
/// edits through `python3 - <<EOF`, `cat > file <<EOF` and `sed -i`,
/// every edit of a working day was invisible: the session read `—`
/// while it rewrote four files (#374). The heredoc bodies say what was
/// written where; redirections, in-place editors and copying programs
/// name their files; a script that writes names its files in its
/// openers. Lines are counted only where the text is on the line —
/// a heredoc into a file — and are otherwise unknown.
fn shell_write(cmd: &str, cwd: Option<&str>) -> Option<ShellWrite> {
    let mut files: Vec<Written> = Vec::new();
    let mut wrote = false;
    let mut pathless = false;
    // The heredoc bodies, in the order their stages open them.
    let mut docs = heredocs(cmd).into_iter().map(|(_, body)| body);
    for (stage, here) in stages_in_place(cmd, cwd) {
        let at = here.as_deref();
        let body = if opens_heredoc(&stage) { docs.next() } else { None };
        let stripped = strip_redirections(&stage);
        let Some(label) = stage_label(&stripped) else { continue };
        let targets = redirection_targets(&stage, at);
        if let Some(body) = &body {
            // `cat > file <<EOF` and `tee file <<EOF`: the file IS the body —
            // all of it, unless the call only appends (`>>`, `tee -a`).
            let written: Vec<(String, bool)> = match label.as_str() {
                "cat" => targets.clone(),
                "tee" => {
                    let appends = stripped.split_whitespace().any(|w| w == "-a" || w == "--append");
                    stripped
                        .split_whitespace()
                        .skip(1)
                        .filter_map(|w| written_path(w, at))
                        .map(|p| (p, appends))
                        .collect()
                }
                _ => Vec::new(),
            };
            if !written.is_empty() {
                wrote = true;
                let lines = line_count(body.trim_end_matches('\n'));
                // Each file keeps its OWN body's shape: one call writing
                // four different files is not one text in four places.
                let shape = snippet_key(body);
                for (path, appends) in written {
                    note_written(&mut files, Written { path, lines: Some(lines), shape, whole: !appends });
                }
            } else if STDIN_INTERPRETERS.contains(&label.as_str())
                && stripped.split_whitespace().any(|w| w == "-")
                && script_writes(body)
            {
                wrote = true;
                let named = script_paths(body, at);
                // A script that writes through a variable names nothing,
                // and is a write all the same (#415 review).
                pathless |= named.is_empty();
                for p in named {
                    note_written(&mut files, touched(p));
                }
            }
        }
        // Redirections, in-place editors, copying programs.
        for (t, _) in targets {
            wrote = true;
            note_written(&mut files, touched(t));
        }
        let words: Vec<&str> = stripped.split_whitespace().collect();
        if edits_in_place(&label, &stripped) {
            // `sed -i`, `awk -i inplace`, `perl -i`: the file is the last
            // path.
            wrote = true;
            match words.iter().rev().find_map(|w| written_path(w, at)) {
                Some(p) => note_written(&mut files, touched(p)),
                None => pathless = true,
            }
        } else if WRITE_PROGRAMS.contains(&label.as_str()) {
            wrote = true;
            let named: Vec<String> = if matches!(label.as_str(), "cp" | "mv" | "install") {
                // The destination is the file changed.
                words.iter().rev().find_map(|w| written_path(w, at)).into_iter().collect()
            } else {
                words.iter().skip(1).filter_map(|w| written_path(w, at)).collect()
            };
            pathless |= named.is_empty();
            for p in named {
                note_written(&mut files, touched(p));
            }
        } else if WRITE_VERBS.contains(&label.as_str()) {
            wrote = true;
            pathless = true;
        } else if let Some(inner) = handed_command(&label, &stripped) {
            // `find -exec sed -i …`, `xargs perl -pi …`: it edits the files
            // it is handed, which the line does not name.
            if stage_label(&inner).is_some_and(|l| edits_in_place(&l, &inner) || WRITE_PROGRAMS.contains(&l.as_str())) {
                wrote = true;
                pathless = true;
            }
        }
    }
    // A write that names no file we can read is still a write; one that
    // names only files outside the work is the caller's to drop (#412).
    pathless |= files.is_empty();
    wrote.then_some(ShellWrite { files, pathless })
}

/// What a shell stage SHIPPED (#381): outcomes, where every other
/// instrument counts effort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ship {
    Commit,
    Push,
    PrOpened,
    PrMerged,
}

/// The shipping events in a command, with the PR number when the line
/// names one (`gh pr merge 384`). `git merge` is local and `gh pr view`
/// only looks; neither ships.
fn shipping(cmd: &str) -> Vec<(Ship, Option<String>)> {
    let mut out = Vec::new();
    for stage in split_stages(&strip_heredocs(cmd)) {
        let stripped = strip_redirections(&stage);
        let Some(label) = stage_label(&stripped) else { continue };
        match label.as_str() {
            "git commit" => out.push((Ship::Commit, None)),
            "git push" => out.push((Ship::Push, None)),
            "gh pr" => {
                let words: Vec<&str> = stripped.split_whitespace().collect();
                let Some(i) = words.iter().position(|w| *w == "pr") else { continue };
                match words.get(i + 1).copied() {
                    Some("create") => out.push((Ship::PrOpened, None)),
                    Some("merge") => {
                        let num = words[i + 2..]
                            .iter()
                            .find(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()))
                            .map(|w| format!("#{w}"));
                        out.push((Ship::PrMerged, num));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether the call commits — its output then carries the commit's
/// shortstat, the one place the transcript states lines COMMITTED,
/// however the edits were made.
fn commits(cmd: &str) -> bool {
    labelled_stages(cmd).iter().any(|(label, _)| label == "git commit")
}

/// `N files changed, X insertions(+), Y deletions(-)` as git prints it
/// after a commit: insertions and deletions, either absent when zero.
/// The last such line wins — a chain prints one per commit.
fn shortstat(text: &str) -> Option<(i64, i64)> {
    let mut found = None;
    for line in text.lines() {
        if !(line.contains(" changed") && (line.contains("insertion") || line.contains("deletion"))) {
            continue;
        }
        let (mut ins, mut del) = (0i64, 0i64);
        for part in line.split(',') {
            let mut w = part.split_whitespace();
            let (Some(n), Some(kind)) = (w.next(), w.next()) else { continue };
            let Ok(v) = n.parse::<i64>() else { continue };
            if kind.starts_with("insertion") {
                ins = v;
            } else if kind.starts_with("deletion") {
                del = v;
            }
        }
        found = Some((ins, del));
    }
    found
}

/// Lines a commit's OWN output says it committed (#412): the shortstat
/// git prints right under the `[branch abc1234] subject` line of each
/// commit. A shortstat anywhere else in the output belongs to another
/// command of the chain — `git show --stat`, `git log --shortstat` — and
/// `git commit -q`, which every commit of the last week ran, prints none.
/// `None` when no commit line carried one; the page says so rather than
/// reading zero.
fn commit_shortstat(text: &str) -> Option<(i64, i64)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found: Option<(i64, i64)> = None;
    for (i, line) in lines.iter().enumerate() {
        if commit_header(line).is_none() {
            continue;
        }
        // The stat is in the indented block git prints under the header —
        // ` Author:`, ` 3 files changed, …`, ` create mode …`. An empty or
        // merge commit prints none, and a `git show --stat` after it is
        // not its stat (#415 review).
        if let Some((ins, del)) = lines[i + 1..]
            .iter()
            .take_while(|l| l.starts_with(' '))
            .find_map(|l| shortstat(l))
        {
            let acc = found.get_or_insert((0, 0));
            acc.0 += ins;
            acc.1 += del;
        }
    }
    found
}

/// Did this shipping event happen, as far as the call's output says
/// (#412)? A refused call never gets here — it ran nothing. Failures are
/// read from what git and gh print when they fail; a PR counts as opened
/// only when gh printed the new PR's address. When the call itself
/// `failed`, a stage of it did, and a quiet `git commit -q` or `git push
/// -q` before or after that stage cannot be told apart: only what the
/// output shows happened counts (#415 review).
fn shipped(ship: Ship, text: &str, cmd: &str, failed: bool) -> bool {
    let has = |m: &str| text.contains(m);
    match ship {
        Ship::Commit => {
            if text.lines().any(|l| commit_header(l).is_some()) {
                return true;
            }
            if failed {
                return false;
            }
            // A refused commit says "nothing to commit" — and so does every
            // `git status` of a clean tree. Only the ones beyond the
            // statuses the call ran are the commit's own.
            let refusals: usize = ["nothing to commit", "no changes added to commit", "nothing added to commit"]
                .iter()
                .map(|m| text.matches(m).count())
                .sum();
            refusals <= long_statuses(cmd)
        }
        Ship::Push => {
            let pushed = text.lines().any(|l| l.starts_with("To ")) && text.contains(" -> ");
            let refused = has("[rejected]") || has("[remote rejected]") || has("failed to push") || has("Everything up-to-date");
            !refused && (pushed || !failed)
        }
        Ship::PrOpened => text.contains("/pull/") && !(has("already exists") || has("create failed")),
        Ship::PrMerged => {
            let merged = has("Merged pull request") || has("and merged pull request");
            let refused = has("is not mergeable")
                || has("GraphQL:")
                || has("was already merged")
                || has("could not merge")
                || has("X Pull request");
            !refused && (merged || !failed)
        }
    }
}

/// How many `git status` runs in a command print the long form, which says
/// "nothing to commit" of a clean tree.
fn long_statuses(cmd: &str) -> usize {
    labelled_stages(cmd)
        .iter()
        .filter(|(label, stage)| {
            label == "git status"
                && !stage.split_whitespace().any(|w| {
                    w == "--short" || w.starts_with("--porcelain") || (w.starts_with('-') && !w.starts_with("--") && w.contains('s'))
                })
        })
        .count()
}

/// The three gates a pull request is held to (#392).
#[derive(Clone, Copy)]
enum Gate {
    Test,
    Lint,
    Audit,
}

/// An exit code a session echoed for itself — `CLIPPY=0`, `AUDIT_EXIT=1`,
/// `skill_audit exit=0` — which is how these sessions report a gate whose
/// output they piped away.
fn echoed_exit(text: &str, names: &[&str]) -> Option<bool> {
    let mut seen = None;
    for line in text.lines() {
        let low = line.to_ascii_lowercase();
        for name in names {
            // A word start: `latest=1` is not `test=1`.
            let Some(i) = low
                .match_indices(name)
                .map(|(i, _)| i)
                .find(|&i| low[..i].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric()))
            else {
                continue;
            };
            let rest = low[i + name.len()..].trim_start_matches(['_', ' ']);
            let rest = rest.strip_prefix("exit").unwrap_or(rest).trim_start();
            let Some(code) = rest.strip_prefix('=') else { continue };
            let digits: String = code.trim_start().chars().take_while(char::is_ascii_digit).collect();
            if let Ok(c) = digits.parse::<i64>() {
                seen = Some(c == 0);
            }
        }
    }
    seen
}

/// Did a gate pass, as its output shows (#412)? Exit status cannot say —
/// these gates are routinely piped through `tail` — so the tools' own
/// verdicts are read: a failing tally or a compiler error fails, `test
/// result: ok`, a clean `Finished` or `SKILL AUDIT CLEAN` passes, and an
/// output that shows neither is not counted as a pass. An exit code the
/// session echoed for the gate (`TEST_EXIT=0`) is the verdict itself —
/// another command's `error:` line in the same call does not overturn it
/// — unless the gate was `piped`, when `$?` was the pipe's last program's.
fn gate_passed(gate: Gate, text: Option<&str>, piped: bool) -> bool {
    let Some(text) = text else { return false };
    let echoed = |names: &[&str]| if piped { None } else { echoed_exit(text, names) };
    let errored = text.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("error[") || t.starts_with("error:")
    });
    match gate {
        Gate::Test => echoed(&["test"]).unwrap_or_else(|| {
            !errored && !text.contains("test result: FAILED") && text.contains("test result: ok")
        }),
        Gate::Lint => echoed(&["clippy"]).unwrap_or_else(|| !errored && text.contains("Finished")),
        Gate::Audit => echoed(&["skill_audit", "audit"])
            .unwrap_or_else(|| !text.contains("SKILL AUDIT FAILED") && text.contains("SKILL AUDIT CLEAN")),
    }
}

/// Was a gate's stage piped into another program? `cargo test | tail -3;
/// echo TEST_EXIT=$?` echoes tail's exit code, not the tests'.
fn gate_piped(cmd: &str, gate: Gate) -> bool {
    split_stages_piped(&strip_heredocs(cmd)).iter().any(|(stage, piped)| {
        *piped
            && match gate {
                Gate::Test => stage_label(&strip_redirections(stage)).as_deref() == Some("cargo test"),
                Gate::Lint => stage_label(&strip_redirections(stage)).as_deref() == Some("cargo clippy"),
                Gate::Audit => stage.contains("skill_audit"),
            }
    })
}

/// What a shipping call's output said, for the live row: the PR number
/// `gh pr create` printed, else the commit's short hash from
/// `[branch abc1234] message`.
fn shipping_detail(text: &str) -> Option<String> {
    if let Some(i) = text.find("/pull/") {
        let num: String = text[i + 6..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if !num.is_empty() {
            return Some(format!("#{num}"));
        }
    }
    // The first line that IS a commit's header: a hook's `[WARNING] …`
    // may come before it (#415 review).
    text.lines().find_map(commit_header).map(str::to_string)
}

/// The short hash in the header git prints for a commit it made —
/// `[main abc1234] subject`, `[main (root-commit) abc1234] subject` — when
/// this line is one.
fn commit_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let hash = rest[..close].split_whitespace().last()?;
    (hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some(hash)
}

/// The key a call repeats under: the whole line, noise stages dropped,
/// whitespace folded, bounded. Keyed on the PROGRAM, `echo` won the
/// "fighting something" badge every day on a live instance (#367).
fn repeat_key(cmd: &str) -> Option<String> {
    let stripped = strip_heredocs(cmd);
    let real: Vec<String> = split_stages(&stripped)
        .iter()
        .map(|stage| strip_redirections(stage))
        .filter(|stage| stage_label(stage).is_some_and(|label| !is_noise(&label)))
        .map(|stage| stage.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    if real.is_empty() {
        return None;
    }
    let mut key = real.join(" && ");
    if key.len() > REPEAT_KEY_CHARS {
        let cut = key
            .char_indices()
            .nth(REPEAT_KEY_CHARS)
            .map_or(key.len(), |(i, _)| i);
        key.truncate(cut);
    }
    Some(key)
}

/// Characters a repeat key keeps.
const REPEAT_KEY_CHARS: usize = 160; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Split a command line into stages at the separators that are
/// really separators.
///
/// Quoting is the whole point. `grep -E "passed|failed"` is ONE call,
/// not two, and splitting it blind invents a stage whose program is
/// `failed"`. Probing 5,493 live shell commands, a blind split
/// produced 30,897 stages across 3,377 distinct labels — 1,633 of
/// them junk like `Co-Authored-By:` and `print('`. Respecting quotes
/// gives 24,606 stages across 258 labels, 25 of them junk. The 6,291
/// difference was never work; it was the insides of strings.
///
/// `&&` splits, a lone `&` does not: `2>&1` contains one, and
/// splitting there produced a stage whose program was `1` — the
/// most-repeated "command" on a live instance (issue #335).
fn split_stages(cmd: &str) -> Vec<String> {
    split_stages_piped(cmd).into_iter().map(|(stage, _)| stage).collect()
}

/// [`split_stages`], saying of each stage whether it fed a pipe — a lone
/// `|`, not the `||` of a fallback.
fn split_stages_piped(cmd: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut single, mut double) = (false, false);
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !double => {
                single = !single;
                cur.push(c);
            }
            '"' if !single => {
                double = !double;
                cur.push(c);
            }
            '|' if !single && !double && chars.peek() == Some(&'|') => {
                // `a || b` keeps the two splits it always made.
                chars.next();
                out.push((std::mem::take(&mut cur), false));
                out.push((String::new(), false));
            }
            '|' if !single && !double => out.push((std::mem::take(&mut cur), true)),
            ';' | '\n' | '\r' if !single && !double => out.push((std::mem::take(&mut cur), false)),
            '&' if !single && !double && chars.peek() == Some(&'&') => {
                chars.next();
                out.push((std::mem::take(&mut cur), false));
            }
            _ => cur.push(c),
        }
    }
    out.push((cur, false));
    out
}

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
    let mut out: Vec<&str> = Vec::new();
    let mut target_next = false;
    for w in stage.split_whitespace() {
        if target_next {
            // The file a bare operator points at: `> out.txt`. It is
            // written, never read, and never a program's argument (#412).
            target_next = false;
            continue;
        }
        if is_redirection(w) {
            target_next = bare_redirection(w);
            continue;
        }
        out.push(w);
    }
    out.join(" ")
}

/// A redirection word: an operator, with or without its target attached.
fn is_redirection(w: &str) -> bool {
    w.contains(">&")
        || w.contains("&>")
        || w.starts_with('>')
        || w.starts_with('<')
        || w.starts_with("2>")
        || w.starts_with("1>")
        || w == "&"
}

/// An operator whose target is the NEXT word: `> out.txt`.
fn bare_redirection(w: &str) -> bool {
    matches!(w, ">" | ">>" | "<" | "1>" | "1>>" | "2>" | "2>>" | "&>" | "&>>")
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
    // A language suffix is letters and digits: `rs`, `py`, `ipynb`,
    // `h`. `superx.prev-5001959` and `notes.2026-09-11` are versioned
    // names, and they ranked as languages on the live page (#388).
    if stem.is_empty()
        || ext.len() > 10
        || !ext.chars().all(|c| c.is_ascii_alphanumeric())
        || !ext.chars().any(|c| c.is_ascii_alphabetic())
    {
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
    /// Every working directory the range's messages were sent from —
    /// the repositories to ask what landed (#386).
    cwds: HashSet<String>,
    lines_added: i64,
    lines_removed: i64,
    /// Writes whose replaced half is unknown — shell edits, notebook
    /// cells — so no ratio built on `lines_removed` reads them as zero (#383).
    replaced_unknown: i64,
    writes: i64,
    reads: i64,
    tests: i64,
    builds: i64,
    git: i64,
    /// What shipped (#381): commits, pushes, PRs opened and merged, and
    /// the lines git reported committed.
    commits: i64,
    pushes: i64,
    prs_opened: i64,
    prs_merged: i64,
    committed_added: i64,
    committed_removed: i64,
    /// Commit events whose own output carried a shortstat (#412). Fewer
    /// than `commits` means the committed lines are partial.
    commits_with_stat: i64,
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
    // ── time-graded outcomes and the cost of time (#337) ────────
    /// Outcomes per hour, keyed `YYYY-MM-DDTHH`.
    quality: HashMap<String, (i64, i64, i64)>,
    /// (calls, failures) by hour of day.
    by_hour: HashMap<i64, (i64, i64)>,
    /// Every `durationMs` seen, for median and p95.
    waits: Vec<i64>,
    slowest: Vec<SlowOp>,
    interrupted: i64,
    /// Per session: first/last instant, agent, repo, message count.
    spans: HashMap<String, Span>,
    /// Replaced lines with and without a human instruction behind them.
    churn_directed: i64,
    churn_self: i64,
    /// The same split counted in EDITS, not lines. A shell edit
    /// replaces an unknown number of lines (#383), so a line-weighted
    /// split reads zero for a whole day's work — an edit is one event
    /// whatever its size, and whether anyone asked for it is knowable
    /// either way (#388).
    edits_directed: i64,
    edits_self: i64,
    // ── quality, from what the commands printed (#327) ───────────
    tests_passed: i64,
    tests_failed: i64,
    compile_errors: i64,
    denials: i64,
    compactions: i64,
    interventions: i64,
    // ── the repo and model dimensions (#325, #328) ───────────────
    repos: HashMap<String, RepoAgg>,
    /// Per (repo, branch) — the dimension `repos` collapses (#350).
    branches: HashMap<(String, String), BranchAgg>,
    /// Newest branch each session was on, so post-walk reductions
    /// keyed by session (verify gaps) can reach a branch.
    session_branch: HashMap<String, (String, String)>,
    /// Path → the (repo, branch) that owned it, for files-created.
    path_branch: HashMap<String, (String, String)>,
    models: HashMap<String, ModelAgg>,
    /// Outcomes per reasoning level (#337).
    efforts: HashMap<String, EffortAgg>,
    /// Productivity and cost per agent (#337).
    agents: HashMap<String, AgentAgg>,
    // ── the work cube: agent × repo × bucket (#340) ──────────────
    cells: HashMap<(String, String, String), CellAgg>,
    /// path → the tool of its OLDEST event in the window. The walk is
    /// newest-first, so the last write here wins and that is the
    /// oldest one — created-in-window falls out for free.
    path_origin: HashMap<String, bool>,
    /// path → repo, so created/modified splits per repo.
    path_repo: HashMap<String, String>,
    /// session → the repo seen at the previous (newer) message, for
    /// counting crossings.
    last_repo: HashMap<String, String>,
    repo_switches: i64,
    /// Hour → (output, thinking, prompt, cache read). Every other
    /// series on the page moves over time; tokens were only ever a
    /// total, so nothing said WHEN the money went (#391).
    burn: BTreeMap<String, (i64, i64, i64, i64)>,
    /// Output tokens spent on a message with no human turn in the
    /// steering window before it — the unsupervised share (#391).
    unattended_out: i64,
    /// Hour → how many fronts were open and what moved (#395).
    intensity: BTreeMap<String, IntensityAgg>,
    /// session → the repositories it moved between (#395).
    session_repos: HashMap<String, HashSet<String>>,
    /// session → what it did that the gates care about, with when (#392).
    gate_events: HashMap<String, Vec<(chrono::DateTime<chrono::Utc>, GateEvent)>>,
    /// Writes into territory a module lane must never touch: the
    /// kernel's crate, and schema files (#392).
    bright_line: BTreeMap<String, i64>,
    /// Sessions already met. The walk is newest-first, so the first
    /// message seen for a session is its newest (#381 D).
    seen_sessions: HashSet<String>,
    /// (model, effort) → outcomes (#391).
    model_effort: HashMap<(String, String), ModelEffortAgg>,
    /// (session, window between your turns) → the distinct directories
    /// worked in it. One instruction should mean one thing (#406).
    focus: HashMap<(String, usize), HashSet<String>>,
    /// The shape of a written snippet → every path it was written to.
    /// One artifact in several files drifts apart by construction
    /// (#406) — it is what made a Jira task and a README disagree.
    written_shapes: HashMap<u64, Vec<String>>,
    /// session → the branches it moved between (#406).
    session_branches: HashMap<String, HashSet<String>>,
    /// Branches opened in the range.
    branches_opened: i64,
    /// session → (denials, interruptions) met since its last reply that
    /// named a pair. An interruption or a refusal names no model — it
    /// rides the user's turn or the result line — so it is held, the
    /// way a tool result is held for its call, and drained into the
    /// next older reply that names one: the reply that was stopped
    /// (#391, #413).
    pending_steps: HashMap<String, (i64, i64)>,
    /// Agent-clock hours that saw any row of the range (#413) — the
    /// denominator every per-hour rate on the page needs, instead of
    /// the last day's hours whatever the range.
    active_hours: HashSet<String>,
    /// session → (time, is_write) events, reduced after the walk into
    /// how long a write waited for its verification.
    verify_events: HashMap<String, Vec<(chrono::DateTime<chrono::Utc>, bool)>>,
    /// snippet key → when a LATER edit removed it. Meeting the write
    /// that created it (earlier in the walk) yields its lifetime.
    removed_at: HashMap<u64, chrono::DateTime<chrono::Utc>>,
    survivals: Vec<i64>,
    repo_survivals: HashMap<String, Vec<i64>>,
    /// session → compaction facts (#340).
    compact_by_session: HashMap<String, CompactAgg>,
    // ── what left this machine (#337) ────────────────────────────
    in_tokens: i64,
    cache_write: i64,
    cache_read: i64,
    content_bytes: i64,
    files_read: HashSet<String>,
    repos_exposed: HashSet<String>,
    attachments: i64,
    outside_reads: i64,
    secret_hits: i64,
    secret_paths: HashSet<String>,
    /// Per-session live state, keyed by session uuid.
    live: HashMap<String, LiveAgg>,
    languages: HashMap<String, i64>,
    commands: HashMap<String, i64>,
    /// Whole command lines, for the repeat signal (#367).
    command_lines: HashMap<String, i64>,
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
///
/// A NotebookEdit that replaces or deletes a cell takes out text the
/// call never carried — the cell's prior source is not in the input —
/// so it replaces an UNKNOWN number of lines. That is reported as
/// none rather than invented: notebook rework reads as added lines
/// only, and the churn ratio understates it by exactly that (#346).
fn replaced_lines(name: &str, input: &Object) -> i64 {
    if !WRITE_TOOLS.contains(&name) || name == "NotebookEdit" {
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
    churn_directed: i64,
    churn_self: i64,
    /// The same split counted in EDITS, not lines. A shell edit
    /// replaces an unknown number of lines (#383), so a line-weighted
    /// split reads zero for a whole day's work — an edit is one event
    /// whatever its size, and whether anyone asked for it is knowable
    /// either way (#388).
    edits_directed: i64,
    edits_self: i64,
    reverts: i64,
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

/// One session's span: first seen, last seen, agent, repo, messages.
type Span = (
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
    String,
    Option<String>,
    i64,
);

/// One cell of the work cube (#340).
#[derive(Default)]
struct CellAgg {
    added: i64,
    removed: i64,
    files: HashSet<String>,
    out_tokens: i64,
    messages: i64,
}

/// What compaction cost one session (#340).
#[derive(Default)]
struct CompactAgg {
    agent: String,
    repo: Option<String>,
    durations: Vec<i64>,
    pre_tokens_max: i64,
    auto: i64,
    manual: i64,
}

/// Per-agent productivity and cost (#337).
#[derive(Default)]
struct AgentAgg {
    sessions: HashSet<String>,
    messages: i64,
    lines_added: i64,
    lines_removed: i64,
    out_tokens: i64,
    in_tokens: i64,
    tool_failures: i64,
    reverts: i64,
    repos: HashSet<String>,
    repo_switches: i64,
    verify_gaps: Vec<i64>,
    compactions: i64,
    compaction_ms: i64,
    // ── outcome, not volume (#350) ───────────────────────────────
    /// Two agents can write the same number of lines and only one of
    /// them was asked to rewrite what it rewrote.
    churn_directed: i64,
    churn_self: i64,
    /// The same split counted in EDITS, not lines. A shell edit
    /// replaces an unknown number of lines (#383), so a line-weighted
    /// split reads zero for a whole day's work — an edit is one event
    /// whatever its size, and whether anyone asked for it is knowable
    /// either way (#388).
    edits_directed: i64,
    edits_self: i64,
    tests_passed: i64,
    tests_failed: i64,
    compile_errors: i64,
}

/// A branch's quality as five components and one blend (#350).
///
/// The weights are HERE, in the open, and the UI renders every
/// component beside the blend — a composite nobody can audit is worse
/// than no composite. Each sub-score is 0..100 with 100 good:
///
/// | weight | component | why it carries that much |
/// |---|---|---|
/// | 30 | steering — `100 - self_churn_pct` | rewriting with nobody asking is the spaghetti signal |
/// | 25 | keep rate — `100 - rework_pct` | work that undid work |
/// | 20 | test pass rate | whether it actually ran |
/// | 15 | tool success | an agent fighting its tools |
/// | 10 | durability — half-life against `DURABLE_MINS` | thrash vs a design that moved |
///
/// A component with NO data is dropped and the remaining weights are
/// renormalised, so a branch that ran no tests does not read like a
/// branch whose tests all failed. When nothing at all is measurable
/// the blend is -1, which the UI renders as a dash.
const DURABLE_MINS: i64 = 120; // skill-allow: §9-const — analysis scale, render-layer

fn pct(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        return 0;
    }
    (part * 100 / whole).clamp(0, 100)
}

/// Every derived figure a branch row carries. Computed together
/// because they all come from the same aggregate and the blend needs
/// each of them — nine loose arguments was the wrong shape for one
/// question about one struct.
struct BranchDerived {
    self_churn_pct: i64,
    rework_pct: i64,
    test_pass_pct: i64,
    failures_per_100: i64,
    survival_p50_mins: i64,
    edit_to_verify_p50_secs: i64,
    quality_pct: i64,
}

fn branch_derived(b: &BranchAgg) -> BranchDerived {
    let churn = b.churn_directed + b.churn_self;
    let tests = b.tests_passed + b.tests_failed;
    // Replaced lines where the transcript can see them, else edits: a
    // branch worked entirely through shell edits scored a perfect
    // steering component because its line-weighted churn was zero
    // (#388).
    let self_churn_pct = if churn > 0 {
        pct(b.churn_self, churn)
    } else {
        pct(b.edits_self, b.edits_directed + b.edits_self)
    };
    let rework_pct = pct(b.lines_removed, b.lines_added);
    // -1, not 0: a branch that ran no tests must not read like one
    // whose tests all failed.
    let test_pass_pct = if tests > 0 { pct(b.tests_passed, tests) } else { -1 };
    // `pct` already scales by 100 — multiplying first made one failure
    // in a hundred calls read as 100, and zeroed the tool-success
    // component for any branch above a 1% failure rate.
    let failures_per_100 = pct(b.tool_failures, b.tool_calls);
    // No data is not 0: a branch where everything was overwritten inside
    // a minute medians to 0, and 0 used to render as a dash on precisely
    // the worst branch, while the blend docked it 10 points (#354 review).
    let survival_p50_mins = median(&mut b.survivals.clone());
    let mut gaps = b.verify_gaps.clone();

    let mut parts: Vec<(i64, i64)> = Vec::new();
    if churn > 0 {
        parts.push((30, 100 - self_churn_pct));
    }
    if b.lines_added > 0 {
        parts.push((25, (100 - rework_pct).clamp(0, 100)));
    }
    if test_pass_pct >= 0 {
        parts.push((20, test_pass_pct));
    }
    if b.tool_calls > 0 {
        parts.push((15, (100 - failures_per_100).clamp(0, 100)));
    }
    if !b.survivals.is_empty() {
        parts.push((10, pct(survival_p50_mins, DURABLE_MINS)));
    }
    let weight: i64 = parts.iter().map(|(w, _)| w).sum();

    BranchDerived {
        self_churn_pct,
        rework_pct,
        test_pass_pct,
        failures_per_100,
        survival_p50_mins,
        edit_to_verify_p50_secs: median(&mut gaps),
        quality_pct: if weight == 0 {
            -1
        } else {
            parts.iter().map(|(w, s)| w * s).sum::<i64>() / weight
        },
    }
}

/// The middle value, or zero for nothing (#340).
/// What a median of nothing reads as: no data, which the page renders
/// as a dash (#413). Never 0 — the page renders 0 as "under a minute".
const NO_DATA: i64 = -1;

/// The middle value, or [`NO_DATA`] for an empty sample — one guard for
/// every median on the page, where seven sites each wrote their own
/// (#415 review).
fn median(v: &mut [i64]) -> i64 {
    if v.is_empty() {
        return NO_DATA;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// The agent's own workspace is not your material. Scratchpads, task
/// files, memory and agent state live outside the repositories by
/// design; counting them buries the work that matters. Probing 25 live
/// transcripts, this filter took 64 flagged reads down to 6 — and all
/// six were an agent in one repo reaching into a different one, which
/// is precisely the signal (#338).
///
/// A path inside a checkout the range worked in is never scratch, even
/// under `.claude/`: a repository's worktrees live in
/// `.claude/worktrees/`, and matching `/.claude/` alone threw away every
/// shell edit made in one (#412).
fn scratch_path(path: &str) -> bool {
    path.contains("/.claude/")
        || path.starts_with("/tmp/")
        || path.starts_with("/private/tmp/")
        || path.starts_with("/var/folders/")
        || path.starts_with("/private/var/folders/")
}

/// Is a path the work — a file in one of the range's repositories, or
/// at least not the agent's own scratch (#412)?
fn is_work_path(path: &str, checkouts: &crate::checkout::Checkouts) -> bool {
    checkouts.holding(path).is_some() || !scratch_path(path)
}

/// Did a read reach outside the repository the agent stood in (#338)?
/// Against the checkout, not the directory: reading `../README.md` from
/// a subdirectory is the same repository, and a read into a worktree is
/// not the agent's scratch.
fn reads_outside(path: &str, cwd: Option<&str>, checkouts: &crate::checkout::Checkouts) -> bool {
    if checkouts.holding(path).is_none() && scratch_path(path) {
        return false;
    }
    match (cwd.and_then(|c| checkouts.of(c)), checkouts.holding(path)) {
        (Some(here), Some(there)) => here.common != there.common,
        (Some(_), None) => true,
        (None, _) => cwd.is_some_and(|c| !path.starts_with(c)),
    }
}

/// What Claude Code recorded a file tool as having done (#410): its
/// result carries `toolUseResult.structuredPatch` — the diff hunks it
/// applied — and, for a Write, whether the file was created or updated.
#[derive(Clone, Copy, Debug)]
struct RecordedDiff {
    added: i64,
    removed: i64,
    /// `Some(true)` when the call created the file.
    created: Option<bool>,
    /// A refused or failed call changed nothing.
    failed: bool,
}

/// Read the recorded diff off a tool-result row. `None` when the result
/// carries no patch: the lines are then estimated from the call.
fn recorded_diff(raw: &Object, failed: bool) -> Option<RecordedDiff> {
    if failed {
        return Some(RecordedDiff { added: 0, removed: 0, created: None, failed: true });
    }
    let tur = raw.get("toolUseResult").and_then(obj)?;
    let Some(Value::Array(hunks)) = tur.get("structuredPatch") else { return None };
    let (mut added, mut removed) = (0i64, 0i64);
    for hunk in hunks.iter().filter_map(obj) {
        let Some(Value::Array(lines)) = hunk.get("lines") else { continue };
        for line in lines.iter().filter_map(|l| match l {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }) {
            if line.starts_with('+') {
                added += 1;
            } else if line.starts_with('-') {
                removed += 1;
            }
        }
    }
    let created = match get_str(tur, "type") {
        Some("create") => {
            // A new file has no hunks: all of it is added.
            added = get_str(tur, "content").map_or(added, line_count);
            Some(true)
        }
        Some("update") => Some(false),
        // An Edit changes a file that is there.
        _ => Some(false),
    };
    Some(RecordedDiff { added, removed, created, failed: false })
}

/// A tool result's text: the string, or the text blocks of a list (#413).
/// Agent, ToolSearch and MCP results arrive as lists — 16% of results —
/// and were invisible to the byte count and the secret scan.
fn tool_result_text(block: &Object) -> Option<String> {
    match block.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let text: Vec<&str> = parts
                .iter()
                .filter_map(obj)
                .filter(|p| get_str(p, "type") == Some("text"))
                .filter_map(|p| get_str(p, "text"))
                .collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

/// Shapes that mean a credential is in the text. Deliberately narrow:
/// a false positive here sends someone hunting for a leak that is not
/// there, which is worse than silence (#337).
///
/// A prefix alone is not a credential (#413): this file names every
/// prefix it looks for, so reading it lit the lamp. A hit needs the
/// token's body after the prefix — its alphabet, at its length.
fn looks_like_secret(text: &str) -> bool {
    let body = |prefix: &str, min: usize, ok: fn(char) -> bool| {
        text.match_indices(prefix)
            .any(|(i, _)| text[i + prefix.len()..].chars().take_while(|c| ok(*c)).count() >= min)
    };
    fn token(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_' || c == '-'
    }
    pem_private_key(text)
        || body("AKIA", 16, |c| c.is_ascii_uppercase() || c.is_ascii_digit())
        || body("ghp_", 36, |c| c.is_ascii_alphanumeric())
        || body("github_pat_", 22, token)
        || body("xoxb-", 10, token)
        || body("sk-ant-", 20, token)
        // Atlassian API tokens (#413): one went by in a command unflagged.
        || body("ATATT", 30, |c| token(c) || c == '=')
}

/// A private key is a PEM block: `-----BEGIN … PRIVATE KEY-----`, then
/// its base64 body (#413). Naming the two markers anywhere in a text — as
/// the scanner's own source does — is not one. The block arrives wrapped
/// every way a tool shows it (#415 review): as lines, each maybe behind a
/// `Read` result's line number and tab or `grep -n`'s `12:`; as one line
/// with `\n` escapes, the way a service-account JSON file or an `.env`
/// value holds it; and, encrypted, behind its `Proc-Type:` and
/// `DEK-Info:` headers.
fn pem_private_key(text: &str) -> bool {
    const MARKER: &str = "PRIVATE KEY-----";
    text.match_indices(MARKER).any(|(at, _)| {
        let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
        if !text[line_start..at].contains("-----BEGIN ") {
            return false;
        }
        let after = &text[at + MARKER.len()..];
        let end = after
            .char_indices()
            .nth(PEM_LOOKAHEAD)
            .map_or(after.len(), |(i, _)| i);
        let after = after[..end].replace("\\r\\n", "\n").replace("\\n", "\n");
        // The rest of the marker's own line, then the headers and the blank
        // line an encrypted key carries, then the body's first line.
        after
            .lines()
            .skip(1)
            .filter(|l| {
                !numbered(l).1.trim().is_empty() && !l.contains("Proc-Type:") && !l.contains("DEK-Info:")
            })
            .take(1)
            .any(pem_body_line)
    })
}

/// A line split into the number a tool put in front of it and the rest.
/// A PEM body holds no tab, colon or dash, so the number ends at the last
/// of them: `  12\t`, `12:`, `12-`, `path:12:`.
fn numbered(line: &str) -> (&str, &str) {
    match line.rfind(['\t', ':', '-']) {
        Some(i) => (&line[..=i], &line[i + 1..]),
        None => ("", line),
    }
}

/// Characters of a PEM block read past its marker, at most: its headers
/// and the first line of its body.
const PEM_LOOKAHEAD: usize = 400; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A line of a PEM body: 40 or more characters of base64, indented or
/// behind nothing but the line number a tool put in front of it.
fn pem_body_line(line: &str) -> bool {
    let (prefix, body) = numbered(line);
    let body = body.trim();
    let prefix: Vec<char> = prefix.trim_end_matches(' ').chars().collect();
    let behind_a_number = match prefix.as_slice() {
        [] => true,
        [.., digit, '\t' | ':' | '-'] => digit.is_ascii_digit(),
        p => p.iter().all(|c| c.is_whitespace()),
    };
    behind_a_number
        && body.len() >= 40
        && body.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Outcomes attributable to one reasoning level (#337).
#[derive(Default)]
struct EffortAgg {
    messages: i64,
    lines_added: i64,
    lines_removed: i64,
    out_tokens: i64,
    thinking_tokens: i64,
    tool_failures: i64,
    reverts: i64,
    tests_passed: i64,
    tests_failed: i64,
}

/// How hard the machine was working in one bucket (#395). Burn says
/// what was spent and churn says what moved; neither says how many
/// fronts were open at once, which is the difference between one agent
/// thinking and four agents rewriting four repositories.
#[derive(Default)]
struct IntensityAgg {
    sessions: HashSet<String>,
    repos: HashSet<String>,
    added: i64,
    removed: i64,
    out_tokens: i64,
}

/// What a session did that the gates care about (#392). Collected with
/// its time because the question is an ORDER — did the checks run after
/// the last change and before the pull request — and the walk runs
/// newest-first, so it can only be answered once it has finished.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GateEvent {
    Wrote,
    Tested,
    Linted,
    Audited,
    OpenedPr,
}

/// Outcomes attributable to one (model, reasoning level) pair (#391).
/// Two separate tables cannot answer "does thinking harder pay" when
/// the model and the level change together, which is how they are
/// actually switched — the pair has to be one key.
#[derive(Default)]
struct ModelEffortAgg {
    sessions: HashSet<String>,
    messages: i64,
    out_tokens: i64,
    thinking_tokens: i64,
    lines_added: i64,
    lines_removed: i64,
    edits_directed: i64,
    edits_self: i64,
    tool_calls: i64,
    tool_failures: i64,
    reverts: i64,
    interventions: i64,
    denials: i64,
    tests_passed: i64,
    tests_failed: i64,
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

/// One branch of one repo (#350). Everything `RepoAgg` carries, plus
/// the outcome fields that previously only existed globally — a branch
/// with no quality attached cannot be compared to another.
#[derive(Default)]
struct BranchAgg {
    messages: i64,
    sessions: HashSet<String>,
    agents: HashSet<String>,
    lines_added: i64,
    lines_removed: i64,
    files: HashSet<String>,
    churn_directed: i64,
    churn_self: i64,
    /// The same split counted in EDITS, not lines. A shell edit
    /// replaces an unknown number of lines (#383), so a line-weighted
    /// split reads zero for a whole day's work — an edit is one event
    /// whatever its size, and whether anyone asked for it is knowable
    /// either way (#388).
    edits_directed: i64,
    edits_self: i64,
    /// Test INVOCATIONS, so an unparsed run is distinguishable from a
    /// branch that never ran one (#354 review).
    tests_run: i64,
    tests_passed: i64,
    tests_failed: i64,
    compile_errors: i64,
    tool_calls: i64,
    tool_failures: i64,
    reverts: i64,
    out_tokens: i64,
    last_active: Option<chrono::DateTime<chrono::Utc>>,
    survivals: Vec<i64>,
    verify_gaps: Vec<i64>,
}

/// What one session is doing, for the live panel (#325).
#[derive(Default)]
struct LiveAgg {
    agent: String,
    repo: Option<String>,
    branch: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    last_tool: Option<String>,
    messages: i64,
    lines_added: i64,
    lines_removed: i64,
    replaced_unknown: i64,
    /// Its newest message was the agent speaking without calling a
    /// tool: it has stopped, and the next move is yours (#381 D).
    awaiting: bool,
    /// The newest thing this session shipped, and when (#381).
    shipped: Option<String>,
    shipped_at: Option<chrono::DateTime<chrono::Utc>>,
    out_tokens: i64,
    tool_failures: i64,
    newest: Option<chrono::DateTime<chrono::Utc>>,
    // ── #350: what it is doing, and whether it is circling ───────
    /// Newest-first, capped — the walk sees the freshest calls first.
    files_now: Vec<String>,
    doing: Option<String>,
    /// The reply whose calls say what the session is doing: its newest
    /// tool-calling one (#413). The walk is newest-first, so the first
    /// tool call met for the session sets it.
    doing_reply: Option<String>,
    thinking_tokens: i64,
    last_op_ms: i64,
    /// Context at the newest usage-bearing message — first sighting
    /// wins, the walk being newest-first (#367).
    context_tokens: Option<i64>,
    churn_directed: i64,
    churn_self: i64,
    /// The same split counted in EDITS, not lines. A shell edit
    /// replaces an unknown number of lines (#383), so a line-weighted
    /// split reads zero for a whole day's work — an edit is one event
    /// whatever its size, and whether anyone asked for it is knowable
    /// either way (#388).
    edits_directed: i64,
    edits_self: i64,
    /// How many times each path was written in this session, for the
    /// rework-of-rework count.
    path_hits: HashMap<String, i64>,
}

/// What a live session is doing, strongest claim first. Three sites
/// used to race with `get_or_insert_with`, so the state was decided by
/// content-block ARRAY ORDER: a message emitting `[Read(lib.rs),
/// Bash(cargo test)]` reported `reading` because the exposure branch
/// ran first, and reversing the blocks changed the answer (#354
/// review). Verifying beats writing beats reading, whatever order the
/// blocks arrive in.
fn doing_rank(state: &str) -> u8 {
    match state {
        "verifying" => 4,
        "writing" => 3,
        "reading" => 2,
        // A call that is none of those — a fetch, a browser, an MCP tool:
        // the session is working, not thinking (#415 QA).
        "working" => 1,
        _ => 0,
    }
}

/// Claim what a session is doing NOW (#413): only the calls of its newest
/// tool-calling reply may claim, and the strongest of those wins. It used
/// to keep the strongest claim of the whole range, so a session that ran
/// `cargo test` twenty hours ago read "verifying" while it was reading.
fn claim_doing(l: &mut LiveAgg, reply: Option<&str>, state: &str) {
    if l.doing_reply.as_deref() != Some(reply.unwrap_or("")) {
        return;
    }
    if l.doing.as_deref().map(doing_rank).unwrap_or(0) < doing_rank(state) {
        l.doing = Some(state.to_string());
    }
}

/// How many paths the live panel names per row, and the touch count at
/// which a file counts as revisited (#350).
const LIVE_FILES: usize = 4; // skill-allow: §9-const — render-layer cap
const REVISIT_AT: i64 = 3; // skill-allow: §9-const — analysis threshold, render-layer
/// Runs of one command line at which it reads as fighting something.
const REPEAT_AT: i64 = 3; // skill-allow: §9-const — analysis threshold, render-layer

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

/// How recently a human turn must have happened for a rewrite to
/// count as DIRECTED rather than self-inflicted (#337). Ten minutes
/// is long enough to cover "do it differently" landing before the
/// rewrite, short enough that unrelated later work is not credited
/// to it.
const STEERING_MINUTES: i64 = 10; // skill-allow: §9-const — analysis window, render-layer

/// Rows the range walk will read at most. A month of heavy agent work
/// is far more than a page needs; the payload says when it truncated
/// rather than pretending the sample is the whole range.
pub const RANGE_ROW_CAP: u32 = 20_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Rows per page of the range walk (#367). One `LIMIT 20000` over
/// `message` carried every raw payload in a single frame, and the
/// engine reset the connection on `30d` and `all`. Pages keep each
/// frame small; their union is the same walk.
pub const RANGE_PAGE: u32 = 1_000; // skill-allow: §9-const — read-path bound, not a policy tunable

const WALK_FIRST: &str = "SELECT * FROM message ORDER BY valid_from DESC LIMIT $limit";
const WALK_FIRST_SINCE: &str = "SELECT * FROM message WHERE valid_from > $since \
     ORDER BY valid_from DESC LIMIT $limit";
const WALK_NEXT: &str = "SELECT * FROM message WHERE valid_from <= $before \
     ORDER BY valid_from DESC LIMIT $limit";
const WALK_NEXT_SINCE: &str = "SELECT * FROM message WHERE valid_from <= $before \
     AND valid_from > $since ORDER BY valid_from DESC LIMIT $limit";

/// The newest rows of `message`, newest first, in pages. `since`
/// bounds the range; `cap` bounds the rows. Each page continues from
/// `valid_from <= cursor` and is deduplicated by id, so rows sharing
/// the boundary instant are neither dropped nor counted twice; a page
/// that adds nothing new ends the walk rather than spinning on it.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn walk_messages(
    kernel: &Kernel,
    since: Option<chrono::DateTime<chrono::Utc>>,
    cap: u32,
    page: u32,
) -> Result<Vec<MessageRecord>> {
    let page = page.max(1);
    let mut out: Vec<MessageRecord> = Vec::new();
    // Keyed by the row's uuid, as every session map in this file is:
    // `RecordId` carries interior mutability clippy will not key on.
    let mut seen: HashSet<String> = HashSet::new();
    let mut before: Option<chrono::DateTime<chrono::Utc>> = None;
    // Rows asked for this round. A full page even near the cap: the
    // boundary rows the dedupe removes would otherwise eat the
    // remainder. Widened only when an instant holds more rows than a
    // page — the one case a `<=` cursor cannot step past on its own.
    let mut want = page;
    while (out.len() as u32) < cap {
        let query = match (before, since) {
            (None, None) => WALK_FIRST,
            (None, Some(_)) => WALK_FIRST_SINCE,
            (Some(_), None) => WALK_NEXT,
            (Some(_), Some(_)) => WALK_NEXT_SINCE,
        };
        let mut q = kernel.db().query(query).bind(("limit", want));
        if let Some(b) = before {
            q = q.bind(("before", b));
        }
        if let Some(s) = since {
            q = q.bind(("since", s));
        }
        let rows: Vec<MessageRecord> = q.await?.take(0)?;
        let fetched = rows.len() as u32;
        let mut added = 0u32;
        for row in rows {
            if (out.len() as u32) >= cap {
                break;
            }
            if seen.insert(superx_ops::record_uuid(&row.id)) {
                out.push(row);
                added += 1;
            }
        }
        if fetched < want {
            break; // the engine ran out of rows
        }
        if added == 0 {
            // Every row came back already held: the boundary instant
            // is wider than the page. Ask for more of it, up to the cap.
            if want >= cap {
                break;
            }
            want = want.saturating_mul(2).min(cap.max(page));
            continue;
        }
        want = page;
        before = out.last().map(|m| m.valid_from);
    }
    Ok(out)
}

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
/// Returns what THIS run contributed — passed, failed, compile errors
/// — so the caller can attribute it to the branch and the agent that
/// produced it (#350). The hour bucket is attributed here because it
/// has no other caller; branch and agent do.
fn score_output(text: &str, code: &mut CodeAgg, hour_key: &str) -> (i64, i64, i64) {
    let before = (code.tests_passed, code.tests_failed, code.compile_errors);
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
        // pytest / jest / vitest: "5 passed, 2 failed" — every count word
        // right after its number. A line that puts the word first, as a
        // script's own summary does (`passed 266 failed 0`), paired each
        // number with the next word: it read as 266 failed, and six such
        // lines lit the lamp with 1.6k failures in a day (#415 QA).
        if l.contains("passed") || l.contains("failed") {
            let words: Vec<&str> = l.split_whitespace().collect();
            let counts: Vec<(usize, &str)> = words
                .iter()
                .enumerate()
                .filter(|(_, w)| matches!(count_word(w), "passed" | "failed"))
                .map(|(i, w)| (i, count_word(w)))
                .collect();
            let number_before = |i: usize| i.checked_sub(1).and_then(|j| words[j].parse::<i64>().ok());
            if counts.iter().all(|(i, _)| number_before(*i).is_some()) {
                for (i, word) in counts {
                    let v = number_before(i).unwrap_or(0);
                    if word == "passed" {
                        code.tests_passed += v;
                    } else {
                        code.tests_failed += v;
                    }
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
    // Attribute this run's tallies to the hour it happened in, so
    // quality reads as a trend (#337).
    let slot = code.quality.entry(hour_key.to_string()).or_insert((0, 0, 0));
    slot.0 += code.tests_passed - before.0;
    slot.1 += code.tests_failed - before.1;
    (
        code.tests_passed - before.0,
        code.tests_failed - before.1,
        code.compile_errors - before.2,
    )
}

/// Judge one shell call against what it printed (#412): which gates it
/// passed, and what it shipped. A gate that ran and failed is not a gate
/// passed, and a merge that was refused is not a merge.
fn judge_shell(
    code: &mut CodeAgg,
    sid: &str,
    when: chrono::DateTime<chrono::Utc>,
    cmd: &str,
    out: &str,
    failed: bool,
) {
    {
        let events = code.gate_events.entry(sid.to_string()).or_default();
        let passed = |gate: Gate| gate_passed(gate, Some(out), gate_piped(cmd, gate));
        for label in command_labels(cmd) {
            match label.as_str() {
                "cargo test" if passed(Gate::Test) => {
                    events.push((when, GateEvent::Tested));
                }
                "cargo clippy" if passed(Gate::Lint) => {
                    events.push((when, GateEvent::Linted));
                }
                _ => {}
            }
        }
        if cmd.contains("skill_audit") && passed(Gate::Audit) {
            events.push((when, GateEvent::Audited));
        }
    }
    // The lines a commit's own output says it committed (#381) — read
    // here, where every shell call is judged once whichever of call and
    // output the walk met first.
    if commits(cmd) {
        if let Some((ins, del)) = commit_shortstat(out) {
            code.committed_added += ins;
            code.committed_removed += del;
            code.commits_with_stat += 1;
        }
    }
    // What the output said about shipping — a PR number, a commit hash —
    // for the live row (#381).
    let detail_text = shipping_detail(out);
    for (ship, num) in shipping(cmd) {
        if !shipped(ship, out, cmd, failed) {
            continue;
        }
        let detail = |wanted_pr: bool| {
            detail_text
                .as_deref()
                .filter(|d| d.starts_with('#') == wanted_pr)
                .map(|d| format!(" {d}"))
                .unwrap_or_default()
        };
        let text = match ship {
            Ship::Commit => {
                code.commits += 1;
                format!("commit{}", detail(false))
            }
            Ship::Push => {
                code.pushes += 1;
                "pushed".to_string()
            }
            Ship::PrOpened => {
                code.prs_opened += 1;
                code.gate_events
                    .entry(sid.to_string())
                    .or_default()
                    .push((when, GateEvent::OpenedPr));
                format!("PR{} opened", detail(true))
            }
            Ship::PrMerged => {
                code.prs_merged += 1;
                format!("PR{} merged", num.as_deref().map(|n| format!(" {n}")).unwrap_or_default())
            }
        };
        // Newest-first: the first shipping event met is the session's
        // latest.
        let l = code.live.entry(sid.to_string()).or_default();
        if l.shipped.is_none() {
            l.shipped = Some(text);
            l.shipped_at = Some(when);
        }
    }
}

/// Hang one command's outcome on the branch and the agent that ran it
/// (#350). Both are the point: a branch cannot be ranked without an
/// outcome, and an agent compared on volume alone is not compared.
fn attribute_quality(
    code: &mut CodeAgg,
    branch_pair: &Option<(String, String)>,
    agent_name: &Option<String>,
    effort: &Option<String>,
    me_key: &Option<(String, String)>,
    (passed, failed, errors): (i64, i64, i64),
) {
    if passed == 0 && failed == 0 && errors == 0 {
        return;
    }
    if let Some(key) = branch_pair {
        let b = code.branches.entry(key.clone()).or_default();
        b.tests_passed += passed;
        b.tests_failed += failed;
        b.compile_errors += errors;
    }
    if let Some(an) = agent_name {
        let a = code.agents.entry(an.clone()).or_default();
        a.tests_passed += passed;
        a.tests_failed += failed;
        a.compile_errors += errors;
    }
    // `EffortStat` has carried these fields all along and nothing ever
    // incremented them, so the API reported that every reasoning level
    // had run zero tests. This is the third consumer of the same
    // deltas (#354 review).
    if let Some(e) = effort {
        let ea = code.efforts.entry(e.clone()).or_default();
        ea.tests_passed += passed;
        ea.tests_failed += failed;
    }
    // The pair the operator actually switches (#391).
    if let Some(k) = me_key {
        let me = code.model_effort.entry(k.clone()).or_default();
        me.tests_passed += passed;
        me.tests_failed += failed;
    }
}

/// A write into territory a module lane must never touch (#392): the
/// kernel's own crate, or a schema file. The rule is absolute, and the
/// path is already extracted for every write (#374, #382).
fn note_bright_line(code: &mut CodeAgg, path: &str) {
    if path.contains("/crates/superx-kernel/") || path.ends_with(".surql") {
        *code.bright_line.entry(path.to_string()).or_insert(0) += 1;
    }
}

/// One in-engine `count() GROUP ALL` over a table.
async fn count_rows(kernel: &Kernel, query: &'static str) -> Result<i64> {
    let rows: Vec<Value> = kernel.db().query(query).await?.take(0)?;
    Ok(rows.first().and_then(|r| obj(r).map(|o| get_int(o, "c"))).unwrap_or(0))
}

/// Output tokens, one count per reply (#409), over every row or over the
/// rows emitted after `since` on the agent's clock.
///
/// A reply's rows share a key — Claude Code's `message.id`, Gemini's
/// record `id` — and each carries the reply's whole usage, so the engine
/// takes one value per key before it sums. Summing the rows counted a
/// Claude reply two to four times over. A row with neither key is its
/// own reply.
async fn reply_output_tokens(
    kernel: &Kernel,
    since: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<i64> {
    let window = if since.is_some() {
        "AND valid_from > $cut AND (emitted_at ?? valid_from) > $cut"
    } else {
        ""
    };
    let query = format!(
        "SELECT math::sum(o) AS c FROM (\
             SELECT {REPLY_KEY_SQL} AS k, math::max({OUT_TOKENS_SQL}) AS o FROM message \
             WHERE (raw.message.usage != NONE OR raw.tokens != NONE) {window} \
             GROUP BY k\
         ) GROUP ALL"
    );
    let mut q = kernel.db().query(query);
    if let Some(cut) = since {
        q = q.bind(("cut", cut));
    }
    let rows: Vec<Value> = q.await?.take(0)?;
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
    stats_for_range_capped(kernel, window, range, RANGE_ROW_CAP, chrono::Offset::fix(&chrono::Utc)).await
}

/// [`stats_for_range`] on the viewer's `clock` (#415 review): every hour
/// and day a chart buckets by is the viewer's, so the hour-of-day chart
/// and the Sortie log agree about when a session ran. Instants, ages and
/// range membership are the same on every clock.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn stats_for_range_on(
    kernel: &Kernel,
    window: u32,
    range: &str,
    clock: chrono::FixedOffset,
) -> Result<StatsSummary> {
    stats_for_range_capped(kernel, window, range, RANGE_ROW_CAP, clock).await
}

/// [`stats_for_range`] with the time-bounded ranges' row cap as a
/// parameter — the production cap is far more rows than a test wants
/// to write, and the cap's one observable effect (`truncated`) is
/// worth asserting (#372).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn stats_for_range_capped(
    kernel: &Kernel,
    window: u32,
    range: &str,
    row_cap: u32,
    clock: chrono::FixedOffset,
) -> Result<StatsSummary> {
    // ── cheap in-engine totals ──────────────────────────────────────
    let events_total =
        count_rows(kernel, "SELECT count() AS c FROM telemetry_stream GROUP ALL").await?;
    let messages_total = count_rows(kernel, "SELECT count() AS c FROM message GROUP ALL").await?;
    let output_tokens_total = reply_output_tokens(kernel, None).await?;

    // Active sessions: distinct sessions with a message inside the
    // activity threshold (parameter on the ui entity; default matches
    // the Sessions page's liveness window) — on the agent's clock, as the
    // live list is. `valid_from` stays as the cheap indexed bound: a row
    // is never captured before it was written (#413).
    let active_secs = resolved_active_secs(kernel).await;
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(active_secs);
    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT session FROM message \
             WHERE valid_from > $cutoff AND (emitted_at ?? valid_from) > $cutoff \
             GROUP BY session",
        )
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
    // uuid → the agent that owns the session (#337).
    let mut agent_of: HashMap<String, String> = HashMap::new();
    for s in &sessions {
        let name = match &s.payload {
            Value::Object(o) => get_str(o, "name").unwrap_or("?").to_string(),
            _ => "?".to_string(),
        };
        let agent = name.split('/').next().unwrap_or("?").to_string();
        let uuid = superx_ops::record_uuid(&s.entity_id);
        identity.insert(uuid.clone(), format!("{agent}/{}", &uuid[..uuid.len().min(8)]));
        agent_of.insert(uuid, agent);
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
    let cap = if range == "window" { window } else { row_cap };
    let since = cutoff.map(|d| chrono::Utc::now() - d);
    let mut msgs = walk_messages(kernel, since, cap, RANGE_PAGE).await?;
    // The altitude gauge's ceiling (#367), resolved once per request.
    let context_window = crate::resolved_context_window(kernel).await;
    // Truncation is judged on the walk itself, before the clock filter
    // below: a capped walk is a sample whatever the filter keeps. The
    // fixed window is BY DEFINITION the newest N and never a sample
    // (review of #330); every other range — `all` included, which has
    // no cutoff but has the cap — is one the moment the cap cut it
    // short. `all` used to say "every row in range" over a 20,000-row
    // sample of 39,000 (#372).
    let truncated = range != "window" && msgs.len() as u32 >= cap;
    // Range MEMBERSHIP follows the agent's clock, as every hour-graded
    // figure has since #340. The walk's `valid_from > since` stays as
    // the cheap bound — a row cannot be captured before it was emitted,
    // so nothing in range lies outside it — and here the rows whose
    // work happened before the range are dropped. Without this, a
    // restart that backfills six days of transcripts puts all six
    // inside "last hour": `1h` and `24h` returned byte-identical
    // payloads while clock coverage said 1/24 (#372).
    if let Some(s) = since {
        msgs.retain(|m| m.emitted_at.unwrap_or(m.valid_from) > s);
    }

    // Every working directory the range was written in, resolved to its
    // repository and checkout once (#411).
    let range_cwds: HashSet<String> = msgs
        .iter()
        .filter_map(|m| m.raw.as_ref().and_then(|r| get_str(r, "cwd")).map(str::to_string))
        .collect();
    let checkouts = crate::checkout::Checkouts::resolve(range_cwds.iter()).await;

    // Churn has two very different causes (operator insight, #337):
    // the agent rewriting its own work, or the design moving under it.
    // A rewrite that FOLLOWS a human turn is directed; one with nobody
    // steering is the agent going in circles. Collect the human turns
    // per session first, so each write can ask "was anyone steering?"
    let mut human_turns: HashMap<String, Vec<chrono::DateTime<chrono::Utc>>> = HashMap::new();
    for m in &msgs {
        if m.role == "user" {
            human_turns
                .entry(superx_ops::record_uuid(&m.session))
                .or_default()
                .push(m.emitted_at.unwrap_or(m.valid_from));
        }
    }
    for v in human_turns.values_mut() {
        v.sort_unstable();
    }
    let mut tools: HashMap<String, i64> = HashMap::new();
    // 7d and longer would otherwise produce a cell per agent, per
    // repo, per HOUR — thousands of rows nobody can read (#340).
    let fold_days = matches!(range, "7d" | "30d" | "all");
    let mut code = CodeAgg::default();
    let mut lines_written = 0i64;
    let mut per_session: HashMap<String, SessAgg> = HashMap::new();
    let mut outcomes: HashMap<String, Outcome> = HashMap::new();
    // tool_use_id → where its output came from (the path it read, else the
    // call), and the ids whose output already looked like a credential
    // before the call went by (#337).
    let mut call_paths: HashMap<String, String> = HashMap::new();
    let mut secret_pending: HashSet<String> = HashSet::new();
    // tool_use_id → (tool name, model, repo). A `tool_result` message
    // carries NO model of its own, so a failure attributed from the
    // result would land on `unknown`; it belongs to whoever made the
    // call (#328).
    // Tool name, and the model / repo / BRANCH the CALL was made on —
    // not the result message's, which may sit elsewhere. The branch was
    // missing, so failures resolved through this arm never reached a
    // branch row while `tool_calls` still counted them (#354 review).
    type CallCtx = (String, Option<String>, Option<String>, Option<(String, String)>);
    let mut call_names: HashMap<String, CallCtx> = HashMap::new();
    // Results seen before their call (the walk is newest-first).
    let mut pending_results: HashMap<String, bool> = HashMap::new();
    // Output text held until the call names the tool that produced it,
    // and whether the call failed.
    let mut pending_output: HashMap<String, (String, bool)> = HashMap::new();
    // Shell calls seen before their output — the reverse order, which
    // happens with interleaved sidechains. Without this the text is
    // stashed forever and silently dropped.
    let mut shell_calls: HashSet<String> = HashSet::new();
    // tool_use_id → what the call changed, read off its result (#410).
    let mut pending_diffs: HashMap<String, RecordedDiff> = HashMap::new();
    // Calls that were refused and never ran (#412).
    let mut denied_calls: HashSet<String> = HashSet::new();
    // Shell calls met before their output: (session, when, command), to be
    // judged once the output arrives (#412).
    let mut pending_shell: HashMap<String, (String, chrono::DateTime<chrono::Utc>, String)> =
        HashMap::new();
    // Replies already met (#409). A reply's usage rides every line Claude
    // Code writes for it, and a Gemini record is re-emitted, fuller each
    // time, as it streams. The walk is newest-first, so the first row met
    // for a reply is its last line, or its fullest emission.
    let mut seen_replies: HashSet<String> = HashSet::new();
    for m in &msgs {
        let reply = m.raw.as_ref().and_then(reply_key);
        let fresh_reply = reply.as_ref().is_none_or(|k| seen_replies.insert(k.clone()));
        // An older, shorter emission of a Gemini record already read in
        // full. A Claude line is never skipped: each carries its own block.
        if !fresh_reply && m.raw.as_ref().is_some_and(|r| r.get("message").is_none()) {
            continue;
        }
        let sid = superx_ops::record_uuid(&m.session);
        // A message is a reply, once, whatever number of lines Claude Code
        // wrote for it — every "Msgs" on the page counts the same thing
        // (#409, #415 review). A row that is no reply is its own message.
        let one = i64::from(fresh_reply);
        let agg = per_session.entry(sid).or_default();
        agg.messages += one;
        // Shape of the working day (#324): when messages landed, and
        // how many sessions were live at once.
        // The AGENT'S clock, not ours. `valid_from` is when
        // SuperX captured the line; during a backfill that is
        // one narrow window for months of work, which would
        // pile every hour-graded figure into the hour of the
        // backfill. `emitted_at` is when the work happened
        // (insights.rs already reads it this way) (#340).
        let when = m.emitted_at.unwrap_or(m.valid_from);
        // The same moment on the viewer's clock, for every hour and day a
        // chart buckets by.
        let local = when.with_timezone(&clock);
        code.instants.push(when);
        // Hour AND minute from the agent's clock: the minute used to come
        // from the capture clock, so a backfill folded a whole hour of
        // sessions into one five-minute bucket (#413).
        let minute = chrono::Timelike::minute(&when);
        let bucket5 = format!("{}-{}", when.format("%Y-%m-%dT%H"), minute / 5);
        code.concurrency
            .entry(bucket5)
            .or_default()
            .insert(superx_ops::record_uuid(&m.session));
        // An hour counts only if it began inside the range. The range's
        // first clock hour began before it did, and counting that sliver
        // made a 1h range read "2 active hours" and the 24h range one more
        // than the coverage strip, which counts whole clock hours (#415 QA).
        let into_hour = i64::from(chrono::Timelike::minute(&local)) * 60 + i64::from(chrono::Timelike::second(&local));
        let hour_began = when
            - chrono::Duration::seconds(into_hour)
            - chrono::Duration::nanoseconds(i64::from(when.timestamp_subsec_nanos()));
        if since.is_none_or(|s| hour_began >= s) {
            code.active_hours.insert(local.format("%Y-%m-%dT%H").to_string());
        }

        let Some(raw) = &m.raw else { continue };

        // Reasoning level, where the agent reports it (#337): does
        // thinking harder produce keepable code, or just cost more?
        // `get_str` hands back `Some("")` for an empty JSON string, and
        // a first-sighting latch would take that as the answer and mask
        // the real effort on an older message — the same trap `branch`
        // sidesteps below (#344 review).
        let effort = get_str(raw, "effort").filter(|e| !e.is_empty()).map(str::to_string);
        // A reply, not a line: one reply is several lines, and more of
        // them for a model that thinks and calls tools in one go (#409).
        if let Some(e) = effort.as_ref().filter(|_| fresh_reply) {
            code.efforts.entry(e.clone()).or_default().messages += 1;
        }
        let hour_key = local.format("%Y-%m-%dT%H").to_string();
        // On `when`, not `valid_from`: the failure-by-hour-of-day series
        // (#328, first drawn in #369) put every backfilled call into the
        // hour of the backfill — one bar at 09 for six days of work
        // (#372).
        let hour_of_day = i64::from(chrono::Timelike::hour(&local));
        // Was anyone steering when this message happened? Hoisted from
        // the write path so the token accounting can ask it too (#391):
        // the same ten-minute window, one computation per message.
        let steered = human_turns
            .get(&superx_ops::record_uuid(&m.session))
            .is_some_and(|turns| {
                let cut = when - chrono::Duration::minutes(STEERING_MINUTES);
                turns.iter().rev().skip_while(|h| **h > when).take(1).any(|h| *h >= cut)
            });

        // The 24×7 picture (#337): every session's span in the range.
        {
            let sid = superx_ops::record_uuid(&m.session);
            let entry = code.spans.entry(sid).or_insert((
                when,
                when,
                superx_ops::record_uuid(&m.agent),
                None,
                0,
            ));
            if when < entry.0 {
                entry.0 = when;
            }
            if when > entry.1 {
                entry.1 = when;
            }
            entry.4 += one;
        }

        // Live state (#325): newest-first, so the first sighting of a
        // session carries its freshest facts.
        {
            let sid = superx_ops::record_uuid(&m.session);
            let l = code.live.entry(sid).or_default();
            l.messages += one;
            if l.newest.is_none() {
                l.newest = Some(when);
                l.agent = superx_ops::record_uuid(&m.agent);
            }
        }

        // What the agents WAITED on (#337). durationMs sits on the
        // system lines that close a long operation; the median in a
        // real transcript is ~100s, so this is real wall-clock.
        if let Some(Value::Number(n)) = raw.get("durationMs") {
            if let Some(ms) = n.to_int().filter(|&v| v > 0) {
                code.waits.push(ms);
                // The newest long operation this session reported.
                // Elapsed time of something FINISHED — context, not a
                // claim that it is still running (#354 review).
                {
                    let l = code.live.entry(superx_ops::record_uuid(&m.session)).or_default();
                    if l.last_op_ms == 0 {
                        l.last_op_ms = ms;
                    }
                }
                if code.slowest.len() < 400 {
                    code.slowest.push(SlowOp {
                        label: get_str(raw, "slug")
                            .or_else(|| get_str(raw, "type"))
                            .unwrap_or("operation")
                            .to_string(),
                        ms,
                        at: when.to_rfc3339(),
                    });
                }
            }
        }
        // Commands that were stopped before finishing.
        if let Some(Value::Object(tur)) = raw.get("toolUseResult") {
            if matches!(tur.get("interrupted"), Some(Value::Bool(true))) {
                code.interrupted += 1;
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
        // not attributed. That holds for every figure, not only the
        // message count: lines, tokens, failures and reverts on a
        // model-less message go to no row at all (#345).
        // `<synthetic>` is Claude Code's marker for a line the runtime
        // wrote itself; it sat in the model comparison and on the live
        // panel as if something had been prompted (#367). Not a model.
        // Gemini names its model on the record itself (#409).
        let model_opt = raw
            .get("message")
            .and_then(obj)
            .and_then(|m| get_str(m, "model"))
            .or_else(|| get_str(raw, "model"))
            .filter(|m| !m.is_empty() && !m.starts_with('<'))
            .map(str::to_string);
        if let Some(known) = model_opt.as_ref().filter(|_| fresh_reply) {
            code.models.entry(known.clone()).or_default().messages += 1;
        }
        // How many fronts were open in this bucket (#395). The repo
        // half joins below, once the working directory is known.
        code.intensity
            .entry(hour_key.clone())
            .or_default()
            .sessions
            .insert(superx_ops::record_uuid(&m.session));
        // Is this session waiting on YOU (#381 D)? The walk is
        // newest-first, so the first message met for a session is its
        // newest. An assistant turn that called no tool is the agent
        // having stopped and said something — a question, or a report —
        // and nothing has happened since.
        {
            let sid = superx_ops::record_uuid(&m.session);
            if code.seen_sessions.insert(sid.clone()) {
                let called_a_tool = raw
                    .get("message")
                    .and_then(obj)
                    .and_then(|msg| msg.get("content"))
                    .and_then(|c| match c {
                        Value::Array(a) => Some(a),
                        _ => None,
                    })
                    .is_some_and(|blocks| {
                        blocks
                            .iter()
                            .filter_map(obj)
                            .any(|b| get_str(b, "type") == Some("tool_use"))
                    });
                if m.role == "assistant" && !called_a_tool {
                    code.live.entry(sid).or_default().awaiting = true;
                }
            }
        }
        // Model and reasoning level ride the same messages, so the pair
        // is a key (#391).
        let me_key = model_opt.clone().zip(effort.clone());
        let sid_here = superx_ops::record_uuid(&m.session);
        if let Some(k) = &me_key {
            // Refusals and interruptions met since the last pair belong
            // to THIS one: the walk is newest-first, so the next older
            // reply is the one whose call was refused or stopped. Handing
            // them to the session's newest pair gave a model switched in
            // later every step taken before it (#413).
            let held = code.pending_steps.remove(&sid_here);
            let me = code.model_effort.entry(k.clone()).or_default();
            if fresh_reply {
                me.messages += 1;
            }
            me.sessions.insert(sid_here.clone());
            if let Some((d, i)) = held {
                me.denials += d;
                me.interventions += i;
            }
        }

        {
            let sid = superx_ops::record_uuid(&m.session);
            let l = code.live.entry(sid).or_default();
            if l.model.is_none() {
                l.model = model_opt.clone();
            }
            // Effort is switched mid-session, and it rides different
            // messages than the model does — so it is picked up on its
            // own first sighting, not alongside the model (#343).
            if l.effort.is_none() {
                l.effort = effort.clone();
            }
        }
        // The cube's bucket: hourly on short ranges, daily on long
        // ones, folded HERE so the payload stays bounded however many
        // agent/repo pairs exist (#340).
        let bucket = if fold_days {
            local.format("%Y-%m-%d").to_string()
        } else {
            local.format("%Y-%m-%dT%H").to_string()
        };
        // The repository this row was written in, and the branch that
        // checkout was on at the time — both read from git (#411). The
        // last segment of `cwd` named a subdirectory as often as a
        // repository, and `gitBranch` is the branch of the directory the
        // session was LAUNCHED in, whatever checkout it worked in since.
        let cwd = get_str(raw, "cwd");
        let repo_key = cwd.and_then(|c| checkouts.of(c)).map(|c| c.repo.clone());
        let branch_key = cwd.and_then(|c| checkouts.branch_at(c, when));
        // Being stopped or refused belongs to whatever the session was
        // running, not to the message that carries the flag — that one
        // names no model (#391). It is held for the next older reply
        // that names a pair, which is the one that was stopped (#413).
        if get_str(raw, "toolDenialKind").is_some()
            || raw.get("interruptedMessageId").is_some()
            || raw.get("userFeedback").is_some()
        {
            let denied = get_str(raw, "toolDenialKind").is_some();
            let e = code.pending_steps.entry(sid_here.clone()).or_insert((0, 0));
            if denied {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
        // Per-agent productivity (#337). Sessions are `agent/uuid`,
        // so the owning agent is resolved through the session.
        let sid_now = superx_ops::record_uuid(&m.session);
        let agent_name = agent_of.get(&sid_now).cloned();
        if let Some(an) = &agent_name {
            let a = code.agents.entry(an.clone()).or_default();
            a.messages += one;
            a.sessions.insert(sid_now.clone());
        }
        // Which repo the agent was standing in (#308, #325), and which
        // branch of it (#350).
        if let Some(b) = &branch_key {
            code.session_branches
                .entry(superx_ops::record_uuid(&m.session))
                .or_default()
                .insert(b.clone());
        }
        if let Some(rk) = &repo_key {
            code.intensity.entry(hour_key.clone()).or_default().repos.insert(rk.clone());
            code.session_repos
                .entry(superx_ops::record_uuid(&m.session))
                .or_default()
                .insert(rk.clone());
        }
        if let Some(c) = cwd {
            code.cwds.insert(c.to_string());
        }
        // The (repo, branch) pair, present only when both are known —
        // a write with no branch belongs to no branch row rather than
        // to a guessed one.
        let branch_pair: Option<(String, String)> = repo_key
            .as_ref()
            .zip(branch_key.as_ref())
            .map(|(r, b)| (r.clone(), b.clone()));
        if let Some(rk) = &repo_key {
            let sid = superx_ops::record_uuid(&m.session);
            let l = code.live.entry(sid.clone()).or_default();
            if l.repo.is_none() {
                l.repo = Some(rk.clone());
                l.branch = branch_key.clone();
            }
            // The span's repo was declared and never filled, so the
            // sortie log read a dash on every row (#367). Newest-first:
            // the first sighting is where the session is now.
            if let Some(span) = code.spans.get_mut(&sid) {
                if span.3.is_none() {
                    span.3 = Some(rk.clone());
                }
            }
            if let Some(an) = &agent_name {
                code.agents.entry(an.clone()).or_default().repos.insert(rk.clone());
                code.cells
                    .entry((an.clone(), rk.clone(), bucket.clone()))
                    .or_default()
                    .messages += one;
            }
            // Crossing repos mid-session. The walk is newest-first, so
            // this counts the same boundaries from the other side —
            // a crossing is a crossing in either direction (#340).
            let sid = superx_ops::record_uuid(&m.session);
            match code.last_repo.get(&sid) {
                Some(prev) if prev != rk => {
                    code.repo_switches += 1;
                    if let Some(an) = &agent_name {
                        code.agents.entry(an.clone()).or_default().repo_switches += 1;
                    }
                    code.last_repo.insert(sid, rk.clone());
                }
                None => {
                    code.last_repo.insert(sid, rk.clone());
                }
                _ => {}
            }
            let r = code.repos.entry(rk.clone()).or_default();
            r.messages += one;
            r.agents.insert(superx_ops::record_uuid(&m.agent));
            if r.last_active.is_none_or(|prev| when > prev) {
                r.last_active = Some(when);
            }
            if let Some(b) = &branch_key {
                r.branch.get_or_insert_with(|| b.clone());
            }
            // The branch dimension (#350). `repos` keeps the newest
            // branch as a LABEL; this keys on it, so two branches in
            // one repo stop summing into one row.
            if let Some(bk) = &branch_key {
                let key = (rk.clone(), bk.clone());
                // Newest-first, so the first sighting is the branch the
                // session is on now.
                code.session_branch.entry(sid_now.clone()).or_insert_with(|| key.clone());
                let b = code.branches.entry(key).or_default();
                b.messages += one;
                b.sessions.insert(sid_now.clone());
                b.agents.insert(superx_ops::record_uuid(&m.agent));
                if b.last_active.is_none_or(|prev| when > prev) {
                    b.last_active = Some(when);
                }
            }
        }
        // Projects are repositories (#411): a directory outside any is
        // not one, and a subdirectory is its repository.
        if let Some(project) = &repo_key {
            if let Some(branch) = &branch_key {
                code.project_branch
                    .entry(project.clone())
                    .or_insert_with(|| branch.clone());
            }
            *code.projects.entry(project.clone()).or_insert(0) += 1;
        }
        // Compaction is dead time (#340): the agent stops, re-reads
        // its own history and resumes with less of it. The transcript
        // records the trigger, the duration and how much context was
        // in play — `compact_boundary` lines are captured whole.
        if let Some(Value::Object(cm)) = raw.get("compactMetadata") {
            let sid = superx_ops::record_uuid(&m.session);
            let c = code.compact_by_session.entry(sid.clone()).or_default();
            if c.agent.is_empty() {
                c.agent = agent_name.clone().unwrap_or_else(|| "?".to_string());
            }
            if c.repo.is_none() {
                c.repo = repo_key.clone();
            }
            c.durations.push(get_int(cm, "durationMs"));
            c.pre_tokens_max = c.pre_tokens_max.max(get_int(cm, "preTokens"));
            match get_str(cm, "trigger") {
                Some("manual") => c.manual += 1,
                _ => c.auto += 1,
            }
            if let Some(an) = &agent_name {
                let a = code.agents.entry(an.clone()).or_default();
                a.compactions += 1;
                a.compaction_ms += get_int(cm, "durationMs");
            }
        }
        // What this reply spent — once per reply, whichever agent wrote
        // it (#409). The walk used to add the usage of every LINE, and a
        // reply is two to four lines in Claude Code's transcript.
        if let Some(u) = reply_usage(raw).filter(|_| fresh_reply) {
            let out = u.out;
            {
                let l = code.live.entry(superx_ops::record_uuid(&m.session)).or_default();
                l.out_tokens += out;
                // Per session (#350): idle-because-reasoning reads
                // differently from idle-because-blocked.
                l.thinking_tokens += u.thinking;
                // Context pressure (#367): the prompt behind the newest
                // reply — first sighting wins, the walk being newest-first.
                if l.context_tokens.is_none() && u.context > 0 {
                    l.context_tokens = Some(u.context);
                }
            }
            agg.out_tokens += out;
            code.out_tokens += out;
            code.thinking += u.thinking;
            // Spent with nobody steering (#391).
            if !steered {
                code.unattended_out += out;
            }
            if let Some(k) = &me_key {
                let me = code.model_effort.entry(k.clone()).or_default();
                me.out_tokens += out;
                me.thinking_tokens += u.thinking;
            }
            if let Some(an) = &agent_name {
                let a = code.agents.entry(an.clone()).or_default();
                a.out_tokens += out;
                a.in_tokens += u.input + u.cache_write;
            }
            if let Some(known) = &model_opt {
                code.models.entry(known.clone()).or_default().out_tokens += out;
            }
            if let Some(e) = &effort {
                let ea = code.efforts.entry(e.clone()).or_default();
                ea.out_tokens += out;
                ea.thinking_tokens += u.thinking;
            }
            if let Some(rk) = &repo_key {
                code.repos.entry(rk.clone()).or_default().out_tokens += out;
                if let Some(an) = &agent_name {
                    code.cells
                        .entry((an.clone(), rk.clone(), bucket.clone()))
                        .or_default()
                        .out_tokens += out;
                }
            }
            if let Some(key) = &branch_pair {
                code.branches.entry(key.clone()).or_default().out_tokens += out;
            }
            // WHEN the money went (#391), in the same buckets the churn
            // chart uses. What left this machine (#337): `input` is the
            // prompt sent fresh this turn; cache CREATION is what the
            // vendor wrote to its own store to reuse; cache READS are
            // that content served back. Together, the transmitted volume.
            code.intensity.entry(hour_key.clone()).or_default().out_tokens += out;
            let b = code.burn.entry(hour_key.clone()).or_insert((0, 0, 0, 0));
            b.0 += out;
            b.1 += u.thinking;
            b.2 += u.input + u.cache_write;
            b.3 += u.cache_read;
            code.in_tokens += u.input;
            code.cache_write += u.cache_write;
            code.cache_read += u.cache_read;
        }
        // Claude-style blocks: raw.message.content[].
        if let Some(Value::Object(msg)) = raw.get("message") {
            if let Some(Value::Array(blocks)) = msg.get("content") {
                let results_in_row = blocks
                    .iter()
                    .filter_map(obj)
                    .filter(|b| get_str(b, "type") == Some("tool_result"))
                    .count();
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
                                if l.doing_reply.is_none() {
                                    l.doing_reply = Some(reply.clone().unwrap_or_default());
                                }
                                claim_doing(l, reply.as_deref(), "working");
                            }
                            let entry = outcomes.entry(name.clone()).or_default();
                            entry.calls += 1;
                            // Denominator for failures-per-100 (#350).
                            if let Some(key) = &branch_pair {
                                code.branches.entry(key.clone()).or_default().tool_calls += 1;
                            }
                            // The result may already have gone by.
                            if let Some(id) = get_str(block, "id") {
                                match pending_results.remove(id) {
                                    Some(true) => {
                                        entry.failed += 1;
                                        code.by_hour.entry(hour_of_day).or_insert((0, 0)).1 += 1;
                                        code.quality
                                            .entry(hour_key.clone())
                                            .or_insert((0, 0, 0))
                                            .2 += 1;
                                        if let Some(known) = &model_opt {
                                            code.models.entry(known.clone()).or_default().tool_failures += 1;
                                        }
                                        if let Some(e) = &effort {
                                            code.efforts.entry(e.clone()).or_default().tool_failures += 1;
                                        }
                                        if let Some(k) = &me_key {
                                            code.model_effort.entry(k.clone()).or_default().tool_failures += 1;
                                        }
                                        if let Some(key) = &branch_pair {
                                            code.branches
                                                .entry(key.clone())
                                                .or_default()
                                                .tool_failures += 1;
                                        }
                                        if let Some(rk) = &repo_key {
                                            code.repos.entry(rk.clone()).or_default().tool_failures += 1;
                                        }
                                        if let Some(an) = &agent_name {
                                            code.agents.entry(an.clone()).or_default().tool_failures += 1;
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
                                            (
                                                name.clone(),
                                                model_opt.clone(),
                                                repo_key.clone(),
                                                branch_pair.clone(),
                                            ),
                                        );
                                    }
                                }
                            }
                            // Now the tool is known: score its output
                            // if — and only if — it was a shell call.
                            // What the shell printed, kept for the gates and
                            // the shipping events, which count only what the
                            // output says happened (#412).
                            let mut shell_text: Option<(String, bool)> = None;
                            // A refused call never ran: it wrote nothing,
                            // shipped nothing and verified nothing (#412).
                            let refused = get_str(block, "id").is_some_and(|id| denied_calls.contains(id));
                            if let Some(id) = get_str(block, "id") {
                                match pending_output.remove(id) {
                                    Some((text, failed)) if SHELL_TOOLS.contains(&name.as_str()) => {
                                        shell_text = Some((text.clone(), failed));
                                        let d = score_output(&text, &mut code, &hour_key);
                                        attribute_quality(
                                            &mut code, &branch_pair, &agent_name, &effort, &me_key,
                                            d,
                                        );
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
                            // A verification closes the edit→verify
                            // pair (#340): did the agent check its
                            // work, and how long did it wait? A refused
                            // run checked nothing (#415 review).
                            if SHELL_TOOLS.contains(&name.as_str()) && !refused {
                                if let Some(Value::Object(input)) = block.get("input") {
                                    if let Some(cmd) = get_str(input, "command") {
                                        let verifies = command_labels(cmd).iter().any(|l| {
                                            let (test, build, _) = classify_command(l);
                                            test || build
                                        });
                                        if verifies {
                                            code.verify_events
                                                .entry(superx_ops::record_uuid(&m.session))
                                                .or_default()
                                                .push((when, false));
                                            // A test or build run is
                                            // the session VERIFYING,
                                            // which `Bash` alone cannot
                                            // say (#350).
                                            claim_doing(
                                                code.live
                                                    .entry(superx_ops::record_uuid(&m.session))
                                                    .or_default(),
                                                reply.as_deref(),
                                                "verifying",
                                            );
                                        }
                                    }
                                }
                            }
                            // When does work go wrong (#337)?
                            code.by_hour.entry(hour_of_day).or_insert((0, 0)).0 += 1;
                            if let Some(k) = &me_key {
                                code.model_effort.entry(k.clone()).or_default().tool_calls += 1;
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
                            // A credential the agent RAN or WROTE left the
                            // machine as surely as one it read (#413): one went
                            // by in a shell command, and only tool output was
                            // ever scanned.
                            if let Some(input) = block.get("input").and_then(obj) {
                                if ["command", "content", "new_string"]
                                    .iter()
                                    .any(|f| get_str(input, f).is_some_and(looks_like_secret))
                                {
                                    code.secret_hits += 1;
                                    code.secret_paths.insert(match touched_path(input) {
                                        Some(p) => p.to_string(),
                                        None => format!(
                                            "{name} input in {}",
                                            repo_key.as_deref().or(cwd).unwrap_or("an unknown directory")
                                        ),
                                    });
                                }
                            }
                            if READ_TOOLS.contains(&name.as_str()) {
                                code.reads += 1;
                            }
                            // Exposure (#337): a read puts file text
                            // into the next prompt, so the path is
                            // what left the machine. A path outside
                            // the directory the agent was working in
                            // is exposure nobody asked for.
                            if let Some(Value::Object(input)) = block.get("input") {
                                if let Some(path) = touched_path(input) {
                                    if READ_TOOLS.contains(&name.as_str()) {
                                        code.files_read.insert(path.to_string());
                                        {
                                            let l = code
                                                .live
                                                .entry(superx_ops::record_uuid(&m.session))
                                                .or_default();
                                            claim_doing(l, reply.as_deref(), "reading");
                                            if l.files_now.len() < LIVE_FILES
                                                && !l.files_now.iter().any(|x| x == path)
                                            {
                                                l.files_now.push(path.to_string());
                                            }
                                        }
                                        if let Some(rk) = &repo_key {
                                            code.repos_exposed.insert(rk.clone());
                                        }
                                        if reads_outside(path, cwd, &checkouts) {
                                            code.outside_reads += 1;
                                        }
                                    }
                                }
                                // Where a credential in this call's output
                                // came from: the file it read, else the call
                                // itself — a shell's output named nothing, so
                                // its hits went on the page without a place
                                // (#415 review).
                                if let Some(id) = get_str(block, "id") {
                                    let place = touched_path(input).map_or_else(
                                        || {
                                            format!(
                                                "{name} output in {}",
                                                repo_key.as_deref().or(cwd).unwrap_or("an unknown directory")
                                            )
                                        },
                                        str::to_string,
                                    );
                                    if secret_pending.remove(id) {
                                        code.secret_paths.insert(place);
                                    } else {
                                        call_paths.insert(id.to_string(), place);
                                    }
                                }
                            }
                            if let Some(Value::Object(input)) = block.get("input") {
                                // What the call CHANGED (#410): the diff Claude
                                // Code recorded on its result, when the result is
                                // in the walk. An Edit's strings carry the unchanged
                                // lines around the change — removed read 2.14x the
                                // real figure — and a Write over a file carries all
                                // of it. A refused or failed call changed nothing.
                                let recorded =
                                    get_str(block, "id").and_then(|id| pending_diffs.remove(id));
                                let applied = recorded.as_ref().is_none_or(|d| !d.failed);
                                // Only a change to the work counts as file work
                                // (#412): a read is not a write, and the agent's
                                // scratchpad is not the product.
                                // Derived once: every instrument below reads
                                // this, so none can drop the filter (#415
                                // review).
                                let work_path = touched_path(input).filter(|p| {
                                    WRITE_TOOLS.contains(&name.as_str()) && applied && is_work_path(p, &checkouts)
                                });
                                let work = work_path.is_some();
                                let (n, replaced) = match (&recorded, work) {
                                    (_, false) => (0, 0),
                                    (Some(d), true) => (d.added, d.removed),
                                    (None, true) => (block_lines(&name, input), replaced_lines(&name, input)),
                                };
                                // A notebook cell replaced or deleted had a
                                // prior text the call never carried — a write
                                // of unknown replaced size (#346, #383).
                                let notebook_rewrite = work
                                    && name == "NotebookEdit"
                                    && get_str(input, "edit_mode") != Some("insert");
                                if work {
                                    code.writes += 1;
                                }
                                if notebook_rewrite {
                                    code.replaced_unknown += 1;
                                    code.live
                                        .entry(superx_ops::record_uuid(&m.session))
                                        .or_default()
                                        .replaced_unknown += 1;
                                }
                                if n > 0 || replaced > 0 {
                                    let hour = local.format("%Y-%m-%dT%H").to_string();
                                    let slot = code.churn.entry(hour.clone()).or_insert((0, 0));
                                    slot.0 += n;
                                    slot.1 += replaced;
                                    let it = code.intensity.entry(hour).or_default();
                                    it.added += n;
                                    it.removed += replaced;
                                }
                                // Undo detection (#324). The walk is
                                // newest-first, so `removed_text`
                                // holds what LATER edits took out.
                                // An edit whose new_string is in that
                                // set had its work thrown away by a
                                // later edit — one undo relationship.
                                // A flip-flop therefore scores twice,
                                // which is the honest reading.
                                // Directed or self-inflicted? A rewrite
                                // that follows a human turn is the
                                // design moving; one with nobody
                                // steering is the agent going in
                                // circles (operator insight, #337).
                                // Did this call REWRITE something? The lines it
                                // replaced may be unknown; the event is not
                                // (#388). A whole new file is not a rewrite —
                                // counting it as one made a branch that only
                                // created files read as 100% self-inflicted.
                                let rewrote = replaced > 0 || notebook_rewrite;
                                if replaced > 0 {
                                    if steered {
                                        code.churn_directed += replaced;
                                    } else {
                                        code.churn_self += replaced;
                                    }
                                }
                                if rewrote {
                                    if steered {
                                        code.edits_directed += 1;
                                    } else {
                                        code.edits_self += 1;
                                    }
                                }
                                if let Some(e) = &effort {
                                    let ea = code.efforts.entry(e.clone()).or_default();
                                    ea.lines_added += n;
                                    ea.lines_removed += replaced;
                                }
                                if let Some(k) = &me_key {
                                    let me = code.model_effort.entry(k.clone()).or_default();
                                    me.lines_added += n;
                                    me.lines_removed += replaced;
                                    if rewrote {
                                        if steered {
                                            me.edits_directed += 1;
                                        } else {
                                            me.edits_self += 1;
                                        }
                                    }
                                }
                                if let Some(key) = &branch_pair {
                                    let b = code.branches.entry(key.clone()).or_default();
                                    b.lines_added += n;
                                    b.lines_removed += replaced;
                                    if replaced > 0 {
                                        if steered {
                                            b.churn_directed += replaced;
                                        } else {
                                            b.churn_self += replaced;
                                        }
                                    }
                                    if rewrote {
                                        if steered {
                                            b.edits_directed += 1;
                                        } else {
                                            b.edits_self += 1;
                                        }
                                    }
                                    if let Some(pth) = work_path {
                                        b.files.insert(pth.to_string());
                                        // Only a real write OWNS a path.
                                        // Unguarded, every Read mapped
                                        // its path to a branch for a
                                        // files-created lookup that only
                                        // ever asks about writes.
                                        if n > 0 || replaced > 0 {
                                            code.path_branch
                                                .insert(pth.to_string(), key.clone());
                                        }
                                    }
                                }
                                if let Some(rk) = &repo_key {
                                    let r = code.repos.entry(rk.clone()).or_default();
                                    r.lines_added += n;
                                    r.lines_removed += replaced;
                                    if replaced > 0 {
                                        if steered {
                                            r.churn_directed += replaced;
                                        } else {
                                            r.churn_self += replaced;
                                        }
                                    }
                                    if rewrote {
                                        if steered {
                                            r.edits_directed += 1;
                                        } else {
                                            r.edits_self += 1;
                                        }
                                    }
                                    if let Some(pth) = work_path {
                                        r.files.insert(pth.to_string());
                                    }
                                }
                                if let Some(an) = &agent_name {
                                    let a = code.agents.entry(an.clone()).or_default();
                                    a.lines_added += n;
                                    a.lines_removed += replaced;
                                    if replaced > 0 {
                                        if steered {
                                            a.churn_directed += replaced;
                                        } else {
                                            a.churn_self += replaced;
                                        }
                                    }
                                    if rewrote {
                                        if steered {
                                            a.edits_directed += 1;
                                        } else {
                                            a.edits_self += 1;
                                        }
                                    }
                                    if let Some(rk) = &repo_key {
                                        let cell = code
                                            .cells
                                            .entry((an.clone(), rk.clone(), bucket.clone()))
                                            .or_default();
                                        cell.added += n;
                                        cell.removed += replaced;
                                        if let Some(pth) = work_path {
                                            cell.files.insert(pth.to_string());
                                        }
                                    }
                                }
                                // Did this file EXIST before the window,
                                // or did the agent create it here? The
                                // walk is newest-first, so the last
                                // value written wins — and that is the
                                // oldest event for the path (#340).
                                if let Some(pth) = work_path {
                                    // Created, as the result says it was — every
                                    // Write used to count as a new file, and one in
                                    // four overwrote one that was there (#410).
                                    let creates = recorded
                                        .as_ref()
                                        .and_then(|d| d.created)
                                        .unwrap_or(name == "Write");
                                    code.path_origin.insert(pth.to_string(), creates);
                                    if let Some(rk) = &repo_key {
                                        code.path_repo.insert(pth.to_string(), rk.clone());
                                    }
                                }
                                if rewrote || n > 0 {
                                    code.gate_events
                                        .entry(superx_ops::record_uuid(&m.session))
                                        .or_default()
                                        .push((when, GateEvent::Wrote));
                                    if let Some(pth) = work_path {
                                        note_bright_line(&mut code, pth);
                                    }
                                }
                                // A write is one half of the
                                // edit→verify pair (#340).
                                if n > 0 || replaced > 0 {
                                    code.verify_events
                                        .entry(superx_ops::record_uuid(&m.session))
                                        .or_default()
                                        .push((when, true));
                                }
                                // Guarded like its two siblings above:
                                // without it every Bash and Read block
                                // allocated a session key to add zero.
                                if n > 0 || replaced > 0 {
                                    let l = code
                                        .live
                                        .entry(superx_ops::record_uuid(&m.session))
                                        .or_default();
                                    l.lines_added += n;
                                    l.lines_removed += replaced;
                                    if replaced > 0 {
                                        if steered {
                                            l.churn_directed += replaced;
                                        } else {
                                            l.churn_self += replaced;
                                        }
                                    }
                                    if rewrote {
                                        if steered {
                                            l.edits_directed += 1;
                                        } else {
                                            l.edits_self += 1;
                                        }
                                    }
                                    // Which files, and how often each —
                                    // a path written three times is
                                    // rework of rework (#350).
                                    if let Some(pth) = work_path {
                                        *l.path_hits.entry(pth.to_string()).or_insert(0) += 1;
                                        if l.files_now.len() < LIVE_FILES
                                            && !l.files_now.iter().any(|f| f == pth)
                                        {
                                            l.files_now.push(pth.to_string());
                                        }
                                    }
                                    claim_doing(l, reply.as_deref(), "writing");
                                }
                                if let Some(known) = &model_opt {
                                    let mm = code.models.entry(known.clone()).or_default();
                                    mm.lines_added += n;
                                    mm.lines_removed += replaced;
                                }
                                if let Some(path) = work_path {
                                    let seen = code.removed_text.entry(path.to_string()).or_default();
                                    // How long did this text live?
                                    // `removed_at` holds when a LATER
                                    // edit took it out; meeting its
                                    // author now gives the lifetime
                                    // (#340).
                                    if let Some(key) =
                                        get_str(input, "new_string").and_then(snippet_key)
                                    {
                                        if let Some(gone) = code.removed_at.remove(&key) {
                                            let mins = (gone - when).num_minutes().max(0);
                                            code.survivals.push(mins);
                                            if let Some(rk) = &repo_key {
                                                code.repo_survivals
                                                    .entry(rk.clone())
                                                    .or_default()
                                                    .push(mins);
                                            }
                                            // Half-life per branch: the
                                            // component that separates
                                            // thrash from a design that
                                            // moved (#350).
                                            if let Some(key) = &branch_pair {
                                                code.branches
                                                    .entry(key.clone())
                                                    .or_default()
                                                    .survivals
                                                    .push(mins);
                                            }
                                        }
                                    }
                                    if let Some(key) =
                                        get_str(input, "new_string").and_then(snippet_key)
                                    {
                                        if seen.contains(&key) {
                                            code.reverts += 1;
                                            if let Some(k) = &me_key {
                                                code.model_effort.entry(k.clone()).or_default().reverts += 1;
                                            }
                                            if let Some(an) = &agent_name {
                                                code.agents.entry(an.clone()).or_default().reverts += 1;
                                            }
                                            if let Some(known) = &model_opt {
                                                code.models.entry(known.clone()).or_default().reverts += 1;
                                            }
                                            if let Some(e) = &effort {
                                                code.efforts.entry(e.clone()).or_default().reverts += 1;
                                            }
                                            if let Some(rk) = &repo_key {
                                                code.repos.entry(rk.clone()).or_default().reverts += 1;
                                            }
                                            if let Some(key) = &branch_pair {
                                                code.branches
                                                    .entry(key.clone())
                                                    .or_default()
                                                    .reverts += 1;
                                            }
                                        }
                                    }
                                    if let Some(key) =
                                        get_str(input, "old_string").and_then(snippet_key)
                                    {
                                        // Remember WHEN it went, so the
                                        // write that authored it can be
                                        // dated when the walk reaches
                                        // it (#340).
                                        code.removed_at.entry(key).or_insert(when);
                                        seen.insert(key);
                                    }
                                }
                                lines_written += n;
                                agg.lines += n;
                                code.lines_added += n;
                                code.lines_removed += replaced;

                                // The file this call touched.
                                if let Some(path) = work_path {
                                    // Which of your instructions was this
                                    // answering, and was it the only thing
                                    // being answered (#406)?
                                    if let Some(dir) = dir_of(path) {
                                        let sid = superx_ops::record_uuid(&m.session);
                                        let w = human_turns
                                            .get(&sid)
                                            .map_or(0, |t| t.partition_point(|h| *h <= when));
                                        code.focus.entry((sid, w)).or_default().insert(dir);
                                    }
                                    // The same text in two files drifts apart
                                    // by construction (#406).
                                    if let Some(text) = get_str(input, "content")
                                        .or_else(|| get_str(input, "new_string"))
                                    {
                                        if let Some(shape) = snippet_key(text) {
                                            let seen = code.written_shapes.entry(shape).or_default();
                                            if !seen.iter().any(|p| p == path) && seen.len() < DUP_PATHS {
                                                seen.push(path.to_string());
                                            }
                                        }
                                    }
                                    *code.files.entry(path.to_string()).or_insert(0) += 1;
                                    if let Some(ext) = extension_of(path) {
                                        *code.languages.entry(ext).or_insert(0) += 1;
                                    }
                                    if let Some(dir) = dir_of(path) {
                                        *code.dirs.entry(dir).or_insert(0) += 1;
                                    }
                                }
                                // The shell command it ran — unless it was
                                // refused, in which case it ran nothing.
                                if let Some(cmd) = get_str(input, "command").filter(|_| !refused) {
                                    if SHELL_TOOLS.contains(&name.as_str()) {
                                        // A call that only looks is the
                                        // agent reading (#367): the same
                                        // act as `Read`, through the shell.
                                        // A write anywhere in the chain makes
                                        // the call a write — `cat a > b` copies;
                                        // only a chain that ONLY looks is reading.
                                        // A write only to the agent's scratch is
                                        // neither a write to the work nor a read
                                        // of it (#412).
                                        let shell = shell_write(cmd, cwd);
                                        let wrote_any = shell.is_some();
                                        let looked_at = inspected_paths(cmd, cwd);
                                        if let Some(w) = shell.and_then(|w| w.into_work(&checkouts)) {
                                            // The agent WROTE through the shell
                                            // (#374): the same act as `Edit`, so
                                            // the same instruments move — the
                                            // write count, the live row, the
                                            // files under its hands, and the
                                            // lines where the text is on the
                                            // line. Replaced lines are unknown
                                            // and stay unclaimed.
                                            let n = w.added();
                                            code.writes += 1;
                                            code.replaced_unknown += 1;
                                            // One rewrite, and whether anyone
                                            // asked for it — the half of churn a
                                            // shell edit can still answer (#388).
                                            // A heredoc that writes a file end to
                                            // end creates it, as `Write` does.
                                            let rewrote = w.rewrote();
                                            if rewrote {
                                                if steered {
                                                    code.edits_directed += 1;
                                                } else {
                                                    code.edits_self += 1;
                                                }
                                            }
                                            if let Some(key) = &branch_pair {
                                                let b = code.branches.entry(key.clone()).or_default();
                                                if rewrote {
                                                    if steered {
                                                        b.edits_directed += 1;
                                                    } else {
                                                        b.edits_self += 1;
                                                    }
                                                }
                                            }
                                            if let Some(rk) = &repo_key {
                                                let r = code.repos.entry(rk.clone()).or_default();
                                                if rewrote {
                                                    if steered {
                                                        r.edits_directed += 1;
                                                    } else {
                                                        r.edits_self += 1;
                                                    }
                                                }
                                            }
                                            if let Some(an) = &agent_name {
                                                let a = code.agents.entry(an.clone()).or_default();
                                                if rewrote {
                                                    if steered {
                                                        a.edits_directed += 1;
                                                    } else {
                                                        a.edits_self += 1;
                                                    }
                                                }
                                            }
                                            if let Some(k) = &me_key {
                                                let me = code.model_effort.entry(k.clone()).or_default();
                                                me.lines_added += n;
                                                if rewrote {
                                                    if steered {
                                                        me.edits_directed += 1;
                                                    } else {
                                                        me.edits_self += 1;
                                                    }
                                                }
                                            }
                                            let sid = superx_ops::record_uuid(&m.session);
                                            if n > 0 {
                                                let hour = local.format("%Y-%m-%dT%H").to_string();
                                                code.churn.entry(hour.clone()).or_insert((0, 0)).0 += n;
                                                code.intensity.entry(hour).or_default().added += n;
                                                if let Some(e) = &effort {
                                                    code.efforts.entry(e.clone()).or_default().lines_added += n;
                                                }
                                                if let Some(key) = &branch_pair {
                                                    code.branches.entry(key.clone()).or_default().lines_added += n;
                                                }
                                                if let Some(rk) = &repo_key {
                                                    code.repos.entry(rk.clone()).or_default().lines_added += n;
                                                }
                                                if let Some(an) = &agent_name {
                                                    code.agents.entry(an.clone()).or_default().lines_added += n;
                                                    if let Some(rk) = &repo_key {
                                                        code.cells
                                                            .entry((an.clone(), rk.clone(), bucket.clone()))
                                                            .or_default()
                                                            .added += n;
                                                    }
                                                }
                                                if let Some(known) = &model_opt {
                                                    code.models.entry(known.clone()).or_default().lines_added += n;
                                                }
                                                lines_written += n;
                                                agg.lines += n;
                                                code.lines_added += n;
                                            }
                                            code.verify_events.entry(sid.clone()).or_default().push((when, true));
                                            code.gate_events.entry(sid.clone()).or_default().push((when, GateEvent::Wrote));
                                            for f in &w.files {
                                                note_bright_line(&mut code, &f.path);
                                            }
                                            let l = code.live.entry(sid.clone()).or_default();
                                            l.lines_added += n;
                                            l.replaced_unknown += 1;
                                            if rewrote {
                                                if steered {
                                                    l.edits_directed += 1;
                                                } else {
                                                    l.edits_self += 1;
                                                }
                                            }
                                            claim_doing(l, reply.as_deref(), "writing");
                                            for path in w.files.iter().map(|f| &f.path) {
                                                *l.path_hits.entry(path.clone()).or_insert(0) += 1;
                                                if l.files_now.len() < LIVE_FILES
                                                    && !l.files_now.iter().any(|x| x == path)
                                                {
                                                    l.files_now.push(path.clone());
                                                }
                                            }
                                            for f in &w.files {
                                                let path = &f.path;
                                                if let Some(dir) = dir_of(path) {
                                                    let win = human_turns
                                                        .get(&sid)
                                                        .map_or(0, |t| t.partition_point(|h| *h <= when));
                                                    code.focus.entry((sid.clone(), win)).or_default().insert(dir);
                                                }
                                                if let Some(shape) = f.shape {
                                                    let seen = code.written_shapes.entry(shape).or_default();
                                                    if !seen.iter().any(|p| p == path) && seen.len() < DUP_PATHS {
                                                        seen.push(path.clone());
                                                    }
                                                }
                                                // Created here or already there?
                                                // A whole-file write creates, as
                                                // `Write` does; an in-place edit
                                                // modifies. Without this the
                                                // created/modified split saw none
                                                // of the day's shell edits (#388).
                                                code.path_origin.insert(path.clone(), f.whole);
                                                if let Some(rk) = &repo_key {
                                                    code.path_repo.insert(path.clone(), rk.clone());
                                                }
                                                *code.files.entry(path.clone()).or_insert(0) += 1;
                                                if let Some(ext) = extension_of(path) {
                                                    *code.languages.entry(ext).or_insert(0) += 1;
                                                }
                                                if let Some(dir) = dir_of(path) {
                                                    *code.dirs.entry(dir).or_insert(0) += 1;
                                                }
                                                if let Some(key) = &branch_pair {
                                                    code.branches.entry(key.clone()).or_default().files.insert(path.clone());
                                                }
                                                if let Some(rk) = &repo_key {
                                                    code.repos.entry(rk.clone()).or_default().files.insert(path.clone());
                                                }
                                            }
                                        } else if !wrote_any && shell_inspects(cmd) {
                                            code.reads += 1;
                                            let l = code
                                                .live
                                                .entry(superx_ops::record_uuid(&m.session))
                                                .or_default();
                                            claim_doing(l, reply.as_deref(), "reading");
                                            for path in &looked_at {
                                                if l.files_now.len() < LIVE_FILES
                                                    && !l.files_now.iter().any(|x| x == path)
                                                {
                                                    l.files_now.push(path.clone());
                                                }
                                            }
                                        }
                                        // Exposure, whatever else the call did
                                        // (#413): a stage that looked put what it
                                        // read in front of the model. `ls ~/.netrc
                                        // && grep … ~/.netrc; git config …` read a
                                        // credentials file, and went uncounted
                                        // because the call as a whole was not a
                                        // read.
                                        for path in looked_at {
                                            if let Some(rk) = &repo_key {
                                                code.repos_exposed.insert(rk.clone());
                                            }
                                            if reads_outside(&path, cwd, &checkouts) {
                                                code.outside_reads += 1;
                                            }
                                            code.files_read.insert(path);
                                        }
                                        if let Some(key) = repeat_key(cmd) {
                                            *code.command_lines.entry(key).or_insert(0) += 1;
                                        }
                                    }
                                    // Branch sprawl (#406): a session that
                                    // opens branches faster than it closes
                                    // them ends with overlapping work on each.
                                    if cmd.contains("checkout -b")
                                        || cmd.contains("checkout -B")
                                        || cmd.contains("switch -c")
                                    {
                                        code.branches_opened += 1;
                                    }
                                    // What the gates saw, and what the call
                                    // SHIPPED (#392, #381) — judged against what
                                    // it printed, which is usually already in
                                    // hand: the walk meets the output first. When
                                    // it is not, the judgement waits for it.
                                    if SHELL_TOOLS.contains(&name.as_str()) {
                                        let sid = superx_ops::record_uuid(&m.session);
                                        match (&shell_text, get_str(block, "id")) {
                                            (Some((out, failed)), _) => {
                                                judge_shell(&mut code, &sid, when, cmd, out, *failed)
                                            }
                                            (None, Some(id)) => {
                                                pending_shell.insert(id.to_string(), (sid, when, cmd.to_string()));
                                            }
                                            (None, None) => {}
                                        }
                                    }
                                    // Every stage of the chain counts —
                                    // `cd repo && cargo test` is a test run.
                                    for label in command_labels(cmd) {
                                        let (is_test, is_build, is_git) = classify_command(&label);
                                        if is_test {
                                            code.tests += 1;
                                            if let Some(rk) = &repo_key {
                                                code.repos.entry(rk.clone()).or_default().tests_run += 1;
                                            }
                                            if let Some(key) = &branch_pair {
                                                code.branches.entry(key.clone()).or_default().tests_run += 1;
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
                        Some("image") | Some("document") => code.attachments += 1,
                        Some("tool_result") => {
                            let Some(id) = get_str(block, "tool_use_id") else { continue };
                            let failed = matches!(block.get("is_error"), Some(Value::Bool(true)));
                            // A refused call never ran (#412): Claude Code
                            // stamps every refusal — the operator's, the
                            // auto-mode classifier's, a permission rule's —
                            // with `toolDenialKind` on the result line.
                            let denied = get_str(raw, "toolDenialKind").is_some();
                            if denied {
                                denied_calls.insert(id.to_string());
                            }
                            // What the call changed, as Claude Code recorded
                            // it (#410). `toolUseResult` describes the line's
                            // result, so it is read only when the line holds
                            // one.
                            if results_in_row == 1 {
                                if let Some(d) = recorded_diff(raw, failed || denied) {
                                    pending_diffs.insert(id.to_string(), d);
                                }
                            }
                            // What the command PRINTED is where quality
                            // lives (#327) — but only a SHELL call's
                            // output is a report. The walk is
                            // newest-first, so the tool that produced
                            // this text is not known yet: stash it and
                            // score when the call resolves (review of
                            // #330).
                            if let Some(text) = tool_result_text(block) {
                                let text = text.as_str();
                                // Everything a tool returns is carried
                                // into the next prompt verbatim (#337).
                                code.content_bytes += text.len() as i64;
                                if looks_like_secret(text) {
                                    code.secret_hits += 1;
                                    match call_paths.remove(id) {
                                        Some(path) => {
                                            code.secret_paths.insert(path);
                                        }
                                        None => {
                                            secret_pending.insert(id.to_string());
                                        }
                                    }
                                }
                                if shell_calls.remove(id) {
                                    // The call already went by and it
                                    // was a shell: score immediately.
                                    let d = score_output(text, &mut code, &hour_key);
                                    attribute_quality(&mut code, &branch_pair, &agent_name, &effort, &me_key, d);
                                } else {
                                    pending_output.insert(id.to_string(), (text.to_string(), failed));
                                }
                                if let Some((sid, at, cmd)) = pending_shell.remove(id) {
                                    if !denied {
                                        judge_shell(&mut code, &sid, at, &cmd, text, failed);
                                    }
                                }
                            }

                            match call_names.remove(id) {
                                Some((name, call_model, call_repo, call_branch)) => {
                                    let entry = outcomes.entry(name).or_default();
                                    if failed {
                                        entry.failed += 1;
                                        if let Some(an) = &agent_name {
                                            code.agents.entry(an.clone()).or_default().tool_failures += 1;
                                        }
                                        // The call's model and repo, not
                                        // this result message's.
                                        if let Some(cm) = call_model {
                                            code.models.entry(cm).or_default().tool_failures += 1;
                                        }
                                        if let Some(cr) = call_repo {
                                            code.repos.entry(cr).or_default().tool_failures += 1;
                                        }
                                        if let Some(cb) = call_branch {
                                            code.branches.entry(cb).or_default().tool_failures += 1;
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
        // Gemini-style tool calls: raw.toolCalls[]. Its tokens are read
        // with Claude's, above, by `reply_usage` (#409): they used to
        // reach the per-session figure and nothing else, and its `total`
        // — which includes what it wrote — stood in for its context.
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

    // What a sortie was LIKE (#395) — kept before the map is consumed.
    let session_work: HashMap<String, (i64, i64)> = per_session
        .iter()
        .map(|(k, a)| (k.clone(), (a.lines, a.out_tokens)))
        .collect();

    // Were the gates run (#392)? The question is an order, which only
    // the finished walk can answer. A pull request counts as gated when
    // tests, clippy and the audit all ran after the last write before
    // it. One opened by a session that changed nothing is neither —
    // there was nothing to check — so the two never sum to `prs_opened`
    // and the page says so.
    let (mut prs_gated, mut prs_ungated) = (0i64, 0i64);
    for events in code.gate_events.values_mut() {
        events.sort_by_key(|(t, _)| *t);
        let (mut tested, mut linted, mut audited, mut wrote) = (false, false, false, false);
        for (_, e) in events.iter() {
            match e {
                GateEvent::Wrote => {
                    wrote = true;
                    tested = false;
                    linted = false;
                    audited = false;
                }
                GateEvent::Tested => tested = true,
                GateEvent::Linted => linted = true,
                GateEvent::Audited => audited = true,
                GateEvent::OpenedPr => {
                    if wrote {
                        if tested && linted && audited {
                            prs_gated += 1;
                        } else {
                            prs_ungated += 1;
                        }
                    }
                }
            }
        }
    }

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
        let local = e.valid_from.with_timezone(&clock);
        *per_minute
            .entry(local.format("%Y-%m-%dT%H:%M").to_string())
            .or_insert(0) += 1;
        if e.lifecycle_event == "boot_complete" {
            if let Value::Object(o) = &e.payload {
                boots.push(TimeCount {
                    t: local.format("%m-%d %H:%M").to_string(),
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
    // Every one on the agent's clock (#413). On the capture clock a
    // backfill read as a burst "this hour", and coverage collapsed into
    // the hour of the restart.
    let hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    let messages_last_hour = {
        // Replies, once each, as every other "messages" on the page (#415
        // review).
        let rows: Vec<Value> = kernel
            .db()
            .query(format!(
                "SELECT count() AS c FROM (\
                     SELECT {REPLY_KEY_SQL} AS k FROM message \
                     WHERE valid_from > $cut AND (emitted_at ?? valid_from) > $cut GROUP BY k\
                 ) GROUP ALL"
            ))
            .bind(("cut", hour_ago))
            .await?
            .take(0)?;
        rows.first().and_then(|r| obj(r).map(|o| get_int(o, "c"))).unwrap_or(0)
    };
    let tokens_last_hour = reply_output_tokens(kernel, Some(hour_ago)).await?;
    // Clock coverage: which of the last 24 CLOCK hours — this one and the
    // 23 before it, the buckets the strip draws — saw any activity. A cut
    // 24 hours back reached into a 25th bucket, so the tile could read
    // 25/24 and the gauge 104% (#415 review).
    let now = chrono::Utc::now();
    let this_hour = now
        - chrono::Duration::seconds(now.timestamp().rem_euclid(3600))
        - chrono::Duration::nanoseconds(i64::from(now.timestamp_subsec_nanos()));
    let day_ago = this_hour - chrono::Duration::hours(23);
    let active_hours_list: Vec<String> = {
        let rows: Vec<Value> = kernel
            .db()
            .query(
                // (day, hour), not hour alone: in a rolling 24-hour
                // window the same clock hour occurs twice, and
                // collapsing them caps a round-the-clock operator
                // below 24 (review of #311).
                "SELECT time::format(emitted_at ?? valid_from, '%Y-%m-%dT%H') AS h FROM message \
                 WHERE valid_from > $cut AND (emitted_at ?? valid_from) > $cut GROUP BY h",
            )
            .bind(("cut", day_ago))
            .await?
            .take(0)?;
        let mut hours: Vec<String> = rows
            .iter()
            .filter_map(|r| obj(r).and_then(|o| get_str(o, "h")).map(str::to_string))
            .collect();
        hours.sort();
        hours
    };
    let active_hours_24h = active_hours_list.len() as i64;

    // ── post-walk reductions (#340) ──────────────────────────────
    // Each write waits for the next verification in its session. The
    // events were pushed newest-first, so sort before pairing.
    let mut verify_gaps: Vec<i64> = Vec::new();
    for (sid, events) in &mut code.verify_events {
        events.sort_by_key(|(at, _)| *at);
        let mut pending: Option<chrono::DateTime<chrono::Utc>> = None;
        for (at, is_write) in events.iter() {
            if *is_write {
                // The FIRST write of a streak owns the wait — later
                // writes before the same check are the same batch.
                pending.get_or_insert(*at);
            } else if let Some(start) = pending.take() {
                let secs = (*at - start).num_seconds().max(0);
                verify_gaps.push(secs);
                if let Some(agent) = agent_of.get(sid) {
                    code.agents
                        .entry(agent.clone())
                        .or_default()
                        .verify_gaps
                        .push(secs);
                }
                // Verify latency per branch (#350). The events are
                // keyed by session, so the branch comes through the
                // session's newest branch.
                if let Some(key) = code.session_branch.get(sid).cloned() {
                    code.branches.entry(key).or_default().verify_gaps.push(secs);
                }
            }
        }
    }
    // How much rope did each agent get (#391)? The gap from one human
    // turn to the next, within a session, is how long it flew before it
    // needed anything.
    let mut turn_gaps: Vec<i64> = Vec::new();
    let mut human_turn_count = 0i64;
    for turns in human_turns.values() {
        human_turn_count += turns.len() as i64;
        for w in turns.windows(2) {
            turn_gaps.push((w[1] - w[0]).num_minutes().max(0));
        }
    }
    // -1 for NO data, so 0 can mean what it says (#413): a median under a
    // minute used to render as "no session had two turns".
    let autonomy_p50_mins = median(&mut turn_gaps);

    // A file whose oldest event in the window was a full Write was
    // created here; anything else already existed.
    let mut files_created = 0i64;
    let mut files_modified = 0i64;
    let mut repo_created: HashMap<String, i64> = HashMap::new();
    let mut branch_created: HashMap<(String, String), i64> = HashMap::new();
    for (path, creates) in &code.path_origin {
        if *creates {
            files_created += 1;
            if let Some(rk) = code.path_repo.get(path) {
                *repo_created.entry(rk.clone()).or_insert(0) += 1;
            }
            if let Some(key) = code.path_branch.get(path) {
                *branch_created.entry(key.clone()).or_insert(0) += 1;
            }
        } else {
            files_modified += 1;
        }
    }
    let mut work_cells: Vec<_> = code
        .cells
        .into_iter()
        .map(|((agent, repo, t), c)| WorkCell {
            t,
            agent,
            repo,
            added: c.added,
            removed: c.removed,
            files: c.files.len() as i64,
            out_tokens: c.out_tokens,
            messages: c.messages,
        })
        .collect();
    work_cells.sort_by(|a, b| a.t.cmp(&b.t).then(a.repo.cmp(&b.repo)).then(a.agent.cmp(&b.agent)));
    let mut compaction_sessions: Vec<_> = code
        .compact_by_session
        .into_iter()
        .map(|(sid, mut c)| {
            let total: i64 = c.durations.iter().sum();
            CompactionStat {
                identity: identity.get(&sid).cloned().unwrap_or_else(|| sid.clone()),
                session_id: sid,
                agent: c.agent,
                repo: c.repo,
                count: c.durations.len() as i64,
                total_ms: total,
                median_ms: median(&mut c.durations),
                pre_tokens_max: c.pre_tokens_max,
                auto: c.auto,
                manual: c.manual,
            }
        })
        .collect();
    compaction_sessions.sort_by_key(|c| std::cmp::Reverse(c.total_ms));
    // Unknown, not zero, when compactions happened and none was timed
    // (#413): the timing is on system lines capture does not keep (#373).
    let compaction_total_ms: Option<i64> = if compaction_sessions.is_empty() && code.compactions > 0 {
        None
    } else {
        Some(compaction_sessions.iter().map(|c| c.total_ms).sum())
    };
    let edit_to_verify_p50_secs = median(&mut verify_gaps);
    let survival_p50_mins = median(&mut code.survivals);

    // Per-agent productivity, most productive first (#337).
    let mut agent_stats: Vec<_> = code
        .agents
        .into_iter()
        .map(|(name, mut a)| AgentStat {
            name,
            sessions: a.sessions.len() as i64,
            messages: a.messages,
            lines_added: a.lines_added,
            lines_removed: a.lines_removed,
            out_tokens: a.out_tokens,
            in_tokens: a.in_tokens,
            tool_failures: a.tool_failures,
            reverts: a.reverts,
            repos: a.repos.len() as i64,
            repo_switches: a.repo_switches,
            edit_to_verify_p50_secs: median(&mut a.verify_gaps),
            compactions: a.compactions,
            compaction_ms: a.compaction_ms,
            churn_directed: a.churn_directed,
            churn_self: a.churn_self,
            edits_directed: a.edits_directed,
            edits_self: a.edits_self,
            tests_passed: a.tests_passed,
            tests_failed: a.tests_failed,
            compile_errors: a.compile_errors,
        })
        .collect();
    agent_stats.sort_by_key(|a| std::cmp::Reverse(a.lines_added));
    let mut secret_paths: Vec<String> = code.secret_paths.into_iter().collect();
    secret_paths.sort();
    let exposure = Exposure {
        input_tokens: code.in_tokens,
        cache_write_tokens: code.cache_write,
        cache_read_tokens: code.cache_read,
        content_bytes: code.content_bytes,
        files_read: code.files_read.len() as i64,
        repos_exposed: code.repos_exposed.len() as i64,
        attachments: code.attachments,
        outside_reads: code.outside_reads,
        secret_hits: code.secret_hits,
        secret_paths,
    };
    // What the repositories say landed in this range (#386). The window
    // has no cutoff of its own: its range is the oldest message it holds.
    // A capped range is a sample of its period: git is asked about the
    // period the sample covers, not the whole range — at `all`, every
    // repository's entire history against the newest 20,000 rows (#413).
    let landed_since = if range == "window" || truncated {
        code.instants.iter().min().copied()
    } else {
        since
    };
    let mainlines = crate::resolved_mainline_refs(kernel).await;
    let landed = crate::landed::landed(&code.cwds, &checkouts, landed_since, clock, &mainlines).await;

    Ok(StatsSummary {
        landed,
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
        replaced_unknown: code.replaced_unknown,
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
        active_hours: active_hours_list,
        active_hours_range: code.active_hours.len() as i64,
        tests_run: code.tests,
        builds_run: code.builds,
        git_ops: code.git,
        commits: code.commits,
        pushes: code.pushes,
        prs_opened: code.prs_opened,
        prs_merged: code.prs_merged,
        committed_added: code.committed_added,
        committed_removed: code.committed_removed,
        commits_with_stat: code.commits_with_stat,
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
        // The same threshold the page states (#413): it read a literal 3
        // beside a `revisit_at` that could drift from it.
        thrash_files: code.files.values().filter(|&&n| n >= REVISIT_AT).count() as i64,
        out_tokens_window: code.out_tokens,
        top_repeat: top_n(code.command_lines, 1)
            .into_iter()
            .find(|c| c.value >= REPEAT_AT),
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
        // The branch dimension (#350), ranked worst-quality first —
        // the branch with the most to fix is the one you want to see.
        branches: {
            let mut v: Vec<BranchStat> = code
                .branches
                .iter()
                .map(|((repo, branch), b)| {
                    let d = branch_derived(b);
                    BranchStat {
                        repo: repo.clone(),
                        branch: branch.clone(),
                        messages: b.messages,
                        sessions: b.sessions.len() as i64,
                        agents: b.agents.len() as i64,
                        lines_added: b.lines_added,
                        lines_removed: b.lines_removed,
                        files_touched: b.files.len() as i64,
                        files_created: branch_created
                            .get(&(repo.clone(), branch.clone()))
                            .copied()
                            .unwrap_or(0),
                        churn_directed: b.churn_directed,
                        churn_self: b.churn_self,
                        edits_directed: b.edits_directed,
                        edits_self: b.edits_self,
                        tests_run: b.tests_run,
                        tests_passed: b.tests_passed,
                        tests_failed: b.tests_failed,
                        compile_errors: b.compile_errors,
                        tool_calls: b.tool_calls,
                        tool_failures: b.tool_failures,
                        reverts: b.reverts,
                        survival_p50_mins: d.survival_p50_mins,
                        edit_to_verify_p50_secs: d.edit_to_verify_p50_secs,
                        out_tokens: b.out_tokens,
                        last_active: b
                            .last_active
                            .map(|t| t.to_rfc3339())
                            .unwrap_or_default(),
                        self_churn_pct: d.self_churn_pct,
                        rework_pct: d.rework_pct,
                        test_pass_pct: d.test_pass_pct,
                        failures_per_100: d.failures_per_100,
                        quality_pct: d.quality_pct,
                    }
                })
                .collect();
            // Unscorable branches sort last, not first: -1 must not
            // masquerade as the worst branch on the machine.
            v.sort_by(|a, b| {
                let key = |s: &BranchStat| if s.quality_pct < 0 { i64::MAX } else { s.quality_pct };
                // Final tiebreak by name, as every sibling sort does,
                // so tied rows do not reshuffle between refreshes and
                // change which survive the truncate (#354 review).
                key(a)
                    .cmp(&key(b))
                    .then(b.messages.cmp(&a.messages))
                    .then(a.repo.cmp(&b.repo))
                    .then(a.branch.cmp(&b.branch))
            });
            v.truncate(16);
            v
        },
        revisit_at: REVISIT_AT,
        repos: {
            let mut v: Vec<RepoStat> = code
                .repos
                .iter()
                .map(|(name, r)| RepoStat {
                    survival_p50_mins: code.repo_survivals.get(name).map_or(NO_DATA, |v| median(&mut v.clone())),
                    files_created: repo_created.get(name).copied().unwrap_or(0),
                    name: name.clone(),
                    branch: r.branch.clone(),
                    messages: r.messages,
                    lines_added: r.lines_added,
                    lines_removed: r.lines_removed,
                    files_touched: r.files.len() as i64,
                    tests_run: r.tests_run,
                    tool_failures: r.tool_failures,
                    out_tokens: r.out_tokens,
                    churn_directed: r.churn_directed,
                    churn_self: r.churn_self,
                    edits_directed: r.edits_directed,
                    edits_self: r.edits_self,
                    reverts: r.reverts,
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
        churn_directed: code.churn_directed,
        churn_self: code.churn_self,
        burn: code
            .burn
            .into_iter()
            .map(|(t, (out, thinking, input, cache_read))| BurnPoint {
                t,
                out,
                thinking,
                input,
                cache_read,
            })
            .collect(),
        model_effort: {
            let mut v: Vec<ModelEffortStat> = code
                .model_effort
                .into_iter()
                .map(|((model, effort), me)| ModelEffortStat {
                    model,
                    effort,
                    sessions: me.sessions.len() as i64,
                    messages: me.messages,
                    out_tokens: me.out_tokens,
                    thinking_tokens: me.thinking_tokens,
                    lines_added: me.lines_added,
                    lines_removed: me.lines_removed,
                    edits_directed: me.edits_directed,
                    edits_self: me.edits_self,
                    tool_calls: me.tool_calls,
                    tool_failures: me.tool_failures,
                    reverts: me.reverts,
                    interventions: me.interventions,
                    denials: me.denials,
                    tests_passed: me.tests_passed,
                    tests_failed: me.tests_failed,
                })
                .collect();
            v.sort_by(|a, b| {
                b.messages
                    .cmp(&a.messages)
                    .then(a.model.cmp(&b.model))
                    .then(a.effort.cmp(&b.effort))
            });
            v
        },
        intensity: code
            .intensity
            .iter()
            .map(|(t, i)| IntensityPoint {
                t: t.clone(),
                sessions: i.sessions.len() as i64,
                repos: i.repos.len() as i64,
                lines_added: i.added,
                lines_removed: i.removed,
                out_tokens: i.out_tokens,
            })
            .collect(),
        peak_sessions: code.intensity.values().map(|i| i.sessions.len() as i64).max().unwrap_or(0),
        peak_repos: code.intensity.values().map(|i| i.repos.len() as i64).max().unwrap_or(0),
        prs_gated,
        prs_ungated,
        bright_line_writes: code.bright_line.values().sum(),
        bright_line_paths: code.bright_line.keys().take(BRIGHT_LINE_SHOWN).cloned().collect(),
        // How scattered each session was between your turns (#406).
        focus: {
            let mut per: HashMap<String, Vec<i64>> = HashMap::new();
            for ((sid, _), dirs) in &code.focus {
                per.entry(sid.clone()).or_default().push(dirs.len() as i64);
            }
            let mut v: Vec<FocusStat> = per
                .into_iter()
                .map(|(sid, mut widths)| {
                    widths.sort_unstable();
                    FocusStat {
                        model: code.live.get(&sid).and_then(|l| l.model.clone()),
                        windows: widths.len() as i64,
                        median_streams: widths[widths.len() / 2],
                        max_streams: widths.last().copied().unwrap_or(0),
                        branches: code.session_branches.get(&sid).map_or(0, |b| b.len() as i64),
                        identity: identity.get(&sid).cloned().unwrap_or(sid),
                    }
                })
                .collect();
            v.sort_by_key(|f| std::cmp::Reverse(f.max_streams));
            v.truncate(FOCUS_ROWS);
            v
        },
        // One artifact, several files — guaranteed to drift (#406).
        duplicates: {
            let mut v: Vec<DuplicateWrite> = code
                .written_shapes
                .values()
                .filter(|paths| paths.len() > 1)
                .map(|paths| DuplicateWrite {
                    copies: paths.len() as i64,
                    paths: paths.clone(),
                })
                .collect();
            v.sort_by_key(|d| std::cmp::Reverse(d.copies));
            v.truncate(FOCUS_ROWS);
            v
        },
        branches_opened: code.branches_opened,
        human_turns: human_turn_count,
        autonomy_p50_mins,
        unattended_out_tokens: code.unattended_out,
        edits_directed: code.edits_directed,
        edits_self: code.edits_self,
        efforts: {
            let mut v: Vec<EffortStat> = code
                .efforts
                .iter()
                .map(|(name, e)| EffortStat {
                    name: name.clone(),
                    messages: e.messages,
                    lines_added: e.lines_added,
                    lines_removed: e.lines_removed,
                    out_tokens: e.out_tokens,
                    thinking_tokens: e.thinking_tokens,
                    tool_failures: e.tool_failures,
                    reverts: e.reverts,
                    tests_passed: e.tests_passed,
                    tests_failed: e.tests_failed,
                })
                .collect();
            v.sort_by(|a, b| b.messages.cmp(&a.messages).then(a.name.cmp(&b.name)));
            v
        },
        quality_series: {
            let mut pts: Vec<QualityPoint> = code
                .quality
                .iter()
                .map(|(t, (p, f, tf))| QualityPoint {
                    t: t.clone(),
                    tests_passed: *p,
                    tests_failed: *f,
                    tool_failures: *tf,
                })
                .collect();
            pts.sort_by(|a, b| a.t.cmp(&b.t));
            pts
        },
        fail_by_hour: {
            let mut v: Vec<HourRate> = code
                .by_hour
                .iter()
                .map(|(h, (calls, failures))| HourRate {
                    hour: *h,
                    calls: *calls,
                    failures: *failures,
                })
                .collect();
            v.sort_by_key(|h| h.hour);
            v
        },
        wait_ms_total: code.waits.iter().sum(),
        wait_ms_median: {
            let mut w = code.waits.clone();
            w.sort_unstable();
            w.get(w.len() / 2).copied().unwrap_or(0)
        },
        wait_ms_p95: {
            let mut w = code.waits.clone();
            w.sort_unstable();
            let idx = (w.len() as f64 * 0.95) as usize;
            w.get(idx.min(w.len().saturating_sub(1))).copied().unwrap_or(0)
        },
        slowest: {
            let mut v = code.slowest.clone();
            v.sort_by_key(|o| std::cmp::Reverse(o.ms));
            v.truncate(8);
            v
        },
        interrupted_calls: code.interrupted,
        timeline: {
            let mut v: Vec<SessionSpan> = code
                .spans
                .iter()
                .map(|(sid, (start, end, agent, repo, msgs))| SessionSpan {
                    identity: sid.clone(),
                    agent: agent.clone(),
                    repo: repo.clone(),
                    lines_added: session_work.get(sid).map_or(0, |w| w.0),
                    out_tokens: session_work.get(sid).map_or(0, |w| w.1),
                    repos: code.session_repos.get(sid).map_or(0, |r| r.len() as i64),
                    start: start.to_rfc3339(),
                    end: end.to_rfc3339(),
                    messages: *msgs,
                })
                .collect();
            v.sort_by(|a, b| b.end.cmp(&a.end));
            v.truncate(20);
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
                    // Live means a message inside the activity window.
                    // This read the bare `300` the Sessions page also
                    // hardcodes, so raising `attr_ui_active_session_secs`
                    // moved `sessions_active` and left this panel behind
                    // (#344 review, §9). One resolved value now.
                    if idle > active_secs {
                        return None;
                    }
                    Some(LiveSession {
                        identity: sid.clone(),
                        agent: l.agent.clone(),
                        repo: l.repo.clone(),
                        branch: l.branch.clone(),
                        model: l.model.clone(),
                        effort: l.effort.clone(),
                        last_tool: l.last_tool.clone(),
                        messages: l.messages,
                        lines_added: l.lines_added,
                        lines_removed: l.lines_removed,
                        replaced_unknown: l.replaced_unknown,
                        out_tokens: l.out_tokens,
                        tool_failures: l.tool_failures,
                        idle_secs: idle,
                        last_seen_at: Some(newest.to_rfc3339()),
                        context_tokens: l.context_tokens,
                        context_pct: l
                            .context_tokens
                            .map(|c| ((c * 100) / context_window.max(1)).clamp(0, 100)),
                        files_now: l.files_now.clone(),
                        awaiting: l.awaiting,
                        shipped: l.shipped.clone(),
                        shipped_at: l.shipped_at.map(|t| t.to_rfc3339()),
                        // No classifiable call in the window: say what
                        // the silence IS rather than leaving it blank —
                        // reasoning, blocked on a long call, or simply
                        // quiet (#350).
                        // No classifiable call: say what the silence
                        // IS. `waiting` used to live here, inferred
                        // from a `durationMs` that reports a FINISHED
                        // operation — so it announced a session was
                        // blocked on a build that had already returned,
                        // and any session that ever reported a duration
                        // could never read as quiet (#354 review). The
                        // duration is still carried, as context.
                        doing: l.doing.clone().unwrap_or_else(|| {
                            if l.thinking_tokens > 0 {
                                "thinking".to_string()
                            } else {
                                "quiet".to_string()
                            }
                        }),
                        thinking_tokens: l.thinking_tokens,
                        last_op_secs: l.last_op_ms / 1000,
                        // Lines when the transcript can see them, else
                        // edits — the signal must not go dark because
                        // the session edits through the shell (#388).
                        self_churn_pct: if l.churn_directed + l.churn_self > 0 {
                            pct(l.churn_self, l.churn_directed + l.churn_self)
                        } else {
                            pct(l.edits_self, l.edits_directed + l.edits_self)
                        },
                        files_revisited: l
                            .path_hits
                            .values()
                            .filter(|&&h| h >= REVISIT_AT)
                            .count() as i64,
                    })
                })
                .collect();
            // Every live session, not the busiest eight: the Waiting,
            // Circling and Context lamps read this list, and a ninth live
            // session lit none of them (#413). The panel lists them all,
            // busiest first — who is in the air is the question it answers.
            v.sort_by(|a, b| b.messages.cmp(&a.messages).then(a.identity.cmp(&b.identity)));
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
        agent_stats,
        exposure,
        work_cells,
        files_created,
        files_modified,
        repo_switches: code.repo_switches,
        edit_to_verify_p50_secs,
        survival_p50_mins,
        compaction_sessions,
        compaction_total_ms,
    })
}

/// Resolve the active-session threshold from the ui module's
/// parameter, else the default.
pub(crate) async fn resolved_active_secs(kernel: &Kernel) -> i64 {
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
