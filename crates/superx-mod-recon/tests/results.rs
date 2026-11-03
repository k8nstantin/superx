//! The tab's Results (#539): the models, day by day and over all time, from
//! the rows the crawler wrote of captured lines.

pub mod common;

use common::*;
use superx_mod_recon::crawler::Crawler;
use superx_mod_recon::params::INCLUDE_PARAM;
use superx_mod_recon::results::compare::compare;
use superx_mod_recon::results::{day, models, Measures};

/// An assistant line of `model`, one content block of reply `id`.
fn reply(id: &str, model: &str, content: serde_json::Value, output: i64, extra: serde_json::Value) -> serde_json::Value {
    let mut line = serde_json::json!({
        "type": "assistant", "cwd": "/repo", "gitBranch": "main", "requestId": "req", "isSidechain": false,
        "message": { "id": id, "model": model, "stop_reason": "tool_use", "content": [content],
            "usage": { "input_tokens": 5, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 2000, "output_tokens": output,
                "output_tokens_details": { "thinking_tokens": 10 } } }
    });
    if let (Some(l), serde_json::Value::Object(e)) = (line.as_object_mut(), extra) {
        l.extend(e);
    }
    line
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_model_counts_its_replies_newest_tokens_thinking_tool_calls_edited_lines_sessions_and_the_commits_tied_to_its_calls() {
    const OPUS: &str = "claude-opus-5-5";
    const SONNET: &str = "claude-sonnet-5-5";
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "one\n")], "first");
    let second = commit(&dir, &[("a.txt", "one\ntwo\nthree\n")], "second");

    let mut c = Capture::new(&kernel, "one").await;
    c.worked_in(&dir).await;
    c.message(reply("a1", OPUS, serde_json::json!({ "type": "thinking", "thinking": "", "signature": "s" }), 900, serde_json::json!({ "thinkingDurationMs": 3100 }))).await;
    c.message(reply("a1", OPUS, serde_json::json!({ "type": "text", "text": "Editing." }), 900, serde_json::json!({}))).await;
    c.message(reply("a2", OPUS, serde_json::json!({ "type": "tool_use", "id": "call_edit", "name": "Edit", "input": { "file_path": "/repo/a.rs", "old_string": "a", "new_string": "b" } }), 800, serde_json::json!({}))).await;
    c.message(serde_json::json!({
        "type": "user", "cwd": "/repo", "gitBranch": "main",
        "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_edit", "content": "updated", "is_error": false }] },
        "toolUseResult": { "filePath": "/repo/a.rs", "oldString": "a", "newString": "b", "userModified": false, "structuredPatch": [{ "lines": ["-a", "+b", "+c"] }] }
    }))
    .await;
    // The operator corrects Opus, before Sonnet takes over.
    c.message(serde_json::json!({ "type": "user", "cwd": "/repo", "gitBranch": "main", "origin": { "kind": "human" }, "message": { "role": "user", "content": "no, do it again" } })).await;
    c.message(reply("s3", SONNET, serde_json::json!({ "type": "tool_use", "id": "call_ls", "name": "Bash", "input": { "command": "ls" } }), 300, serde_json::json!({}))).await;
    // Opus commits: its Bash call printed the new commit's hash.
    c.bash(&dir, "git commit -m 'second'", &format!("[main {}] second\n 1 file changed, 2 insertions(+)", &second[..7])).await;
    let mut d = Capture::new(&kernel, "two").await;
    d.message(reply("s9", SONNET, serde_json::json!({ "type": "text", "text": "Done." }), 200, serde_json::json!({}))).await;

    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;
    assert_eq!(count(&db, "commit_session").await, 1, "the commit tied to the call that printed it");

    let read = models(&db).await.expect("models");
    let total = |m: &str| read.totals.iter().find(|t| t.model == m).unwrap_or_else(|| panic!("{m}: {read:?}")).clone();
    let opus = total(OPUS);
    assert_eq!(
        (opus.sessions, opus.measures.replies, opus.measures.thinking_spans, opus.measures.thinking_ms, opus.measures.tool_calls),
        (1, 3, 1, 3100, 2),
        "two replies of its own and the one that committed; the edit and the commit's calls: {opus:?}"
    );
    assert_eq!((opus.measures.lines_added, opus.measures.lines_removed), (2, 1), "the edit's patch");
    assert_eq!(
        (opus.measures.output_tokens, opus.measures.input_tokens, opus.measures.cache_read_tokens),
        (900 + 800 + 1, 5 + 5 + 1, 2000 + 2000),
        "each reply's newest state, once however many lines repeat it"
    );
    assert_eq!((opus.measures.commits, opus.measures.commit_lines_added, opus.measures.commit_lines_removed), (1, 2, 0), "the commit tied to its call");
    let sonnet = total(SONNET);
    assert_eq!(
        (sonnet.sessions, sonnet.measures.replies, sonnet.measures.tool_calls, sonnet.measures.thinking_spans, sonnet.measures.output_tokens, sonnet.measures.commits),
        (2, 2, 1, 0, 500, 0),
        "{sonnet:?}"
    );
    assert_eq!(read.totals[0].model, OPUS, "the model with the most replies first");

    // The days add up to the totals, model by model.
    for t in &read.totals {
        let mut summed = Measures::default();
        for d in read.days.iter().filter(|d| d.model == t.model) {
            summed.replies += d.measures.replies;
            summed.output_tokens += d.measures.output_tokens;
            summed.thinking_ms += d.measures.thinking_ms;
            summed.tool_calls += d.measures.tool_calls;
            summed.lines_added += d.measures.lines_added;
            summed.commits += d.measures.commits;
        }
        assert_eq!(
            (summed.replies, summed.output_tokens, summed.thinking_ms, summed.tool_calls, summed.lines_added, summed.commits),
            (t.measures.replies, t.measures.output_tokens, t.measures.thinking_ms, t.measures.tool_calls, t.measures.lines_added, t.measures.commits),
            "{}",
            t.model
        );
    }

    // What is behind a bar: the day of the commit, and the day of the work.
    let on = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").expect("a day");
    let committed = read.days.iter().find(|d| d.model == OPUS && d.measures.commits > 0).expect("the commit's day").day.clone();
    let behind = day(&db, OPUS, on(&committed)).await.expect("the commit's day");
    assert_eq!(behind.commits.len(), 1, "{behind:?}");
    let c = &behind.commits[0];
    assert_eq!(
        (c.hash.as_str(), c.repo.as_str(), c.subject.as_str(), c.added, c.removed, c.landed, c.place.as_deref()),
        (second.as_str(), "work", "second", 2, 0, true, Some("mainline")),
        "{c:?}"
    );
    assert_eq!(c.alive, Some(2), "the two lines it wrote, alive at the tip");
    assert_eq!(c.files.iter().map(|f| (f.path.as_str(), f.added, f.removed)).collect::<Vec<_>>(), [("a.txt", 2, 0)]);
    assert!(c.session.is_some(), "the session whose call made it");
    let worked = read.days.iter().find(|d| d.model == OPUS && d.measures.lines_added > 0).expect("the edit's day").day.clone();
    let behind = day(&db, OPUS, on(&worked)).await.expect("the work's day");
    assert_eq!(
        behind.edits.iter().map(|e| (e.path.as_str(), e.added, e.removed, e.tool.as_str())).collect::<Vec<_>>(),
        [("/repo/a.rs", 2, 1, "Edit")],
        "{behind:?}"
    );
    assert_eq!(behind.sessions.iter().map(|s| s.replies).collect::<Vec<_>>(), [3], "its one session, with its three replies");
    let behind = day(&db, SONNET, on(&worked)).await.expect("sonnet's day");
    assert_eq!((behind.sessions.len(), behind.edits.len(), behind.commits.len()), (2, 0, 0), "{behind:?}");

    // The comparison: the commit Opus's call printed is Opus's by evidence;
    // its spend counts the replies made in the repository it judged.
    let compared = compare(&db, &params(&[])).await.expect("the comparison");
    let opus = compared.deviations.iter().find(|d| d.model == "opus 5.5").unwrap_or_else(|| panic!("{compared:?}"));
    assert_eq!(
        (opus.commits, opus.added, opus.removed, opus.alive, opus.survived_pct),
        (1, 2, 0, 2, 100),
        "its one landed commit, all of it alive: {opus:?}"
    );
    assert_eq!((opus.messages, opus.out_tokens), (1, 1), "the reply that committed, in the repository: {opus:?}");
    assert_eq!((opus.operator_turns, opus.corrections, opus.corrections_per_100), (2, 1, 50), "its run's two prompts, one correcting");
    assert_eq!((opus.abandoned_lines, opus.in_flight_lines), (0, 0));
    assert!(compared.unjudged.is_empty(), "{compared:?}");
    assert_eq!(compared.repos.iter().map(|r| (r.repo.as_str(), r.model.as_str(), r.commits)).collect::<Vec<_>>(), [("work", "opus 5.5", 1)]);
    std::fs::remove_dir_all(&root).expect("clean up");
}
