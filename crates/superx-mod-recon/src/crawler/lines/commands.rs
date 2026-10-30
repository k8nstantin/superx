//! What an agent's shell command did, read from the command and what it
//! printed: the git and GitHub actions it performed, the tests it ran and
//! the code it built.
//!
//! A command line is split into the simple commands it runs, the way a
//! shell would: quotes, escapes, `&&`, `||`, `;`, pipes, subshells,
//! comments and heredoc bodies, and `cd` followed. What a command printed
//! confirms what it did, so a command that failed, a message that only
//! mentions git, or a file that is only read adds nothing. Only the
//! command line and its output are read, never a repository (#446).

use crate::crawler::lines::facts::{BuildRun, Builder, GitAction, GitEvent, TestRun, TestRunner};
use crate::crawler::lines::formats::is_hash;

/// What one shell command did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub git: Vec<GitEvent>,
    pub test: Option<TestRun>,
    pub build: Option<BuildRun>,
}

/// What `command` did, from what it printed. `ok`: it exited with success.
/// `dir`: where it was started.
#[must_use]
pub fn read(command: &str, output: &str, ok: bool, dir: Option<&str>) -> Ran {
    let simple = split(command, dir);
    Ran { git: git(&simple, output, ok), test: test(&simple, output), build: build(&simple, output, ok) }
}

/// One simple command: its words, and the directory it ran in when the
/// command line says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Simple {
    words: Vec<String>,
    dir: Option<String>,
}

/// Words that run the command after them.
const WRAPPERS: [&str; 7] = ["env", "time", "command", "exec", "nohup", "sudo", "builtin"];

/// The simple commands a command line runs, in order.
fn split(command: &str, dir: Option<&str>) -> Vec<Simple> {
    let mut s = Splitter { out: Vec::new(), cwd: dir.map(str::to_string), words: Vec::new(), word: None };
    let chars: Vec<char> = command.chars().collect();
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                let w = s.word.get_or_insert_with(String::new);
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    w.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                let w = s.word.get_or_insert_with(String::new);
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '"' | '\\' | '$' | '`') {
                        w.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    w.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '\\' => {
                if let Some(&next) = chars.get(i + 1) {
                    if next != '\n' {
                        s.word.get_or_insert_with(String::new).push(next);
                    }
                }
                i += 2;
            }
            ' ' | '\t' | '\r' => {
                s.end_word();
                i += 1;
            }
            '\n' => {
                s.end_command();
                i += 1;
                for (delimiter, tabs) in heredocs.drain(..) {
                    while i < chars.len() {
                        let start = i;
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                        let line: String = chars[start..i].iter().collect();
                        i += 1;
                        let line = if tabs { line.trim_start_matches('\t') } else { line.as_str() };
                        if line == delimiter {
                            break;
                        }
                    }
                }
            }
            ';' | '&' | '|' | '(' | ')' | '`' => {
                s.end_command();
                i += 1;
            }
            '<' if chars.get(i + 1) == Some(&'<') => {
                s.end_word();
                i += 2;
                if chars.get(i) == Some(&'<') {
                    // A here-string: the word after it is input, not a body.
                    i += 1;
                    continue;
                }
                let tabs = chars.get(i) == Some(&'-');
                if tabs {
                    i += 1;
                }
                while i < chars.len() && matches!(chars[i], ' ' | '\t') {
                    i += 1;
                }
                let mut delimiter = String::new();
                while i < chars.len() && !matches!(chars[i], ' ' | '\t' | '\n' | ';' | '&' | '|' | ')') {
                    if !matches!(chars[i], '\'' | '"' | '\\') {
                        delimiter.push(chars[i]);
                    }
                    i += 1;
                }
                if !delimiter.is_empty() {
                    heredocs.push((delimiter, tabs));
                }
            }
            '#' if s.word.is_none() => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            _ => {
                s.word.get_or_insert_with(String::new).push(c);
                i += 1;
            }
        }
    }
    s.end_command();
    s.out
}

struct Splitter {
    out: Vec<Simple>,
    cwd: Option<String>,
    words: Vec<String>,
    word: Option<String>,
}

impl Splitter {
    fn end_word(&mut self) {
        if let Some(w) = self.word.take() {
            self.words.push(w);
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        let mut words = std::mem::take(&mut self.words);
        loop {
            match words.first() {
                Some(w) if is_assignment(w) || WRAPPERS.contains(&w.as_str()) => {
                    words.remove(0);
                }
                _ => break,
            }
        }
        match words.first().map(String::as_str) {
            None => {}
            Some("cd" | "pushd") => {
                self.cwd = Some(resolve(self.cwd.as_deref(), words.get(1).map_or("~", String::as_str)));
            }
            Some(_) => self.out.push(Simple { words, dir: self.cwd.clone() }),
        }
    }
}

/// `NAME=value`, set for the command after it.
fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// `target` from `from`, as `cd` would go: an absolute or home path as it
/// is, a relative one joined on and `..` stepped back.
fn resolve(from: Option<&str>, target: &str) -> String {
    if target.starts_with('/') || target.starts_with('~') {
        return target.to_string();
    }
    let Some(from) = from else {
        return target.to_string();
    };
    let mut parts: Vec<&str> = from.split('/').collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if joined.is_empty() {
        "/".to_string()
    } else {
        joined
    }
}

/// A git command: its subcommand, its arguments and where it ran.
struct Git<'a> {
    sub: &'a str,
    args: &'a [String],
    dir: Option<String>,
}

fn git_command(s: &Simple) -> Option<Git<'_>> {
    if s.words.first().map(String::as_str) != Some("git") {
        return None;
    }
    let mut dir = s.dir.clone();
    let mut i = 1;
    while let Some(word) = s.words.get(i) {
        match word.as_str() {
            "-C" => {
                dir = Some(resolve(dir.as_deref(), s.words.get(i + 1)?));
                i += 2;
            }
            "-c" => i += 2,
            w if w.starts_with('-') => i += 1,
            sub => return Some(Git { sub, args: &s.words[i + 1..], dir }),
        }
    }
    None
}

/// The arguments that are not options.
fn operands(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter().map(String::as_str).filter(|a| !a.starts_with('-'))
}

/// The git and GitHub actions a command performed, as its output confirms.
fn git(simple: &[Simple], output: &str, ok: bool) -> Vec<GitEvent> {
    let gits: Vec<Git<'_>> = simple.iter().filter_map(git_command).collect();
    let mut events = Vec::new();

    // A new commit is printed `[branch hash] subject`, whichever command
    // made it.
    let makers: Vec<(GitAction, Option<String>)> = gits
        .iter()
        .filter_map(|g| match g.sub {
            "commit" => Some((GitAction::Commit, g.dir.clone())),
            "revert" => Some((GitAction::Revert, g.dir.clone())),
            "cherry-pick" => Some((GitAction::CherryPick, g.dir.clone())),
            _ => None,
        })
        .collect();
    if !makers.is_empty() {
        for (n, line) in output.lines().filter_map(new_commit).enumerate() {
            let (action, dir) = makers[n.min(makers.len() - 1)].clone();
            events.push(GitEvent {
                dir,
                hash: Some(line.1),
                branch: Some(line.0),
                subject: Some(line.2),
                ..GitEvent::of(action)
            });
        }
    }
    events.extend(quiet_commits(&gits, &events, output, ok));

    for g in &gits {
        match g.sub {
            "push" => events.extend(push_lines(output).filter_map(|l| pushed(l, g.dir.clone()))),
            "merge" if operands(g.args).next().is_some() => {
                let merged = output.lines().any(|l| l.starts_with("Merge made by") || l.trim() == "Fast-forward");
                if merged {
                    let hash = output
                        .lines()
                        .find_map(|l| l.strip_prefix("Updating "))
                        .and_then(|range| range.split("..").nth(1))
                        .map(str::trim)
                        .filter(|h| is_hash(h))
                        .map(str::to_string);
                    events.push(GitEvent {
                        dir: g.dir.clone(),
                        hash,
                        branch: operands(g.args).next().map(str::to_string),
                        ..GitEvent::of(GitAction::Merge)
                    });
                }
            }
            "rebase" => {
                for line in output.lines() {
                    if let Some(rest) = line.trim().strip_prefix("Successfully rebased and updated ") {
                        let branch = rest.trim_end_matches('.').trim_start_matches("refs/heads/");
                        events.push(GitEvent {
                            dir: g.dir.clone(),
                            branch: Some(branch.to_string()),
                            ..GitEvent::of(GitAction::Rebase)
                        });
                    }
                }
            }
            "reset" => {
                for line in output.lines() {
                    if let Some(rest) = line.trim().strip_prefix("HEAD is now at ") {
                        let (hash, subject) = rest.split_once(' ').unwrap_or((rest, ""));
                        if is_hash(hash) {
                            events.push(GitEvent {
                                dir: g.dir.clone(),
                                hash: Some(hash.to_string()),
                                subject: (!subject.is_empty()).then(|| subject.to_string()),
                                ..GitEvent::of(GitAction::Reset)
                            });
                        }
                    }
                }
            }
            "branch" if g.args.iter().any(|a| matches!(a.as_str(), "-d" | "-D" | "--delete")) => {
                for line in output.lines() {
                    if let Some(rest) = line.trim().strip_prefix("Deleted branch ") {
                        let (branch, was) = rest.split_once(" (was ").unwrap_or((rest, ""));
                        let hash = was.trim_end_matches(").").trim_end_matches(')');
                        events.push(GitEvent {
                            dir: g.dir.clone(),
                            branch: Some(branch.to_string()),
                            hash: is_hash(hash).then(|| hash.to_string()),
                            ..GitEvent::of(GitAction::BranchDeleted)
                        });
                    }
                }
            }
            _ => {}
        }
    }

    for s in simple {
        let words: Vec<&str> = s.words.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["gh", "pr", "create", ..] => {
                if let Some((number, url)) = output.lines().find_map(pull_url) {
                    events.push(GitEvent {
                        dir: s.dir.clone(),
                        pr_number: Some(number),
                        url: Some(url),
                        ..GitEvent::of(GitAction::PrCreated)
                    });
                }
            }
            ["gh", "pr", "merge", rest @ ..] if ok && !output.contains("will be automatically merged") => {
                let number = rest
                    .iter()
                    .find(|a| !a.starts_with('-'))
                    .and_then(|a| a.trim_start_matches('#').parse::<i64>().ok().or_else(|| pull_url(a).map(|p| p.0)))
                    .or_else(|| output.lines().find_map(merged_number));
                events.push(GitEvent {
                    dir: s.dir.clone(),
                    pr_number: number.filter(|n| *n > 0),
                    ..GitEvent::of(GitAction::PrMerged)
                });
            }
            _ => {}
        }
    }
    events
}

/// The commits of `git commit` commands that printed no `[branch hash]`
/// line: run quietly (`-q`), or with their output cut. One counts when
/// what was printed shows it was made: a `git log --oneline` line with its
/// subject, git's `N files changed`, or, for a quiet commit, a command
/// that ended well with no word from git that nothing was committed.
fn quiet_commits(gits: &[Git<'_>], shown: &[GitEvent], output: &str, ok: bool) -> Vec<GitEvent> {
    let commits: Vec<&Git<'_>> =
        gits.iter().filter(|g| g.sub == "commit" && !g.args.iter().any(|a| a == "--dry-run")).collect();
    let printed = shown.iter().filter(|e| e.action == GitAction::Commit).count();
    if commits.len() <= printed {
        return Vec::new();
    }
    let refused = ["nothing to commit", "nothing added to commit", "no changes added to commit", "Aborting commit", "fatal: "]
        .iter()
        .any(|m| output.contains(m));
    let changed = output.lines().any(|l| {
        let l = l.trim_start();
        l.split_once(' ').is_some_and(|(n, rest)| n.parse::<u64>().is_ok() && rest.starts_with("file"))
            && l.contains(" changed")
    });
    let mut out = Vec::new();
    for g in commits {
        let subject = commit_message(g.args).and_then(|m| m.lines().next()).map(str::trim).filter(|s| !s.is_empty());
        if subject.is_some_and(|s| shown.iter().any(|e| e.subject.as_deref() == Some(s))) {
            continue;
        }
        let logged = subject.and_then(|s| {
            output.lines().find_map(|l| {
                let (hash, rest) = l.trim().split_once(' ')?;
                (is_hash(hash) && rest.trim() == s).then(|| hash.to_string())
            })
        });
        let quiet = g.args.iter().any(|a| a == "-q" || a == "--quiet" || (a.starts_with('-') && !a.starts_with("--") && a.contains('q')));
        let made = logged.is_some() || (!refused && (changed || (quiet && ok)));
        if made {
            out.push(GitEvent {
                dir: g.dir.clone(),
                hash: logged,
                subject: subject.map(str::to_string),
                ..GitEvent::of(GitAction::Commit)
            });
        }
    }
    out
}

/// A commit's message as the command gives it: `-m <msg>`, `-am <msg>`,
/// `-m<msg>` or `--message=<msg>`; the first one, which holds its subject.
fn commit_message(args: &[String]) -> Option<&str> {
    for (i, a) in args.iter().enumerate() {
        if a == "-m" || a == "--message" || (a.len() > 2 && !a.starts_with("--") && a.starts_with('-') && a.ends_with('m')) {
            return args.get(i + 1).map(String::as_str);
        }
        if let Some(m) = a.strip_prefix("--message=") {
            return Some(m);
        }
        if let Some(m) = a.strip_prefix("-m").filter(|m| !m.is_empty()) {
            return Some(m);
        }
    }
    None
}

/// `[branch hash] subject`: the line git prints for a commit it made.
/// Returns the branch, the hash and the subject.
fn new_commit(line: &str) -> Option<(String, String, String)> {
    let rest = line.strip_prefix('[')?;
    let (inside, subject) = rest.split_once("] ")?;
    let (branch, hash) = inside.rsplit_once(' ')?;
    if !is_hash(hash) {
        return None;
    }
    let branch = branch.trim_end_matches(" (root-commit)");
    Some((branch.to_string(), hash.to_string(), subject.to_string()))
}

/// The lines `git push` prints about refs: those after `To <remote>`, and
/// none of a fetch's, which follow `From <remote>`.
fn push_lines(output: &str) -> impl Iterator<Item = &str> {
    let mut pushing = false;
    output.lines().filter(move |l| {
        if l.starts_with("To ") {
            pushing = true;
            return false;
        }
        if l.starts_with("From ") {
            pushing = false;
        }
        pushing
    })
}

/// A ref `git push` updated or deleted, as it prints one.
fn pushed(line: &str, dir: Option<String>) -> Option<GitEvent> {
    let line = line.trim_start();
    let line = line.strip_prefix(['*', '+', '-', '=', ' ']).unwrap_or(line).trim_start();
    if let Some(rest) = line.strip_prefix("[deleted]") {
        return Some(GitEvent {
            dir,
            branch: Some(rest.split_whitespace().next()?.to_string()),
            ..GitEvent::of(GitAction::BranchDeleted)
        });
    }
    let (what, rest) = line.split_once(char::is_whitespace)?;
    let hash = if what.starts_with("[new branch]") || what == "[new" {
        None
    } else {
        let (_, new) = what.split_once("..")?;
        let new = new.trim_start_matches('.');
        if !is_hash(new) {
            return None;
        }
        Some(new.to_string())
    };
    let rest = if what == "[new" { rest.trim_start().strip_prefix("branch]")? } else { rest };
    let (_, to) = rest.split_once(" -> ")?;
    Some(GitEvent {
        dir,
        hash,
        branch: Some(to.split_whitespace().next()?.to_string()),
        ..GitEvent::of(GitAction::Push)
    })
}

/// A pull request's number and URL, from a line that holds its URL.
fn pull_url(line: &str) -> Option<(i64, String)> {
    let start = line.find("https://")?;
    let url = line[start..].split_whitespace().next()?;
    let (_, number) = url.rsplit_once("/pull/")?;
    let number: i64 = number.trim_end_matches('/').parse().ok()?;
    Some((number, url.to_string()))
}

/// `… merged pull request owner/repo#N …`: the number GitHub's CLI names.
fn merged_number(line: &str) -> Option<i64> {
    let at = line.to_lowercase().find("merged pull request")?;
    let rest = &line[at..];
    let hash = rest.find('#')?;
    rest[hash + 1..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

/// The test runner a command names; `None` within `Some` when it runs a
/// project's own test script, whose runner its output shows.
fn test_runner(s: &Simple) -> Option<Option<TestRunner>> {
    let w: Vec<&str> = s.words.iter().map(String::as_str).collect();
    match w.as_slice() {
        ["cargo", "test" | "t" | "nextest", ..] => Some(Some(TestRunner::Cargo)),
        ["go", "test", ..] => Some(Some(TestRunner::Go)),
        ["node", rest @ ..] if rest.contains(&"--test") => Some(Some(TestRunner::Node)),
        ["npm" | "pnpm" | "yarn" | "bun", "test" | "t", ..] => Some(None),
        ["npm" | "pnpm" | "yarn" | "bun", "run", script, ..] if script.starts_with("test") => Some(None),
        _ if w.iter().any(|x| matches!(*x, "pytest" | "py.test")) => Some(Some(TestRunner::Pytest)),
        _ if w.contains(&"vitest") => Some(Some(TestRunner::Vitest)),
        _ if w.contains(&"jest") => Some(Some(TestRunner::Jest)),
        _ => None,
    }
}

/// What a command that ran tests printed, summed over its suites; none
/// when it printed no summary its runner writes.
fn test(simple: &[Simple], output: &str) -> Option<TestRun> {
    let named = simple.iter().find_map(test_runner)?;
    let runners = match named {
        Some(r) => vec![r],
        None => vec![TestRunner::Vitest, TestRunner::Jest, TestRunner::Node, TestRunner::Pytest],
    };
    runners.into_iter().find_map(|runner| {
        let counts = match runner {
            TestRunner::Cargo => cargo_tests(output),
            TestRunner::Pytest => pytest(output),
            TestRunner::Jest => labelled(output, "Tests:", ','),
            TestRunner::Vitest => vitest(output),
            TestRunner::Go => go_tests(output),
            TestRunner::Node => node_tests(output),
        }?;
        Some(TestRun { runner, passed: counts.0, failed: counts.1, ignored: counts.2 })
    })
}

/// Passed, failed and not run.
type Tally = (i64, i64, i64);

/// `N label` pieces counted into a tally: a label it does not know makes
/// the piece no count at all.
fn tally(pieces: impl Iterator<Item = String>) -> Option<Tally> {
    let mut t = (0, 0, 0);
    let mut any = false;
    for piece in pieces {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (n, label) = piece.split_once(' ')?;
        let n: i64 = n.parse().ok()?;
        match label.trim() {
            "passed" | "xpassed" | "pass" => t.0 += n,
            "failed" | "error" | "errors" | "fail" => t.1 += n,
            "skipped" | "xfailed" | "todo" | "ignored" | "deselected" => t.2 += n,
            "total" | "tests" | "measured" | "filtered out" | "warning" | "warnings" | "rerun" => {}
            _ => return None,
        }
        any = true;
    }
    any.then_some(t)
}

/// cargo: `test result: ok. 12 passed; 0 failed; 1 ignored; …` per test
/// binary; nextest: `N tests run: N passed, N failed, N skipped`.
fn cargo_tests(output: &str) -> Option<Tally> {
    let mut sum: Option<Tally> = None;
    for line in output.lines() {
        let t = if let Some(rest) = line.split_once("test result: ").map(|(_, r)| r) {
            let rest = rest.split_once(". ").map_or(rest, |(_, r)| r);
            tally(rest.split(';').filter(|p| !p.trim_start().starts_with("finished in")).map(str::to_string))
        } else if let Some((_, rest)) = line.split_once(" tests run: ") {
            tally(rest.split(',').map(str::to_string))
        } else {
            None
        };
        if let Some(t) = t {
            let s = sum.get_or_insert((0, 0, 0));
            s.0 += t.0;
            s.1 += t.1;
            s.2 += t.2;
        }
    }
    sum
}

/// pytest's last summary: `== 1 failed, 2 passed, 3 skipped in 0.12s ==`.
fn pytest(output: &str) -> Option<Tally> {
    output.lines().rev().find_map(|line| {
        let line = line.trim().trim_matches('=').trim();
        let (counts, _) = line.rsplit_once(" in ")?;
        tally(counts.split(", ").map(str::to_string))
    })
}

/// A summary line that starts with `prefix`, its pieces split at `sep`.
fn labelled(output: &str, prefix: &str, sep: char) -> Option<Tally> {
    output.lines().rev().find_map(|line| {
        let rest = line.trim().strip_prefix(prefix)?;
        tally(rest.split(sep).map(|p| p.trim().to_string()))
    })
}

/// vitest: `Tests  1 failed | 10 passed | 2 skipped (13)`.
fn vitest(output: &str) -> Option<Tally> {
    output.lines().rev().find_map(|line| {
        let rest = line.trim().strip_prefix("Tests ")?;
        let rest = rest.rsplit_once(" (").map_or(rest, |(r, _)| r);
        tally(rest.split('|').map(|p| p.trim().to_string()))
    })
}

/// go: each `--- PASS/FAIL/SKIP:` a test; without `-v`, each package's
/// `ok` or `FAIL` line.
fn go_tests(output: &str) -> Option<Tally> {
    let mut t = (0, 0, 0);
    for line in output.lines() {
        match line.trim_start() {
            l if l.starts_with("--- PASS:") => t.0 += 1,
            l if l.starts_with("--- FAIL:") => t.1 += 1,
            l if l.starts_with("--- SKIP:") => t.2 += 1,
            _ => {}
        }
    }
    if t == (0, 0, 0) {
        for line in output.lines() {
            if line.starts_with("ok  \t") || line.starts_with("ok \t") {
                t.0 += 1;
            } else if line.starts_with("FAIL\t") {
                t.1 += 1;
            }
        }
    }
    (t != (0, 0, 0)).then_some(t)
}

/// node's test runner: `# pass 5`, `# fail 0` (TAP) or `ℹ pass 5`.
fn node_tests(output: &str) -> Option<Tally> {
    let mut t = (0, 0, 0);
    let mut any = false;
    for line in output.lines() {
        let Some(rest) = line.trim().strip_prefix("# ").or_else(|| line.trim().strip_prefix("ℹ ")) else {
            continue;
        };
        let Some((label, n)) = rest.split_once(' ') else { continue };
        let Ok(n) = n.trim().parse::<i64>() else { continue };
        match label {
            "pass" => t.0 += n,
            "fail" => t.1 += n,
            "skipped" | "todo" => t.2 += n,
            _ => continue,
        }
        any = true;
    }
    any.then_some(t)
}

/// The builder a command runs.
fn builder(s: &Simple) -> Option<Builder> {
    let w: Vec<&str> = s.words.iter().map(String::as_str).collect();
    match w.as_slice() {
        ["cargo", sub, ..]
            if matches!(
                *sub,
                "build" | "b" | "check" | "c" | "clippy" | "test" | "t" | "run" | "r" | "bench" | "doc" | "install" | "nextest"
            ) =>
        {
            Some(Builder::Cargo)
        }
        ["go", "build", ..] => Some(Builder::Go),
        ["npm" | "pnpm" | "yarn" | "bun", "run", script, ..] if script.starts_with("build") => Some(Builder::Npm),
        ["pnpm" | "yarn" | "bun", script, ..] if script.starts_with("build") => Some(Builder::Npm),
        _ if w.windows(2).any(|p| p == ["vite", "build"]) => Some(Builder::Vite),
        _ if w.iter().any(|x| matches!(*x, "tsc" | "vue-tsc")) => Some(Builder::Tsc),
        _ => None,
    }
}

/// What a command that built code printed.
fn build(simple: &[Simple], output: &str, ok: bool) -> Option<BuildRun> {
    let builder = simple.iter().find_map(builder)?;
    match builder {
        Builder::Cargo => cargo_build(output),
        Builder::Tsc => {
            let errors = found_errors(output).unwrap_or_else(|| count_lines(output, |l| l.contains("error TS")));
            Some(BuildRun { builder, ok: ok && errors == 0, errors, warnings: 0 })
        }
        Builder::Vite => {
            let errors = i64::from(output.contains("error during build"));
            Some(BuildRun { builder, ok: ok && errors == 0, errors, warnings: 0 })
        }
        Builder::Npm => {
            let errors =
                count_lines(output, |l| l.contains("error TS")) + i64::from(output.contains("error during build"));
            let failed = output.contains("npm ERR!") || output.lines().any(|l| l.starts_with("npm error"));
            Some(BuildRun { builder, ok: ok && errors == 0 && !failed, errors, warnings: 0 })
        }
        Builder::Go => {
            let errors = count_lines(output, go_error);
            Some(BuildRun { builder, ok: ok && errors == 0, errors, warnings: 0 })
        }
    }
}

/// cargo: built when it says `Finished`, failed when it says it could not
/// compile; errors and warnings from the summaries it prints for each
/// crate. Output cut before either shows only compile errors, when it
/// shows any; otherwise how the build ended is not known, and no build is
/// counted. A test that failed after the build is not a build error.
fn cargo_build(output: &str) -> Option<BuildRun> {
    let finished = output.lines().any(|l| l.trim_start().starts_with("Finished `"));
    let failed = output.contains("could not compile") || output.contains("error: aborting due to");
    let mut errors = 0;
    let mut warnings = 0;
    let mut warning_summaries = false;
    for line in output.lines() {
        if let Some(rest) = line.split_once("could not compile").and_then(|(_, r)| r.split_once("due to ")).map(|(_, r)| r) {
            errors += if rest.starts_with("previous error") {
                1
            } else {
                rest.split_whitespace().next().and_then(|n| n.parse::<i64>().ok()).unwrap_or(1)
            };
            if let Some((_, emitted)) = rest.split_once("; ") {
                warning_summaries = true;
                warnings += emitted.split_whitespace().next().and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
            }
        } else if line.trim_start().starts_with("warning:") {
            if let Some((_, rest)) = line.split_once(" generated ") {
                warning_summaries = true;
                warnings += rest.split_whitespace().next().and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
            }
        }
    }
    let diagnostics = count_lines(output, |l| l.starts_with("error["));
    if failed && errors == 0 {
        errors = diagnostics.max(1);
    }
    if !finished && !failed {
        if diagnostics == 0 {
            return None;
        }
        errors = diagnostics;
    }
    if !warning_summaries {
        warnings = count_lines(output, |l| l.starts_with("warning: ") && !l.contains(" generated "));
    }
    Some(BuildRun { builder: Builder::Cargo, ok: finished && !failed && errors == 0, errors, warnings })
}

/// tsc's `Found N errors`.
fn found_errors(output: &str) -> Option<i64> {
    output.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("Found ")?;
        let (n, label) = rest.split_once(' ')?;
        label.starts_with("error").then(|| n.parse().ok()).flatten()
    })
}

/// `file.go:12:3: message`: a Go compiler error.
fn go_error(line: &str) -> bool {
    line.split_once(".go:").is_some_and(|(_, rest)| rest.split(':').next().is_some_and(|n| n.parse::<u64>().is_ok()))
}

fn count_lines(output: &str, pred: impl Fn(&str) -> bool) -> i64 {
    i64::try_from(output.lines().filter(|l| pred(l)).count()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(command: &str) -> Vec<Vec<String>> {
        split(command, None).into_iter().map(|s| s.words).collect()
    }

    #[test]
    fn a_command_line_splits_as_a_shell_would() {
        assert_eq!(
            words("cd repo && git add a.rs b.rs; git commit -m \"fix: a && b\" | tail -3"),
            vec![
                vec!["git", "add", "a.rs", "b.rs"],
                vec!["git", "commit", "-m", "fix: a && b"],
                vec!["tail", "-3"],
            ]
        );
        assert_eq!(words("FOO=1 env BAR=2 time cargo test # a comment"), vec![vec!["cargo", "test"]]);
        assert_eq!(words("echo 'it'\\''s' \\\n  done"), vec![vec!["echo", "it's", "done"]]);
    }

    #[test]
    fn a_heredoc_body_is_input_not_commands() {
        let command = "git commit -F - <<'EOF'\ngit push origin main\nEOF\ngit status";
        assert_eq!(words(command), vec![vec!["git", "commit", "-F", "-"], vec!["git", "status"]]);
        let quoted = "git commit -m \"$(cat <<'EOF'\nsubject\n\ngit push in the body\nEOF\n)\"";
        let w = words(quoted);
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0][..2], ["git", "commit"]);
    }

    #[test]
    fn cd_is_followed_and_git_c_names_its_own_directory() {
        let s = split("cd sub && cd ../other && git -C /abs/repo log && git status", Some("/home/me/work"));
        assert_eq!(s[0].dir.as_deref(), Some("/home/me/work/other"));
        let g = git_command(&s[0]).expect("git");
        assert_eq!((g.sub, g.dir.as_deref()), ("log", Some("/abs/repo")));
        assert_eq!(git_command(&s[1]).expect("git").dir.as_deref(), Some("/home/me/work/other"));
    }

    #[test]
    fn a_commit_is_taken_from_the_line_git_prints() {
        let out = "[feat/508-lines 4b9bba8] feat(lines): the crawler\n 3 files changed\n";
        let ran = read("git add -A && git commit -m x", out, true, Some("/repo"));
        assert_eq!(
            ran.git,
            vec![GitEvent {
                dir: Some("/repo".to_string()),
                hash: Some("4b9bba8".to_string()),
                branch: Some("feat/508-lines".to_string()),
                subject: Some("feat(lines): the crawler".to_string()),
                ..GitEvent::of(GitAction::Commit)
            }]
        );
        let root = read("git commit -m first", "[main (root-commit) 0a1b2c3] first\n", true, None);
        assert_eq!(root.git[0].branch.as_deref(), Some("main"));
        let detached = read("git commit -m x", "[detached HEAD 0a1b2c3] x\n", true, None);
        assert_eq!(detached.git[0].branch.as_deref(), Some("detached HEAD"));
    }

    #[test]
    fn a_quiet_commit_is_found_by_what_followed_it() {
        let logged = read(
            "cd repo\ngit add a\ngit commit -q -m \"fix: repair the tooling\n\nbody\"\necho done\ngit log --oneline -1",
            "done\nb958c7d fix: repair the tooling\n",
            true,
            None,
        );
        assert_eq!(
            logged.git,
            vec![GitEvent {
                dir: Some("repo".to_string()),
                hash: Some("b958c7d".to_string()),
                subject: Some("fix: repair the tooling".to_string()),
                ..GitEvent::of(GitAction::Commit)
            }]
        );
        let silent = read("git commit -qam 'wip' && echo ok", "ok\n", true, None);
        assert_eq!((silent.git.len(), silent.git[0].hash.as_deref(), silent.git[0].subject.as_deref()), (1, None, Some("wip")));
        let cut = read("git commit -m x | tail -2", " 2 files changed, 151 deletions(-)\n delete mode 100644 a.py\n", true, None);
        assert_eq!(cut.git.len(), 1, "git's summary shows the commit was made");
        assert!(read("git commit -q -m x", "nothing to commit, working tree clean\n", false, None).git.is_empty());
        assert!(read("git commit -q -m x; echo done", "nothing to commit, working tree clean\ndone\n", true, None).git.is_empty());
        assert!(read("git commit --dry-run -m x", "", true, None).git.is_empty());
        let both = read("git commit -m one && git commit -q -m two", "[main 1111111] one\n", true, None);
        assert_eq!(both.git.iter().map(|e| e.subject.as_deref()).collect::<Vec<_>>(), [Some("one"), Some("two")]);
    }

    #[test]
    fn a_failed_commit_or_a_mention_of_git_is_no_commit() {
        assert!(read("git commit -m x", "nothing to commit, working tree clean\n", false, None).git.is_empty());
        assert!(read("cat log.txt", "[main 0a1b2c3] an old commit\n", true, None).git.is_empty());
        assert!(read("echo git push origin main", "git push origin main\n", true, None).git.is_empty());
    }

    #[test]
    fn a_revert_and_a_cherry_pick_are_named_for_what_made_them() {
        let ran = read("git revert --no-edit abc1234", "[main 9f8e7d6] Revert \"x\"\n", true, None);
        assert_eq!(ran.git[0].action, GitAction::Revert);
        let ran = read("git cherry-pick a b", "[main 1111111] one\n[main 2222222] two\n", true, None);
        assert_eq!(ran.git.iter().map(|e| e.action).collect::<Vec<_>>(), [GitAction::CherryPick; 2]);
    }

    #[test]
    fn a_push_names_each_ref_it_updated_and_deleted() {
        let out = "To github.com:k8nstantin/superx.git\n   5574e81..4b9bba8  main -> main\n * [new branch]      feat/x -> feat/x\n + 1a2b3c4...5d6e7f8 wip -> wip (forced update)\n - [deleted]         old/branch\n ! [rejected]        dev -> dev (fetch first)\n";
        let ran = read("git push origin main feat/x", out, true, Some("/repo"));
        let got: Vec<(GitAction, Option<&str>, Option<&str>)> =
            ran.git.iter().map(|e| (e.action, e.branch.as_deref(), e.hash.as_deref())).collect();
        assert_eq!(
            got,
            vec![
                (GitAction::Push, Some("main"), Some("4b9bba8")),
                (GitAction::Push, Some("feat/x"), None),
                (GitAction::Push, Some("wip"), Some("5d6e7f8")),
                (GitAction::BranchDeleted, Some("old/branch"), None),
            ]
        );
        assert!(read("git push", "Everything up-to-date\n", true, None).git.is_empty());
    }

    #[test]
    fn merge_rebase_reset_and_branch_deletion_are_confirmed_by_their_output() {
        let merged = read("git merge origin/main", "Updating 1a2b3c4..5d6e7f8\nFast-forward\n a | 1 +\n", true, None);
        assert_eq!((merged.git[0].action, merged.git[0].hash.as_deref()), (GitAction::Merge, Some("5d6e7f8")));
        assert_eq!(merged.git[0].branch.as_deref(), Some("origin/main"));
        assert!(read("git merge origin/main", "Already up to date.\n", true, None).git.is_empty());
        let rebased = read("git rebase main", "Successfully rebased and updated refs/heads/feat/x.\n", true, None);
        assert_eq!((rebased.git[0].action, rebased.git[0].branch.as_deref()), (GitAction::Rebase, Some("feat/x")));
        let reset = read("git reset --hard origin/main", "HEAD is now at 4b9bba8 feat: x\n", true, None);
        assert_eq!(
            (reset.git[0].action, reset.git[0].hash.as_deref(), reset.git[0].subject.as_deref()),
            (GitAction::Reset, Some("4b9bba8"), Some("feat: x"))
        );
        let deleted = read("git branch -D feat/506-x", "Deleted branch feat/506-x (was ade5a50).\n", true, None);
        assert_eq!(
            (deleted.git[0].action, deleted.git[0].branch.as_deref(), deleted.git[0].hash.as_deref()),
            (GitAction::BranchDeleted, Some("feat/506-x"), Some("ade5a50"))
        );
        assert!(read("git checkout abc1234", "HEAD is now at abc1234 x\n", true, None).git.is_empty());
    }

    #[test]
    fn pull_requests_created_and_merged() {
        let created = read(
            "gh pr create --title t --body-file b.md",
            "https://github.com/k8nstantin/superx/pull/507\n",
            true,
            None,
        );
        assert_eq!(
            (created.git[0].action, created.git[0].pr_number, created.git[0].url.as_deref()),
            (GitAction::PrCreated, Some(507), Some("https://github.com/k8nstantin/superx/pull/507"))
        );
        let merged = read("gh pr merge 507 --squash --admin", "", true, None);
        assert_eq!((merged.git[0].action, merged.git[0].pr_number), (GitAction::PrMerged, Some(507)));
        let by_output = read("gh pr merge --squash", "✓ Squashed and merged pull request k8nstantin/superx#505 (x)\n", true, None);
        assert_eq!(by_output.git[0].pr_number, Some(505));
        assert!(read("gh pr merge 507", "GraphQL: not mergeable", false, None).git.is_empty());
        assert!(read("gh pr merge 9 --auto", "✓ Pull request #9 will be automatically merged", true, None).git.is_empty());
    }

    #[test]
    fn cargo_tests_are_summed_over_every_test_binary() {
        let out = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\nrunning 5 tests\ntest result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s\n";
        assert_eq!(
            read("cargo test --workspace 2>&1 | tail -40", out, false, None).test,
            Some(TestRun { runner: TestRunner::Cargo, passed: 7, failed: 1, ignored: 1 })
        );
        assert_eq!(read("cat target/test.log", out, true, None).test, None, "no test command ran");
        assert_eq!(read("cargo test", "error[E0425]: cannot find value `x`\n", false, None).test, None);
    }

    #[test]
    fn other_runners_are_read_by_their_own_summaries() {
        let pytest = read("python -m pytest -q", "..F.\n=== 1 failed, 3 passed, 2 skipped in 0.42s ===\n", false, None);
        assert_eq!(pytest.test, Some(TestRun { runner: TestRunner::Pytest, passed: 3, failed: 1, ignored: 2 }));
        let jest = read("npx jest", "Tests:       1 failed, 2 skipped, 10 passed, 13 total\n", false, None);
        assert_eq!(jest.test, Some(TestRun { runner: TestRunner::Jest, passed: 10, failed: 1, ignored: 2 }));
        let vitest = read("npm test", " Test Files  1 passed (1)\n      Tests  12 passed | 1 skipped (13)\n", true, None);
        assert_eq!(vitest.test, Some(TestRun { runner: TestRunner::Vitest, passed: 12, failed: 0, ignored: 1 }));
        let go = read("go test ./...", "ok  \texample.com/a\t0.01s\nFAIL\texample.com/b\t0.02s\n", false, None);
        assert_eq!(go.test, Some(TestRun { runner: TestRunner::Go, passed: 1, failed: 1, ignored: 0 }));
        let node = read("node --test", "# tests 4\n# pass 3\n# fail 1\n# skipped 0\n", false, None);
        assert_eq!(node.test, Some(TestRun { runner: TestRunner::Node, passed: 3, failed: 1, ignored: 0 }));
    }

    #[test]
    fn a_cargo_build_counts_its_errors_and_warnings_from_its_summaries() {
        let ok = "   Compiling a v0.1.0\nwarning: `a` (lib) generated 2 warnings\n    Finished `release` profile [optimized] target(s) in 15m 53s\n";
        assert_eq!(
            read("cargo build --release", ok, true, None).build,
            Some(BuildRun { builder: Builder::Cargo, ok: true, errors: 0, warnings: 2 })
        );
        let failed = "error[E0425]: cannot find value `x`\nerror[E0308]: mismatched types\nerror: could not compile `a` (lib) due to 2 previous errors; 1 warning emitted\n";
        assert_eq!(
            read("cargo clippy -- -D warnings", failed, false, None).build,
            Some(BuildRun { builder: Builder::Cargo, ok: false, errors: 2, warnings: 1 })
        );
        assert_eq!(read("cargo fmt --check", "", true, None).build, None);
        let tests_failed = "    Finished `test` profile [unoptimized] target(s) in 1.2s\ntest result: FAILED. 1 passed; 1 failed;\nerror: test failed, to rerun pass `-p a --lib`\n";
        assert_eq!(
            read("cargo test", tests_failed, false, None).build,
            Some(BuildRun { builder: Builder::Cargo, ok: true, errors: 0, warnings: 0 }),
            "the build was fine; a test failed"
        );
        let cut = "error[E0428]: the name `X` is defined multiple times\nerror[E0119]: conflicting implementations\n";
        assert_eq!(
            read("cargo build 2>&1 | grep -E '^error' | head", cut, true, None).build,
            Some(BuildRun { builder: Builder::Cargo, ok: false, errors: 2, warnings: 0 })
        );
        assert_eq!(read("cargo build | tail -1", "   Compiling a v0.1.0\n", true, None).build, None, "cut before it ended");
        assert_eq!(read("cargo build", "", false, None).build, None, "a build that printed nothing is not known");
    }

    #[test]
    fn tsc_vite_npm_and_go_builds() {
        let tsc = read("npx tsc --noEmit", "src/a.ts(1,1): error TS2304: x\nFound 1 error in src/a.ts:1\n", false, None);
        assert_eq!(tsc.build, Some(BuildRun { builder: Builder::Tsc, ok: false, errors: 1, warnings: 0 }));
        assert_eq!(read("npx tsc -b", "", true, None).build, Some(BuildRun { builder: Builder::Tsc, ok: true, errors: 0, warnings: 0 }));
        let vite = read("npx vite build", "vite v5.4.0 building for production...\n✓ built in 2.31s\n", true, None);
        assert_eq!(vite.build, Some(BuildRun { builder: Builder::Vite, ok: true, errors: 0, warnings: 0 }));
        let npm = read("npm run build", "npm error code 2\n", false, None);
        assert_eq!(npm.build, Some(BuildRun { builder: Builder::Npm, ok: false, errors: 0, warnings: 0 }));
        let go = read("go build ./...", "./main.go:12:3: undefined: x\n", false, None);
        assert_eq!(go.build, Some(BuildRun { builder: Builder::Go, ok: false, errors: 1, warnings: 0 }));
    }
}
