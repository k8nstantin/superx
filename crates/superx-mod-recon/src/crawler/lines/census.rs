//! Every field the agents write (#537): each agent, kind of line and path a
//! line shows is a `line_field` row the first time it is seen, saying
//! whether a column holds it, and one row more each time that changes.
//!
//! A path is the keys from the line's top to the field, joined by a dot; a
//! list's items take the list's path with `[]` after it. A key that is data
//! rather than a name is written `<k>`: a key that is not a name (a name is
//! made of ASCII letters, digits, `_`, `$` and `-` and begins with a letter,
//! `_` or `$`), a key that is a UUID, and every key of a map the agent's
//! format names as keyed by data. Each format names its own shapes
//! ([`Shapes`]): Claude Code names a block of `message.content` by its type,
//! a tool call by its tool, and a tool result and `toolUseResult` by the
//! tool of the call they answer, `?` where the crawler holds no such call.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use superx_kernel::types::{Uuid, Value};

use crate::crawler::lines::formats;
use crate::crawler::lines::kernel::Line;
use crate::crawler::store::{Batch, Row};
use crate::crawler::value::{list, text};

/// What a key that is data is written as.
pub const DATA_KEY: &str = "<k>";

/// What a tool the crawler does not know is named as.
pub const UNKNOWN_TOOL: &str = "?";

/// How an agent's lines name their fields, beside the rules every line
/// follows.
#[derive(Debug, Clone, Copy, Default)]
pub struct Shapes {
    /// The list whose items are content blocks, each named by its type: a
    /// tool call by its tool, a tool result by the tool of its call.
    pub content: Option<&'static str>,
    /// The field, at the line's top, named by the tool of the one result
    /// the line carries.
    pub result: Option<&'static str>,
    /// The maps whose keys are data, by their path.
    pub keyed_by_data: &'static [&'static str],
}

/// The paths of every field of a line. `call_tool` names the tool of a
/// call the crawler holds, by the call's id.
#[must_use]
pub fn paths(line: &Value, shapes: &Shapes, call_tool: &dyn Fn(&str) -> Option<String>) -> BTreeSet<String> {
    let result_tool = shapes.result.map(|_| result_tool(line, shapes, call_tool)).unwrap_or_default();
    let walk = Walk { shapes, call_tool, result_tool };
    let mut out = BTreeSet::new();
    walk.value(line, "", &mut out);
    out
}

struct Walk<'a> {
    shapes: &'a Shapes,
    call_tool: &'a dyn Fn(&str) -> Option<String>,
    result_tool: String,
}

impl Walk<'_> {
    fn value(&self, v: &Value, path: &str, out: &mut BTreeSet<String>) {
        match v {
            Value::Object(o) => {
                let data = self.shapes.keyed_by_data.contains(&path);
                for (key, child) in o.iter() {
                    let at = if path.is_empty() && Some(key.as_str()) == self.shapes.result {
                        format!("{key}[{}]", self.result_tool)
                    } else {
                        let segment = if data || !is_name(key) || is_uuid(key) { DATA_KEY } else { key.as_str() };
                        if path.is_empty() {
                            segment.to_string()
                        } else {
                            format!("{path}.{segment}")
                        }
                    };
                    out.insert(at.clone());
                    self.value(child, &at, out);
                }
            }
            Value::Array(items) => {
                let blocks = Some(path) == self.shapes.content;
                for item in items.iter() {
                    let at = match blocks.then(|| self.block(item)).flatten() {
                        Some(name) => format!("{path}[{name}]"),
                        None => format!("{path}[]"),
                    };
                    out.insert(at.clone());
                    self.value(item, &at, out);
                }
            }
            _ => {}
        }
    }

    /// A content block's name: its type; a tool call's, its tool; a tool
    /// result's, the tool of its call. None for a block with no type.
    fn block(&self, item: &Value) -> Option<String> {
        let kind = text(item, &["type"])?;
        Some(match kind {
            "tool_use" => format!("tool_use:{}", text(item, &["name"]).unwrap_or(UNKNOWN_TOOL)),
            "tool_result" => format!("tool_result:{}", self.tool_of(item)),
            other => other.to_string(),
        })
    }

    fn tool_of(&self, result: &Value) -> String {
        text(result, &["tool_use_id"]).and_then(|id| (self.call_tool)(id)).unwrap_or_else(|| UNKNOWN_TOOL.to_string())
    }
}

/// The tool of the one result a line carries; `?` when it carries several,
/// or its call is not one the crawler holds.
fn result_tool(line: &Value, shapes: &Shapes, call_tool: &dyn Fn(&str) -> Option<String>) -> String {
    let results: Vec<&Value> = shapes
        .content
        .map(|content| content.split('.').collect::<Vec<_>>())
        .map(|path| list(line, &path).iter().filter(|b| text(b, &["type"]) == Some("tool_result")).collect())
        .unwrap_or_default();
    match results.as_slice() {
        [one] => text(one, &["tool_use_id"]).and_then(call_tool).unwrap_or_else(|| UNKNOWN_TOOL.to_string()),
        _ => UNKNOWN_TOOL.to_string(),
    }
}

/// A key that names a field: ASCII letters, digits, `_`, `$` and `-`,
/// beginning with a letter, `_` or `$`.
#[must_use]
pub fn is_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '-'))
}

/// A key that is a UUID, in its canonical form.
#[must_use]
pub fn is_uuid(key: &str) -> bool {
    key.len() == 36 && uuid::Uuid::try_parse(key).is_ok()
}

/// Whether a declared path covers `path`: `*` stands for any tool's name
/// within brackets, and a declared path ending in `.**` is held whole,
/// standing for itself and every path under it.
#[must_use]
pub fn covers(declared: &str, path: &str) -> bool {
    match declared.strip_suffix(".**") {
        Some(whole) => {
            glob(whole, path)
                || path
                    .char_indices()
                    .filter(|(_, c)| matches!(c, '.' | '['))
                    .any(|(i, _)| glob(whole, &path[..i]))
        }
        None => glob(declared, path),
    }
}

/// `pattern` matches `text`, a `*` matching any run of characters but `]`.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((before, after)) => {
            let Some(rest) = text.strip_prefix(before) else { return false };
            rest.char_indices()
                .take_while(|(_, c)| *c != ']')
                .map(|(i, _)| i)
                .chain(std::iter::once(rest.find(']').unwrap_or(rest.len())))
                .any(|i| glob(after, &rest[i..]))
        }
    }
}

/// What is kept of each field, by its agent and kind of line, then its path.
type ByKind<T> = HashMap<(String, String), HashMap<String, T>>;

/// A field's newest `line_field` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub stored: bool,
    pub first_at: DateTime<Utc>,
    pub first_line: Uuid,
}

/// The newest `line_field` row of every field, by agent and kind, then
/// path: read from the database at the crawler's first pass, and kept as
/// rows are added.
#[derive(Debug, Default)]
pub struct Fields {
    by_kind: ByKind<Field>,
}

/// What one pass saw of the fields: whether a column holds each, and how
/// many it wrote a first row for.
#[derive(Debug, Default)]
pub struct Seen {
    by_kind: ByKind<bool>,
    new: i64,
}

impl Seen {
    /// The fields the pass's lines showed.
    #[must_use]
    pub fn seen(&self) -> i64 {
        count(self.by_kind.values().map(HashMap::len).sum())
    }

    /// Those it wrote a first `line_field` row for.
    #[must_use]
    pub fn new_fields(&self) -> i64 {
        self.new
    }

    /// Those no column holds.
    #[must_use]
    pub fn not_stored(&self) -> i64 {
        count(self.by_kind.values().flat_map(HashMap::values).filter(|stored| !**stored).count())
    }
}

impl Fields {
    /// Fields as the database holds them: each one's newest row.
    #[must_use]
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String, String, Field, DateTime<Utc>)>) -> Self {
        let mut newest: ByKind<(Field, DateTime<Utc>)> = HashMap::new();
        for (agent, kind, path, field, valid_from) in rows {
            let paths = newest.entry((agent, kind)).or_default();
            match paths.get(&path) {
                Some((_, seen)) if *seen >= valid_from => {}
                _ => {
                    paths.insert(path, (field, valid_from));
                }
            }
        }
        Self {
            by_kind: newest
                .into_iter()
                .map(|(k, paths)| (k, paths.into_iter().map(|(p, (f, _))| (p, f)).collect()))
                .collect(),
        }
    }

    /// One line's fields: a first row for each field of its agent and kind
    /// not seen before, and a row more for one whose `stored` changed.
    /// Whether a column holds a field is the agent's format's to say, once
    /// a pass.
    pub fn see(&mut self, seen: &mut Seen, (agent, kind): (&str, &str), line: &Line, paths: BTreeSet<String>, batch: &mut Batch) {
        let key = (agent.to_string(), kind.to_string());
        let held = self.by_kind.entry(key.clone()).or_default();
        let saw = seen.by_kind.entry(key).or_default();
        for path in paths {
            if saw.contains_key(&path) {
                continue;
            }
            let stored = formats::stored(agent, kind, &path);
            let field = match held.get(&path) {
                Some(field) if field.stored == stored => None,
                Some(field) => Some(Field { stored, ..field.clone() }),
                None => {
                    seen.new += 1;
                    Some(Field { stored, first_at: line.at, first_line: line.id })
                }
            };
            if let Some(field) = field {
                batch.create(
                    "line_field",
                    Row::new()
                        .text("agent", agent)
                        .text("kind", kind)
                        .text("path", path.clone())
                        .time("first_at", field.first_at)
                        .uuid("first_line", field.first_line)
                        .flag("stored", field.stored),
                );
                held.insert(path.clone(), field);
            }
            saw.insert(path, stored);
        }
    }
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use superx_kernel::message::line_value_from_json;

    use super::*;
    use crate::crawler::lines::formats::claude_code;

    fn paths_of(json: &serde_json::Value, shapes: &Shapes) -> BTreeSet<String> {
        let held = |id: &str| (id == "toolu_held").then(|| "Bash".to_string());
        paths(&line_value_from_json(json), shapes, &held)
    }

    #[test]
    fn a_path_names_content_blocks_by_their_type_a_tool_call_and_its_result_and_tool_use_result_by_the_tools_name_a_lists_items_by_brackets_and_a_result_whose_call_the_crawler_does_not_hold_by_a_question_mark() {
        let asked = paths_of(
            &serde_json::json!({ "type": "assistant", "message": { "content": [
                { "type": "text", "text": "a" },
                { "type": "tool_use", "id": "toolu_1", "name": "Edit", "input": { "file_path": "/a" } }
            ] } }),
            &claude_code::SHAPES,
        );
        for path in ["message.content", "message.content[text]", "message.content[text].text", "message.content[tool_use:Edit].input.file_path"] {
            assert!(asked.contains(path), "{path}: {asked:?}");
        }
        let answered = paths_of(
            &serde_json::json!({ "type": "user",
                "message": { "content": [{ "type": "tool_result", "tool_use_id": "toolu_held", "content": "ok" }] },
                "toolUseResult": { "stdout": "ok" }, "sandboxViolations": [{ "host": "h" }] }),
            &claude_code::SHAPES,
        );
        for path in [
            "message.content[tool_result:Bash].tool_use_id",
            "toolUseResult[Bash]",
            "toolUseResult[Bash].stdout",
            "sandboxViolations[]",
            "sandboxViolations[].host",
        ] {
            assert!(answered.contains(path), "{path}: {answered:?}");
        }
        assert!(!answered.contains("toolUseResult"), "the field itself is named by its tool: {answered:?}");
        let unheld = paths_of(
            &serde_json::json!({ "type": "user",
                "message": { "content": [{ "type": "tool_result", "tool_use_id": "toolu_other", "content": "ok" }] },
                "toolUseResult": "Error" }),
            &claude_code::SHAPES,
        );
        assert!(unheld.contains("message.content[tool_result:?].content") && unheld.contains("toolUseResult[?]"), "{unheld:?}");
        let gemini = paths_of(&serde_json::json!({ "type": "gemini", "toolCalls": [{ "id": "c", "args": { "command": "ls" } }] }), &Shapes::default());
        assert_eq!(
            gemini.into_iter().collect::<Vec<_>>(),
            ["toolCalls", "toolCalls[]", "toolCalls[].args", "toolCalls[].args.command", "toolCalls[].id", "type"]
        );
    }

    #[test]
    fn a_key_that_is_not_a_name_a_uuid_and_each_key_of_each_map_step_2_names_is_written_k_and_a_name_elsewhere_is_not() {
        let seen = paths_of(
            &serde_json::json!({
                "type": "file-history-snapshot",
                "plain_name-2$": { "has space": 1, "1st": 2, "a.b": 3, "0193a5b0-7e6f-7a1c-9d2e-4f5a6b7c8d9e": 4, "kept": 5 },
                "snapshot": { "trackedFileBackups": { "Makefile": { "version": 1 } } },
                "modelUsage": { "claude-opus-5-5": { "inputTokens": 1 } },
                "artifacts": { "page": 1 },
                "wireToolInputs": { "Bash": 1 },
                "wireIngestContext": { "context": 1 },
                "toolUseResult": { "answers": { "Which": "A" }, "annotations": { "note": "n" } },
                "message": { "content": [
                    { "type": "tool_use", "id": "t", "name": "Artifact", "input": { "files": { "readme": "x" } } },
                    { "type": "tool_result", "tool_use_id": "t" }
                ] }
            }),
            &claude_code::SHAPES,
        );
        let held = |id: &str| (id == "t").then(|| "AskUserQuestion".to_string());
        let asked = paths(
            &line_value_from_json(&serde_json::json!({
                "message": { "content": [{ "type": "tool_result", "tool_use_id": "t" }] },
                "toolUseResult": { "answers": { "Which": "A" }, "annotations": { "note": "n" }, "plain": { "name": 1 } }
            })),
            &claude_code::SHAPES,
            &held,
        );
        for path in [
            "plain_name-2$.<k>",
            "plain_name-2$.kept",
            "snapshot.trackedFileBackups.<k>",
            "snapshot.trackedFileBackups.<k>.version",
            "modelUsage.<k>",
            "modelUsage.<k>.inputTokens",
            "artifacts.<k>",
            "wireToolInputs.<k>",
            "wireIngestContext.<k>",
            "message.content[tool_use:Artifact].input.files.<k>",
        ] {
            assert!(seen.contains(path), "{path}: {seen:?}");
        }
        for key in ["has space", "1st", "a.b", "0193a5b0", "Makefile", "claude-opus", "page", "Bash", "context", "readme"] {
            assert!(!seen.iter().any(|p| p.contains(key)), "{key} is data: {seen:?}");
        }
        for path in [
            "toolUseResult[AskUserQuestion].answers.<k>",
            "toolUseResult[AskUserQuestion].annotations.<k>",
            "toolUseResult[AskUserQuestion].plain.name",
        ] {
            assert!(asked.contains(path), "{path}: {asked:?}");
        }
        assert!(is_name("_x$-1") && is_name("$set") && !is_name("-x") && !is_name("") && !is_name("é"));
    }
}
