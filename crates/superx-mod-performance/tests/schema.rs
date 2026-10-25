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
        fields: &[
            "session", "kernel_message", "at", "kind", "text", "chars", "images", "prompt_id", "source", "permission_mode",
            "turn_index", "prompt_index", "valid_from",
        ],
        indexes: &["idx_prompt_line", "idx_prompt_session", "idx_prompt_at", "idx_prompt_id"],
    },
    Table {
        name: "reply",
        fields: &[
            "session", "agent_id", "request_id", "kernel_message", "at", "model", "requested_model", "advisor_model",
            "effort", "turn_effort", "stop_reason", "subagent", "request", "prompt_id", "client_version", "skill", "plugin",
            "mcp_server", "mcp_tool", "refusal", "refusal_text", "cache_miss", "cache_missed", "thinking_dropped", "valid_from",
        ],
        indexes: &["idx_reply_agent", "idx_reply_session", "idx_reply_at", "idx_reply_model", "idx_reply_prompt", "idx_reply_skill"],
    },
    Table {
        name: "reply_state",
        fields: &[
            "reply", "input_tokens", "cache_read_tokens", "cache_write_tokens", "output_tokens", "thinking_tokens",
            "tool_tokens", "web_searches", "web_fetches", "service_tier", "text_chars", "cache_write_1h_tokens",
            "cache_write_5m_tokens", "usage", "valid_from",
        ],
        indexes: &["idx_reply_state"],
    },
    Table {
        name: "thinking",
        fields: &[
            "reply", "position", "kernel_message", "at", "ms", "chars", "display", "text", "subject", "signature_chars",
            "valid_from",
        ],
        indexes: &["idx_thinking", "idx_thinking_at"],
    },
    Table {
        name: "tool_call",
        fields: &[
            "reply", "call_id", "kernel_message", "at", "tool", "path", "command", "description", "pattern", "url",
            "query", "subagent_type", "background", "input_bytes", "skill", "model", "sandbox_off", "timeout_ms", "valid_from",
        ],
        indexes: &["idx_tool_call", "idx_tool_call_at", "idx_tool_call_tool"],
    },
    Table {
        name: "tool_result",
        fields: &[
            "tool_call", "kernel_message", "at", "outcome", "ms", "bytes", "decision", "decided_by", "decision_source",
            "decision_reason", "denial", "operator_feedback", "sandboxed", "sandbox_hosts", "sandbox_hosts.*",
            "persisted_bytes", "return_note", "background_task", "text", "valid_from",
        ],
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
            "added", "removed", "author_name", "committer_name", "parent_hashes", "parent_hashes.*", "tree", "valid_from",
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
    // The deep crawl (#512): tables 34–53 from the lines, 54–59 from the
    // repositories.
    Table {
        name: "trigger",
        fields: &["session", "kernel_message", "at", "kind", "source", "prompt_id", "text", "chars", "valid_from"],
        indexes: &["idx_trigger_line", "idx_trigger_session"],
    },
    Table {
        name: "said",
        fields: &["reply", "position", "kernel_message", "at", "text", "chars", "valid_from"],
        indexes: &["idx_said", "idx_said_at"],
    },
    Table {
        name: "line",
        fields: &["session", "kernel_message", "at", "agent", "kind", "agent_uuid", "parent_uuid", "prompt_id", "sidechain", "client_version", "bytes", "read_as", "read_as.*", "valid_from"],
        indexes: &["idx_line", "idx_line_session", "idx_line_kind", "idx_line_parent"],
    },
    Table {
        name: "line_field",
        fields: &["agent", "kind", "path", "first_at", "first_line", "stored", "valid_from"],
        indexes: &["idx_line_field"],
    },
    Table {
        name: "request",
        fields: &["session", "kernel_message", "at", "request_key", "model", "effort", "max_tokens", "thinking", "thinking_display", "betas", "betas.*", "context_edits", "context_edits.*", "query_source", "cache_breakpoints", "keep", "tail", "tail.*", "shape_hash", "digest", "params", "valid_from"],
        indexes: &["idx_request_line", "idx_request_key", "idx_request_session", "idx_request_model"],
    },
    Table {
        name: "request_part",
        fields: &["hash", "session", "kernel_message", "at", "role", "blocks", "blocks.*", "chars", "valid_from"],
        indexes: &["idx_request_part", "idx_request_part_at"],
    },
    Table {
        name: "request_shape",
        fields: &["shape_hash", "session", "kernel_message", "at", "system_parts", "system_chars", "tools", "tools.*", "valid_from"],
        indexes: &["idx_request_shape"],
    },
    Table {
        name: "context",
        fields: &["session", "kernel_message", "at", "kind", "names", "names.*", "removed", "removed.*", "failed", "failed.*", "chars", "changed", "reason", "tokens_left", "valid_from"],
        indexes: &["idx_context_line", "idx_context_session", "idx_context_kind"],
    },
    Table {
        name: "hook_run",
        fields: &["session", "kernel_message", "position", "at", "event", "command", "ms", "errors", "prevented", "chars", "valid_from"],
        indexes: &["idx_hook_run", "idx_hook_run_session"],
    },
    Table {
        name: "session_state",
        fields: &["session", "kernel_message", "at", "title", "permission_mode", "mode", "valid_from"],
        indexes: &["idx_session_state"],
    },
    Table {
        name: "session_cost",
        fields: &["session", "kernel_message", "at", "usd", "api_ms", "api_ms_no_retries", "tool_ms", "wall_ms", "lines_added", "lines_removed", "unknown_cost", "valid_from"],
        indexes: &["idx_session_cost"],
    },
    Table {
        name: "model_cost",
        fields: &["session_cost", "model", "usd", "input", "output", "cache_read", "cache_write", "thinking", "web_searches", "valid_from"],
        indexes: &["idx_model_cost"],
    },
    Table {
        name: "session_link",
        fields: &["session", "kind", "url", "number", "key", "repository", "title", "first_at", "kernel_message", "valid_from"],
        indexes: &["idx_session_link", "idx_session_link_url"],
    },
    Table {
        name: "git_state",
        fields: &["session", "kernel_message", "at", "cwd", "root", "branch", "default_branch", "clean", "modified", "staged", "untracked", "remote_host", "remote", "visibility", "valid_from"],
        indexes: &["idx_git_state", "idx_git_state_root"],
    },
    Table {
        name: "notice",
        fields: &["session", "kernel_message", "at", "kind", "status", "model", "text", "limit", "resets_at", "valid_from"],
        indexes: &["idx_notice_line", "idx_notice_kind"],
    },
    Table {
        name: "question",
        fields: &["tool_call", "position", "at", "header", "question", "multi", "options", "options.*", "answer", "note", "valid_from"],
        indexes: &["idx_question", "idx_question_at"],
    },
    Table {
        name: "task_event",
        fields: &["tool_call", "at", "action", "task_key", "subject", "status", "text", "valid_from"],
        indexes: &["idx_task_event", "idx_task_event_at"],
    },
    Table {
        name: "plan",
        fields: &["tool_call", "at", "text", "chars", "file", "edited", "valid_from"],
        indexes: &["idx_plan"],
    },
    Table {
        name: "subagent_run",
        fields: &["tool_call", "at", "agent_key", "agent_type", "requested_model", "model", "status", "background", "ms", "tokens", "tool_calls", "tool_stats", "usage", "valid_from"],
        indexes: &["idx_subagent_run", "idx_subagent_run_at"],
    },
    Table {
        name: "queue_event",
        fields: &["session", "kernel_message", "at", "operation", "reason", "chars", "valid_from"],
        indexes: &["idx_queue_event", "idx_queue_event_sess"],
    },
    Table {
        name: "remote_state",
        fields: &["repository", "crawl", "name", "url", "push_url", "host", "path", "valid_from"],
        indexes: &["idx_remote_state", "idx_remote_state_path"],
    },
    Table {
        name: "ref_move",
        fields: &["repository", "checkout", "crawl", "ref", "at", "old", "new", "identity", "action", "valid_from"],
        indexes: &["idx_ref_move", "idx_ref_move_at"],
    },
    Table {
        name: "commit_rename",
        fields: &["commit", "from_path", "to_path", "similarity", "valid_from"],
        indexes: &["idx_commit_rename"],
    },
    Table {
        name: "tip_file",
        fields: &["repository", "crawl", "path", "bytes", "lines", "present", "valid_from"],
        indexes: &["idx_tip_file"],
    },
    Table {
        name: "repository_state",
        fields: &["repository", "crawl", "origin_head", "hooks", "hooks.*", "hooks_path", "shallow", "valid_from"],
        indexes: &["idx_repository_state"],
    },
    Table {
        name: "commit_session",
        fields: &["commit", "session", "tool_call", "via", "at", "valid_from"],
        indexes: &["idx_commit_session", "idx_commit_session_sess"],
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
        ("thinking_dropped", "0"),
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
        ("text", "''"),
        ("signature_chars", "315860"),
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
        ("sandbox_off", "false"),
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
        ("sandbox_hosts", "[]"),
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
    let (repository, hash, tree) = (repository.to_string(), format!("'{hash}'"), format!("'{HASH_B}'"));
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
        ("author_name", "'Me'"),
        ("committer_name", "'GitHub'"),
        ("parent_hashes", "[]"),
        ("tree", tree.as_str()),
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

/// Rows the deep crawl's rules are tried against twice: once taken, then
/// refused as read twice.
const HOOK: &str = "CREATE hook_run CONTENT { session: session:s1, kernel_message: u'0199f0a0-0000-7000-8000-000000000015', \
    position: 0, at: time::now(), event: 'Stop', command: 'tools/stop.sh', ms: 878, errors: 0, prevented: false, chars: 0, \
    valid_from: time::now() }";
const PLAN: &str = "CREATE plan CONTENT { tool_call: tool_call:t2, at: time::now(), text: '# Plan', chars: 6, file: NONE, \
    edited: true, valid_from: time::now() }";
const SUBAGENT: &str = "CREATE subagent_run CONTENT { tool_call: tool_call:t2, at: time::now(), agent_key: 'a1', \
    agent_type: 'Explore', requested_model: NONE, model: 'claude-opus-5-5', status: 'completed', background: false, \
    ms: 51000, tokens: 259623, tool_calls: 94, tool_stats: { bashCount: 94 }, usage: { output_tokens: 23961 }, \
    valid_from: time::now() }";
const QUEUE: &str = "CREATE queue_event CONTENT { session: session:s1, kernel_message: u'0199f0a0-0000-7000-8000-000000000016', \
    at: time::now(), operation: 'remove', reason: 'absorbed_mid_turn', chars: 61, valid_from: time::now() }";

/// The kernel lines the deep crawl's rows were read from.
const LINE_2: &str = "u'0199f0a0-0000-7000-8000-000000000010'";
const LINE_3: &str = "u'0199f0a0-0000-7000-8000-000000000011'";

fn trigger(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", LINE_2),
        ("at", "time::now()"),
        ("kind", "'task-notification'"),
        ("source", "'session-task'"),
        ("prompt_id", "'p-2'"),
        ("text", "'<task-notification>done</task-notification>'"),
        ("chars", "44"),
        ("valid_from", "time::now()"),
    ];
    row("trigger", "", &base, set)
}

fn said(set: &[(&str, &str)]) -> String {
    let base = [
        ("reply", "reply:r1"),
        ("position", "2"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("text", "'Commits first, then lines.'"),
        ("chars", "26"),
        ("valid_from", "time::now()"),
    ];
    row("said", "", &base, set)
}

fn line(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", LINE_3),
        ("at", "time::now()"),
        ("agent", "'claude_code'"),
        ("kind", "'assistant'"),
        ("agent_uuid", "'u-2'"),
        ("parent_uuid", "'u-1'"),
        ("prompt_id", "NONE"),
        ("sidechain", "false"),
        ("client_version", "'2.1.304'"),
        ("bytes", "4096"),
        ("read_as", "['reply', 'reply_state', 'thinking']"),
        ("valid_from", "time::now()"),
    ];
    row("line", "", &base, set)
}

fn request(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "u'0199f0a0-0000-7000-8000-000000000012'"),
        ("at", "time::now()"),
        ("request_key", "'e760654a-c503-4922-9e2f-38ea629d95a4'"),
        ("model", "'claude-opus-5-5'"),
        ("effort", "'max'"),
        ("max_tokens", "128000"),
        ("thinking", "'adaptive'"),
        ("thinking_display", "'updates'"),
        ("betas", "['claude-code-20250219', 'interleaved-thinking-2025-05-14']"),
        ("context_edits", "['clear_thinking_20251015']"),
        ("query_source", "'repl_main_thread'"),
        ("cache_breakpoints", "3"),
        ("keep", "1436"),
        ("tail", "['h-part-1']"),
        ("shape_hash", "'f00d'"),
        ("digest", "'beef'"),
        ("params", "{ model: 'claude-opus-5-5', max_tokens: 128000, output_config: { effort: 'max' } }"),
        ("valid_from", "time::now()"),
    ];
    row("request", id, &base, set)
}

fn context(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "u'0199f0a0-0000-7000-8000-000000000013'"),
        ("at", "time::now()"),
        ("kind", "'deferred_tools_delta'"),
        ("names", "['mcp__claude_ai_Claude_Docs__batch']"),
        ("removed", "[]"),
        ("failed", "['gdx-kn']"),
        ("chars", "5410"),
        ("changed", "NONE"),
        ("reason", "NONE"),
        ("tokens_left", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("context", "", &base, set)
}

fn session_cost(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("usd", "16.818315799999993"),
        ("api_ms", "2303415"),
        ("api_ms_no_retries", "2301031"),
        ("tool_ms", "367250"),
        ("wall_ms", "142908002"),
        ("lines_added", "1655"),
        ("lines_removed", "56"),
        ("unknown_cost", "false"),
        ("valid_from", "time::now()"),
    ];
    row("session_cost", id, &base, set)
}

fn model_cost(set: &[(&str, &str)]) -> String {
    let base = [
        ("session_cost", "session_cost:sc1"),
        ("model", "'claude-opus-5-5'"),
        ("usd", "12.1578218"),
        ("input", "818"),
        ("output", "236268"),
        ("cache_read", "26301614"),
        ("cache_write", "433247"),
        ("thinking", "171420"),
        ("web_searches", "0"),
        ("valid_from", "time::now()"),
    ];
    row("model_cost", "", &base, set)
}

fn session_link(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kind", "'pr'"),
        ("url", "'https://github.com/k8nstantin/superx/pull/511'"),
        ("number", "511"),
        ("key", "NONE"),
        ("repository", "'k8nstantin/superx'"),
        ("title", "NONE"),
        ("first_at", "time::now()"),
        ("kernel_message", "rand::uuid::v7()"),
        ("valid_from", "time::now()"),
    ];
    row("session_link", "", &base, set)
}

fn git_state(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "rand::uuid::v7()"),
        ("at", "time::now()"),
        ("cwd", "'/w/a'"),
        ("root", "'/w/a'"),
        ("branch", "'feat/512'"),
        ("default_branch", "'main'"),
        ("clean", "false"),
        ("modified", "4"),
        ("staged", "1"),
        ("untracked", "2"),
        ("remote_host", "'github.com'"),
        ("remote", "'k8nstantin/superx'"),
        ("visibility", "'public'"),
        ("valid_from", "time::now()"),
    ];
    row("git_state", "", &base, set)
}

fn notice(set: &[(&str, &str)]) -> String {
    let base = [
        ("session", "session:s1"),
        ("kernel_message", "u'0199f0a0-0000-7000-8000-000000000014'"),
        ("at", "time::now()"),
        ("kind", "'rate_limit'"),
        ("status", "429"),
        ("model", "'claude-opus-5-5'"),
        ("text", "'You have hit your limit'"),
        ("limit", "'five_hour'"),
        ("resets_at", "d'2026-10-25T15:00:00Z'"),
        ("valid_from", "time::now()"),
    ];
    row("notice", "", &base, set)
}

fn question(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t2"),
        ("position", "0"),
        ("at", "time::now()"),
        ("header", "'Approach'"),
        ("question", "'Which crawl first?'"),
        ("multi", "false"),
        ("options", "['Commits', 'Lines']"),
        ("answer", "'Commits'"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("question", "", &base, set)
}

fn task_event(set: &[(&str, &str)]) -> String {
    let base = [
        ("tool_call", "tool_call:t2"),
        ("at", "time::now()"),
        ("action", "'topic'"),
        ("task_key", "NONE"),
        ("subject", "'Calibrate'"),
        ("status", "NONE"),
        ("text", "'intent: compare the builds'"),
        ("valid_from", "time::now()"),
    ];
    row("task_event", "", &base, set)
}

fn ref_move(set: &[(&str, &str)]) -> String {
    let (old, new) = (format!("'{HASH_B}'"), format!("'{HASH_A}'"));
    let base = [
        ("repository", "repository:r1"),
        ("checkout", "checkout:k1"),
        ("crawl", "crawl:c1"),
        ("ref", "'HEAD'"),
        ("at", "d'2026-10-25T09:00:00Z'"),
        ("old", old.as_str()),
        ("new", new.as_str()),
        ("identity", "'me@x'"),
        ("action", "'commit (amend): x'"),
        ("valid_from", "time::now()"),
    ];
    row("ref_move", "", &base, set)
}

fn commit_rename(set: &[(&str, &str)]) -> String {
    let base = [
        ("commit", "commit:m1"),
        ("from_path", "'src/reader/mod.rs'"),
        ("to_path", "'src/crawler/lines/mod.rs'"),
        ("similarity", "87"),
        ("valid_from", "time::now()"),
    ];
    row("commit_rename", "", &base, set)
}

fn tip_file(set: &[(&str, &str)]) -> String {
    let base = [
        ("repository", "repository:r1"),
        ("crawl", "crawl:c1"),
        ("path", "'a.rs'"),
        ("bytes", "1200"),
        ("lines", "40"),
        ("present", "true"),
        ("valid_from", "time::now()"),
    ];
    row("tip_file", "", &base, set)
}

fn commit_session(set: &[(&str, &str)]) -> String {
    let base = [
        ("commit", "commit:m1"),
        ("session", "session:s1"),
        ("tool_call", "tool_call:t1"),
        ("via", "'printed'"),
        ("at", "time::now()"),
        ("valid_from", "time::now()"),
    ];
    row("commit_session", "", &base, set)
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
    let parents = format!("['{HASH_A}', '{HASH_B}']");
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
        // The deep crawl's tables and values (#512).
        edit(&[("path", "'/w/gone.rs'"), ("kind", "'deleted'"), ("counted_from", "'shell'"), ("added", "0"), ("removed", "12")]),
        trigger(&[]),
        said(&[]),
        line(&[]),
        line(&[
            ("session", "NONE"),
            ("kernel_message", "rand::uuid::v7()"),
            ("agent", "'claude_desktop'"),
            ("kind", "'desktop/main.log'"),
            ("agent_uuid", "NONE"),
            ("parent_uuid", "NONE"),
            ("client_version", "NONE"),
            ("read_as", "[]"),
        ]),
        format!(
            "CREATE line_field CONTENT {{ agent: 'claude_code', kind: 'assistant', path: 'message.usage.output_tokens', \
             first_at: time::now(), first_line: {LINE_3}, stored: true, valid_from: time::now() }}"
        ),
        format!(
            "CREATE line_field CONTENT {{ agent: 'claude_code', kind: 'assistant', path: 'message.usage.output_tokens', \
             first_at: time::now(), first_line: {LINE_3}, stored: false, valid_from: time::now() + 1s }}"
        ),
        request(":q1", &[]),
        reply(
            ":r3",
            &[
                ("agent_id", "'msg_03'"),
                ("request", "request:q1"),
                ("prompt_id", "'p-1'"),
                ("skill", "'zero-trust-execution'"),
                ("cache_miss", "'tools_changed'"),
                ("cache_missed", "761770"),
                ("thinking_dropped", "2"),
            ],
        ),
        "CREATE request_part CONTENT { hash: 'h-part-1', session: session:s1, kernel_message: rand::uuid::v7(), \
         at: time::now(), role: 'system', blocks: ['text'], chars: 41000, valid_from: time::now() }"
            .to_string(),
        "CREATE request_shape CONTENT { shape_hash: 'f00d', session: session:s1, kernel_message: rand::uuid::v7(), \
         at: time::now(), system_parts: 3, system_chars: 52000, tools: ['Bash', 'Edit'], valid_from: time::now() }"
            .to_string(),
        context(&[]),
        context(&[
            ("kernel_message", "rand::uuid::v7()"),
            ("kind", "'total_tokens_reminder'"),
            ("names", "[]"),
            ("failed", "[]"),
            ("chars", "49"),
            ("tokens_left", "14836633"),
        ]),
        HOOK.to_string(),
        "CREATE session_state CONTENT { session: session:s1, kernel_message: rand::uuid::v7(), at: time::now(), \
         title: 'ENG-8074 task spec', permission_mode: 'auto', mode: 'normal', valid_from: time::now() }"
            .to_string(),
        session_cost(":sc1", &[]),
        session_cost(":sc2", &[("usd", "0")]),
        model_cost(&[]),
        session_link(&[]),
        session_link(&[
            ("kind", "'issue'"),
            ("url", "'https://x.atlassian.net/browse/ENG-8074'"),
            ("number", "NONE"),
            ("key", "'ENG-8074'"),
            ("repository", "NONE"),
            ("title", "'Build the source'"),
        ]),
        git_state(&[]),
        notice(&[]),
        question(&[]),
        task_event(&[]),
        PLAN.to_string(),
        SUBAGENT.to_string(),
        QUEUE.to_string(),
        "CREATE remote_state CONTENT { repository: repository:r1, crawl: crawl:c1, name: 'origin', \
         url: 'git@github.com:k8nstantin/superx.git', push_url: NONE, host: 'github.com', path: 'k8nstantin/superx', \
         valid_from: time::now() }"
            .to_string(),
        ref_move(&[]),
        ref_move(&[
            ("checkout", "NONE"),
            ("ref", "'refs/heads/main'"),
            ("old", "NONE"),
            ("identity", "NONE"),
            ("action", "'branch: Created from HEAD'"),
        ]),
        commit_rename(&[]),
        tip_file(&[]),
        tip_file(&[("path", "'logo.png'"), ("lines", "NONE")]),
        "CREATE repository_state CONTENT { repository: repository:r1, crawl: crawl:c1, origin_head: 'main', hooks: [], \
         hooks_path: NONE, shallow: false, valid_from: time::now() }"
            .to_string(),
        commit_session(&[]),
        commit("m4", "repository:r1", "4123456789abcdef0123456789abcdef01234567", &[("parents", "2"), ("parent_hashes", &parents)]),
        crawl_step(&[("step", "'setup'")]),
        crawl_step(&[("step", "'tip'")]),
        git_call(&[("step", "'tip'"), ("exit", "0"), ("timed_out", "false")]),
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
        (edit(&[("path", "'/w/b.rs'"), ("kind", "'renamed'")]), "field `kind`", "an edit of a kind outside the list"),
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
        // The deep crawl's rules (#512).
        (
            line(&[("kernel_message", "rand::uuid::v7()"), ("read_as", "['reply', 'nowhere']")]),
            "field `read_as`",
            "a line read into a table that does not exist",
        ),
        (line(&[]), "index `idx_line`", "a line read twice"),
        (line(&[("kernel_message", "rand::uuid::v7()"), ("bytes", "-1")]), "field `bytes`", "a line of negative size"),
        (
            trigger(&[("kernel_message", "rand::uuid::v7()"), ("session", "session:nope")]),
            "field `session`",
            "a trigger in a session that does not exist",
        ),
        (trigger(&[]), "index `idx_trigger_line`", "a trigger read twice"),
        (said(&[("chars", "-1")]), "field `chars`", "a text of negative length"),
        (said(&[("reply", "reply:nope")]), "field `reply`", "a text of a reply that does not exist"),
        (prompt(&[("turn_index", "-1")]), "field `turn_index`", "a prompt at a negative turn"),
        (
            reply(":r4", &[("agent_id", "'msg_04'"), ("request", "request:nope")]),
            "field `request`",
            "a reply naming a request that does not exist",
        ),
        (
            reply(":r5", &[("agent_id", "'msg_05'"), ("thinking_dropped", "-1")]),
            "field `thinking_dropped`",
            "negative thinking left out",
        ),
        (thinking(&[("position", "5"), ("signature_chars", "-1")]), "field `signature_chars`", "a signature of negative length"),
        (tool_call(":t6", &[("call_id", "'toolu_06'"), ("timeout_ms", "-5")]), "field `timeout_ms`", "a negative time limit"),
        (
            tool_result(&[("tool_call", "tool_call:t2"), ("persisted_bytes", "-1")]),
            "field `persisted_bytes`",
            "a negative saved size",
        ),
        (request(":q2", &[]), "index `idx_request_line`", "a request read twice"),
        (
            request(":q3", &[("kernel_message", "rand::uuid::v7()"), ("cache_breakpoints", "-1")]),
            "field `cache_breakpoints`",
            "negative cache breakpoints",
        ),
        (
            "CREATE request_part CONTENT { hash: 'h-part-1', session: session:s1, kernel_message: rand::uuid::v7(), \
             at: time::now(), role: 'system', blocks: [], chars: 1, valid_from: time::now() }"
                .to_string(),
            "index `idx_request_part`",
            "a request piece kept twice",
        ),
        (
            "CREATE request_shape CONTENT { shape_hash: 'f00d', session: session:s1, kernel_message: rand::uuid::v7(), \
             at: time::now(), system_parts: 0, system_chars: 0, tools: [], valid_from: time::now() }"
                .to_string(),
            "index `idx_request_shape`",
            "a request shape kept twice",
        ),
        (context(&[]), "index `idx_context_line`", "an attachment read twice"),
        (
            context(&[("kernel_message", "rand::uuid::v7()"), ("tokens_left", "-1")]),
            "field `tokens_left`",
            "negative tokens left",
        ),
        (HOOK.to_string(), "index `idx_hook_run`", "a hook read twice"),
        (session_cost(":sc3", &[("usd", "-0.5")]), "field `usd`", "a cost below zero"),
        (model_cost(&[]), "index `idx_model_cost`", "one model costed twice in one cost"),
        (session_link(&[("kind", "'wiki'"), ("url", "'https://x'")]), "field `kind`", "a link of a kind outside the list"),
        (session_link(&[]), "index `idx_session_link`", "a session linked twice to one pull request"),
        (
            session_link(&[("url", "'https://github.com/k8nstantin/superx/pull/0'"), ("number", "0")]),
            "field `number`",
            "pull request 0",
        ),
        (git_state(&[("modified", "-1")]), "field `modified`", "negative modified files"),
        (notice(&[]), "index `idx_notice_line`", "a notice read twice"),
        (question(&[("question", "'again?'")]), "index `idx_question`", "a question asked twice in one place"),
        (
            task_event(&[("tool_call", "tool_call:t1"), ("action", "'deleted'")]),
            "field `action`",
            "a task change of a kind outside the list",
        ),
        (task_event(&[("action", "'created'")]), "index `idx_task_event`", "a task change read twice"),
        (PLAN.to_string(), "index `idx_plan`", "a plan read twice"),
        (SUBAGENT.to_string(), "index `idx_subagent_run`", "a subagent read twice"),
        (QUEUE.to_string(), "index `idx_queue_event`", "a queue change read twice"),
        (
            commit("d5", "repository:r1", "5123456789abcdef0123456789abcdef01234567", &[("parent_hashes", "['5574e81']")]),
            "field `parent_hashes`",
            "a parent hash that is not a whole commit id",
        ),
        (
            commit("d6", "repository:r1", "6123456789abcdef0123456789abcdef01234567", &[("tree", "'abc'")]),
            "field `tree`",
            "a tree that is not a whole id",
        ),
        (commit_rename(&[("from_path", "'x'"), ("similarity", "101")]), "field `similarity`", "a rename over 100 percent"),
        (commit_rename(&[("similarity", "90")]), "index `idx_commit_rename`", "a rename read twice"),
        (ref_move(&[("ref", "'refs/heads/x'"), ("new", "'5574e81'")]), "field `new`", "a reflog entry with a short hash"),
        (ref_move(&[("identity", "'other'")]), "index `idx_ref_move`", "a reflog entry read twice"),
        (tip_file(&[("path", "'b.rs'"), ("bytes", "-1")]), "field `bytes`", "a file at a tip of negative size"),
        (
            "CREATE remote_state CONTENT { repository: repository:r1, crawl: crawl:nope, name: 'origin', url: NONE, \
             push_url: NONE, host: NONE, path: NONE, valid_from: time::now() }"
                .to_string(),
            "field `crawl`",
            "a remote no crawl read",
        ),
        (
            "CREATE repository_state CONTENT { repository: repository:r1, crawl: crawl:nope, origin_head: NONE, hooks: [], \
             hooks_path: NONE, shallow: false, valid_from: time::now() }"
                .to_string(),
            "field `crawl`",
            "a setup no crawl read",
        ),
        (commit_session(&[("via", "'window'")]), "field `via`", "a commit tied to a session by a guess"),
        (commit_session(&[]), "index `idx_commit_session`", "a commit tied twice to one session"),
    ];
    for (sql, rule, what) in refused {
        let Some(why) = refusal(&db, &sql).await else {
            panic!("{what} must be refused: {sql}");
        };
        assert!(why.contains(rule), "{what} must be refused by {rule}, not: {why}\n{sql}");
    }
}
