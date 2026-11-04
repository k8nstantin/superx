//! What the watcher says (#566, step 14): a report, its kind, its level and
//! the words behind it, whole. Nothing is cut: the page clamps what it shows.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

/// How loud a report is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Alert,
}

/// What a report is about: one of the kinds steps 8 to 12 name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Quiet,
    BehindAJob,
    Stalled,
    QuestionUnanswered,
    StatusAsked,
    WaitingOnYou,
    ActedOnQuestion,
    UnorderedAction,
    UnorderedWork,
    StopNotHonoured,
    RepeatedOrder,
    LongEstimate,
    LongRun,
    Struggling,
    MemoryWritten,
    Compacted,
    Frustration,
    ForbiddenFile,
    OutsideOwns,
    SpecVersionChanged,
}

impl Kind {
    /// The kind's word, as a report's id and the page carry it.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::BehindAJob => "behind_a_job",
            Self::Stalled => "stalled",
            Self::QuestionUnanswered => "question_unanswered",
            Self::StatusAsked => "status_asked",
            Self::WaitingOnYou => "waiting_on_you",
            Self::ActedOnQuestion => "acted_on_question",
            Self::UnorderedAction => "unordered_action",
            Self::UnorderedWork => "unordered_work",
            Self::StopNotHonoured => "stop_not_honoured",
            Self::RepeatedOrder => "repeated_order",
            Self::LongEstimate => "long_estimate",
            Self::LongRun => "long_run",
            Self::Struggling => "struggling",
            Self::MemoryWritten => "memory_written",
            Self::Compacted => "compacted",
            Self::Frustration => "frustration",
            Self::ForbiddenFile => "forbidden_file",
            Self::OutsideOwns => "outside_owns",
            Self::SpecVersionChanged => "spec_version_changed",
        }
    }

    /// The level its step gives it.
    #[must_use]
    pub fn level(self) -> Level {
        match self {
            Self::WaitingOnYou | Self::Compacted => Level::Info,
            Self::Quiet
            | Self::BehindAJob
            | Self::LongEstimate
            | Self::LongRun
            | Self::Struggling
            | Self::MemoryWritten
            | Self::SpecVersionChanged => Level::Warn,
            Self::Stalled
            | Self::QuestionUnanswered
            | Self::StatusAsked
            | Self::ActedOnQuestion
            | Self::UnorderedAction
            | Self::UnorderedWork
            | Self::StopNotHonoured
            | Self::RepeatedOrder
            | Self::Frustration
            | Self::ForbiddenFile
            | Self::OutsideOwns => Level::Alert,
        }
    }
}

/// Whose words a report quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Whose {
    Operator,
    Agent,
    Call,
}

/// Words behind a report, whole, with whose they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Words {
    pub whose: Whose,
    pub text: String,
}

impl Words {
    #[must_use]
    pub fn operator(text: &str) -> Self {
        Self { whose: Whose::Operator, text: text.to_string() }
    }

    #[must_use]
    pub fn agent(text: &str) -> Self {
        Self { whose: Whose::Agent, text: text.to_string() }
    }

    #[must_use]
    pub fn call(text: &str) -> Self {
        Self { whose: Whose::Call, text: text.to_string() }
    }
}

/// One report: its id, the session's id, its kind and its time joined, the
/// same at every look and after a restart that reads its lines back, so no
/// report is made twice and the page tells a new one by its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub id: String,
    pub session: String,
    pub kind: Kind,
    pub level: Level,
    pub at: DateTime<Utc>,
    pub what: String,
    pub words: Vec<Words>,
}

impl Report {
    #[must_use]
    pub fn new(session: &str, kind: Kind, at: DateTime<Utc>, what: String, words: Vec<Words>) -> Self {
        Self {
            id: format!("{session}/{}/{}", kind.word(), at.to_rfc3339_opts(SecondsFormat::Micros, true)),
            session: session.to_string(),
            kind,
            level: kind.level(),
            at,
            what,
            words,
        }
    }
}
