//! The trail (#539): a session's work followed into its commit and what
//! became of it, and the findings, the window and the commit read from it.

pub mod common;

use common::*;
use superx_mod_recon::crawler::Crawler;
use superx_mod_recon::params::{BULK_LINES_PARAM, INCLUDE_PARAM};
use superx_mod_recon::results::commit::commit as whole;
use superx_mod_recon::results::findings::findings;
use superx_mod_recon::results::trail::load;
use superx_mod_recon::results::window::window;

/// An assistant line of `model` in `cwd`, one content block of reply `id`.
fn reply(cwd: &std::path::Path, id: &str, model: &str, content: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant", "cwd": cwd, "gitBranch": "main", "requestId": "req", "isSidechain": false,
        "message": { "id": id, "model": model, "stop_reason": "tool_use", "content": [content],
            "usage": { "input_tokens": 5, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 2000, "output_tokens": 40 } }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_is_followed_from_the_session_that_tested_and_printed_it_to_what_landed_and_lives() {
    const OPUS: &str = "claude-opus-5-5";
    const SONNET: &str = "claude-sonnet-5-5";
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "one\n")], "first");
    let second = commit(&dir, &[("a.txt", "one\ntwo\nthree\n")], "second");

    // Opus edits, is corrected, runs the tests and commits; Sonnet answers
    // in a session of its own, in no repository.
    let mut c = Capture::new(&kernel, "one").await;
    c.worked_in(&dir).await;
    c.message(reply(&dir, "a1", OPUS, serde_json::json!({ "type": "tool_use", "id": "call_edit", "name": "Edit", "input": { "file_path": "/repo/a.rs", "old_string": "a", "new_string": "b" } }))).await;
    c.message(serde_json::json!({
        "type": "user", "cwd": &dir, "gitBranch": "main",
        "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_edit", "content": "updated", "is_error": false }] },
        "toolUseResult": { "filePath": "/repo/a.rs", "oldString": "a", "newString": "b", "userModified": false, "structuredPatch": [{ "lines": ["-a", "+b", "+c"] }] }
    }))
    .await;
    c.message(serde_json::json!({ "type": "user", "cwd": &dir, "gitBranch": "main", "origin": { "kind": "human" }, "message": { "role": "user", "content": "no, do it again" } })).await;
    c.bash(&dir, "cargo test", "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s").await;
    c.bash(&dir, "git commit -m 'second'", &format!("[main {}] second\n 1 file changed, 2 insertions(+)", &second[..7])).await;
    let mut d = Capture::new(&kernel, "two").await;
    d.message(reply(std::path::Path::new("/nowhere"), "s1", SONNET, serde_json::json!({ "type": "text", "text": "Done." }))).await;
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;

    let p = params(&[]);
    let trail = load(&db, &p).await.expect("the trail");
    let found = findings(&trail, &p);
    let c = found.commits.iter().find(|c| c.hash == second).unwrap_or_else(|| panic!("{:?}", found.commits));
    assert_eq!(
        (c.repo.as_str(), c.model.as_str(), c.by, c.place.as_deref(), c.in_flight, c.files),
        ("work", "opus 5.5", Some("printed"), Some("mainline"), false, 1),
        "the session's own output named it: {c:?}"
    );
    assert_eq!((c.added, c.landed, c.alive, c.never_landed), (2, 2, Some(2), 0), "on the main line, all of it alive at the tip");
    assert_eq!((c.tested, c.tests_green, c.built), (Some(true), Some(true), Some(false)), "it ran cargo test, green, before committing");
    let week = found.weeks.iter().find(|w| w.model == "opus 5.5" && w.repo == "work").unwrap_or_else(|| panic!("{:?}", found.weeks));
    assert_eq!(
        (week.replies, week.edits, week.edit_added, week.edit_removed, week.tests, week.tests_failed),
        (3, 1, 2, 1, 1, 0),
        "the edit's, the test's and the commit's replies: {week:?}"
    );
    assert_eq!((week.prompts, week.corrections, week.escalations), (2, 1, 0), "the operator's two prompts, the second correcting Opus");
    assert!(found.weeks.iter().any(|w| w.model == "sonnet 5.5" && w.repo.is_empty() && w.replies == 1), "Sonnet's reply, in no repository: {:?}", found.weeks);

    // A window on the work holds both sessions, each where it worked; a
    // window on the commits' day, the commits (the helper dates them).
    let now = chrono::Utc::now();
    let w = window(&trail, &p, now - chrono::TimeDelta::days(1), now + chrono::TimeDelta::days(1), 3600);
    let work = w.repos.iter().find(|r| r.repo == "work").unwrap_or_else(|| panic!("{w:?}"));
    let t = &work.totals;
    assert_eq!(
        (t.sessions, t.replies, t.prompts, t.corrections, t.edits, t.edit_added, t.edit_removed, t.tests, t.commits),
        (1, 3, 2, 1, 1, 2, 1, 1, 0),
        "{t:?}"
    );
    assert_eq!(work.timeline.iter().map(|s| s.replies).sum::<i64>(), 3, "every reply in a step");
    assert_eq!(work.checks.iter().map(|k| (k.test, k.passed, k.failed)).collect::<Vec<_>>(), [(true, 3, 0)]);
    assert_eq!(work.files.iter().map(|f| (f.path.as_str(), f.edits, f.edit_added)).collect::<Vec<_>>(), [("/repo/a.rs", 1, 2)]);
    let nowhere = w.repos.iter().find(|r| r.repo.is_empty()).unwrap_or_else(|| panic!("{w:?}"));
    assert_eq!((nowhere.totals.replies, nowhere.totals.sessions), (1, 1), "Sonnet's reply is somewhere");
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).expect("a time").with_timezone(&chrono::Utc);
    let day = window(&trail, &p, at("2026-10-01T00:00:00Z"), at("2026-10-02T00:00:00Z"), 3600);
    let work = day.repos.iter().find(|r| r.repo == "work").unwrap_or_else(|| panic!("{day:?}"));
    assert_eq!((work.totals.commits, work.totals.commits_mainline, work.totals.replies), (2, 2, 0), "{:?}", work.totals);
    assert_eq!(work.models.iter().map(|m| (m.model.as_str(), m.commits, m.landed)).collect::<Vec<_>>(), [("opus 5.5", 1, 2)], "its credited commit");
    let mine = work.commits.iter().find(|c| c.hash == second).expect("the commit");
    assert_eq!(mine.files.iter().map(|f| (f.path.as_str(), f.added, f.removed)).collect::<Vec<_>>(), [("a.txt", 2, 0)]);
    let before = window(&trail, &p, at("2026-09-01T00:00:00Z"), at("2026-09-02T00:00:00Z"), 3600);
    assert!(before.repos.is_empty(), "nothing was done then: {before:?}");

    // The commit, whole.
    let one = whole(&trail, "work", &second).expect("the commit");
    assert_eq!((one.model.as_str(), one.by, one.landed, one.alive), ("opus 5.5", Some("printed"), 2, Some(2)));
    assert_eq!(one.checks.iter().map(|k| (k.what.as_str(), k.passed)).collect::<Vec<_>>(), [("cargo", 3)], "the test its session ran before it");
    assert!(whole(&trail, "elsewhere", &second).is_none(), "named by its repository");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_rewritten_onto_the_main_line_landed_and_counts_once_as_the_copy_its_session_made() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    let lines = |first: &str, last: &str| format!("{first}\n2\n3\n4\n5\n6\n7\n8\n9\n{last}\n");
    commit(&dir, &[("a.txt", &lines("1", "10"))], "first");
    // The session's change, on a branch: line 1.
    git(&dir, &["checkout", "-q", "-b", "feat"]);
    let made = commit(&dir, &[("a.txt", &lines("one", "10"))], "work");
    // The main line changes line 10, then takes the change: the same patch,
    // another file version, another hash.
    git(&dir, &["checkout", "-q", "main"]);
    commit(&dir, &[("a.txt", &lines("1", "ten"))], "tail");
    git_at(&dir, &["cherry-pick", &made], Some("2026-10-02T12:00:00+02:00"));
    let landed = git(&dir, &["rev-parse", "HEAD"]);
    assert_ne!(landed, made, "the same change under another hash");

    let mut c = Capture::new(&kernel, "one").await;
    c.worked_in(&dir).await;
    c.bash(&dir, "git commit -am 'work'", &format!("[feat {}] work\n 1 file changed, 1 insertion(+), 1 deletion(-)", &made[..7])).await;
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;

    let p = params(&[]);
    let trail = load(&db, &p).await.expect("the trail");
    let found = findings(&trail, &p);
    let work = found.commits.iter().find(|c| c.hash == made).unwrap_or_else(|| panic!("{:?}", found.commits));
    assert_eq!(
        (work.model.as_str(), work.by, work.place.as_deref(), work.landed_as.as_deref(), work.in_flight),
        ("opus 5.5", Some("printed"), Some("branch"), Some(landed.as_str()), false),
        "the session's copy, landed as the main line's: {work:?}"
    );
    assert_eq!((work.added, work.landed, work.never_landed, work.alive), (1, 1, 0, Some(1)), "its line landed and is alive at the tip: {work:?}");
    assert!(found.commits.iter().all(|c| c.hash != landed), "the main line's copy is not counted again: {:?}", found.commits);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_merge_authors_nothing_a_squash_keeps_its_pieces_blame_and_lines_are_put_to_the_test_where_the_main_line_changed_again() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    let lines = |prefix: &str, n: usize| (1..=n).map(|i| format!("{prefix}{i}\n")).collect::<String>();
    commit_at(&dir, &[("base.txt", "base\n")], "base", "2026-10-01T09:00:00+00:00");
    // An attempt abandoned: one bulk drop, with a file the next attempt
    // writes the same.
    git(&dir, &["checkout", "-q", "-b", "v1"]);
    let drop = commit_at(&dir, &[("big.txt", &lines("b", 30)), ("shared.txt", &lines("s", 3))], "v1: all at once", "2026-10-01T10:00:00+00:00");
    // The attempt that landed, in two pieces, squash-merged: a bulk commit
    // too, as #557 was.
    git(&dir, &["checkout", "-q", "main"]);
    git(&dir, &["checkout", "-q", "-b", "v2"]);
    let first = commit_at(&dir, &[("shared.txt", &lines("s", 3)), ("x.txt", &lines("x", 5))], "v2: x", "2026-10-01T11:00:00+00:00");
    let second = commit_at(&dir, &[("y.txt", &lines("y", 5))], "v2: y", "2026-10-01T11:30:00+00:00");
    git(&dir, &["checkout", "-q", "main"]);
    git(&dir, &["merge", "-q", "--squash", "v2"]);
    git_at(&dir, &["commit", "-q", "-m", "v2 (#2)"], Some("2026-10-01T12:00:00+00:00"));
    let squash = git(&dir, &["rev-parse", "HEAD"]);
    // A branch merged by a merge commit: its lines are its commit's.
    git(&dir, &["checkout", "-q", "-b", "side"]);
    let side = commit_at(&dir, &[("z.txt", &lines("z", 3))], "side: z", "2026-10-01T13:00:00+00:00");
    git(&dir, &["checkout", "-q", "main"]);
    git_at(&dir, &["merge", "-q", "--no-ff", "-m", "Merge side", "side"], Some("2026-10-01T14:00:00+00:00"));
    let merge = git(&dir, &["rev-parse", "HEAD"]);
    // The main line changes x.txt again; y.txt and shared.txt never.
    let later = commit_at(&dir, &[("x.txt", &format!("{}X5\n", lines("x", 4)))], "x again", "2026-10-02T09:00:00+00:00");

    let mut c = Capture::new(&kernel, "one").await;
    c.worked_in(&dir).await;
    for (hash, branch, subject) in [
        (&drop, "v1", "v1: all at once"),
        (&first, "v2", "v2: x"),
        (&second, "v2", "v2: y"),
        (&side, "side", "side: z"),
        (&merge, "main", "Merge side"),
        (&later, "main", "x again"),
    ] {
        c.bash(&dir, &format!("git commit -m '{subject}'"), &format!("[{branch} {}] {subject}\n 1 file changed", &hash[..7])).await;
    }
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir])), (BULK_LINES_PARAM, number(12))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;

    let trail = load(&db, &p).await.expect("the trail");
    let found = findings(&trail, &p);
    let get = |hash: &str| found.commits.iter().find(|c| c.hash == hash).unwrap_or_else(|| panic!("{hash}: {:?}", found.commits));
    // The merge authored nothing: the side commit's lines are the side commit's.
    let m = get(&merge);
    assert_eq!((m.merge, m.added, m.landed, m.model.as_str()), (true, 0, 0, "opus 5.5"), "{m:?}");
    assert_eq!((get(&side).landed, get(&side).merge), (3, false));
    // The squash's blame went to its pieces, though an abandoned drop
    // landed a file version through it too: the same lines as a piece's.
    assert!(found.commits.iter().all(|c| c.hash != squash), "the squash is no one's: {:?}", found.commits);
    assert!(get(&drop).bulk, "the drop counts nowhere");
    let (x, y) = (get(&first), get(&second));
    assert_eq!((x.fate, x.landed, y.fate, y.landed), ("landed", 8, "landed", 5), "{x:?} {y:?}");
    let (xa, ya) = (x.alive.unwrap_or(-1), y.alive.unwrap_or(-1));
    assert!(xa > 0 && ya > 0 && xa <= 8 && ya <= 5 && xa + ya <= 12, "the squash's 12 alive lines, split over its pieces: {x:?} {y:?}");
    // Put to the test: x.txt changed again on the main line after it
    // landed; shared.txt and y.txt never did.
    assert_eq!((x.landed_changed, y.landed_changed), (5, 0), "{x:?} {y:?}");
    // One row per file, model and kind of credit, the main line's changes
    // to it beside.
    let xf = found.files.iter().find(|f| f.path == "x.txt").unwrap_or_else(|| panic!("{:?}", found.files));
    assert_eq!((xf.model.as_str(), xf.printed, xf.added, xf.landed, xf.never, xf.main_changes), ("opus 5.5", true, 6, 6, 0, 2), "{xf:?}");
    assert_eq!(xf.commits.iter().map(|&i| found.commits[i].hash.as_str()).collect::<Vec<_>>(), [first.as_str(), later.as_str()]);
    assert!(found.files.iter().all(|f| f.path != "big.txt"), "a bulk drop authors no file's lines: {:?}", found.files);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_commit_has_one_fate_and_a_snapshot_counts_nowhere() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    let base = commit(&dir, &[("a.txt", "one\n")], "first");
    // A savepoint kept while the work goes on, then dropped.
    let savepoint = commit(&dir, &[("a.txt", "one\ntwo\n")], "ENG-1 SAVEPOINT (not for merge): halfway");
    git(&dir, &["reset", "-q", "--hard", &base]);
    // Work reset away, to be done again.
    let thrown = commit(&dir, &[("b.txt", "x\n")], "try one");
    git(&dir, &["reset", "-q", "--hard", &base]);
    // A branch never merged.
    git(&dir, &["checkout", "-q", "-b", "idea"]);
    let parked = commit(&dir, &[("c.txt", "y\n")], "an idea");
    git(&dir, &["checkout", "-q", "main"]);
    // Work stashed: git's own commits.
    std::fs::write(dir.join("a.txt"), "one\nstashed\n").expect("an edit");
    git(&dir, &["stash", "-q"]);
    let stashed = git(&dir, &["rev-parse", "refs/stash"]);
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;

    let trail = load(&db, &params(&[])).await.expect("the trail");
    let fate = |hash: &str| trail.commits.iter().find(|c| c.hash == hash).map(|c| (c.place.as_deref().unwrap_or_default().to_string(), c.fate));
    assert_eq!(fate(&base), Some(("mainline".to_string(), "mainline")));
    assert_eq!(fate(&savepoint), Some(("recovered".to_string(), "snapshot")), "its subject says it is not for merge");
    assert_eq!(fate(&thrown), Some(("recovered".to_string(), "reset")), "only the reflog holds it");
    assert_eq!(fate(&parked), Some(("branch".to_string(), "branch")), "a branch holds it and it never landed");
    assert_eq!(fate(&stashed).map(|f| f.1), Some("snapshot"), "git's own stash commit");
    std::fs::remove_dir_all(&root).expect("clean up");
}
