//! Does the database end up looking like the schema? Apply the module's
//! schema file, read the result back OUT of the engine, and compare it
//! with the contract below: every table, field and index, the service
//! account, and nothing else. Then the rules: rows the schema allows are
//! taken, and each row it forbids is refused by the one rule it breaks.
//!
//! The contract is written out here rather than parsed from the file, so
//! this test holds no schema statement of its own (§7): a change to the
//! file that is not made here too fails it.

use std::collections::BTreeSet;

use superx_kernel::types::Value;
use superx_mod_performance::SCHEMA_DDL;

const PW: &str = "test-password";

/// A table's fields and its indexes.
struct Table {
    name: &'static str,
    fields: &'static [&'static str],
    indexes: &'static [&'static str],
}

/// Every table, its fields and its indexes.
const CONTRACT: &[Table] = &[
    Table {
        name: "session",
        fields: &["kernel_session", "agent", "agent_id", "entrypoint", "client_version", "first_at", "valid_from"],
        indexes: &["idx_session_kernel", "idx_session_first"],
    },
    Table {
        name: "session_place",
        fields: &["session", "cwd", "branch", "at", "valid_from"],
        indexes: &["idx_session_place"],
    },
    Table {
        name: "prompt",
        fields: &["session", "kernel_message", "at", "kind", "text", "chars", "images", "valid_from"],
        indexes: &["idx_prompt_line", "idx_prompt_session", "idx_prompt_at"],
    },
    Table {
        name: "reply",
        fields: &[
            "session", "agent_id", "request_id", "kernel_message", "at", "model", "requested_model", "advisor_model",
            "effort", "turn_effort", "stop_reason", "subagent", "valid_from",
        ],
        indexes: &["idx_reply_agent", "idx_reply_session", "idx_reply_at", "idx_reply_model"],
    },
    Table {
        name: "reply_state",
        fields: &[
            "reply", "input_tokens", "cache_read_tokens", "cache_write_tokens", "output_tokens", "thinking_tokens",
            "tool_tokens", "web_searches", "web_fetches", "service_tier", "text_chars", "usage", "valid_from",
        ],
        indexes: &["idx_reply_state"],
    },
    Table {
        name: "thinking",
        fields: &["reply", "position", "kernel_message", "at", "ms", "chars", "display", "valid_from"],
        indexes: &["idx_thinking", "idx_thinking_at"],
    },
    Table {
        name: "tool_call",
        fields: &[
            "reply", "call_id", "kernel_message", "at", "tool", "path", "command", "description", "pattern", "url",
            "query", "subagent_type", "background", "input_bytes", "valid_from",
        ],
        indexes: &["idx_tool_call", "idx_tool_call_at", "idx_tool_call_tool"],
    },
    Table {
        name: "tool_result",
        fields: &["tool_call", "kernel_message", "at", "outcome", "ms", "bytes", "valid_from"],
        indexes: &["idx_tool_result", "idx_tool_result_at"],
    },
    Table {
        name: "edit",
        fields: &["tool_call", "path", "kind", "added", "removed", "counted_from", "operator_changed", "valid_from"],
        indexes: &["idx_edit", "idx_edit_path"],
    },
    Table {
        name: "git_event",
        fields: &["tool_call", "at", "action", "dir", "hash", "branch", "subject", "pr_number", "url", "valid_from"],
        indexes: &["idx_git_event", "idx_git_event_hash", "idx_git_event_at"],
    },
    Table {
        name: "test_run",
        fields: &["tool_call", "at", "runner", "passed", "failed", "ignored", "valid_from"],
        indexes: &["idx_test_run", "idx_test_run_at"],
    },
    Table {
        name: "build_run",
        fields: &["tool_call", "at", "builder", "ok", "errors", "warnings", "valid_from"],
        indexes: &["idx_build_run", "idx_build_run_at"],
    },
    Table {
        name: "turn",
        fields: &["session", "kernel_message", "at", "ms", "lines", "valid_from"],
        indexes: &["idx_turn_line", "idx_turn_session"],
    },
    Table {
        name: "compaction",
        fields: &[
            "session", "kernel_message", "at", "trigger", "tokens_before", "tokens_after", "tokens_dropped", "ms",
            "valid_from",
        ],
        indexes: &["idx_compaction_line", "idx_compaction_session"],
    },
    Table {
        name: "away",
        fields: &["session", "kernel_message", "at", "text", "valid_from"],
        indexes: &["idx_away_line", "idx_away_session"],
    },
    Table {
        name: "repository",
        fields: &["common_dir", "name", "found_at", "valid_from"],
        indexes: &["idx_repository_dir"],
    },
    Table {
        name: "checkout",
        fields: &["repository", "toplevel", "found_at", "valid_from"],
        indexes: &["idx_checkout_top", "idx_checkout_repo"],
    },
    Table {
        name: "checkout_state",
        fields: &["checkout", "branch", "first_branch", "present", "valid_from"],
        indexes: &["idx_checkout_state"],
    },
    Table { name: "directory", fields: &["path", "source", "checkout", "exact", "valid_from"], indexes: &["idx_directory_path"] },
    Table { name: "head_move", fields: &["checkout", "at", "branch", "valid_from"], indexes: &["idx_head_move"] },
    Table {
        name: "commit",
        fields: &[
            "repository", "hash", "authored_at", "committed_at", "author", "committer", "subject", "body", "parents",
            "added", "removed", "valid_from",
        ],
        indexes: &["idx_commit_hash", "idx_commit_author"],
    },
    Table {
        name: "commit_file",
        fields: &["commit", "path", "change", "added", "removed", "binary", "valid_from"],
        indexes: &["idx_commit_file", "idx_file_path"],
    },
    Table { name: "placement", fields: &["commit", "crawl", "place", "first_parent", "valid_from"], indexes: &["idx_placement"] },
    Table {
        name: "landed_via",
        fields: &["commit", "path", "via", "crawl", "valid_from"],
        indexes: &["idx_landed_via", "idx_landed_via_via"],
    },
    Table { name: "survival", fields: &["commit", "crawl", "alive", "valid_from"], indexes: &["idx_survival"] },
    Table {
        name: "survival_file",
        fields: &["commit", "crawl", "path", "alive", "valid_from"],
        indexes: &["idx_survival_file"],
    },
    // The first approval's fields; `CRAWL_COUNTS` are added in `fields_of`.
    Table {
        name: "crawl",
        fields: &[
            "repository", "started_at", "finished_at", "mainline", "mainline_from", "identity", "tip", "since", "outcome",
            "commits_read", "files_blamed", "files_left", "git_calls", "git_failed", "note", "valid_from",
        ],
        indexes: &["idx_crawl_repo"],
    },
    Table {
        name: "ref_state",
        fields: &["repository", "crawl", "name", "tip", "rewritten", "valid_from"],
        indexes: &["idx_ref_state"],
    },
    Table {
        name: "worktree_file",
        fields: &["checkout", "crawl", "path", "from_path", "state", "staged", "added", "removed", "binary", "valid_from"],
        indexes: &["idx_worktree_file"],
    },
    Table {
        name: "crawl_step",
        fields: &[
            "crawl", "step", "started_at", "ms", "git_calls", "git_failed", "git_timeouts", "objects_read", "bytes_read",
            "rows_written", "note", "valid_from",
        ],
        indexes: &["idx_crawl_step"],
    },
    // An array field's elements are a field of their own the engine
    // defines (`args.*`, `crawls.*`).
    Table {
        name: "git_call",
        fields: &[
            "crawl", "step", "dir", "args", "args.*", "started_at", "ms", "exit", "timed_out", "bytes_out", "lines_out",
            "stderr", "valid_from",
        ],
        indexes: &["idx_git_call", "idx_git_call_failed"],
    },
    Table {
        name: "pass",
        fields: &["started_at", "finished_at", "rows_written", "outcome", "note", "valid_from"],
        indexes: &["idx_pass"],
    },
    Table {
        name: "pass_source",
        fields: &[
            "pass", "source", "started_at", "finished_at", "items", "unread", "calls", "failed", "timed_out", "rows",
            "counts", "from_id", "to_id", "crawls", "crawls.*", "outcome", "note", "valid_from",
        ],
        indexes: &["idx_pass_source", "idx_pass_source_source"],
    },
];

/// The counts of a crawl added by the second approval (#501).
const CRAWL_COUNTS: &[&str] = &[
    "checkouts_read", "refs_read", "files_read", "reflog_read", "unreachable_read", "blame_lines_read", "bytes_read",
    "commits_new", "commit_files_new", "lines_added_new", "lines_removed_new", "refs_new", "refs_moved", "refs_gone",
    "refs_rewritten", "head_moves_new", "placements_changed", "landed_new", "survival_changed", "survival_delta",
    "git_timeouts", "git_ms", "rows_written",
];

/// A table's fields as the contract has them.
fn fields_of(table: &Table) -> BTreeSet<String> {
    let extra: &[&str] = if table.name == "crawl" { CRAWL_COUNTS } else { &[] };
    table.fields.iter().chain(extra).map(|f| (*f).to_string()).collect()
}

fn keys_of(v: &Value, section: &str) -> BTreeSet<String> {
    let Value::Object(top) = v else { return BTreeSet::new() };
    match top.get(section) {
        Some(Value::Object(inner)) => inner.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

fn set_of(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|i| (*i).to_string()).collect()
}

async fn connect() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("performance").await.expect("nsdb");
    db
}

async fn provision(db: &superx_kernel::Db) {
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", PW);
    db.query(ddl).await.expect("provision").check().expect("provision is clean");
}

async fn info_for_db(db: &superx_kernel::Db) -> Value {
    db.query("INFO FOR DB").await.expect("info").take(0).expect("decodes")
}

fn contract_tables() -> BTreeSet<String> {
    CONTRACT.iter().map(|t| t.name.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_leaves_the_database_exactly_as_the_contract_says() {
    let db = connect().await;
    provision(&db).await;
    let info = info_for_db(&db).await;
    assert_eq!(keys_of(&info, "tables"), contract_tables(), "the database's tables are not the contract's");
    assert_eq!(keys_of(&info, "users"), set_of(&["superx_mod_performance"]), "its own service account, and no other");

    for table in CONTRACT {
        let info: Value =
            db.query(format!("INFO FOR TABLE {}", table.name)).await.expect("info").take(0).expect("decodes");
        for (section, want) in [("fields", fields_of(table)), ("indexes", set_of(table.indexes))] {
            let got = keys_of(&info, section);
            assert_eq!(
                got,
                want,
                "the {section} of `{}` are not the contract's.\n  in the contract only: {:?}\n  in the database only: {:?}",
                table.name,
                want.difference(&got).collect::<Vec<_>>(),
                got.difference(&want).collect::<Vec<_>>(),
            );
        }
    }
}

/// Provisioning over a database that already holds something leaves
/// nothing of it behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_that_was_there_survives_provisioning() {
    let db = connect().await;
    db.query("CREATE leftover:1 SET junk = 'x'").await.expect("junk").check().expect("junk written");
    assert!(keys_of(&info_for_db(&db).await, "tables").contains("leftover"));

    provision(&db).await;
    assert_eq!(keys_of(&info_for_db(&db).await, "tables"), contract_tables(), "a table from before survived provisioning");
}

/// Run one statement: `None` when the engine took it, its refusal when not.
async fn refusal(db: &superx_kernel::Db, sql: &str) -> Option<String> {
    db.query(sql).await.expect("query").check().err().map(|e| e.to_string())
}

/// The kernel line the first prompt was read from.
const LINE: &str = "u'0199f0a0-0000-7000-8000-000000000001'";

/// A whole row of `table`, its fields as given with `set` laid over them:
/// so a row the engine must refuse differs from a valid one in the one
/// field under test, and is refused for that alone.
fn row(table: &str, id: &str, base: &[(&str, &str)], set: &[(&str, &str)]) -> String {
    let mut fields: Vec<(String, String)> = base.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    for (k, v) in set {
        match fields.iter_mut().find(|(name, _)| name == k) {
            Some(field) => field.1 = (*v).to_string(),
            None => fields.push(((*k).to_string(), (*v).to_string())),
        }
    }
    let body = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ");
    format!("CREATE {table}{id} CONTENT {{ {body} }}")
}

fn session(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("kernel_session", "u'0199f0a0-0000-7000-8000-0000000000aa'"),
        ("agent", "'claude_code'"),
        ("agent_id", "'8eadecd6'"),
        ("entrypoint", "'cli'"),
        ("client_version", "'2.1.304'"),
        ("first_at", "time::now()"),
        ("valid_from", "time::now()"),
    ];
    row("session", id, &base, set)
}

fn place(set: &[(&str, &str)]) -> String {
    let base =
        [("session", "session:s1"), ("cwd", "'/w'"), ("branch", "'main'"), ("at", "time::now()"), ("valid_from", "time::now()")];
    row("session_place", "", &base, set)
}

fn prompt(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("kind", "'typed'"),
        ("text", "'measure the models'"),
        ("chars", "18"),
        ("images", "0"),
        ("valid_from", "time::now()"),
    ];
    row("prompt", "", &base, set)
}

fn reply(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("agent_id", "'msg_01'"),
        ("request_id", "'req_01'"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("model", "'claude-opus-5-5'"),
        ("requested_model", "'claude-opus-5-5'"),
        ("advisor_model", "NONE"),
        ("effort", "'max'"),
        ("turn_effort", "'max'"),
        ("stop_reason", "'tool_use'"),
        ("subagent", "false"),
        ("valid_from", "time::now()"),
    ];
    row("reply", id, &base, set)
}

fn reply_state(set: &[(&str, &str)]) -> String {
    let base = [
        ("reply", "reply:r1"),
        ("input_tokens", "2"),
        ("cache_read_tokens", "239698"),
        ("cache_write_tokens", "2601"),
        ("output_tokens", "451"),
        ("thinking_tokens", "244"),
        ("tool_tokens", "NONE"),
        ("web_searches", "0"),
        ("web_fetches", "0"),
        ("service_tier", "'standard'"),
        ("text_chars", "0"),
        ("usage", "{ input_tokens: 2, output_tokens: 451, cache_creation: { ephemeral_1h_input_tokens: 2601 }, speed: NONE }"),
        ("valid_from", "time::now()"),
    ];
    row("reply_state", "", &base, set)
}

fn thinking(set: &[(&str, &str)]) -> String {
    let base = [
        ("reply", "reply:r1"),
        ("position", "0"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("ms", "2800"),
        ("chars", "0"),
        ("display", "'updates'"),
        ("valid_from", "time::now()"),
    ];
    row("thinking", "", &base, set)
}

fn tool_call(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("reply", "reply:r1"),
        ("call_id", "'toolu_01'"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("tool", "'Bash'"),
        ("path", "NONE"),
        ("command", "'git commit -m x'"),
        ("description", "'commit'"),
        ("pattern", "NONE"),
        ("url", "NONE"),
        ("query", "NONE"),
        ("subagent_type", "NONE"),
        ("background", "false"),
        ("input_bytes", "40"),
        ("valid_from", "time::now()"),
    ];
    row("tool_call", id, &base, set)
}

fn tool_result(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t1"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("outcome", "'ok'"),
        ("ms", "NONE"),
        ("bytes", "120"),
        ("valid_from", "time::now()"),
    ];
    row("tool_result", "", &base, set)
}

fn edit(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t1"),
        ("path", "'/w/a.rs'"),
        ("kind", "'modified'"),
        ("added", "3"),
        ("removed", "1"),
        ("counted_from", "'patch'"),
        ("operator_changed", "false"),
        ("valid_from", "time::now()"),
    ];
    row("edit", "", &base, set)
}

fn git_event(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t1"),
        ("at", "time::now()"),
        ("action", "'commit'"),
        ("dir", "'/w'"),
        ("hash", "'5574e81'"),
        ("branch", "'main'"),
        ("subject", "'x'"),
        ("pr_number", "NONE"),
        ("url", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("git_event", "", &base, set)
}

fn test_run(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t1"),
        ("at", "time::now()"),
        ("runner", "'cargo'"),
        ("passed", "443"),
        ("failed", "0"),
        ("ignored", "0"),
        ("valid_from", "time::now()"),
    ];
    row("test_run", "", &base, set)
}

fn build_run(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t1"),
        ("at", "time::now()"),
        ("builder", "'cargo'"),
        ("ok", "true"),
        ("errors", "0"),
        ("warnings", "0"),
        ("valid_from", "time::now()"),
    ];
    row("build_run", "", &base, set)
}

fn turn(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "u'0199f0a0-0000-7000-8000-000000000002'"),
        ("at", "time::now()"),
        ("ms", "212571"),
        ("lines", "66"),
        ("valid_from", "time::now()"),
    ];
    row("turn", "", &base, set)
}

fn compaction(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "u'0199f0a0-0000-7000-8000-000000000003'"),
        ("at", "time::now()"),
        ("trigger", "'auto'"),
        ("tokens_before", "967174"),
        ("tokens_after", "16471"),
        ("tokens_dropped", "950703"),
        ("ms", "57277"),
        ("valid_from", "time::now()"),
    ];
    row("compaction", "", &base, set)
}

fn away(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "u'0199f0a0-0000-7000-8000-000000000004'"),
        ("at", "time::now()"),
        ("text", "'recap'"),
        ("valid_from", "time::now()"),
    ];
    row("away", "", &base, set)
}

fn pass(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("rows_written", "1204"),
        ("outcome", "'complete'"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("pass", id, &base, set)
}

fn pass_source(set: &[(&str, &str)]) -> String {
    let base = [
        ("pass", "pass:p1"),
        ("source", "'lines'"),
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("items", "500"),
        ("unread", "0"),
        ("calls", "0"),
        ("failed", "0"),
        ("timed_out", "0"),
        ("rows", "{ reply: 31, prompt: 4 }"),
        ("counts", "{ sessions_read: 2 }"),
        ("from_id", "rand::uuid::v7()"),
        ("to_id", "rand::uuid::v7()"),
        ("crawls", "[]"),
        ("outcome", "'complete'"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("pass_source", "", &base, set)
}

const HASH_A: &str = "0123456789abcdef0123456789abcdef01234567";
const HASH_B: &str = "1123456789abcdef0123456789abcdef01234567";

fn commit(id: &str, repository: &str, hash: &str, set: &[(&str, &str)]) -> String {
    let (repository, hash) = (repository.to_string(), format!("'{hash}'"));
    let base = [
        ("repository", repository.as_str()),
        ("hash", hash.as_str()),
        ("authored_at", "time::now()"),
        ("committed_at", "time::now()"),
        ("author", "'me@x'"),
        ("committer", "'noreply@github.com'"),
        ("subject", "'s'"),
        ("body", "'Why.\\n\\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'"),
        ("parents", "1"),
        ("added", "3"),
        ("removed", "0"),
        ("valid_from", "time::now()"),
    ];
    row("commit", &format!(":{id}"), &base, set)
}

fn crawl(id: &str, set: &[(&str, &str)]) -> String {
    let mut base = vec![
        ("repository", "repository:r1"),
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("mainline", "'origin/main'"),
        ("mainline_from", "'origin_head'"),
        ("identity", "'me@x'"),
        ("tip", "NONE"),
        ("since", "NONE"),
        ("outcome", "'complete'"),
        ("commits_read", "2"),
        ("files_blamed", "1"),
        ("files_left", "0"),
        ("git_calls", "3"),
        ("git_failed", "0"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    base.extend(CRAWL_COUNTS.iter().map(|c| (*c, "0")));
    row("crawl", &format!(":{id}"), &base, set)
}

fn commit_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("commit", "commit:m1"),
        ("path", "'a.rs'"),
        ("change", "'added'"),
        ("added", "3"),
        ("removed", "0"),
        ("binary", "false"),
        ("valid_from", "time::now()"),
    ];
    row("commit_file", "", &base, set)
}

fn ref_state(set: &[(&str, &str)]) -> String {
    let tip = format!("'{HASH_A}'");
    let base = [
        ("repository", "repository:r1"),
        ("crawl", "crawl:c1"),
        ("name", "'refs/heads/feat/x'"),
        ("tip", tip.as_str()),
        ("rewritten", "false"),
        ("valid_from", "time::now()"),
    ];
    row("ref_state", "", &base, set)
}

fn worktree_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("checkout", "checkout:k1"),
        ("crawl", "crawl:c1"),
        ("path", "'src/new.rs'"),
        ("from_path", "NONE"),
        ("state", "'untracked'"),
        ("staged", "false"),
        ("added", "120"),
        ("removed", "0"),
        ("binary", "false"),
        ("valid_from", "time::now()"),
    ];
    row("worktree_file", "", &base, set)
}

fn crawl_step(set: &[(&str, &str)]) -> String {
    let base = [
        ("crawl", "crawl:c1"),
        ("step", "'blame'"),
        ("started_at", "time::now()"),
        ("ms", "2100"),
        ("git_calls", "40"),
        ("git_failed", "0"),
        ("git_timeouts", "0"),
        ("objects_read", "88000"),
        ("bytes_read", "7000000"),
        ("rows_written", "6"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("crawl_step", "", &base, set)
}

fn git_call(set: &[(&str, &str)]) -> String {
    let base = [
        ("crawl", "crawl:c1"),
        ("step", "'log'"),
        ("dir", "'/w/a'"),
        ("args", "['log', 'origin/main', '--numstat']"),
        ("started_at", "time::now()"),
        ("ms", "120000"),
        ("exit", "NONE"),
        ("timed_out", "true"),
        ("bytes_out", "0"),
        ("lines_out", "0"),
        ("stderr", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("git_call", "", &base, set)
}

fn survival_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("commit", "commit:m2"),
        ("crawl", "crawl:c1"),
        ("path", "'a.rs'"),
        ("alive", "3"),
        ("valid_from", "time::now()"),
    ];
    row("survival_file", "", &base, set)
}

/// The engine holds the rules the crawler relies on: one row per session,
/// line, reply, thinking span, tool call, result, file of an edit, test or
/// build command, repository, checkout, commit, file of a commit, HEAD move
/// and landing; links that point at rows that exist; values from the closed
/// lists, the crawler's sources among them; counts never below zero; a
/// commit hash of 7 to 64 characters where an agent printed it and 40 or 64
/// where git gave it whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_refuses_what_the_schema_forbids() {
    let db = connect().await;
    provision(&db).await;

    // A whole, valid chain is taken: one row of every table.
    let hash_64 = format!("'{}'", "a".repeat(64));
    for sql in [
        session(":s1", &[]),
        place(&[]),
        prompt(&[("kernel_message", LINE), ("kind", "'queued'")]),
        prompt(&[("kind", "'shell'"), ("text", "'superx-repos --home ~/.superx --initialize'"), ("chars", "41")]),
        reply(":r1", &[]),
        reply_state(&[]),
        thinking(&[]),
        tool_call(":t1", &[]),
        tool_call(":t2", &[("call_id", "'toolu_02'"), ("tool", "'Read'"), ("path", "'/w/a.rs'"), ("command", "NONE")]),
        tool_result(&[]),
        edit(&[]),
        git_event(&[]),
        git_event(&[("action", "'pr_created'"), ("hash", "NONE"), ("pr_number", "505"), ("url", "'https://x/pull/505'")]),
        git_event(&[("action", "'merge'"), ("hash", hash_64.as_str())]),
        test_run(&[]),
        build_run(&[]),
        turn(&[]),
        compaction(&[]),
        compaction(&[
            ("kernel_message", "rand::uuid::v7()"),
            ("trigger", "'manual'"),
            ("tokens_after", "NONE"),
            ("tokens_dropped", "NONE"),
            ("ms", "NONE"),
        ]),
        away(&[]),
        "CREATE repository:r1 CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout:k1 CONTENT { repository: repository:r1, toplevel: '/w/a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout_state CONTENT { checkout: checkout:k1, branch: 'main', first_branch: NONE, present: true, valid_from: time::now() }".to_string(),
        "CREATE directory CONTENT { path: '/w/a/src', source: 'session', checkout: checkout:k1, exact: true, valid_from: time::now() }".to_string(),
        "CREATE directory CONTENT { path: '/elsewhere', source: 'include', checkout: NONE, exact: false, valid_from: time::now() }".to_string(),
        "CREATE head_move CONTENT { checkout: checkout:k1, at: d'2026-10-20T10:00:00Z', branch: 'feat', valid_from: time::now() }".to_string(),
        crawl("c1", &[("refs_read", "25"), ("survival_delta", "-42")]),
        commit("m1", "repository:r1", HASH_A, &[]),
        commit("m2", "repository:r1", HASH_B, &[("body", "NONE")]),
        commit_file(&[]),
        commit_file(&[("path", "'gone.rs'"), ("change", "'deleted'"), ("added", "0"), ("removed", "40")]),
        "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'branch', first_parent: false, valid_from: time::now() }".to_string(),
        "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }".to_string(),
        "CREATE survival CONTENT { commit: commit:m2, crawl: crawl:c1, alive: 3, valid_from: time::now() }".to_string(),
        survival_file(&[]),
        ref_state(&[]),
        ref_state(&[("name", "'refs/heads/thrown-away'"), ("tip", "NONE")]),
        worktree_file(&[]),
        worktree_file(&[("path", "'src/b.rs'"), ("from_path", "'src/a.rs'"), ("state", "'renamed'"), ("staged", "true")]),
        crawl_step(&[]),
        git_call(&[]),
        git_call(&[("exit", "0"), ("timed_out", "false"), ("bytes_out", "48211"), ("lines_out", "1290")]),
        commit("m3", "repository:r1", &"0123456789abcdef".repeat(4), &[]),
        pass(":p1", &[]),
        pass(":p2", &[("outcome", "'stopped'"), ("rows_written", "0")]),
        pass_source(&[]),
        pass_source(&[
            ("source", "'repos'"),
            ("calls", "149"),
            ("failed", "1"),
            ("rows", "{}"),
            ("counts", "{ repositories_new: 9, checkouts_gone: 0 }"),
            ("from_id", "NONE"),
            ("to_id", "NONE"),
            ("crawls", "[crawl:c1]"),
            ("outcome", "'partial'"),
            ("note", "'1 git call cut off'"),
        ]),
        pass_source(&[("pass", "pass:p2"), ("outcome", "'failed'"), ("note", "'the database did not answer'")]),
    ] {
        assert_eq!(refusal(&db, &sql).await, None, "a valid row must be taken: {sql}");
    }

    // Each differs from a valid row in one field, and is refused by the
    // rule on that field or index, named in the engine's own words.
    let refused = [
        (session(":s2", &[]), "index `idx_session_kernel`", "a second row for one session"),
        (
            session(":s3", &[("kernel_session", "'not-a-uuid'")]),
            "field `kernel_session`",
            "a session whose kernel id is not a UUID",
        ),
        (place(&[("session", "session:nope")]), "field `session`", "a place in a session that does not exist"),
        (prompt(&[("kind", "'chat'")]), "field `kind`", "a prompt of a kind outside the list"),
        (prompt(&[("chars", "-1")]), "field `chars`", "a prompt of negative length"),
        (prompt(&[("kernel_message", LINE)]), "index `idx_prompt_line`", "a prompt from a line already read"),
        (reply(":r2", &[]), "index `idx_reply_agent`", "a reply counted twice"),
        (reply_state(&[("output_tokens", "-5")]), "field `output_tokens`", "negative output tokens"),
        (reply_state(&[("tool_tokens", "-1")]), "field `tool_tokens`", "negative tool tokens"),
        (reply_state(&[("usage", "NONE")]), "field `usage`", "a reply's counts without its usage object"),
        (thinking(&[]), "index `idx_thinking`", "a thinking span counted twice"),
        (thinking(&[("position", "1"), ("ms", "-1")]), "field `ms`", "a thinking span of negative length"),
        (tool_call(":t3", &[]), "index `idx_tool_call`", "a tool call counted twice"),
        (tool_call(":t4", &[("reply", "reply:nope")]), "field `reply`", "a tool call of a reply that does not exist"),
        (tool_call(":t5", &[("call_id", "'toolu_05'"), ("input_bytes", "-1")]), "field `input_bytes`", "a negative input size"),
        (tool_result(&[("tool_call", "tool_call:t2"), ("outcome", "'failed'")]), "field `outcome`", "an outcome outside the list"),
        (tool_result(&[("outcome", "'error'")]), "index `idx_tool_result`", "a second result for one call"),
        (edit(&[("path", "'/w/b.rs'"), ("kind", "'deleted'")]), "field `kind`", "an edit of a kind outside the list"),
        (edit(&[("path", "'/w/b.rs'"), ("counted_from", "'guess'")]), "field `counted_from`", "lines counted some way outside the list"),
        (edit(&[("added", "1")]), "index `idx_edit`", "the same file of one call twice"),
        (git_event(&[("action", "'pull'")]), "field `action`", "a git action outside the list"),
        (git_event(&[("hash", "'abc'")]), "field `hash`", "a commit hash of 3 characters"),
        (git_event(&[("hash", "'5574e8'")]), "field `hash`", "a commit hash of 6 characters"),
        (git_event(&[("hash", &format!("'{}'", "a".repeat(65)))]), "field `hash`", "a commit hash of 65 characters"),
        (git_event(&[("action", "'pr_merged'"), ("hash", "NONE"), ("pr_number", "0")]), "field `pr_number`", "pull request 0"),
        (test_run(&[("tool_call", "tool_call:t2"), ("runner", "'mocha'")]), "field `runner`", "a test runner outside the list"),
        (test_run(&[("passed", "1")]), "index `idx_test_run`", "a second test run for one command"),
        (build_run(&[("tool_call", "tool_call:t2"), ("builder", "'make'")]), "field `builder`", "a builder outside the list"),
        (build_run(&[("tool_call", "tool_call:t2"), ("errors", "-1")]), "field `errors`", "negative build errors"),
        (build_run(&[]), "index `idx_build_run`", "a second build for one command"),
        (turn(&[("kernel_message", "rand::uuid::v7()"), ("ms", "-1")]), "field `ms`", "a turn of negative length"),
        (turn(&[]), "index `idx_turn_line`", "a turn read twice"),
        (compaction(&[("kernel_message", "rand::uuid::v7()"), ("trigger", "'size'")]), "field `trigger`", "a trigger outside the list"),
        (compaction(&[]), "index `idx_compaction_line`", "a compaction read twice"),
        (away(&[]), "index `idx_away_line`", "a recap read twice"),
        (
            "CREATE repository CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }".to_string(),
            "index `idx_repository_dir`",
            "a second row for one repository",
        ),
        (commit("d1", "repository:r1", HASH_A, &[]), "index `idx_commit_hash`", "a second row for one commit"),
        (commit("d2", "repository:r1", &HASH_A[..39], &[]), "field `hash`", "a hash of 39 characters"),
        (
            commit("d3", "repository:nothing", "2123456789abcdef0123456789abcdef01234567", &[]),
            "field `repository`",
            "a commit of a repository that does not exist",
        ),
        (
            commit("d4", "repository:r1", "3123456789abcdef0123456789abcdef01234567", &[("committer", "NONE")]),
            "field `committer`",
            "a commit with no committer",
        ),
        (
            "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'merged', first_parent: false, valid_from: time::now() }".to_string(),
            "field `place`",
            "a place outside the list",
        ),
        (
            "CREATE survival CONTENT { commit: commit:m1, crawl: crawl:c1, alive: -1, valid_from: time::now() }".to_string(),
            "field `alive`",
            "a negative survival",
        ),
        (
            "CREATE head_move CONTENT { checkout: checkout:k1, at: d'2026-10-20T10:00:00Z', branch: 'feat', valid_from: time::now() }".to_string(),
            "index `idx_head_move`",
            "a second row for one HEAD move",
        ),
        (
            "CREATE directory CONTENT { path: '/w/b', source: 'guess', checkout: NONE, exact: false, valid_from: time::now() }".to_string(),
            "field `source`",
            "a directory from a source outside the list",
        ),
        (
            "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }".to_string(),
            "index `idx_landed_via`",
            "a second row for one landing",
        ),
        (crawl("g1", &[("mainline_from", "'guessed'")]), "field `mainline_from`", "a main line chosen some way outside the list"),
        (crawl("g2", &[("refs_read", "-1")]), "field `refs_read`", "a crawl count below zero"),
        (
            commit_file(&[("path", "'b.rs'"), ("change", "'renamed'")]),
            "field `change`",
            "a change outside the list: renames read as a delete and an add",
        ),
        (survival_file(&[("alive", "-1")]), "field `alive`", "a negative survival in a file"),
        (ref_state(&[("tip", "'abc123'")]), "field `tip`", "a ref tip that is not a whole commit id"),
        (worktree_file(&[("state", "'stashed'")]), "field `state`", "an uncommitted state outside the list"),
        (crawl_step(&[("step", "'guess'")]), "field `step`", "a step outside the list"),
        (git_call(&[("ms", "-5")]), "field `ms`", "a git call that took less than no time"),
        (git_call(&[("step", "'write'")]), "field `step`", "a git call in the step that writes rows"),
        (pass(":p3", &[("outcome", "'done'")]), "field `outcome`", "a pass that ended some way outside the list"),
        (pass(":p4", &[("rows_written", "-1")]), "field `rows_written`", "a negative count of rows written"),
        (pass_source(&[("source", "'hosts'")]), "field `source`", "a source the crawler does not have yet"),
        (pass_source(&[("pass", "pass:nope")]), "field `pass`", "a source's record in a pass that does not exist"),
        (pass_source(&[("rows", "{ reply: -1 }")]), "field `rows`", "a negative count of rows"),
        (pass_source(&[("counts", "{ repositories_new: 'two' }")]), "field `counts`", "a count written as text"),
        (pass_source(&[("crawls", "[crawl:c1, crawl:nothing]")]), "field `crawls`", "a crawl that does not exist"),
        (pass_source(&[("outcome", "'aborted'")]), "field `outcome`", "a source that ended some way outside the list"),
        (pass_source(&[("unread", "-1")]), "field `unread`", "a negative count of items not understood"),
    ];
    for (sql, rule, what) in refused {
        let Some(why) = refusal(&db, &sql).await else {
            panic!("{what} must be refused: {sql}");
        };
        assert!(why.contains(rule), "{what} must be refused by {rule}, not: {why}\n{sql}");
    }
}
