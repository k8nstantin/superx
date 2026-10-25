//! Every agent's format, and nothing else: the one place a captured line's
//! own fields are read (`raw`, in the kernel). A line goes in, typed facts
//! come out. An agent this module does not know is read as a line in a
//! shape it does not know, and counted.

pub mod claude_code;
pub mod gemini_cli;

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;

use crate::reader::facts::Read;

/// One captured line, read as its agent writes lines.
#[must_use]
pub fn read(agent: &str, line: &Value, at: DateTime<Utc>) -> Read {
    match agent {
        claude_code::AGENT => claude_code::read(line, at),
        gemini_cli::AGENT => gemini_cli::read(line, at),
        _ => Read::unknown(),
    }
}

/// A tool that runs shell commands: what it ran and printed says what was
/// committed, tested and built.
#[must_use]
pub fn is_shell(tool: &str) -> bool {
    tool == claude_code::SHELL_TOOL || tool == gemini_cli::SHELL_TOOL
}

/// The `system` lines kept as telemetry that the reader reads.
#[must_use]
pub fn system_subtypes() -> Vec<&'static str> {
    claude_code::SYSTEM_SUBTYPES.to_vec()
}

/// The `attachment` lines kept as telemetry that the reader reads.
#[must_use]
pub fn attachment_types() -> Vec<&'static str> {
    claude_code::ATTACHMENT_TYPES.to_vec()
}

/// A commit's hash as git prints it: 7 to 64 hex digits.
#[must_use]
pub fn is_hash(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
