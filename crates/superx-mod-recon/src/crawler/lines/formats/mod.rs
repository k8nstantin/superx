//! Every agent's format, and nothing else: the one place a captured line's
//! own fields are read. A line goes in, typed facts come out. Each format
//! knows the kinds of line it reads into tables, the kinds that feed no
//! table, how its lines name their fields, and which of those fields its
//! reading puts in a column. A line of a kind no format knows, of an agent
//! this module does not know, or that is not JSON is read as a line in a
//! shape it does not know, and counted.

pub mod claude_code;
pub mod claude_desktop;
pub mod gemini_cli;

use superx_kernel::types::Value;

use crate::crawler::lines::census::{covers, Shapes};
use crate::crawler::lines::facts::Read;
use crate::crawler::lines::kernel::Line;
use crate::crawler::value::text;

/// One captured line, read as its agent writes lines.
#[must_use]
pub fn read(agent: &str, line: &Line) -> Read {
    let kernel_kind = line.kernel_kind.as_deref();
    match agent {
        claude_code::AGENT => claude_code::read_kept(&line.body, kernel_kind, line.at),
        gemini_cli::AGENT => gemini_cli::read_kept(&line.body, kernel_kind, line.at),
        claude_desktop::AGENT => claude_desktop::read_kept(&line.body),
        _ => Read::unknown(),
    }
}

/// A line's kind, as its fields are counted: its type; where it holds no
/// type, the kind the kernel gave it.
#[must_use]
pub fn kind(agent: &str, line: &Value, kernel_kind: Option<&str>) -> String {
    match agent {
        claude_code::AGENT => claude_code::kind(line, kernel_kind),
        _ => text(line, &["type"]).or(kernel_kind).unwrap_or_default().to_string(),
    }
}

/// How an agent's lines name their fields.
#[must_use]
pub fn shapes(agent: &str) -> Shapes {
    match agent {
        claude_code::AGENT => claude_code::SHAPES,
        _ => Shapes::default(),
    }
}

/// Whether the agent's format puts the field at `path` of a line of `kind`
/// in a column.
#[must_use]
pub fn stored(agent: &str, kind: &str, path: &str) -> bool {
    let (every, by_kind): (&[&str], &[(&str, &[&str])]) = match agent {
        claude_code::AGENT => (claude_code::STORED_ON_EVERY_LINE, claude_code::STORED),
        gemini_cli::AGENT => (gemini_cli::STORED_ON_EVERY_LINE, gemini_cli::STORED),
        _ => (&[], &[]),
    };
    let of_kind = by_kind.iter().filter(|(k, _)| *k == kind).flat_map(|(_, paths)| paths.iter());
    every.iter().chain(of_kind).any(|declared| covers(declared, path))
}

/// A tool that runs shell commands: what it ran and printed says what was
/// committed, tested and built.
#[must_use]
pub fn is_shell(tool: &str) -> bool {
    tool == claude_code::SHELL_TOOL || tool == gemini_cli::SHELL_TOOL
}

/// A commit's hash as git prints it: 7 to 64 hex digits.
#[must_use]
pub fn is_hash(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
