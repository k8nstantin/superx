//! What the watcher digs out (#573): what costs the agents' time, or the
//! operator's, and shows nowhere while it happens. None of it is an alert:
//! each is listed with what it cost and the call it is about, for the page
//! to read back to its cause in the session's lines.
//!
//! - **Held on your approval**: a foreground call whose result came past
//!   Claude Code's command limit: a permission prompt waited on you.
//! - **Asleep in the foreground**: a command that sleeps and ran
//!   `attr_recon_watch_sleep_secs` or more: the agent deaf to you, most
//!   often polling a job of its own.
//! - **Edited over and over**: one file edited `attr_recon_watch_thrash_edits`
//!   times within `attr_recon_watch_thrash_secs`: trial and error.
//! - **Refused**: a call refused, by you, by auto mode, by a hook, by a
//!   permission rule or by the worktree the session is held in; and how
//!   often the same call was refused before.
//! - **A protected place changed**: a change to a path of
//!   `attr_recon_watch_protected` that neither your message nor the agent's
//!   words you answered named.
//! - **Context lost**: the context compacted, and the files read again
//!   within the hour after.
//! - **Read again**: the same part of a file read again with nothing
//!   changed in it since.
//! - **Flooded**: a result of `attr_recon_watch_flood_chars` characters or
//!   more, poured into the agent's context.
//! - **You stopped it**: your interrupt, and the call it was in.
//! - **You asked how it stands**: a short message holding one of
//!   `attr_recon_watch_status_words`: the agent had not told you, often
//!   while a job of its ran.
//! - **You corrected it**: a message holding one of
//!   `attr_recon_correction_words`; its cause is the agent's turn before it.
//! - **Work undone**: a `git revert`, a `git reset --hard`, files put back,
//!   or a branch's history rewritten with a force push.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use super::alerts::duration;
use super::command::{self, ITS_OWN};
use super::line::{Came, ASK_TOOLS, FILE_TOOLS};
use super::task::has_any;
use super::track::{Run, Track, FOREGROUND_LIMIT_SECS, UNSEEN_END_SECS};
use crate::params::Params;

/// How long after a compaction a file read before it, read again, counts
/// as read again for it.
const AFTER_COMPACTION_SECS: i64 = 3_600; // skill-allow: §9-const — what "after" covers on the page, not a policy tunable

/// The longest status ask: a longer message asks more than how it stands.
const STATUS_CHARS: usize = 200; // skill-allow: §9-const — a status ask is a short message, not a policy tunable

/// The tools that start a subagent: its run is its work, not a wait.
const SUBAGENT_TOOLS: [&str; 2] = ["Task", "Agent"];

/// Who refused a call, by the words Claude Code answers it with.
const REFUSALS: [(&str, &str); 8] = [
    ("doesn't want to proceed with this tool use", "you refused it"),
    ("denied by the Claude Code auto mode", "auto mode denied it"),
    ("Auto mode could not evaluate", "auto mode could not judge it"),
    ("hook error", "a hook refused it"),
    ("isolated in the worktree", "the worktree it is held in refused it"),
    ("Permission to use", "a permission rule denied it"),
    ("the publish was refused", "the artifact service refused the publish"),
    ("Publish refused", "the artifact service refused the same content again"),
];

/// What the watcher dug out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hidden {
    Held,
    Slept,
    Thrash,
    Refused,
    Protected,
    Compacted,
    Reread,
    Flood,
    Interrupted,
    Asked,
    Corrected,
    Undone,
}

impl Hidden {
    /// Its name in an id.
    fn word(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Slept => "slept",
            Self::Thrash => "thrash",
            Self::Refused => "refused",
            Self::Protected => "protected",
            Self::Compacted => "compacted",
            Self::Reread => "reread",
            Self::Flood => "flood",
            Self::Interrupted => "interrupted",
            Self::Asked => "asked",
            Self::Corrected => "corrected",
            Self::Undone => "undone",
        }
    }
}

/// One thing found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    /// The session's id, its kind and what it is about: the same at every
    /// look.
    pub id: String,
    pub session: String,
    pub name: String,
    pub kind: Hidden,
    pub at: DateTime<Utc>,
    /// Where its cause starts: the session's lines from here to `at` tell
    /// it.
    pub from: DateTime<Utc>,
    /// One sentence of the facts.
    pub what: String,
    pub tool: Option<String>,
    /// What the call worked on, or the protected place.
    pub target: Option<String>,
    pub call: Option<String>,
    /// The time it cost, in seconds.
    pub secs: Option<i64>,
    /// The context it cost, in characters.
    pub chars: Option<u64>,
    /// How many: the edits, the refusals of the same call so far, the files
    /// read again.
    pub times: u32,
}

/// What one session's findings rest on.
#[derive(Debug, Default)]
struct Seen {
    /// Each file's edits within the span, and the finding they make.
    edits: HashMap<String, (VecDeque<DateTime<Utc>>, Option<String>)>,
    refusals: HashMap<(String, String), u32>,
    /// The last read of each part of each file, and of each file.
    reads: HashMap<(String, String), DateTime<Utc>>,
    read_files: HashMap<String, DateTime<Utc>>,
    /// The last change of each file.
    changed: HashMap<String, DateTime<Utc>>,
    /// The newest compaction: its finding, when, and the files read before
    /// it not read again yet.
    compacted: Option<(String, DateTime<Utc>, HashSet<String>)>,
    /// The protected places changed, each under the message it answered.
    protected: HashSet<(String, String)>,
}

/// Everything found in the window.
#[derive(Debug, Default)]
pub struct Finds {
    pub list: Vec<Found>,
    seen: HashMap<String, Seen>,
}

impl Finds {
    fn add(&mut self, f: Found) {
        if !self.list.iter().rev().any(|x| x.id == f.id) {
            self.list.push(f);
        }
    }

    fn get(&mut self, id: &str) -> Option<&mut Found> {
        self.list.iter_mut().rev().find(|x| x.id == id)
    }

    /// A run of `t` that ended, with its result's text.
    pub fn ended(&mut self, t: &Track, run: &Run, text: &str, p: &Params) {
        let Some(end) = run.end else { return };
        let secs = run.secs(end);
        let base = |kind: Hidden, what: String| Found {
            id: format!("{}/{}/{}", t.session, kind.word(), run.call),
            session: t.session.clone(),
            name: t.name.clone(),
            kind,
            at: run.start,
            from: run.start,
            what,
            tool: Some(run.tool.clone()),
            target: Some(run.target.clone()),
            call: Some(run.call.clone()),
            secs: None,
            chars: None,
            times: 1,
        };
        let refused = run.failed.then(|| REFUSALS.iter().find(|(said, _)| text.contains(said)).map(|(_, who)| *who)).flatten();
        let named = describe(run);
        let tool = run.tool.as_str();
        if !run.background && !ASK_TOOLS.contains(&tool) && !SUBAGENT_TOOLS.contains(&tool) && secs > FOREGROUND_LIMIT_SECS {
            let how = if refused == Some(REFUSALS[0].1) { ", then you refused it" } else { "" };
            self.add(Found { secs: Some(secs), ..base(Hidden::Held, format!("{named} waited {} on your approval{how}", duration(secs))) });
        }
        if tool == "Bash" && !run.background && secs >= secs_of(p.watch_sleep_secs) && secs <= FOREGROUND_LIMIT_SECS && command::sleeps(&run.what) {
            let job = t.running().find(|r| r.background && end - r.start < TimeDelta::seconds(UNSEEN_END_SECS)).map(|r| r.label(&p.watch_jobs));
            let while_ = job.map_or(String::new(), |j| format!(" while its job {j} ran"));
            self.add(Found { secs: Some(secs), ..base(Hidden::Slept, format!("slept {} in the foreground{while_}, deaf to you", duration(secs))) });
        }
        if let Some(who) = refused {
            let seen = self.seen.entry(t.session.clone()).or_default();
            let n = seen.refusals.entry((run.tool.clone(), run.target.clone())).or_insert(0);
            *n += 1;
            let times = *n;
            let again = if times > 1 { format!(", refused {times} times now") } else { String::new() };
            self.add(Found { times, ..base(Hidden::Refused, format!("{named}: {who}{again}")) });
        }
        let chars = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        if chars >= p.watch_flood_chars {
            self.add(Found { chars: Some(chars), ..base(Hidden::Flood, format!("{named} poured {chars} characters into its context")) });
        }
        if run.failed {
            return;
        }
        if tool == "Read" {
            let seen = self.seen.entry(t.session.clone()).or_default();
            let path = run.target.clone();
            let key = (path.clone(), run.part.clone());
            let again = seen.reads.get(&key).is_some_and(|last| seen.changed.get(&path).is_none_or(|c| c < last));
            seen.reads.insert(key, end);
            seen.read_files.insert(path.clone(), end);
            let mut after = None;
            if let Some((id, at, before)) = &mut seen.compacted {
                if end - *at <= TimeDelta::seconds(AFTER_COMPACTION_SECS) && before.remove(&path) {
                    after = Some(id.clone());
                }
            }
            if let Some(f) = after.and_then(|id| self.get(&id)) {
                f.times += 1;
                f.what = format!("its context compacted; {} files it had read were read again within the hour", f.times);
            }
            if again {
                self.add(Found { chars: Some(chars), ..base(Hidden::Reread, format!("read {} again, unchanged since it last read it", run.target)) });
            }
            return;
        }
        if tool == "Bash" {
            let rewrites = command::pushes(&run.what).into_iter().filter(|p| p.force).map(|p| p.branch).collect::<Vec<_>>();
            let undone = command::undoes(&run.what).map(|u| format!("undid work: {u}")).or_else(|| {
                rewrites.first().map(|b| format!("rewrote the history of {} with a force push", if b.is_empty() { "its branch" } else { b }))
            });
            if let Some(what) = undone {
                self.add(base(Hidden::Undone, what));
            }
        }
        let seen = self.seen.entry(t.session.clone()).or_default();
        // What the call changed: a file tool's path, a command's paths.
        let changed: Vec<String> = if FILE_TOOLS.contains(&tool) {
            run.path.iter().cloned().collect()
        } else if tool == "Bash" {
            command::changed(&run.what)
        } else {
            Vec::new()
        };
        let changed: Vec<String> = changed.into_iter().filter(|c| !ITS_OWN.iter().any(|o| c.contains(o))).collect();
        for c in &changed {
            seen.changed.insert(c.clone(), end);
        }
        if FILE_TOOLS.contains(&tool) {
            if let Some(path) = run.path.clone() {
                self.edited(t, run, &path, end, p);
            }
        }
        // One change names one place: the first of the operator's it holds.
        for place in p.watch_protected.iter().filter_map(|e| Place::of(e)) {
            let Some(hit) = changed.iter().find(|c| c.contains(&place.pattern)) else { continue };
            if place.words.iter().any(|w| t.asked.contains(w.as_str())) {
                break;
            }
            let seen = self.seen.entry(t.session.clone()).or_default();
            if seen.protected.insert((place.pattern.clone(), t.asked.clone())) {
                self.add(Found {
                    target: Some(place.pattern.clone()),
                    ..base(Hidden::Protected, format!("changed {hit}, a protected place ({}), and nothing you said named it", place.words.join(", ")))
                });
            }
            break;
        }
    }

    /// A file edited: a burst of edits within the span is one finding,
    /// growing while it lasts.
    fn edited(&mut self, t: &Track, run: &Run, path: &str, end: DateTime<Utc>, p: &Params) {
        let span = TimeDelta::seconds(secs_of(p.watch_thrash_secs));
        let seen = self.seen.entry(t.session.clone()).or_default();
        let (times, open) = seen.edits.entry(path.to_string()).or_default();
        times.push_back(end);
        while times.front().is_some_and(|f| end - *f > span) {
            times.pop_front();
        }
        let count = u32::try_from(times.len()).unwrap_or(u32::MAX);
        if u64::from(count) < p.watch_thrash_edits {
            *open = None;
            return;
        }
        let first = *times.front().unwrap_or(&end);
        match open.clone() {
            Some(id) => {
                if let Some(f) = self.get(&id) {
                    f.times += 1;
                    let secs = (end - f.from).num_seconds();
                    f.secs = Some(secs);
                    f.what = format!("edited {path} {} times in {}", f.times, duration(secs));
                }
            }
            None => {
                let id = format!("{}/thrash/{}", t.session, run.call);
                *open = Some(id.clone());
                let secs = (end - first).num_seconds();
                self.add(Found {
                    id,
                    session: t.session.clone(),
                    name: t.name.clone(),
                    kind: Hidden::Thrash,
                    at: run.start,
                    from: first,
                    what: format!("edited {path} {count} times in {}", duration(secs)),
                    tool: Some(run.tool.clone()),
                    target: Some(path.to_string()),
                    call: Some(run.call.clone()),
                    secs: Some(secs),
                    chars: None,
                    times: count,
                });
            }
        }
    }

    /// A message of the operator's to `t`, at `at`; `previous` is when the
    /// one before it came.
    pub fn prompt(&mut self, t: &Track, at: DateTime<Utc>, came: Came, text: &str, previous: Option<DateTime<Utc>>, p: &Params) {
        let base = |kind: Hidden, what: String, from: DateTime<Utc>, run: Option<&Run>| Found {
            id: format!("{}/{}/{}", t.session, kind.word(), at.to_rfc3339()),
            session: t.session.clone(),
            name: t.name.clone(),
            kind,
            at,
            from,
            what,
            tool: run.map(|r| r.tool.clone()),
            target: run.map(|r| r.target.clone()),
            call: run.map(|r| r.call.clone()),
            secs: run.map(|r| r.secs(at)),
            chars: None,
            times: 1,
        };
        if came == Came::Interrupt {
            let fore = t.running().filter(|r| !r.background).last().or_else(|| t.runs.last());
            let what = fore.map_or_else(|| "you stopped it".to_string(), |r| format!("you stopped it in {}", describe(r)));
            self.add(base(Hidden::Interrupted, what, fore.map_or(at, |r| r.start.min(at)), fore));
            return;
        }
        let lower = text.to_lowercase();
        let head: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
        if has_any(&lower, &p.correction_words) {
            self.add(base(Hidden::Corrected, format!("you corrected it: \"{head}\""), previous.unwrap_or(at).min(at), None));
        }
        if text.len() > STATUS_CHARS || !has_any(&lower, &p.watch_status_words) {
            return;
        }
        let job = t.running().find(|r| r.background && at - r.start < TimeDelta::seconds(UNSEEN_END_SECS));
        let what = match job {
            Some(job) => format!("you asked how it stands while its job {} ran, {} in: \"{head}\"", job.label(&p.watch_jobs), duration(job.secs(at))),
            None => format!("you asked how it stands: \"{head}\""),
        };
        self.add(base(Hidden::Asked, what, job.map_or(previous.unwrap_or(at).min(at), |j| j.start), job));
    }

    /// The context of `t` compacted at `at`.
    pub fn compacted(&mut self, t: &Track, at: DateTime<Utc>) {
        let id = format!("{}/compacted/{}", t.session, at.to_rfc3339());
        let seen = self.seen.entry(t.session.clone()).or_default();
        let before: HashSet<String> = seen.read_files.keys().cloned().collect();
        seen.compacted = Some((id.clone(), at, before));
        self.add(Found {
            id,
            session: t.session.clone(),
            name: t.name.clone(),
            kind: Hidden::Compacted,
            at,
            from: at,
            what: "its context compacted; no file it had read was read again within the hour".to_string(),
            tool: None,
            target: None,
            call: None,
            secs: None,
            chars: None,
            times: 0,
        });
    }

    /// Forget what was found before `from`, and the sessions no longer
    /// followed.
    pub fn trim(&mut self, from: DateTime<Utc>, sessions: &HashSet<&String>) {
        self.list.retain(|f| f.at >= from);
        self.seen.retain(|s, _| sessions.contains(s));
    }

    /// Everything found, newest first.
    #[must_use]
    pub fn newest_first(&self) -> Vec<Found> {
        let mut v = self.list.clone();
        v.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        v
    }
}

/// A protected place: a path holding `pattern`, named by any of `words`.
struct Place {
    pattern: String,
    words: Vec<String>,
}

impl Place {
    /// `pattern = words`, as the parameter holds it.
    fn of(entry: &str) -> Option<Self> {
        let (pattern, words) = entry.split_once('=')?;
        let pattern = pattern.trim().to_string();
        let words: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
        (!pattern.is_empty() && !words.is_empty()).then_some(Self { pattern, words })
    }
}

/// A call as a sentence names it: its tool and what it worked on.
fn describe(run: &Run) -> String {
    if run.target.is_empty() {
        run.tool.clone()
    } else {
        format!("{} {}", run.tool, run.target)
    }
}

fn secs_of(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX / 2)
}
