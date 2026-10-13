//! What a session did with git, read from the text of its shell calls.
//! Nothing here runs a command: a call is parsed the way a person reads
//! it, and only what it says unambiguously is taken.
//!
//! - A BRANCH EVENT is a branch created (`checkout -b`, `switch -c`,
//!   `branch <name>`, `worktree add -b`), pushed, merged, deleted
//!   (`branch -d/-D`) or deleted on a remote (`push --delete`, `push -d`,
//!   `push <remote> :<name>`).
//! - A COMMIT is a `git commit`, with the message it was given: `-m`
//!   texts, a here-document fed to `-F -`, or the file named by `-F`,
//!   which the caller resolves from what the session wrote.
//! - A REVISION is a commit message naming `<KEY> file version <N>` —
//!   the way the design sessions commit a spec file's new version
//!   (gryphon-data-lake sandbox, 2026-10-08 on).
//!
//! Names built at run time (`$b`, `"$(…)"`) are not guessed at.

/// One branch event a call states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchEvent {
    pub branch: String,
    /// `created`, `pushed`, `merged`, `deleted` or `deleted-remote`
    /// (the schema's ASSERT).
    pub event: &'static str,
    /// The git command as written.
    pub command: String,
}

/// One `git commit` a call makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The `-m` texts and a here-document fed to `-F -`, joined.
    pub message: String,
    /// The file `-F` names, when it is a file.
    pub message_file: Option<String>,
}

/// A spec version a commit message names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub key: String,
    pub version: i64,
    /// The build whose review it came from: `… of its v5 build`.
    pub reviewed: Option<i64>,
    /// The message line that names it.
    pub line: String,
}

/// What one shell call says about git.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub branches: Vec<BranchEvent>,
    pub commits: Vec<Commit>,
}

/// Parse one shell call.
#[must_use]
pub fn activity(command: &str) -> Activity {
    let (script, heredocs) = strip_heredocs(command);
    let mut out = Activity::default();
    for segment in segments(&script) {
        let words = words(&segment);
        let Some((sub, args)) = git_subcommand(&words) else { continue };
        let names = |from: &[String]| -> Vec<String> {
            from.iter().filter(|a| !a.starts_with('-') && is_literal_name(a)).cloned().collect()
        };
        let mut push = |branch: String, event: &'static str| {
            out.branches.push(BranchEvent { branch, event, command: segment.trim().to_string() });
        };
        match sub.as_str() {
            "branch" => {
                if args.iter().any(|a| a == "-d" || a == "-D" || a == "--delete") {
                    for n in names(args) {
                        push(n, "deleted");
                    }
                } else if let Some(first) = args.first() {
                    if !first.starts_with('-') && is_literal_name(first) {
                        push(first.clone(), "created");
                    }
                }
            }
            "checkout" | "switch" => {
                if let Some(i) = args.iter().position(|a| matches!(a.as_str(), "-b" | "-B" | "-c" | "-C")) {
                    if let Some(n) = args.get(i + 1).filter(|n| is_literal_name(n)) {
                        push(n.clone(), "created");
                    }
                }
            }
            "worktree" if args.first().map(String::as_str) == Some("add") => {
                if let Some(i) = args.iter().position(|a| a == "-b" || a == "-B") {
                    if let Some(n) = args.get(i + 1).filter(|n| is_literal_name(n)) {
                        push(n.clone(), "created");
                    }
                }
            }
            "push" => {
                let deleting = args.iter().any(|a| a == "--delete" || a == "-d");
                // The first plain word is the remote; the rest are refs.
                let refs: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).skip(1).collect();
                for r in refs {
                    if let Some(gone) = r.strip_prefix(':') {
                        if is_literal_name(gone) {
                            push(gone.to_string(), "deleted-remote");
                        }
                    } else {
                        let name = r.rsplit_once(':').map_or(r.as_str(), |(_, dst)| dst);
                        if is_literal_name(name) && name != "HEAD" {
                            push(name.to_string(), if deleting { "deleted-remote" } else { "pushed" });
                        }
                    }
                }
            }
            "merge" => {
                // A trial merge (`--no-commit`), an abort or a continue
                // merges no branch.
                if args.iter().any(|a| matches!(a.as_str(), "--no-commit" | "--abort" | "--quit" | "--continue")) {
                    continue;
                }
                let mut skip = false;
                for a in args {
                    if skip {
                        skip = false;
                        continue;
                    }
                    if a == "-m" || a == "-F" || a == "-s" || a == "-X" {
                        skip = true;
                        continue;
                    }
                    if !a.starts_with('-') && is_literal_name(a) {
                        push(a.clone(), "merged");
                    }
                }
            }
            "commit" => {
                let mut messages: Vec<String> = Vec::new();
                let mut message_file = None;
                let mut i = 0;
                while i < args.len() {
                    let a = &args[i];
                    if a == "-m" || a == "--message" {
                        messages.extend(args.get(i + 1).cloned());
                        i += 2;
                        continue;
                    }
                    if let Some(m) = a.strip_prefix("--message=") {
                        messages.push(m.to_string());
                    } else if a == "-F" || a == "--file" {
                        match args.get(i + 1).map(String::as_str) {
                            Some("-") => messages.extend(heredocs.first().cloned()),
                            Some(f) => message_file = Some(f.to_string()),
                            None => {}
                        }
                        i += 2;
                        continue;
                    } else if let Some(f) = a.strip_prefix("--file=") {
                        message_file = Some(f.to_string());
                    }
                    i += 1;
                }
                out.commits.push(Commit { message: messages.join("\n\n"), message_file });
            }
            _ => {}
        }
    }
    out
}

/// The here-documents a call writes to files: `cat > <path> <<'EOF'` —
/// how a session writes a commit message for `-F` in the same call.
#[must_use]
pub fn written_files(command: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lines: Vec<&str> = command.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let (Some(marker), Some(path)) = (heredoc_marker(line), cat_target(line)) {
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != marker {
                body.push(lines[i]);
                i += 1;
            }
            out.push((path, body.join("\n")));
        }
        i += 1;
    }
    out
}

/// The spec versions a commit message names: `<KEY> file version <N>`.
#[must_use]
pub fn revisions(message: &str) -> Vec<Revision> {
    let mut out: Vec<Revision> = Vec::new();
    for line in message.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        for (i, w) in words.iter().enumerate() {
            if *w != "file" || words.get(i + 1) != Some(&"version") || i == 0 {
                continue;
            }
            let key = words[i - 1].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            let Some(version) = words.get(i + 2).and_then(|v| leading_number(v)) else { continue };
            if !is_task_key(key) {
                continue;
            }
            let reviewed = line
                .find(" of its v")
                .and_then(|at| leading_number(&line[at + " of its v".len()..]))
                .filter(|_| line.contains("build"));
            if !out.iter().any(|r| r.key == key && r.version == version) {
                out.push(Revision { key: key.to_string(), version, reviewed, line: line.trim().to_string() });
            }
        }
    }
    out
}

/// The commit hashes a command printed: lines that open with seven or
/// more hex digits and a space, as `git log --oneline` writes them.
#[must_use]
pub fn printed_commits(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|l| {
            let (hash, rest) = l.trim().split_once(' ')?;
            (hash.len() >= 7 && hash.len() <= 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| (hash.to_string(), rest.to_string()))
        })
        .collect()
}

/// `ENG-8072`: letters and digits, a hyphen, digits.
fn is_task_key(s: &str) -> bool {
    let Some((prefix, number)) = s.rsplit_once('-') else { return false };
    !prefix.is_empty()
        && prefix.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && prefix.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

fn leading_number(s: &str) -> Option<i64> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A branch name written out, not built at run time.
fn is_literal_name(s: &str) -> bool {
    !s.is_empty() && !s.contains(['$', '`', '*', '(', ')', '<', '>', '{', '}', '"', '\''])
}

/// `git [global options] <subcommand> <args…>`, after any leading
/// variable assignments.
fn git_subcommand(words: &[String]) -> Option<(String, &[String])> {
    let start = words.iter().position(|w| !is_assignment(w))?;
    if words.get(start).map(String::as_str) != Some("git") {
        return None;
    }
    let mut i = start + 1;
    while let Some(w) = words.get(i) {
        match w.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
            o if o.starts_with('-') => i += 1,
            sub => return Some((sub.to_string(), &words[i + 1..])),
        }
    }
    None
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// The call with its here-document bodies taken out (so text inside
/// them is never read as a command), and those bodies.
fn strip_heredocs(command: &str) -> (String, Vec<String>) {
    let mut script = Vec::new();
    let mut bodies = Vec::new();
    let lines: Vec<&str> = command.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        script.push(lines[i]);
        if let Some(marker) = heredoc_marker(lines[i]) {
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != marker {
                body.push(lines[i]);
                i += 1;
            }
            bodies.push(body.join("\n"));
        }
        i += 1;
    }
    (script.join("\n"), bodies)
}

/// The terminator of a here-document a line opens: `<<'EOF'`,
/// `<<"EOF"`, `<<EOF`, `<<-EOF`.
fn heredoc_marker(line: &str) -> Option<String> {
    let at = line.find("<<")?;
    let rest = line[at + 2..].trim_start_matches('-').trim_start();
    if rest.starts_with('<') {
        return None; // `<<<` is a here-string.
    }
    let marker: String = rest
        .trim_start_matches(['\'', '"'])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!marker.is_empty()).then_some(marker)
}

/// The file a `cat > <path> <<…` line writes.
fn cat_target(line: &str) -> Option<String> {
    // Raw words: [`words`] drops a redirection's file, which is the very
    // thing wanted here.
    let raw: Vec<&str> = line.split_whitespace().collect();
    let cat = raw.iter().position(|w| *w == "cat")?;
    let gt = raw[cat..].iter().position(|w| *w == ">")? + cat;
    raw.get(gt + 1)
        .filter(|p| !p.starts_with("<<"))
        .map(|p| p.trim_matches(['\'', '"']).to_string())
}

/// Split a script at `&&`, `||`, `;`, `|` and newlines, outside quotes.
fn segments(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let chars: Vec<char> = script.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '\n' | ';' | '|' | '&' => {
                    if c == '&' && chars.get(i + 1) != Some(&'&') {
                        cur.push(c);
                    } else {
                        out.push(std::mem::take(&mut cur));
                        if matches!(c, '&' | '|') && chars.get(i + 1) == Some(&c) {
                            i += 1;
                        }
                    }
                }
                _ => cur.push(c),
            },
        }
        i += 1;
    }
    out.push(cur);
    out.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

/// Shell words: whitespace-separated, quotes removed, redirections of
/// file descriptors (`2>/dev/null`, `2>&1`) dropped.
fn words(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in segment.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    // A redirection is no argument: `2>/dev/null` and `2>&1` whole, and
    // `>`/`>>`/`<` with the file after it.
    let mut kept = Vec::new();
    let mut skip_next = false;
    for w in out {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(w.as_str(), ">" | ">>" | "<" | "&>") {
            skip_next = true;
            continue;
        }
        if w.starts_with('>') || (w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('>')) {
            continue;
        }
        kept.push(w);
    }
    kept
}
