//! The agent held to its own words (#573): a fix that covers fewer of a kind
//! of thing than the agent found broken, and the agent's own word that it
//! acted without the operator or said what was not so.
//!
//! The operator, 2026-11-06: "the agent constantly lies". That morning, in
//! one session, the agent counted 724 damaged routines, planned "723
//! routines … the one routine with debugger lines stays as it is, because it
//! reads fine", had the fix merged, and, asked, said "I made that call on my
//! own, and the reason I gave was false". Over all history the count rule
//! raises two alerts, that one and one other where the agent's own numbers
//! disagree; every admission it raises is the agent's own sentence.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::alerts::Alerts;
use super::task::has_any;
use super::track::Track;
use crate::params::Params;

/// How long a count found broken is held against a fix that follows it.
const FOUND_HOLD_SECS: i64 = 21_600; // skill-allow: §9-const — one working stretch's reach, not a policy tunable

/// Words that make the kind before them a word that describes, not the
/// thing counted: `91 row counts` counts counts.
const DESCRIBED: [&str; 16] = [
    "count", "counts", "number", "numbers", "name", "names", "id", "ids", "size", "sizes", "type", "types", "level", "levels", "key", "keys",
]; // skill-allow: §9-const — the grammar of a count, not a policy tunable

/// A count the agent found broken: how many, when, and its sentence.
#[derive(Debug, Clone)]
struct Found {
    n: u64,
    at: DateTime<Utc>,
    sentence: String,
}

/// Each session's newest count found broken, of each kind of thing.
#[derive(Debug, Default)]
pub struct Claims {
    found: HashMap<String, HashMap<String, Found>>,
}

impl Claims {
    /// The agent's words to the operator in `t`, at `at`: a count found
    /// broken is kept; a fix of fewer of the same kind raises its alert; the
    /// first sentence that admits acting without the operator, or saying
    /// what was not so, raises its own.
    pub fn said(&mut self, t: &Track, at: DateTime<Utc>, text: &str, alerts: &mut Alerts, p: &Params) {
        let found = self.found.entry(t.session.clone()).or_default();
        let mut admitted: Option<&str> = None;
        for sentence in sentences(text) {
            let lower = sentence.to_lowercase();
            if admitted.is_none() && admits(&lower, &p.watch_admit_words) {
                admitted = Some(sentence);
            }
            if has_any(&lower, &p.watch_done_words) {
                continue;
            }
            let (broken, covers) = (has_any(&lower, &p.watch_found_words), has_any(&lower, &p.watch_cover_words));
            for (n, unit) in counts(&lower, &p.watch_units) {
                if broken && !covers {
                    found.insert(unit, Found { n, at, sentence: sentence.to_string() });
                } else if covers {
                    let short = found.get(&unit).filter(|f| n < f.n && 2 * n >= f.n && (at - f.at).num_seconds() < FOUND_HOLD_SECS);
                    if let Some(f) = short {
                        alerts.count_gap(t, at, &unit, (f.n, f.at, &f.sentence), (n, sentence));
                    }
                }
            }
        }
        if let Some(sentence) = admitted {
            alerts.admitted(t, at, sentence);
        }
    }

    /// Forget the sessions no longer followed.
    pub fn trim(&mut self, sessions: &HashSet<&String>) {
        self.found.retain(|s, _| sessions.contains(s));
    }
}

/// The sentences of `text`: its lines, its table cells, and what ends in a
/// full stop, a question or an exclamation.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for piece in text.split(['\n', '|']) {
        let mut start = 0;
        let bytes = piece.as_bytes();
        for (i, c) in piece.char_indices() {
            if matches!(c, '.' | '!' | '?') && bytes.get(i + 1).is_some_and(u8::is_ascii_whitespace) {
                out.push(piece[start..=i].trim());
                start = i + 1;
            }
        }
        out.push(piece[start..].trim());
    }
    out.retain(|s| !s.is_empty());
    out
}

/// The counts a lowercase sentence gives of the kinds of thing in `units`:
/// a number, then within two words the kind, `724 damaged routines`.
fn counts(lower: &str, units: &[String]) -> Vec<(u64, String)> {
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || "()[]{}*`'\"|:;!?/".contains(c))
        .map(|w| w.trim_end_matches(['.', ',']))
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let digits = w.chars().all(|c| c.is_ascii_digit() || c == ',') && w.starts_with(|c: char| c.is_ascii_digit());
        let Some(n) = digits.then(|| w.replace(',', "").parse::<u64>().ok()).flatten().filter(|&n| n > 0) else { continue };
        let unit = words.iter().enumerate().skip(i + 1).take(3).find_map(|(k, next)| {
            let unit = units.iter().map(|u| u.trim().to_lowercase()).find(|u| !u.is_empty() && (*next == u || *next == plural(u)))?;
            (!words.get(k + 1).is_some_and(|after| DESCRIBED.contains(after))).then_some(unit)
        });
        if let Some(unit) = unit {
            out.push((n, unit));
        }
    }
    out
}

/// `routine` as `routines`, `body` as `bodies`.
#[must_use]
pub fn plural(unit: &str) -> String {
    match unit.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{unit}s"),
    }
}

/// Whether a lowercase sentence admits, as one of `phrases` says it: each
/// phrase's parts in order, whole words, `…` any words between; a phrase
/// that starts with `!` makes a sentence that has it no admission.
fn admits(lower: &str, phrases: &[String]) -> bool {
    let phrases: Vec<String> = phrases.iter().map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect();
    if phrases.iter().filter_map(|p| p.strip_prefix('!')).any(|not| find(lower, not.trim(), 0).is_some()) {
        return false;
    }
    phrases.iter().filter(|p| !p.starts_with('!')).any(|phrase| {
        let mut from = 0;
        phrase.split('…').map(str::trim).filter(|part| !part.is_empty()).all(|part| match find(lower, part, from) {
            Some(end) => {
                from = end;
                true
            }
            None => false,
        })
    })
}

/// Where `part` ends in `text`, found from `from`: a part that starts or
/// ends with a letter or digit starts or ends a word there.
#[must_use]
pub fn find(text: &str, part: &str, from: usize) -> Option<usize> {
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let (first, last) = (part.chars().next(), part.chars().next_back());
    text.get(from..)?.match_indices(part).map(|(i, _)| from + i).find_map(|i| {
        let before = text[..i].chars().next_back();
        let after = text[i + part.len()..].chars().next();
        let starts = !word(first) || !word(before);
        let ends = !word(last) || !word(after);
        (starts && ends).then_some(i + part.len())
    })
}
