//! The operator's words (#566, step 6): a text's words, a list's word or
//! phrase in them, and what a message of the operator's is.

use serde::Serialize;

use crate::params::Params;

/// What a message of the operator's is, the first that holds of these, in
/// this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// An interrupt, or a message holding a word of attr_recon_stop_words.
    Stop,
    /// A message holding a word or phrase of attr_recon_status_words.
    StatusAsk,
    /// One ending with `?`, or whose first word is of
    /// attr_recon_question_words.
    Question,
    /// One whose first word or phrase is of attr_recon_yes_words.
    Approval,
    /// Any other.
    Order,
}

/// A text's words: its runs of letters, digits, apostrophes and hyphens,
/// lowercased. A typographic apostrophe is read as `'`.
#[must_use]
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        let c = if c == '\u{2019}' { '\'' } else { c };
        if c.is_alphanumeric() || c == '\'' || c == '-' {
            word.extend(c.to_lowercase());
        } else if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Whether `text` holds `entry`: a word of it, or a phrase, its words in a
/// row.
#[must_use]
pub fn holds(text: &[String], entry: &str) -> bool {
    let phrase = words(entry);
    !phrase.is_empty() && text.windows(phrase.len()).any(|w| w == phrase.as_slice())
}

/// The entries of `list` that `text` holds, in the list's order.
#[must_use]
pub fn held<'a>(text: &[String], list: &'a [String]) -> Vec<&'a str> {
    list.iter().filter(|e| holds(text, e)).map(String::as_str).collect()
}

/// Whether `text` begins with an entry of `list`, a word or a phrase.
#[must_use]
pub fn begins_with(text: &[String], list: &[String]) -> bool {
    list.iter().any(|e| {
        let phrase = words(e);
        !phrase.is_empty() && text.starts_with(&phrase)
    })
}

/// Whether one of `text`'s words starts with an entry of `list`.
#[must_use]
pub fn starts_a_word(text: &[String], list: &[String]) -> bool {
    let entries: Vec<String> = list.iter().map(|e| e.to_lowercase()).filter(|e| !e.is_empty()).collect();
    text.iter().any(|w| entries.iter().any(|e| w.starts_with(e.as_str())))
}

/// What a typed or queued message is; an interrupt is a stop whatever it
/// says.
#[must_use]
pub fn class(text: &str, interrupt: bool, params: &Params) -> Class {
    let w = words(text);
    if interrupt || !held(&w, &params.stop_words).is_empty() {
        Class::Stop
    } else if !held(&w, &params.status_words).is_empty() {
        Class::StatusAsk
    } else if text.trim_end().ends_with('?') || w.first().is_some_and(|first| params.question_words.contains(first)) {
        Class::Question
    } else if begins_with(&w, &params.yes_words) {
        Class::Approval
    } else {
        Class::Order
    }
}

/// Whether a message corrects the agent: it holds a word or phrase of
/// attr_recon_correction_words.
#[must_use]
pub fn corrects(text: &str, params: &Params) -> bool {
    !held(&words(text), &params.correction_words).is_empty()
}

/// Whether a message escalates: one of its words starts with an entry of
/// attr_recon_escalation_words.
#[must_use]
pub fn escalates(text: &str, params: &Params) -> bool {
    starts_a_word(&words(text), &params.escalation_words)
}
