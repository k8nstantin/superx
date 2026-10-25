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
        name: "pass",
        fields: &[
            "started_at", "finished_at", "from_line", "to_line", "lines_read", "lines_unread", "sessions_new",
            "places_new", "prompts_new", "replies_new", "reply_states_new", "thinking_new", "tool_calls_new",
            "tool_results_new", "edits_new", "git_events_new", "test_runs_new", "build_runs_new", "turns_new",
            "compactions_new", "aways_new", "rows_written", "outcome", "note", "valid_from",
        ],
        indexes: &["idx_pass"],
    },
];

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
        for (section, want) in [("fields", set_of(table.fields)), ("indexes", set_of(table.indexes))] {
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

fn pass(set: &[(&str, &str)]) -> String {
    let mut base = vec![
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("from_line", "rand::uuid::v7()"),
        ("to_line", "rand::uuid::v7()"),
        ("lines_read", "500"),
        ("outcome", "'complete'"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    for count in [
        "lines_unread", "sessions_new", "places_new", "prompts_new", "replies_new", "reply_states_new", "thinking_new",
        "tool_calls_new", "tool_results_new", "edits_new", "git_events_new", "test_runs_new", "build_runs_new",
        "turns_new", "compactions_new", "aways_new", "rows_written",
    ] {
        base.push((count, "1"));
    }
    row("pass", "", &base, set)
}

/// The engine holds the rules the reader relies on: one row per session,
/// line, reply, thinking span, tool call, result, file of an edit, and test
/// or build command; links that point at rows that exist; values from the
/// closed lists; counts never below zero; a commit hash of 7 to 64
/// characters.
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
        pass(&[]),
        pass(&[("from_line", "NONE"), ("to_line", "NONE"), ("lines_read", "0"), ("outcome", "'stopped'")]),
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
        (pass(&[("outcome", "'done'")]), "field `outcome`", "a pass that ended some way outside the list"),
        (pass(&[("lines_unread", "-1")]), "field `lines_unread`", "a negative count of lines not understood"),
    ];
    for (sql, rule, what) in refused {
        let Some(why) = refusal(&db, &sql).await else {
            panic!("{what} must be refused: {sql}");
        };
        assert!(why.contains(rule), "{what} must be refused by {rule}, not: {why}\n{sql}");
    }
}
