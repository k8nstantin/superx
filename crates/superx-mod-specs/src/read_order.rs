//! Level 1: did a build read its spec in the order its root requires,
//! before it wrote anything?
//!
//! The order is the ROOT'S OWN, read from the version of `root.xml` the
//! session itself read, so a root that reorders its list is judged by its
//! own words. The method's `read` step (2026-10-13) says:
//!
//! > A build starts at the task file (rules.xml kickoff); its start
//! > element sends the model here. Then, in this order, whole: the code
//! > merged on sandbox under gryphon-kn/kn (the platform); this root;
//! > rules.xml; your task file; the design and structure files your task
//! > file names, by the keys it lists; … Nothing else: no earlier spec,
//! > no other branch, no page, no conversation. The order is required: a
//! > session that reads a file out of this order reads again from the
//! > first file in order before it writes anything …
//!
//! So: the list after "in this order" is split at `;`, and each item is
//! recognised by what it names. Reads of the task file before the first
//! other step are the kickoff's start. A read of an earlier step after a
//! later one breaks the order, and only a read of the FIRST step mends
//! it: everything is then read again in order. A spec file of another
//! task is "an earlier spec". The spec files are to be read whole.
//!
//! What this cannot see: a read through the shell (`cat`, `sed`) rather
//! than the Read tool, and a write through the shell. Both are counted
//! as nothing, and the result says what it rests on.

use crate::session::Read;
use crate::spec::Kind;

/// One item of the root's read list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The platform code, under the root's `<platform path>`.
    Platform,
    Root,
    Rules,
    /// The task file the kickoff named.
    Task,
    /// The files the task file names.
    Named,
}

impl Step {
    fn describe(self) -> &'static str {
        match self {
            Step::Platform => "the platform code",
            Step::Root => "root.xml",
            Step::Rules => "rules.xml",
            Step::Task => "the task file",
            Step::Named => "the files the task names",
        }
    }
}

/// The order a root's `read` step requires. `None` when its words do not
/// give a recognisable list: the check then says it cannot judge, rather
/// than judging by a list it guessed.
#[must_use]
pub fn steps(read_text: &str, platform: Option<&str>) -> Option<Vec<Step>> {
    let lower = read_text.to_lowercase();
    let start = lower.find("in this order")?;
    let list = &read_text[start..];
    let list = list.split_once(':').map_or(list, |(_, rest)| rest);
    let platform = platform.map(|p| p.trim_end_matches('/').to_lowercase());
    let mut out: Vec<Step> = Vec::new();
    for item in list.split(';') {
        let item = item.to_lowercase();
        let step = if platform.as_deref().is_some_and(|p| item.contains(p)) {
            Step::Platform
        } else if item.contains("rules.xml") {
            Step::Rules
        } else if item.contains("task file") && item.contains("names") {
            Step::Named
        } else if item.contains("task file") {
            Step::Task
        } else if item.contains("root") {
            Step::Root
        } else {
            continue;
        };
        if !out.contains(&step) {
            out.push(step);
        }
    }
    let core = [Step::Platform, Step::Root, Step::Rules, Step::Task];
    core.iter().all(|s| out.contains(s)).then_some(out)
}

/// What one run's reads say about one clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judgement {
    /// `complied`, `breached` or `no-evidence` (the schema's ASSERT).
    pub result: &'static str,
    pub note: String,
    /// The tool call ids the result rests on.
    pub evidence: Vec<String>,
}

/// What a read is, for the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Step(Step),
    /// A spec file that is not one of this build's: "an earlier spec".
    Foreign,
    /// Anything else: not judged.
    Other,
}

/// The facts a read is classified by.
pub struct Context<'a> {
    pub platform: Option<&'a str>,
    /// The task file as the kickoff named it.
    pub task_path: &'a str,
    pub named: &'a [String],
    /// The kind of spec file each path held when this session read it
    /// whole.
    pub kind_of: &'a dyn Fn(&str) -> Option<Kind>,
}

/// Classify one read.
#[must_use]
pub fn classify(read: &Read, cx: &Context<'_>) -> Class {
    let path = read.path.as_str();
    match (cx.kind_of)(path) {
        Some(Kind::Root) => return Class::Step(Step::Root),
        Some(Kind::Rules) => return Class::Step(Step::Rules),
        Some(Kind::Task) => {
            return if ends_with_path(path, cx.task_path) {
                Class::Step(Step::Task)
            } else {
                Class::Foreign
            };
        }
        None => {}
    }
    if ends_with_path(path, cx.task_path) {
        return Class::Step(Step::Task);
    }
    if let Some(p) = cx.platform {
        if contains_dir(path, p) {
            return Class::Step(Step::Platform);
        }
    }
    let named = cx.named.iter().any(|n| {
        if n.ends_with('/') {
            contains_dir(path, n)
        } else {
            ends_with_path(path, n)
        }
    });
    if named {
        Class::Step(Step::Named)
    } else {
        Class::Other
    }
}

/// Judge the reads a build made before its first write.
#[must_use]
pub fn judge(reads: &[&Read], steps: &[Step], required_named: bool, cx: &Context<'_>) -> Judgement {
    if reads.is_empty() {
        return Judgement {
            result: "no-evidence",
            note: "the session read nothing through the Read tool before its first write".to_string(),
            evidence: Vec::new(),
        };
    }
    let required: Vec<Step> = steps.iter().copied().filter(|s| *s != Step::Named || required_named).collect();
    let mut started = false;
    let mut current: usize = 0;
    let mut visited: Vec<Step> = Vec::new();
    // The read that broke the order, while it stays unmended, and the
    // step it came after.
    let mut broken: Option<&Read> = None;
    let mut broken_after: Option<Step> = None;
    // The first read of the pass in progress: a pass that opened past
    // the first step skipped it, and starting again abandons that pass.
    let mut pass_start: Option<&Read> = None;
    let mut mended: Vec<String> = Vec::new();
    let mut evidence: Vec<String> = Vec::new();
    let mut foreign: Vec<String> = Vec::new();
    let mut partial: Vec<String> = Vec::new();

    for read in reads {
        let step = match classify(read, cx) {
            Class::Other => continue,
            Class::Foreign => {
                foreign.push(read.path.clone());
                evidence.extend(read.tool_use_id.clone());
                continue;
            }
            Class::Step(s) => s,
        };
        if !started && step == Step::Task {
            continue;
        }
        started = true;
        let Some(pos) = steps.iter().position(|s| *s == step) else { continue };
        if !read.whole && matches!(step, Step::Root | Step::Rules | Step::Task) {
            partial.push(read.path.clone());
            evidence.extend(read.tool_use_id.clone());
            continue;
        }
        if pos == 0 && (broken.is_some() || current > 0) {
            // Reading the first file again starts the order over.
            if let Some(b) = broken.take().or(pass_start) {
                mended.push(b.path.clone());
            }
            visited.clear();
            current = 0;
            visited.push(step);
            pass_start = Some(read);
            continue;
        }
        if pass_start.is_none() {
            pass_start = Some(read);
        }
        if broken.is_some() {
            continue;
        }
        if pos < current {
            broken = Some(read);
            broken_after = Some(steps[current]);
            evidence.extend(read.tool_use_id.clone());
            continue;
        }
        current = pos;
        if !visited.contains(&step) {
            visited.push(step);
        }
    }

    let mut problems: Vec<String> = Vec::new();
    if let Some(b) = broken {
        // What follows a break is not counted, so what is "missing" after
        // it is the break's consequence, not news: name the break alone.
        problems.push(format!(
            "read {} after {}, out of the root's order, and did not start again from {}",
            short(&b.path),
            broken_after.map_or("a later file", Step::describe),
            steps[0].describe()
        ));
    } else {
        let missing: Vec<&str> = required.iter().filter(|s| !visited.contains(s)).map(|s| s.describe()).collect();
        if !missing.is_empty() {
            problems.push(format!("did not read, in order: {}", missing.join(", ")));
        }
    }
    if !foreign.is_empty() {
        problems.push(format!("read another task's spec: {}", foreign.iter().map(|p| short(p)).collect::<Vec<_>>().join(", ")));
    }
    if !partial.is_empty() {
        problems.push(format!("read part of a spec file, not the whole: {}", partial.iter().map(|p| short(p)).collect::<Vec<_>>().join(", ")));
    }
    if problems.is_empty() {
        let note = if mended.is_empty() {
            "read the spec in the order its root requires before writing".to_string()
        } else {
            format!(
                "read the spec in the order its root requires before writing, after starting again from {} (out of order first: {})",
                steps[0].describe(),
                mended.iter().map(|p| short(p)).collect::<Vec<_>>().join(", ")
            )
        };
        return Judgement { result: "complied", note, evidence: Vec::new() };
    }
    Judgement { result: "breached", note: problems.join("; "), evidence }
}

/// The last three segments of a path, for a note a person reads.
fn short(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    parts[parts.len().saturating_sub(3)..].join("/")
}

/// `path` ends with the relative path `rel`, at a segment boundary.
fn ends_with_path(path: &str, rel: &str) -> bool {
    let rel = rel.trim_start_matches("./");
    path == rel || path.ends_with(&format!("/{rel}"))
}

/// `path` lies under the relative directory `dir`.
fn contains_dir(path: &str, dir: &str) -> bool {
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    path.starts_with(&format!("{dir}/")) || path.contains(&format!("/{dir}/"))
}
