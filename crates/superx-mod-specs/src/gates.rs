//! Level 3: the gates — what the build reported of them, and whether its
//! own session backs the report.
//!
//! A build ends with a gate table in its PR body (root method `pr`: "the
//! gate table (id, the command as run with its placeholders filled,
//! exit, the matched line, the run id or local)"). The table is the
//! build's CLAIM. This check holds the claim against the task file and
//! against the session itself:
//!
//! - every gate the task lists is in the table;
//! - where a gate's `expect` names the exit (`… ends with `exit 1``),
//!   the reported exit is that one;
//! - a local gate's command was run in the session;
//! - a door gate's reported run id appeared in the session's own command
//!   output — not only in the table the session wrote.
//!
//! What it does not do: read the bucket logs the run ids name. That is a
//! read of something outside the session, for the repository crawler.

use crate::read_order::Judgement;

/// A gate as the task file writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub id: String,
    /// `local` or `door`.
    pub kind: String,
    /// Its commands, placeholders (`<…>`) as written.
    pub cmds: Vec<String>,
    pub expect: String,
}

/// One row of a build's gate table, by column name (lower case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reported {
    pub id: String,
    pub cells: Vec<(String, String)>,
}

impl Reported {
    /// The exit column.
    fn exit(&self) -> Option<&str> {
        self.cells.iter().find(|(h, _)| h.contains("exit")).map(|(_, v)| v.as_str())
    }

    /// The run id column — `run id`, `run id or local` — never a column
    /// that merely mentions a run (`command as run`).
    fn run_id(&self) -> Option<String> {
        let (_, cell) = self
            .cells
            .iter()
            .find(|(h, _)| h.contains("run id"))
            .or_else(|| self.cells.iter().find(|(h, _)| h == "run"))?;
        // The id is the first quoted span, else the first word: a cell may
        // add a comment ("`20261013t195954z` (G3 once more; …)").
        let token = match cell.split_once('`') {
            Some((_, rest)) => rest.split('`').next().unwrap_or(""),
            None => cell.split_whitespace().next().unwrap_or(""),
        };
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        (!token.is_empty()).then(|| token.to_string())
    }
}

/// A gate id at the start of a cell: `G1`, `G1, local`, `**G1**`.
fn gate_id(cell: &str) -> Option<String> {
    let cell = cell.trim().trim_start_matches(['*', '`', ' ']);
    let letters: String = cell.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits: String = cell[letters.len()..].chars().take_while(char::is_ascii_digit).collect();
    (!letters.is_empty() && !digits.is_empty()).then(|| format!("{letters}{digits}"))
}

/// A table row's cells, split at pipes a cell does not escape (`\|`).
fn cells_of(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in inner.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
            cur.push(c);
        } else if c == '|' {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// The last gate table among documents a session wrote or printed, in
/// order: a markdown table whose header's first cell names the gate (`id`,
/// `gate`), and whose rows open with a gate id.
#[must_use]
pub fn report(documents: &[&str]) -> Option<Vec<Reported>> {
    documents.iter().rev().find_map(|doc| table(doc))
}

fn table(doc: &str) -> Option<Vec<Reported>> {
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<Reported> = Vec::new();
    for line in doc.lines().map(str::trim) {
        if !line.starts_with('|') {
            if !rows.is_empty() {
                break;
            }
            header = None;
            continue;
        }
        let cells = cells_of(line);
        if cells.iter().all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) && !c.is_empty()) {
            continue; // the |---|---| line
        }
        match &header {
            None => {
                let first = cells.first().map(|c| c.to_lowercase()).unwrap_or_default();
                if first == "id" || first.starts_with("gate") {
                    header = Some(cells.iter().map(|c| c.to_lowercase()).collect());
                }
            }
            Some(h) => {
                let Some(id) = cells.first().and_then(|c| gate_id(c)) else { continue };
                rows.push(Reported {
                    id,
                    cells: h.iter().cloned().zip(cells.iter().cloned()).skip(1).collect(),
                });
            }
        }
    }
    (!rows.is_empty()).then_some(rows)
}

/// The exit a text names: a bare number, or the number after `exit`.
#[must_use]
pub fn exit_in(text: &str) -> Option<i64> {
    let t = text.trim().trim_matches('`');
    if let Ok(n) = t.parse() {
        return Some(n);
    }
    let lower = text.to_lowercase();
    let at = lower.find("exit")?;
    let digits: String = lower[at + 4..]
        .trim_start_matches([' ', '`', ':', '='])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The words of a command up to its first placeholder: what any filled-in
/// run of it must contain.
fn fixed_part(cmd: &str) -> String {
    cmd.split('<').next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Judge a build's gates.
///
/// `documents` are the files the session wrote, in order; `commands` its
/// shell calls; `outputs` what those calls printed.
#[must_use]
pub fn judge(gates: &[Gate], documents: &[&str], commands: &[&str], outputs: &[&str]) -> Judgement {
    if gates.is_empty() {
        return Judgement { result: "no-evidence", note: "the task file lists no gate".to_string(), evidence: Vec::new() };
    }
    let Some(rows) = report(documents) else {
        return Judgement {
            result: "no-evidence",
            note: "no gate table appears in what the session wrote or printed (a table a script builds is not seen)"
                .to_string(),
            evidence: Vec::new(),
        };
    };
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    // What backs a claim is output the session got from the world, not the
    // claim printed back: an output that is itself a gate table backs
    // nothing.
    let outputs: Vec<&str> = outputs.iter().copied().filter(|o| table(o).is_none()).collect();
    let commands: Vec<String> = commands.iter().map(|c| flat(c)).collect();
    let mut problems: Vec<String> = Vec::new();
    for g in gates {
        let Some(row) = rows.iter().rev().find(|r| r.id == g.id) else {
            problems.push(format!("{} is not in the gate table", g.id));
            continue;
        };
        if let (Some(want), Some(got)) = (exit_in(&g.expect), row.exit().and_then(exit_in)) {
            if want != got {
                problems.push(format!("{} reports exit {got}, its expect says exit {want}", g.id));
            }
        }
        if g.kind == "local" {
            let ran = g.cmds.iter().map(|c| fixed_part(c)).filter(|c| c.len() >= 8).any(|c| commands.iter().any(|run| run.contains(&c)));
            if !ran {
                problems.push(format!("{}'s command was never run in the session", g.id));
            }
        } else if let Some(run) = row.run_id() {
            if run != "local" && !outputs.iter().any(|o| o.contains(&run)) {
                problems.push(format!("{}'s run id {run} never appeared in the session's output", g.id));
            }
        } else {
            problems.push(format!("{} reports no run id", g.id));
        }
    }
    if problems.is_empty() {
        return Judgement {
            result: "complied",
            note: format!("reported all {} gates, each backed by the session", gates.len()),
            evidence: Vec::new(),
        };
    }
    Judgement { result: "breached", note: problems.join("; "), evidence: Vec::new() }
}
