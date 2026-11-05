//! Launches and plans (#573). A launch is the part of a command that starts
//! a job: a simple command that begins with one of the operator's job
//! commands (`attr_recon_watch_jobs`), its words as written, `--wait` and
//! `--async` set aside, so two launches of the same job on the same input
//! are the same text however the command around them differs. A plan is a
//! task file whose `<cmd>` elements launch jobs: one that launches the same
//! job twice runs the same work twice.

use std::collections::BTreeMap;

use serde::Serialize;

/// How much of a launch's input its label shows.
const LABEL_INPUT_CHARS: usize = 48; // skill-allow: §9-const — display width of a label, not a policy tunable

/// The flags that change how a launch waits, not what it runs.
const WAITING: [&str; 2] = ["--wait", "--async"];

/// The launch in `command`, if one of its simple commands begins with one
/// of `jobs`.
#[must_use]
pub fn launch(command: &str, jobs: &[String]) -> Option<String> {
    simple_commands(command).into_iter().find_map(|c| {
        let words = without_redirections(c.split_whitespace().skip_while(|w| is_assignment(w) || *w == "time" || *w == "exec"));
        let joined = words.join(" ");
        jobs.iter().filter(|j| !j.trim().is_empty()).any(|j| joined == j.trim() || joined.starts_with(&format!("{} ", j.trim()))).then(|| {
            words.iter().filter(|w| !WAITING.contains(w)).map(|w| w.trim_matches(|ch| ch == '\'' || ch == '"')).collect::<Vec<_>>().join(" ")
        })
    })
}

/// A launch as the page names it: the job's words after the command, and
/// the input it was given, cut to a line.
#[must_use]
pub fn label(launch: &str, jobs: &[String]) -> String {
    let rest = jobs.iter().map(|j| j.trim()).filter(|j| !j.is_empty()).find_map(|j| launch.strip_prefix(j)).unwrap_or(launch).trim();
    let job = rest.split_whitespace().next().unwrap_or_default();
    match rest.split_once("--args") {
        Some((_, args)) => {
            let input = args.trim().trim_start_matches('=').split_whitespace().next().unwrap_or_default();
            let cut: String = input.chars().take(LABEL_INPUT_CHARS).collect();
            if cut.len() < input.len() {
                format!("{job} {cut}…")
            } else {
                format!("{job} {cut}")
            }
        }
        None => job.to_string(),
    }
}

/// One job a plan launches more than once.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Repeat {
    /// The job and the part of its input that is the same each time.
    pub job: String,
    pub times: usize,
    /// The gates that launch it, each by its id and what it proves.
    pub by: Vec<String>,
}

/// The jobs `xml` launches more than once, in its `<cmd>` elements. A
/// launch's input is compared without its selections: a placeholder in
/// angle brackets, `*`, and what follows them, which name the nodes or the
/// archives the job runs on rather than the job.
#[must_use]
pub fn repeats(xml: &str, jobs: &[String]) -> Vec<Repeat> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (gate, cmd) in commands(xml) {
        if let Some(l) = launch(&cmd, jobs) {
            found.entry(same_work(&l)).or_default().push(gate);
        }
    }
    found.into_iter().filter(|(_, by)| by.len() > 1).map(|(job, by)| Repeat { job, times: by.len(), by }).collect()
}

/// A launch with its selections set aside: the input after `--args` up to
/// its first placeholder or `*`.
fn same_work(launch: &str) -> String {
    let mut out = Vec::new();
    let mut args_next = false;
    for w in launch.split_whitespace() {
        let value = if args_next { Some(("", w)) } else { w.strip_prefix("--args=").map(|v| ("--args=", v)) };
        args_next = w == "--args";
        match value {
            Some((flag, v)) => {
                let kept: Vec<&str> = v.split(',').take_while(|part| !part.starts_with('<') && *part != "*").collect();
                out.push(format!("{flag}{}", kept.join(",")));
            }
            None => out.push(w.to_string()),
        }
    }
    out.join(" ")
}

/// Every `<cmd>` of `xml`, unescaped, with the gate it is in: its id and
/// the start of what it proves, or the file's own name for one outside a
/// gate.
fn commands(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut gate = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if rest.starts_with("<gate ") || rest.starts_with("<gate>") {
            let head = &rest[..rest.find('>').unwrap_or(rest.len())];
            let id = attribute(head, "id").unwrap_or_default();
            let proves: String = attribute(head, "proves").unwrap_or_default().split_whitespace().take(8).collect::<Vec<_>>().join(" ");
            gate = if proves.is_empty() { id } else { format!("{id} ({proves})") };
        } else if rest.starts_with("</gate>") {
            gate.clear();
        } else if let Some(body) = rest.strip_prefix("<cmd>") {
            if let Some(end) = body.find("</cmd>") {
                out.push((if gate.is_empty() { "a command".to_string() } else { gate.clone() }, placeholders_whole(&unescape(&body[..end]))));
            }
        }
        rest = &rest[1..];
    }
    out
}

/// A placeholder written with spaces, `<the name of a table>`, kept as one
/// word.
fn placeholders_whole(cmd: &str) -> String {
    let mut inside = false;
    cmd.chars()
        .map(|c| {
            match c {
                '<' => inside = true,
                '>' => inside = false,
                ' ' if inside => return '_',
                _ => {}
            }
            c
        })
        .collect()
}

fn attribute(head: &str, name: &str) -> Option<String> {
    let at = head.find(&format!("{name}=\""))? + name.len() + 2;
    let end = head[at..].find('"')? + at;
    Some(unescape(&head[at..end]))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A command's simple commands: split at `;`, `&&`, `||`, `|`, `&` and new
/// lines outside quotes.
fn simple_commands(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                cur.push(ch);
            }
            // `2>&1` and `<&3` redirect; they end no command.
            (None, '&') if cur.ends_with('>') || cur.ends_with('<') => cur.push(ch),
            (None, ';' | '\n' | '|' | '&') => {
                if matches!(chars.peek(), Some('|' | '&')) {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            (None, c) => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
}

/// A command's words without its redirections: `2>&1`, `>out`, and a `>`
/// or `<` with the file after it.
fn without_redirections<'a>(words: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut target = false;
    for w in words {
        if target {
            target = false;
            continue;
        }
        let rest = w.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        // `<door.stage_jobs.6.run>` is a task file's placeholder, not a redirection.
        let placeholder = w.len() > 2 && w.starts_with('<') && w.ends_with('>') && !w.starts_with("<<");
        if !placeholder && (rest.starts_with('>') || rest.starts_with('<')) {
            target = rest.trim_start_matches(['>', '<', '&']).is_empty() && !rest.contains('&');
            continue;
        }
        out.push(w);
    }
    out
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}
