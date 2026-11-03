//! The tab's Results (#539): the models, day by day and over all time, from
//! the rows the crawler wrote of captured lines.

pub mod common;

use common::*;
use superx_mod_recon::crawler::Crawler;
use superx_mod_recon::params::INCLUDE_PARAM;
use superx_mod_recon::results::{models, Measures};

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
    std::fs::remove_dir_all(&root).expect("clean up");
}
