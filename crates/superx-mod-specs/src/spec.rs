//! A spec file, parsed: what kind it is, which task it is, and the parts
//! of it a run can be checked against.
//!
//! FORMAT KNOWLEDGE lives here and nowhere else, the way an adapter holds
//! an agent's transcript format. The format is the operator's build spec
//! (gryphon-data-lake `kn-specs/`, read 2026-10-13):
//!
//! - `root.xml`, document element `<root>`: `<identity><platform path>`
//!   names the platform code; `<method>` holds the steps every task is
//!   built by (`read`, `ownership`, `gates`, …), one element each.
//! - `rules.xml`, document element `<rules>`: one `<rule id>` per rule,
//!   its words in `<text>`.
//! - one task file per task, document element `<task jira name
//!   version>`: `<reflects>` and `<reads>` name files by `<file path>`;
//!   `<owns>`, `<forbid>`, `<agent-decides>`, `<start>`, `<done>`; and
//!   `<gates>` holds one `<gate id>` per gate.
//!
//! A file of any other shape is not a spec file, and parses to nothing.

use roxmltree::{Document, Node};

/// The three kinds of spec file, by document element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Root,
    Rules,
    Task,
}

impl Kind {
    /// The name the schema stores (`spec_doc.kind`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Root => "root",
            Kind::Rules => "rules",
            Kind::Task => "task",
        }
    }
}

/// One checkable part of a spec file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    /// `method` (a root's method step), `rule`, `reflects`, `reads`,
    /// `owns`, `forbid`, `agent-decides`, `start`, `done` or `gate`.
    pub kind: &'static str,
    /// The clause's own name where the file gives one: a method step's
    /// element name, a rule's or a gate's id, a named file's path.
    pub ident: Option<String>,
    /// Its words, whitespace collapsed.
    pub text: String,
}

/// A spec file, as far as a check needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecDoc {
    pub kind: Kind,
    /// A task's Jira key (`jira`).
    pub key: Option<String>,
    /// A task's name (`name`).
    pub name: Option<String>,
    /// The file's `version` attribute: a task's file version, the spec
    /// version of a root or a rules file.
    pub version: Option<i64>,
    /// A root's platform path (`<identity><platform path>`).
    pub platform: Option<String>,
    /// The files a task names: every `<file path>` of its `<reflects>`
    /// and `<reads>`.
    pub named: Vec<String>,
    /// The paths a task's `<owns>` names: files, directories, globs.
    pub owns: Vec<String>,
    /// The paths a task's `<forbid>` names.
    pub forbid: Vec<String>,
    /// A task's gates: id, kind, commands, expectation.
    pub gates: Vec<crate::gates::Gate>,
    pub clauses: Vec<Clause>,
}

impl SpecDoc {
    /// The text of a root's method step, e.g. `read`.
    #[must_use]
    pub fn method_step(&self, step: &str) -> Option<&str> {
        self.clauses
            .iter()
            .find(|c| c.kind == "method" && c.ident.as_deref() == Some(step))
            .map(|c| c.text.as_str())
    }
}

/// Parse a file's content. `None` when it is not XML or not a spec file.
#[must_use]
pub fn parse(content: &str) -> Option<SpecDoc> {
    let doc = Document::parse(content).ok()?;
    let top = doc.root_element();
    let kind = match top.tag_name().name() {
        "root" => Kind::Root,
        "rules" => Kind::Rules,
        "task" => Kind::Task,
        _ => return None,
    };
    let mut spec = SpecDoc {
        kind,
        key: top.attribute("jira").map(str::to_string),
        name: top.attribute("name").map(str::to_string),
        version: top.attribute("version").and_then(|v| v.trim().parse().ok()),
        platform: None,
        named: Vec::new(),
        owns: Vec::new(),
        forbid: Vec::new(),
        gates: Vec::new(),
        clauses: Vec::new(),
    };
    match kind {
        Kind::Root => {
            spec.platform = elements(top)
                .find(|n| n.has_tag_name("identity"))
                .and_then(|identity| elements(identity).find(|n| n.has_tag_name("platform")))
                .and_then(|p| p.attribute("path"))
                .map(str::to_string);
            if let Some(method) = elements(top).find(|n| n.has_tag_name("method")) {
                for step in elements(method).filter(|n| !n.has_tag_name("note")) {
                    spec.clauses.push(Clause {
                        kind: "method",
                        ident: Some(step.tag_name().name().to_string()),
                        text: text_of(step),
                    });
                }
            }
        }
        Kind::Rules => {
            for rule in elements(top).filter(|n| n.has_tag_name("rule")) {
                // The whole rule — where it comes from and what catches a
                // breach of it bind as much as its words do.
                spec.clauses.push(Clause {
                    kind: "rule",
                    ident: rule.attribute("id").map(str::to_string),
                    text: clause_text(rule, "id"),
                });
            }
        }
        Kind::Task => {
            for part in elements(top) {
                match part.tag_name().name() {
                    part_name @ ("reflects" | "reads") => {
                        let kind = if part_name == "reads" { "reads" } else { "reflects" };
                        for file in elements(part).filter(|n| n.has_tag_name("file")) {
                            let path = file.attribute("path").map(str::to_string);
                            if let Some(p) = &path {
                                spec.named.push(p.clone());
                            }
                            spec.clauses.push(Clause { kind, ident: path, text: clause_text(file, "path") });
                        }
                    }
                    "gates" => {
                        for gate in elements(part).filter(|n| n.has_tag_name("gate")) {
                            spec.gates.push(crate::gates::Gate {
                                id: gate.attribute("id").unwrap_or_default().to_string(),
                                kind: gate.attribute("kind").unwrap_or_default().to_string(),
                                cmds: elements(gate).filter(|n| n.has_tag_name("cmd")).map(text_of).collect(),
                                expect: elements(gate).find(|n| n.has_tag_name("expect")).map(text_of).unwrap_or_default(),
                            });
                            spec.clauses.push(Clause {
                                kind: "gate",
                                ident: gate.attribute("id").map(str::to_string),
                                text: clause_text(gate, "id"),
                            });
                        }
                    }
                    "owns" => {
                        let text = text_of(part);
                        spec.owns = paths_in(&text);
                        spec.clauses.push(Clause { kind: "owns", ident: None, text });
                    }
                    "forbid" => {
                        let text = text_of(part);
                        spec.forbid = paths_in(&text);
                        spec.clauses.push(Clause { kind: "forbid", ident: None, text });
                    }
                    "agent-decides" => {
                        spec.clauses.push(Clause { kind: "agent-decides", ident: None, text: text_of(part) });
                    }
                    "start" => spec.clauses.push(Clause { kind: "start", ident: None, text: text_of(part) }),
                    "done" => spec.clauses.push(Clause { kind: "done", ident: None, text: text_of(part) }),
                    _ => {}
                }
            }
        }
    }
    Some(spec)
}

/// The task file a kickoff line points at: the line's one token ending
/// in `.xml`, trailing punctuation dropped. `None` unless the message is
/// one line naming exactly one such file (rules.xml kickoff: "A build
/// starts with a model pointed at one task file … Nothing else is said
/// at kickoff"). Whether the file IS a task file is decided by its
/// content, when the session reads it.
#[must_use]
pub fn kickoff_task_path(message: &str) -> Option<String> {
    let line = message.trim();
    if line.is_empty() || line.contains('\n') {
        return None;
    }
    let mut found = line
        .split_whitespace()
        .map(|t| t.trim_end_matches(['.', ',', ';', ':', ')', '"', '\'', '`']))
        .map(|t| t.trim_start_matches(['(', '"', '\'', '`']))
        .filter(|t| t.ends_with(".xml"));
    let path = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(path.to_string())
}

/// A clause's words: the element's attributes other than the one that
/// names it (a gate's `kind` and `proves`, a file's `keys` and
/// `through`), then all of its text. An attribute binds as much as text
/// does, so a change to one is a change to the clause.
fn clause_text(node: Node<'_, '_>, naming: &str) -> String {
    let mut parts: Vec<String> = node
        .attributes()
        .filter(|a| a.name() != naming)
        .map(|a| format!("{}=\"{}\"", a.name(), a.value()))
        .collect();
    let text = text_of(node);
    if !text.is_empty() {
        parts.push(text);
    }
    parts.join(" ")
}

/// The repository paths a sentence names: words with a `/` in them,
/// trailing punctuation dropped. `<owns>` and `<forbid>` are written as
/// prose ("a.py; b.py, c.py and d.py, the changes … and nothing else"),
/// and a path is the only word in them with a slash. A URL is not a path.
#[must_use]
pub fn paths_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == ';' || c == ',') {
        let word = word.trim_matches(|c: char| matches!(c, '.' | ':' | '(' | ')' | '"' | '\'' | '`'));
        if word.contains('/') && !word.contains("://") && !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// The element children of a node.
fn elements<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

/// Every text below a node, in document order, whitespace collapsed.
/// `Node::text` is only the FIRST text child (roxmltree 0.21), so an
/// element with inline children needs its descendants walked.
fn text_of(node: Node<'_, '_>) -> String {
    let joined: String = node
        .descendants()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ");
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}
