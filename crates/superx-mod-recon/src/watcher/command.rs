//! Shell commands as the watcher reads them (#573), from their text as the
//! agent wrote it; nothing is run. A command is cut into its simple
//! commands; its time is counted under the one that does the work, named by
//! its program and the word that says what it does ([`label`]); it may sleep
//! ([`sleeps`]); and it may change files, by the paths it names
//! ([`changed`]).

/// Commands that set a command's stage, not do its work: a change of
/// folder, a variable, a printed separator, a loop's or a test's words.
const STAGE: [&str; 29] = [
    "cd", "pushd", "popd", "export", "set", "unset", "source", ".", "true", ":", "echo", "printf", "for", "while", "until", "if", "elif",
    "case", "select", "done", "fi", "esac", "}", ")", "[", "[[", "test", "read", "wait",
];

/// Words a simple command can start with before its program: a loop's or a
/// branch's body, a negation, a group.
const KEYWORDS: [&str; 6] = ["do", "then", "else", "!", "{", "("];

/// Commands that run the command after them, with their own flags first.
const WRAPPERS: [&str; 9] = ["time", "exec", "nice", "nohup", "env", "timeout", "caffeinate", "command", "sudo"];

/// Programs that run a script: the script names what they do.
const INTERPRETERS: [&str; 10] = ["python", "python3", "node", "bash", "sh", "zsh", "ruby", "perl", "deno", "bun"];

/// What a script's file name ends with.
const SCRIPTS: [&str; 8] = [".py", ".js", ".mjs", ".cjs", ".ts", ".sh", ".rb", ".pl"];

/// Paths an agent keeps for itself, its memory and its scratch: what it
/// writes there changes no work of anyone's.
pub const ITS_OWN: [&str; 4] = ["/.claude/projects/", "/.claude/plans/", "/private/tmp/", "/tmp/"];

/// A command's simple commands: split at `;`, `&&`, `||`, `|`, `&` and new
/// lines outside quotes. A here-document's body, the lines up to its word,
/// is the command's input, not commands: `python3 - <<'EOF'` and the script
/// after it are one simple command.
pub fn simple_commands(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut heredocs: Vec<String> = Vec::new();
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
            // `<<word` and `<<-word` open a here-document; `<<<` is a string.
            (None, '<') if chars.peek() == Some(&'<') => {
                chars.next();
                cur.push_str("<<");
                if chars.peek() == Some(&'<') {
                    chars.next();
                    cur.push('<');
                    continue;
                }
                while matches!(chars.peek(), Some('-' | ' ')) {
                    cur.extend(chars.next());
                }
                let mut word = String::new();
                while let Some(&c) = chars.peek().filter(|c| !c.is_whitespace() && !matches!(c, ';' | '|' | '&' | '>' | '<' | ')')) {
                    word.push(c);
                    chars.next();
                }
                cur.push_str(&word);
                let word = word.trim_matches(['\'', '"', '\\']).to_string();
                if !word.is_empty() {
                    heredocs.push(word);
                }
            }
            (None, ';' | '\n' | '|' | '&') => {
                if matches!(chars.peek(), Some('|' | '&')) {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
                if ch == '\n' {
                    for word in std::mem::take(&mut heredocs) {
                        while chars.peek().is_some() {
                            let line: String = chars.by_ref().take_while(|c| *c != '\n').collect();
                            if line.trim() == word {
                                break;
                            }
                        }
                    }
                }
            }
            (None, c) => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
}

/// A command's words without its redirections: `2>&1`, `>out`, and a `>`
/// or `<` with the file after it.
pub fn without_redirections<'a>(words: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
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

/// Whether a word sets a variable: `NAME=value`.
pub fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// A simple command's words from its program on: a loop's keyword, the
/// variables it sets and the commands that wrap it set aside. A variable
/// set to a command's output, `T=$(gcloud auth …`, runs that command.
pub(super) fn from_program(simple: &str) -> Vec<String> {
    let mut words: Vec<String> = simple.split_whitespace().map(str::to_string).collect();
    loop {
        let Some(first) = words.first() else { return words };
        if KEYWORDS.contains(&first.as_str()) {
            words.remove(0);
        } else if is_assignment(first) {
            let value = first.split_once('=').map_or("", |(_, v)| v).to_string();
            match value.strip_prefix("$(") {
                Some(inner) if !inner.is_empty() => words[0] = inner.to_string(),
                _ => {
                    words.remove(0);
                }
            }
        } else if WRAPPERS.contains(&first.as_str()) {
            words.remove(0);
            while words.first().is_some_and(|w| w.starts_with('-') || w.chars().all(|c| c.is_ascii_digit())) {
                words.remove(0);
            }
        } else {
            return words;
        }
    }
}

/// A program's name as written: its path and its quotes set aside.
pub(super) fn program(word: &str) -> String {
    word.trim_matches(['\'', '"']).rsplit('/').next().unwrap_or_default().to_string()
}

/// A word that names what a program does: `test` in `cargo test`, `status`
/// in `git status`.
fn plain(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// What a command's time is counted under: the program of its first simple
/// command that does work, and the first word after it when that word says
/// what it does (`cargo test`, `git push`, `gcloud run`); a script's name
/// for an interpreter (`python3 check.py`, `python3 -m unittest`). A
/// command that only sets its stage is counted under its first program.
#[must_use]
pub fn label(command: &str) -> String {
    let mut first: Option<String> = None;
    for simple in simple_commands(command) {
        let words = from_program(&simple);
        let Some(head) = words.first() else { continue };
        let prog = program(head);
        if prog.is_empty() {
            continue;
        }
        first.get_or_insert_with(|| prog.clone());
        if STAGE.contains(&prog.as_str()) {
            continue;
        }
        let args = without_redirections(words[1..].iter().map(String::as_str));
        if INTERPRETERS.contains(&prog.as_str()) {
            for (i, a) in args.iter().enumerate() {
                match *a {
                    "-m" => return args.get(i + 1).map_or_else(|| prog.clone(), |m| format!("{prog} -m {m}")),
                    "-c" => return format!("{prog} -c"),
                    "-" => return format!("{prog} stdin"),
                    a if a.starts_with('-') => {}
                    a => {
                        let script = program(a);
                        return if SCRIPTS.iter().any(|x| script.ends_with(x)) { format!("{prog} {script}") } else { prog };
                    }
                }
            }
            return prog;
        }
        // `git -C <folder> status`: a folder or a setting given to a flag
        // names no work.
        let mut rest = args.iter().copied();
        let mut sub = None;
        while let Some(a) = rest.next() {
            if a == "-C" || a == "-c" {
                rest.next();
            } else if !a.starts_with('-') {
                sub = Some(a);
                break;
            }
        }
        return match sub {
            Some(sub) if plain(sub) => format!("{prog} {sub}"),
            _ => prog,
        };
    }
    first.unwrap_or_default()
}

/// Whether a command sleeps: one of its simple commands is `sleep`.
#[must_use]
pub fn sleeps(command: &str) -> bool {
    simple_commands(command).iter().any(|s| from_program(s).first().is_some_and(|w| program(w) == "sleep"))
}

/// The paths a command changes, as written: what `git add`, `rm`, `mv` or
/// `restore` names, `git checkout -- <paths>`, the files `sed -i` edits,
/// what `rm` removes, where `mv` and `cp` write, what `tee` and a `>` write;
/// and an `apply`, `destroy`, `import` or `delete` of terraform or kubectl
/// whole, as the place it changes. A script that writes files names none:
/// what it writes is not in its text.
#[must_use]
pub fn changed(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    for simple in simple_commands(command) {
        let words = from_program(&simple);
        let Some(head) = words.first() else { continue };
        for (i, w) in words.iter().enumerate() {
            let rest = w.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(target) = rest.strip_prefix(">>").or_else(|| rest.strip_prefix('>')) {
                let target = if target.is_empty() { words.get(i + 1).map_or("", String::as_str) } else { target };
                if !target.is_empty() && !target.starts_with('&') && target != "/dev/null" {
                    out.push(target.to_string());
                }
            }
        }
        let args: Vec<&str> = without_redirections(words[1..].iter().map(String::as_str));
        let named = || args.iter().filter(|a| !a.starts_with('-')).map(|a| (*a).to_string()).collect::<Vec<_>>();
        match program(head).as_str() {
            "git" => {
                let Some(at) = args.iter().position(|a| !a.starts_with('-')) else { continue };
                let after: Vec<String> = args[at + 1..].iter().filter(|a| !a.starts_with('-')).map(|a| (*a).to_string()).collect();
                match args[at] {
                    "add" | "rm" | "mv" | "restore" => out.extend(after),
                    "checkout" => {
                        if let Some(dash) = args.iter().position(|a| *a == "--") {
                            out.extend(args[dash + 1..].iter().map(|a| (*a).to_string()));
                        }
                    }
                    _ => {}
                }
            }
            // `sed -i '' 's/a/b/' f`: macOS's empty suffix, then the script,
            // then the files.
            "sed" if args.iter().any(|a| a.starts_with("-i")) => {
                out.extend(named().into_iter().filter(|a| !a.trim_matches(['\'', '"']).is_empty()).skip(1));
            }
            "rm" | "tee" => out.extend(named()),
            "mv" | "cp" => out.extend(named().pop()),
            "terraform" | "kubectl" if args.iter().any(|a| ["apply", "destroy", "import", "delete"].contains(a)) => out.push(simple.clone()),
            _ => {}
        }
    }
    out.extend(changes_tables(command));
    out.into_iter().map(|p| p.trim_matches(['\'', '"']).to_string()).filter(|p| !p.is_empty()).collect()
}

/// A push of a command: the branch it pushes, and whether it rewrites the
/// branch's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Push {
    /// The branch pushed to, as named: a refspec's destination; empty when
    /// the command names none and pushes the branch checked out.
    pub branch: String,
    pub force: bool,
}

/// The pushes of a command: `git push`, with its flags and refspecs.
#[must_use]
pub fn pushes(command: &str) -> Vec<Push> {
    let mut out = Vec::new();
    for simple in simple_commands(command) {
        let words = from_program(&simple);
        let Some(head) = words.first() else { continue };
        if program(head) != "git" {
            continue;
        }
        let args = without_redirections(words[1..].iter().map(String::as_str));
        // `git -C <dir> push`: the folder's value is no subcommand.
        let mut rest = args.iter().copied().peekable();
        let mut sub = None;
        while let Some(a) = rest.next() {
            if a == "-C" || a == "-c" {
                rest.next();
            } else if !a.starts_with('-') {
                sub = Some(a);
                break;
            }
        }
        if sub != Some("push") {
            continue;
        }
        let after: Vec<&str> = rest.collect();
        // A push that deletes a branch pushes no work.
        if after.iter().any(|a| *a == "--delete" || *a == "-d" || (a.starts_with(':') && a.len() > 1)) {
            continue;
        }
        let force = after.iter().any(|a| *a == "-f" || a.starts_with("--force") || (a.starts_with('+') && a.len() > 1));
        let named: Vec<&str> = after.iter().copied().filter(|a| !a.starts_with('-')).collect();
        // The first plain word is the remote; the refspecs follow it.
        // `HEAD` is the branch checked out, as a push that names none.
        let branch = named.get(1).map_or(String::new(), |spec| {
            let spec = spec.trim_start_matches('+');
            let dest = spec.rsplit_once(':').map_or(spec, |(_, d)| d);
            let dest = dest.trim_start_matches("refs/heads/");
            if dest == "HEAD" { String::new() } else { dest.to_string() }
        });
        out.push(Push { branch, force });
    }
    out
}

/// The branch a command makes and checks out: `git switch -c <b>`,
/// `git checkout -b <b>`.
#[must_use]
pub fn new_branch(command: &str) -> Option<String> {
    simple_commands(command).into_iter().find_map(|simple| {
        let words = from_program(&simple);
        if words.first().map(|w| program(w)).as_deref() != Some("git") {
            return None;
        }
        let at = words.iter().position(|w| w == "-c" || w == "-b" || w == "-B" || w == "-C")?;
        let sub = words.iter().skip(1).find(|w| !w.starts_with('-'))?;
        (sub == "switch" || sub == "checkout").then(|| words.get(at + 1).map(|b| b.trim_matches(['\'', '"']).to_string())).flatten()
    })
}

/// The pull request a command merges: `gh pr merge <number, link or
/// branch>`, or a host's merge call on `pullrequests/<number>/merge`; the
/// inner `None` when `gh pr merge` names none and merges the checked-out
/// branch's.
#[must_use]
pub fn merges(command: &str) -> Option<Option<String>> {
    for simple in simple_commands(command) {
        let words = from_program(&simple);
        let plain: Vec<&str> = words.iter().map(String::as_str).filter(|w| !w.starts_with('-')).collect();
        if let Some(at) = plain.windows(3).position(|w| program(w[0]) == "gh" && w[1] == "pr" && w[2] == "merge") {
            return Some(plain.get(at + 3).map(|s| pr_number(s).unwrap_or_else(|| (*s).to_string())));
        }
    }
    let at = command.find("pullrequests/")? + "pullrequests/".len();
    let number: String = command[at..].chars().take_while(char::is_ascii_digit).collect();
    (!number.is_empty() && command[at + number.len()..].starts_with("/merge")).then_some(Some(number))
}

/// Whether a command opens a pull request: `gh pr create`, or a host's
/// call on `pullrequests` that merges none.
#[must_use]
pub fn opens_pull(command: &str) -> bool {
    simple_commands(command).iter().any(|simple| {
        let words = from_program(simple);
        let plain: Vec<&str> = words.iter().map(String::as_str).filter(|w| !w.starts_with('-')).collect();
        plain.windows(3).any(|w| program(w[0]) == "gh" && w[1] == "pr" && w[2] == "create")
    }) || (command.contains("pullrequests") && command.contains("POST") && !command.contains("/merge"))
}

/// A pull request's number from its link (`…/pull/449`,
/// `…/pull-requests/106`) or as written.
#[must_use]
pub fn pr_number(text: &str) -> Option<String> {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        return Some(text.to_string());
    }
    ["/pull/", "/pull-requests/", "/pullrequests/"].iter().find_map(|m| {
        let at = text.find(m)? + m.len();
        let n: String = text[at..].chars().take_while(char::is_ascii_digit).collect();
        (!n.is_empty()).then_some(n)
    })
}

/// One merge commit a result names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCommit {
    pub branch: String,
    pub pull: u64,
    /// The commit, when the line gives it before the message, as
    /// `git log --oneline` does.
    pub commit: Option<String>,
}

/// The merge commits a result names, each as its host writes one:
/// Bitbucket's `Merged in <branch> (pull request #N)` and GitHub's `Merge
/// pull request #N from <owner>/<branch>`.
#[must_use]
pub fn merge_commits(text: &str) -> Vec<MergeCommit> {
    let mut out = Vec::new();
    for (at, word) in text.match_indices("Merged in ") {
        let rest = &text[at + word.len()..];
        let branch: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        let Some(tail) = rest[branch.len()..].strip_prefix(" (pull request #") else { continue };
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if let (false, true, Ok(pull)) = (branch.is_empty(), tail[digits.len()..].starts_with(')'), digits.parse()) {
            out.push(MergeCommit { branch, pull, commit: commit_before(text, at) });
        }
    }
    for (at, word) in text.match_indices("Merge pull request #") {
        let rest = &text[at + word.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Some(from) = rest[digits.len()..].strip_prefix(" from ") else { continue };
        let source: String = from.chars().take_while(|c| !c.is_whitespace()).collect();
        if let (Some((_, branch)), Ok(pull)) = (source.split_once('/'), digits.parse()) {
            if !branch.is_empty() {
                out.push(MergeCommit { branch: branch.to_string(), pull, commit: commit_before(text, at) });
            }
        }
    }
    out
}

/// The commit a line names just before `at`: the word there, when it is
/// one, 7 to 40 hexadecimal digits.
fn commit_before(text: &str, at: usize) -> Option<String> {
    let line = text[..at].rsplit('\n').next().unwrap_or_default();
    let word = line.split_whitespace().next_back()?;
    (word.len() >= 7 && word.len() <= 40 && word.chars().all(|c| c.is_ascii_hexdigit())).then(|| word.to_string())
}

/// Whether two commits, each written whole or cut short, are the same.
#[must_use]
pub fn same_commit(a: &str, b: &str) -> bool {
    let n = a.len().min(b.len());
    n >= 7 && a[..n].eq_ignore_ascii_case(&b[..n])
}

/// The branches a push's answer says it moved by force: each branch, the
/// commit it was and the one it is, from git's ` + <was>...<is> <from> ->
/// <branch> (forced update)`.
#[must_use]
pub fn forced_updates(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter(|l| l.contains("(forced update)"))
        .filter_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let (was, is) = words.iter().find_map(|w| w.split_once("..."))?;
            let branch = words.iter().position(|w| *w == "->").and_then(|i| words.get(i + 1))?;
            Some((branch.trim_start_matches("refs/heads/").to_string(), was.to_string(), is.to_string()))
        })
        .collect()
}

/// What a command undoes: `git revert`, `git reset --hard`, and a file put
/// back with `git restore` or `git checkout --`.
#[must_use]
pub fn undoes(command: &str) -> Option<String> {
    simple_commands(command).into_iter().find_map(|simple| {
        let words = from_program(&simple);
        if words.first().map(|w| program(w)).as_deref() != Some("git") {
            return None;
        }
        let sub = words.iter().skip(1).find(|w| !w.starts_with('-'))?.as_str();
        let undone = match sub {
            "revert" => true,
            "reset" => words.iter().any(|w| w == "--hard"),
            "restore" => !words.iter().any(|w| w == "--staged"),
            "checkout" => words.iter().any(|w| w == "--"),
            _ => false,
        };
        undone.then(|| simple.clone())
    })
}

/// The statements that change a table's or a schema's shape, word by word:
/// the verb, then what it changes.
const DDL: [(&str, &str); 8] = [
    ("ALTER", "TABLE"),
    ("DROP", "TABLE"),
    ("TRUNCATE", "TABLE"),
    ("CREATE", "TABLE"),
    ("DROP", "SCHEMA"),
    ("ALTER", "SCHEMA"),
    ("DROP", "DATABASE"),
    ("DROP", "VIEW"),
];

/// Programs that run SQL against a database, and the words of a SQL
/// file's name that say it changes one.
const SQL_CLIENTS: [&str; 12] = ["bq", "mysql", "psql", "sqlite3", "spark-sql", "beeline", "trino", "duckdb", "clickhouse-client", "sqlcmd", "snowsql", "surreal"];
const DDL_FILES: [&str; 5] = ["alter", "drop", "create", "ddl", "migrat"];

/// What a command changes in a database, as `DDL: <the command>`: a SQL
/// client, or a Spark SQL batch, run on DDL written in the command or on a
/// SQL file whose name says it changes tables. A search for the words runs
/// no client and changes nothing.
#[must_use]
pub fn changes_tables(command: &str) -> Option<String> {
    simple_commands(command).into_iter().find_map(|simple| {
        let words = from_program(&simple);
        let head = program(words.first()?);
        let client = SQL_CLIENTS.contains(&head.as_str()) || (head == "gcloud" && words.iter().any(|w| w == "spark-sql"));
        if !client {
            return None;
        }
        let upper = simple.to_uppercase();
        let lower = simple.to_lowercase();

        let said: Vec<&str> = upper.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty()).collect();
        let ddl = said.windows(2).any(|w| DDL.contains(&(w[0], w[1])))
            || words.iter().any(|w| w.ends_with(".sql") && DDL_FILES.iter().any(|f| program(w).to_lowercase().contains(f)));
        ddl.then(|| format!("DDL: {}", lower.split_whitespace().collect::<Vec<_>>().join(" ")))
    })
}
