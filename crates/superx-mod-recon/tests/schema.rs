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
use superx_mod_recon::SCHEMA_DDL;

const PW: &str = "test-password";

/// A table's fields and its indexes.
struct Table {
    name: &'static str,
    fields: &'static [&'static str],
    indexes: &'static [&'static str],
}


/// Every table, its fields and its indexes, in the structure's order. An
/// array field's elements are a field of their own the engine defines
/// (`args.*`, `crawls.*`).
const CONTRACT: &[Table] = &[
    Table { name: "session", fields: &["kernel_session", "agent", "agent_id", "entrypoint", "client_version", "first_at", "valid_from"], indexes: &["idx_session_kernel", "idx_session_first"] },
    Table { name: "session_place", fields: &["session", "cwd", "branch", "at", "valid_from"], indexes: &["idx_session_place"] },
    Table { name: "prompt", fields: &["session", "kernel_message", "at", "kind", "text", "chars", "images", "prompt_id", "source", "permission_mode", "turn_index", "prompt_index", "valid_from"], indexes: &["idx_prompt_line", "idx_prompt_session", "idx_prompt_at", "idx_prompt_id"] },
    Table { name: "trigger", fields: &["session", "kernel_message", "at", "kind", "source", "prompt_id", "text", "chars", "valid_from"], indexes: &["idx_trigger_line", "idx_trigger_session"] },
    Table { name: "reply", fields: &["session", "agent_id", "request_id", "kernel_message", "at", "model", "requested_model", "advisor_model", "effort", "turn_effort", "stop_reason", "subagent", "prompt_id", "client_version", "skill", "plugin", "mcp_server", "mcp_tool", "refusal", "refusal_text", "cache_miss", "cache_missed", "thinking_dropped", "valid_from"], indexes: &["idx_reply_agent", "idx_reply_session", "idx_reply_at", "idx_reply_model", "idx_reply_prompt", "idx_reply_skill"] },
    Table { name: "reply_state", fields: &["reply", "input_tokens", "cache_read_tokens", "cache_write_tokens", "output_tokens", "thinking_tokens", "tool_tokens", "web_searches", "web_fetches", "service_tier", "text_chars", "cache_write_1h_tokens", "cache_write_5m_tokens", "usage", "valid_from"], indexes: &["idx_reply_state"] },
    Table { name: "thinking", fields: &["reply", "position", "kernel_message", "at", "ms", "chars", "display", "text", "subject", "signature_chars", "valid_from"], indexes: &["idx_thinking", "idx_thinking_at"] },
    Table { name: "tool_call", fields: &["reply", "call_id", "kernel_message", "at", "tool", "path", "command", "description", "pattern", "url", "query", "subagent_type", "background", "input_bytes", "skill", "model", "sandbox_off", "timeout_ms", "valid_from"], indexes: &["idx_tool_call", "idx_tool_call_at", "idx_tool_call_tool"] },
    Table { name: "tool_result", fields: &["tool_call", "kernel_message", "at", "outcome", "ms", "bytes", "decision", "decided_by", "decision_source", "decision_reason", "denial", "operator_feedback", "sandboxed", "sandbox_hosts", "sandbox_hosts.*", "persisted_bytes", "return_note", "background_task", "text", "valid_from"], indexes: &["idx_tool_result", "idx_tool_result_at"] },
    Table { name: "edit", fields: &["tool_call", "path", "kind", "added", "removed", "counted_from", "operator_changed", "valid_from"], indexes: &["idx_edit", "idx_edit_path"] },
    Table { name: "git_event", fields: &["tool_call", "at", "action", "dir", "hash", "branch", "subject", "pr_number", "url", "valid_from"], indexes: &["idx_git_event", "idx_git_event_hash", "idx_git_event_at"] },
    Table { name: "test_run", fields: &["tool_call", "at", "runner", "passed", "failed", "ignored", "valid_from"], indexes: &["idx_test_run", "idx_test_run_at"] },
    Table { name: "build_run", fields: &["tool_call", "at", "builder", "ok", "errors", "warnings", "valid_from"], indexes: &["idx_build_run", "idx_build_run_at"] },
    Table { name: "turn", fields: &["session", "kernel_message", "at", "ms", "lines", "valid_from"], indexes: &["idx_turn_line", "idx_turn_session"] },
    Table { name: "compaction", fields: &["session", "kernel_message", "at", "trigger", "tokens_before", "tokens_after", "tokens_dropped", "ms", "valid_from"], indexes: &["idx_compaction_line", "idx_compaction_session"] },
    Table { name: "away", fields: &["session", "kernel_message", "at", "text", "valid_from"], indexes: &["idx_away_line", "idx_away_session"] },
    Table { name: "line_field", fields: &["agent", "kind", "path", "first_at", "first_line", "stored", "valid_from"], indexes: &["idx_line_field"] },
    Table { name: "repository", fields: &["common_dir", "name", "found_at", "valid_from"], indexes: &["idx_repository_dir"] },
    Table { name: "checkout", fields: &["repository", "toplevel", "found_at", "valid_from"], indexes: &["idx_checkout_top", "idx_checkout_repo"] },
    Table { name: "checkout_state", fields: &["checkout", "branch", "first_branch", "present", "operations", "operations.*", "operation_commits", "operation_commits.*", "rebase_branch", "rebase_onto", "rebase_done", "rebase_left", "bisect_start", "valid_from"], indexes: &["idx_checkout_state"] },
    Table { name: "directory", fields: &["path", "source", "checkout", "exact", "valid_from"], indexes: &["idx_directory_path"] },
    Table { name: "commit", fields: &["repository", "hash", "authored_at", "committed_at", "author", "committer", "subject", "body", "parents", "added", "removed", "author_name", "committer_name", "parent_hashes", "parent_hashes.*", "tree", "author_offset", "committer_offset", "patch_id", "signature", "valid_from"], indexes: &["idx_commit_hash", "idx_commit_author", "idx_commit_patch"] },
    Table { name: "commit_file", fields: &["commit", "path", "change", "added", "removed", "added_ignoring_space", "removed_ignoring_space", "binary", "valid_from"], indexes: &["idx_commit_file", "idx_file_path"] },
    Table { name: "placement", fields: &["commit", "crawl", "place", "first_parent", "valid_from"], indexes: &["idx_placement"] },
    Table { name: "landed_via", fields: &["commit", "path", "via", "crawl", "valid_from"], indexes: &["idx_landed_via", "idx_landed_via_via"] },
    Table { name: "survival", fields: &["commit", "crawl", "alive", "valid_from"], indexes: &["idx_survival"] },
    // The first approval's fields; `CRAWL_COUNTS` are added in `fields_of`.
    Table { name: "crawl", fields: &["repository", "started_at", "finished_at", "mainline", "mainline_from", "identity", "tip", "since", "outcome", "commits_read", "files_blamed", "files_left", "git_calls", "git_failed", "note", "valid_from"], indexes: &["idx_crawl_repo"] },
    Table { name: "git_call", fields: &["crawl", "step", "dir", "args", "args.*", "started_at", "ms", "exit", "timed_out", "bytes_out", "lines_out", "stderr", "valid_from"], indexes: &["idx_git_call", "idx_git_call_failed"] },
    Table { name: "remote_state", fields: &["repository", "crawl", "name", "url", "push_url", "host", "path", "valid_from"], indexes: &["idx_remote_state", "idx_remote_state_path"] },
    Table { name: "ref_move", fields: &["repository", "checkout", "mirror", "crawl", "ref", "at", "old", "new", "identity", "action", "valid_from"], indexes: &["idx_ref_move", "idx_ref_move_mirror", "idx_ref_move_at"] },
    Table { name: "commit_session", fields: &["commit", "session", "tool_call", "via", "pull_request", "at", "valid_from"], indexes: &["idx_commit_session", "idx_commit_session_sess"] },
    Table { name: "host_repository", fields: &["host", "path", "host_id", "found_at", "valid_from"], indexes: &["idx_host_repository"] },
    Table { name: "host_object", fields: &["host_repository", "kind", "key", "object", "hash", "call", "at", "valid_from"], indexes: &["idx_host_object", "idx_host_object_kind"] },
    Table { name: "host_user", fields: &["host", "host_id", "login", "name", "kind", "found_at", "valid_from"], indexes: &["idx_host_user", "idx_host_user_login"] },
    Table { name: "host_crawl", fields: &["host_repository", "crawl", "started_at", "finished_at", "calls", "not_modified", "failed", "rate_waits", "waited_ms", "objects_new", "objects_changed", "rows_written", "outcome", "note", "valid_from"], indexes: &["idx_host_crawl"] },
    Table { name: "host_call", fields: &["host_crawl", "method", "path", "started_at", "ms", "status", "not_modified", "bytes", "etag", "rate_limit", "rate_remaining", "rate_reset", "retry_after_ms", "objects", "next", "error", "valid_from"], indexes: &["idx_host_call", "idx_host_call_path"] },
    Table { name: "mirror", fields: &["host_repository", "url", "folder", "created_at", "valid_from"], indexes: &["idx_mirror"] },
    Table { name: "pull_request", fields: &["host_repository", "number", "host_id", "author", "created_at", "source_branch", "target_branch", "source_path", "url", "object", "valid_from"], indexes: &["idx_pull_request", "idx_pull_request_created", "idx_pull_request_url"] },
    Table { name: "pull_request_state", fields: &["pull_request", "state", "merged", "draft", "title", "head_sha", "base_sha", "merge_sha", "merged_at", "merged_by", "closed_at", "closed_by", "reason", "additions", "deletions", "changed_files", "commits", "comments", "review_comments", "updated_at", "object", "valid_from"], indexes: &["idx_pull_request_state", "idx_pull_request_merge"] },
    Table { name: "pull_request_commit", fields: &["pull_request", "hash", "position", "present", "object", "valid_from"], indexes: &["idx_pull_request_commit", "idx_pull_request_commit_hash"] },
    Table { name: "ci_run", fields: &["host_repository", "kind", "host_id", "name", "commit_sha", "ref", "parent", "pull_request", "url", "created_at", "object", "valid_from"], indexes: &["idx_ci_run", "idx_ci_run_commit"] },
    Table { name: "ci_run_state", fields: &["ci_run", "status", "conclusion", "started_at", "completed_at", "attempt", "number", "actor", "runner", "object", "valid_from"], indexes: &["idx_ci_run_state"] },
    Table { name: "pass", fields: &["started_at", "finished_at", "rows_written", "outcome", "note", "valid_from"], indexes: &["idx_pass"] },
    Table { name: "pass_source", fields: &["pass", "source", "started_at", "finished_at", "items", "unread", "calls", "failed", "timed_out", "rows", "counts", "from_id", "to_id", "crawls", "crawls.*", "outcome", "note", "valid_from"], indexes: &["idx_pass_source", "idx_pass_source_source"] },
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
    db.use_ns("superx").use_db("recon").await.expect("nsdb");
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
async fn provisioning_leaves_exactly_the_structures_tables_fields_and_indexes_and_superx_mod_recon_alone() {
    let db = connect().await;
    provision(&db).await;
    let info = info_for_db(&db).await;
    assert_eq!(keys_of(&info, "tables"), contract_tables(), "the database's tables are not the contract's");
    assert_eq!(keys_of(&info, "users"), set_of(&["superx_mod_recon"]), "its own service account, and no other");

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
        ("author_offset", "-240"),
        ("committer_offset", "0"),
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
        ("added_ignoring_space", "3"),
        ("removed_ignoring_space", "0"),
        ("binary", "false"),
        ("valid_from", "time::now()"),
    ];
    row("commit_file", "", &base, set)
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
/// build command, repository, checkout, commit, file of a commit, reflog
/// entry and landing; links that point at rows that exist; values from the
/// closed lists, the crawler's sources among them; counts never below zero;
/// a commit hash of 7 to 64 characters where an agent printed it and 40 or
/// 64 where git gave it whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_refuses_what_the_schema_forbids() {
    let db = connect().await;
    provision(&db).await;

    // A whole, valid chain is taken: one row of every table the deep crawl
    // gave the lines and the repositories.
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
        crawl("c1", &[("refs_read", "25"), ("survival_delta", "-42")]),
        commit("m1", "repository:r1", HASH_A, &[]),
        commit("m2", "repository:r1", HASH_B, &[("body", "NONE")]),
        commit_file(&[]),
        commit_file(&[("path", "'gone.rs'"), ("change", "'deleted'"), ("added", "0"), ("removed", "40")]),
        "CREATE placement CONTENT { commit: commit:m1, crawl: crawl:c1, place: 'branch', first_parent: false, valid_from: time::now() }".to_string(),
        "CREATE landed_via CONTENT { commit: commit:m1, path: 'a.rs', via: commit:m2, crawl: crawl:c1, valid_from: time::now() }".to_string(),
        "CREATE survival CONTENT { commit: commit:m2, crawl: crawl:c1, alive: 3, valid_from: time::now() }".to_string(),
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
        format!(
            "CREATE line_field CONTENT {{ agent: 'claude_code', kind: 'assistant', path: 'message.usage.output_tokens', \
             first_at: time::now(), first_line: {LINE_3}, stored: true, valid_from: time::now() }}"
        ),
        format!(
            "CREATE line_field CONTENT {{ agent: 'claude_code', kind: 'assistant', path: 'message.usage.output_tokens', \
             first_at: time::now(), first_line: {LINE_3}, stored: false, valid_from: time::now() + 1s }}"
        ),
        reply(
            ":r3",
            &[
                ("agent_id", "'msg_03'"),
                ("prompt_id", "'p-1'"),
                ("skill", "'zero-trust-execution'"),
                ("cache_miss", "'tools_changed'"),
                ("cache_missed", "761770"),
                ("thinking_dropped", "2"),
            ],
        ),
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
        commit_session(&[]),
        commit("m4", "repository:r1", "4123456789abcdef0123456789abcdef01234567", &[("parents", "2"), ("parent_hashes", &parents)]),
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
        (git_call(&[("ms", "-5")]), "field `ms`", "a git call that took less than no time"),
        (git_call(&[("step", "'write'")]), "field `step`", "a git call in the step that writes rows"),
        (pass(":p3", &[("outcome", "'done'")]), "field `outcome`", "a pass that ended some way outside the list"),
        (pass(":p4", &[("rows_written", "-1")]), "field `rows_written`", "a negative count of rows written"),
        (pass_source(&[("source", "'tabs'")]), "field `source`", "a source the crawler does not have"),
        (pass_source(&[("pass", "pass:nope")]), "field `pass`", "a source's record in a pass that does not exist"),
        (pass_source(&[("rows", "{ reply: -1 }")]), "field `rows`", "a negative count of rows"),
        (pass_source(&[("counts", "{ repositories_new: 'two' }")]), "field `counts`", "a count written as text"),
        (pass_source(&[("crawls", "[crawl:c1, crawl:nothing]")]), "field `crawls`", "a crawl that does not exist"),
        (pass_source(&[("outcome", "'aborted'")]), "field `outcome`", "a source that ended some way outside the list"),
        (pass_source(&[("unread", "-1")]), "field `unread`", "a negative count of items not understood"),
        // The deep crawl's rules (#512).
        (
            trigger(&[("kernel_message", "rand::uuid::v7()"), ("session", "session:nope")]),
            "field `session`",
            "a trigger in a session that does not exist",
        ),
        (trigger(&[]), "index `idx_trigger_line`", "a trigger read twice"),
        (prompt(&[("turn_index", "-1")]), "field `turn_index`", "a prompt at a negative turn"),
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
        (ref_move(&[("ref", "'refs/heads/x'"), ("new", "'5574e81'")]), "field `new`", "a reflog entry with a short hash"),
        (ref_move(&[("identity", "'other'")]), "index `idx_ref_move`", "a reflog entry read twice"),
        (
            "CREATE remote_state CONTENT { repository: repository:r1, crawl: crawl:nope, name: 'origin', url: NONE, \
             push_url: NONE, host: NONE, path: NONE, valid_from: time::now() }"
                .to_string(),
            "field `crawl`",
            "a remote no crawl read",
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

// The structure's rows from the hosts and the mirrors, and the rules it adds.

/// A content hash of the length a SHA-256 has, and another.
const SHA_A: &str = "'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'";
const SHA_B: &str = "'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'";

/// A provisioned database.
async fn provisioned() -> superx_kernel::Db {
    let db = connect().await;
    provision(&db).await;
    db
}

/// Every row is taken.
async fn take(db: &superx_kernel::Db, rows: &[String]) {
    for sql in rows {
        assert_eq!(refusal(db, sql).await, None, "a valid row must be taken: {sql}");
    }
}

/// Every row is refused by the rule named beside it, in the engine's words.
async fn refuse(db: &superx_kernel::Db, cases: &[(String, &str, &str)]) {
    for (sql, rule, what) in cases {
        let Some(why) = refusal(db, sql).await else {
            panic!("{what} must be refused: {sql}");
        };
        assert!(why.contains(rule), "{what} must be refused by {rule}, not: {why}\n{sql}");
    }
}

fn checkout_state(set: &[(&str, &str)]) -> String {
    let base = [
        ("checkout", "checkout:k1"),
        ("branch", "'main'"),
        ("first_branch", "NONE"),
        ("present", "true"),
        ("valid_from", "time::now()"),
    ];
    row("checkout_state", "", &base, set)
}

fn host_repository(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host", "'github.com'"),
        ("path", "'acme/widgets'"),
        ("host_id", "1296269"),
        ("found_at", "time::now()"),
        ("valid_from", "time::now()"),
    ];
    row("host_repository", id, &base, set)
}

fn host_crawl(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host_repository", "host_repository:h1"),
        ("crawl", "NONE"),
        ("started_at", "time::now()"),
        ("finished_at", "time::now()"),
        ("calls", "3"),
        ("not_modified", "1"),
        ("failed", "0"),
        ("rate_waits", "0"),
        ("waited_ms", "0"),
        ("objects_new", "2"),
        ("objects_changed", "0"),
        ("rows_written", "4"),
        ("outcome", "'complete'"),
        ("note", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("host_crawl", id, &base, set)
}

fn host_call(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host_crawl", "host_crawl:hc1"),
        ("method", "'GET'"),
        ("path", "'/repos/acme/widgets/pulls?state=all&per_page=100'"),
        ("started_at", "time::now()"),
        ("ms", "180"),
        ("status", "200"),
        ("not_modified", "false"),
        ("bytes", "5120"),
        ("etag", "NONE"),
        ("rate_limit", "5000"),
        ("rate_remaining", "4990"),
        ("rate_reset", "d'2026-10-29T13:00:00Z'"),
        ("retry_after_ms", "NONE"),
        ("objects", "1"),
        ("next", "false"),
        ("error", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("host_call", id, &base, set)
}

fn host_object(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host_repository", "host_repository:h1"),
        ("kind", "'pull_request'"),
        ("key", "7"),
        ("object", "{ number: 7, title: 'Widgets', user: { login: 'octo-tester' } }"),
        ("hash", SHA_A),
        ("call", "host_call:call1"),
        ("at", "time::now()"),
        ("valid_from", "time::now()"),
    ];
    row("host_object", id, &base, set)
}

fn host_user(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host", "'github.com'"),
        ("host_id", "583231"),
        ("login", "'octo-tester'"),
        ("name", "NONE"),
        ("kind", "'User'"),
        ("found_at", "time::now()"),
        ("valid_from", "time::now()"),
    ];
    row("host_user", id, &base, set)
}

fn mirror(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host_repository", "host_repository:h1"),
        ("url", "'https://github.com/acme/widgets.git'"),
        ("folder", "'github.com/acme/widgets.git'"),
        ("created_at", "time::now()"),
        ("valid_from", "time::now()"),
    ];
    row("mirror", id, &base, set)
}

fn pull_request(id: &str, set: &[(&str, &str)]) -> String {
    let base = [
        ("host_repository", "host_repository:h1"),
        ("number", "7"),
        ("host_id", "9007"),
        ("author", "host_user:u1"),
        ("created_at", "time::now()"),
        ("source_branch", "'feat/widgets'"),
        ("target_branch", "'main'"),
        ("source_path", "NONE"),
        ("url", "'https://github.com/acme/widgets/pull/7'"),
        ("object", "host_object:o1"),
        ("valid_from", "time::now()"),
    ];
    row("pull_request", id, &base, set)
}

fn pull_request_state(set: &[(&str, &str)]) -> String {
    let (head, base_sha) = (format!("'{HASH_A}'"), format!("'{HASH_B}'"));
    let base = [
        ("pull_request", "pull_request:pr1"),
        ("state", "'closed'"),
        ("merged", "true"),
        ("draft", "false"),
        ("title", "'Widgets'"),
        ("head_sha", head.as_str()),
        ("base_sha", base_sha.as_str()),
        ("merge_sha", "NONE"),
        ("merged_at", "time::now()"),
        ("merged_by", "host_user:u1"),
        ("closed_at", "time::now()"),
        ("closed_by", "NONE"),
        ("reason", "NONE"),
        ("additions", "12"),
        ("deletions", "3"),
        ("changed_files", "2"),
        ("commits", "1"),
        ("comments", "0"),
        ("review_comments", "0"),
        ("updated_at", "time::now()"),
        ("object", "host_object:o1"),
        ("valid_from", "time::now()"),
    ];
    row("pull_request_state", "", &base, set)
}

fn pull_request_commit(set: &[(&str, &str)]) -> String {
    let hash = format!("'{HASH_A}'");
    let base = [
        ("pull_request", "pull_request:pr1"),
        ("hash", hash.as_str()),
        ("position", "0"),
        ("present", "true"),
        ("object", "NONE"),
        ("valid_from", "time::now()"),
    ];
    row("pull_request_commit", "", &base, set)
}

fn ci_run(id: &str, set: &[(&str, &str)]) -> String {
    let sha = format!("'{HASH_A}'");
    let base = [
        ("host_repository", "host_repository:h1"),
        ("kind", "'workflow_run'"),
        ("host_id", "555"),
        ("name", "'CI'"),
        ("commit_sha", sha.as_str()),
        ("ref", "'main'"),
        ("parent", "NONE"),
        ("pull_request", "NONE"),
        ("url", "NONE"),
        ("created_at", "time::now()"),
        ("object", "host_object:o1"),
        ("valid_from", "time::now()"),
    ];
    row("ci_run", id, &base, set)
}

fn ci_run_state(set: &[(&str, &str)]) -> String {
    let base = [
        ("ci_run", "ci_run:ci1"),
        ("status", "'completed'"),
        ("conclusion", "'success'"),
        ("started_at", "time::now()"),
        ("completed_at", "time::now()"),
        ("attempt", "1"),
        ("number", "12"),
        ("actor", "host_user:u1"),
        ("runner", "NONE"),
        ("object", "host_object:o1"),
        ("valid_from", "time::now()"),
    ];
    row("ci_run_state", "", &base, set)
}

/// A repository on a host, its crawl and call, the object the call held, an
/// account, a pull request and a CI run: what the hosts' rules are tried on.
fn hosts() -> Vec<String> {
    vec![
        host_repository(":h1", &[]),
        host_crawl(":hc1", &[]),
        host_call(":call1", &[]),
        host_object(":o1", &[]),
        host_user(":u1", &[]),
        pull_request(":pr1", &[]),
        ci_run(":ci1", &[]),
    ]
}

/// A session, its reply and call, a repository with a checkout, a crawl, a
/// commit and a pass: what the repositories' rules are tried on.
fn repositories() -> Vec<String> {
    vec![
        session(":s1", &[]),
        reply(":r1", &[]),
        tool_call(":t1", &[]),
        "CREATE repository:r1 CONTENT { common_dir: '/w/a/.git', name: 'a', found_at: time::now(), valid_from: time::now() }".to_string(),
        "CREATE checkout:k1 CONTENT { repository: repository:r1, toplevel: '/w/a', found_at: time::now(), valid_from: time::now() }".to_string(),
        crawl("c1", &[]),
        commit("m1", "repository:r1", HASH_A, &[]),
        pass(":p1", &[]),
    ]
}

#[test]
fn schema_ddl_is_the_bytes_of_the_structure_file() {
    let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/schema/recon.surql")).expect("the structure");
    assert!(SCHEMA_DDL.as_bytes() == file.as_slice(), "SCHEMA_DDL differs from schema/recon.surql");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_other_than_github_or_bitbucket_and_a_repository_on_a_host_twice_are_refused() {
    let db = provisioned().await;
    take(&db, &[host_repository(":h1", &[])]).await;
    refuse(
        &db,
        &[
            (host_repository("", &[("host", "'gitlab.com'"), ("path", "'acme/other'")]), "field `host`", "a repository on a host the structure does not list"),
            (host_user("", &[("host", "'gitlab.com'")]), "field `host`", "an account on a host the structure does not list"),
            (host_repository("", &[]), "index `idx_host_repository`", "one repository on a host twice"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_object_of_another_kind_hash_key_or_call_is_refused_and_one_content_twice() {
    let db = provisioned().await;
    take(&db, &hosts()).await;
    refuse(
        &db,
        &[
            (host_object("", &[("kind", "'gist'")]), "field `kind`", "an object of a kind the structure does not list"),
            (host_object("", &[("hash", "'abc123'")]), "field `hash`", "a hash that is not of a SHA-256's length"),
            (host_object("", &[("key", "7.5")]), "field `key`", "a key that is a float"),
            (host_object("", &[("hash", SHA_B), ("call", "host_call:nope")]), "field `call`", "an object from a call that does not exist"),
            (host_object("", &[]), "index `idx_host_object`", "the same content of one object twice"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_call_that_is_no_get_or_has_negative_rate_left_and_a_host_crawl_outside_its_words_or_counts_are_refused() {
    let db = provisioned().await;
    take(&db, &[host_repository(":h1", &[]), host_crawl(":hc1", &[])]).await;
    refuse(
        &db,
        &[
            (host_call("", &[("method", "'POST'")]), "field `method`", "a call that is not a GET"),
            (host_call("", &[("rate_remaining", "-1")]), "field `rate_remaining`", "a negative rate limit left"),
            (host_crawl("", &[("outcome", "'done'")]), "field `outcome`", "a host crawl that ended outside its words"),
            (host_crawl("", &[("calls", "-1")]), "field `calls`", "a host crawl of negative calls"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_account_twice_and_a_second_mirror_of_one_repository_are_refused() {
    let db = provisioned().await;
    take(&db, &[host_repository(":h1", &[]), host_user(":u1", &[]), mirror(":mi1", &[])]).await;
    refuse(
        &db,
        &[
            (host_user("", &[("login", "'octo-renamed'")]), "index `idx_host_user`", "the same account twice"),
            (mirror("", &[("folder", "'github.com/acme/widgets-2.git'")]), "index `idx_mirror`", "a second mirror of one repository on a host"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_request_numbered_0_twice_or_by_a_missing_account_and_its_short_or_negative_facts_are_refused() {
    let db = provisioned().await;
    take(&db, &hosts()).await;
    refuse(
        &db,
        &[
            (pull_request("", &[("number", "0")]), "field `number`", "a pull request numbered 0"),
            (pull_request("", &[]), "index `idx_pull_request`", "the same pull request twice"),
            (pull_request("", &[("number", "8"), ("author", "host_user:nope")]), "field `author`", "a pull request by an account that does not exist"),
            (pull_request_state(&[("head_sha", "'abc12'")]), "field `head_sha`", "a head commit shorter than a hash"),
            (pull_request_state(&[("additions", "-1")]), "field `additions`", "negative lines added"),
            (pull_request_commit(&[("hash", "'abc12'")]), "field `hash`", "a pull request's commit shorter than a hash"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ci_run_of_another_kind_twice_or_under_a_missing_parent_and_a_negative_attempt_are_refused() {
    let db = provisioned().await;
    take(&db, &hosts()).await;
    refuse(
        &db,
        &[
            (ci_run("", &[("kind", "'nightly'"), ("host_id", "556")]), "field `kind`", "a CI run of a kind the structure does not list"),
            (ci_run("", &[]), "index `idx_ci_run`", "the same CI run twice"),
            (ci_run("", &[("host_id", "557"), ("parent", "ci_run:nope")]), "field `parent`", "a CI run under a parent that does not exist"),
            (ci_run_state(&[("attempt", "-1")]), "field `attempt`", "a negative attempt"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reflog_entry_of_a_missing_mirror_and_one_entry_of_a_mirror_twice_are_refused() {
    let db = provisioned().await;
    let mut rows = repositories();
    rows.extend([host_repository(":h1", &[]), mirror(":mi1", &[])]);
    rows.push(ref_move(&[("checkout", "NONE"), ("mirror", "mirror:mi1")]));
    take(&db, &rows).await;
    refuse(
        &db,
        &[
            (
                ref_move(&[("checkout", "NONE"), ("mirror", "mirror:nope"), ("ref", "'refs/heads/other'")]),
                "field `mirror`",
                "a reflog entry of a mirror that does not exist",
            ),
            (ref_move(&[("checkout", "NONE"), ("mirror", "mirror:mi1")]), "index `idx_ref_move_mirror`", "the same reflog entry of a mirror twice"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tie_a_source_a_git_call_step_and_a_main_line_outside_their_words_are_refused() {
    let db = provisioned().await;
    take(&db, &repositories()).await;
    refuse(
        &db,
        &[
            (commit_session(&[("via", "'guess'")]), "field `via`", "a commit tied to a session a way other than printed or pull_request"),
            (
                commit_session(&[("via", "'pull_request'"), ("pull_request", "pull_request:nope")]),
                "field `pull_request`",
                "a commit tied through a pull request that does not exist",
            ),
            (pass_source(&[("source", "'files'")]), "field `source`", "a source other than lines, repos and hosts"),
            (git_call(&[("step", "'hosts'")]), "field `step`", "a git call of the step hosts"),
            (crawl("g1", &[("mainline_from", "'guessed'")]), "field `mainline_from`", "a main line chosen a way outside its words"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_words_the_structure_adds_are_taken() {
    let db = provisioned().await;
    let mut rows = repositories();
    rows.extend(hosts());
    rows.push(mirror(":mi1", &[]));
    for step in ["fetch", "tags", "merges", "patches"] {
        rows.push(git_call(&[("step", &format!("'{step}'"))]));
    }
    rows.push(pass_source(&[("source", "'hosts'")]));
    rows.push(crawl("c2", &[("mainline_from", "'host_default'")]));
    rows.push(commit_session(&[("via", "'pull_request'"), ("pull_request", "pull_request:pr1")]));
    rows.push(ref_move(&[("checkout", "NONE"), ("mirror", "mirror:mi1")]));
    for operation in ["merge", "am", "rebase", "rebase_interactive", "cherry_pick", "revert", "bisect"] {
        rows.push(checkout_state(&[("operations", &format!("['{operation}']"))]));
    }
    take(&db, &rows).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_without_its_offsets_a_short_patch_id_and_negative_lines_ignoring_space_are_refused() {
    let db = provisioned().await;
    take(&db, &repositories()).await;
    refuse(
        &db,
        &[
            (
                commit("d1", "repository:r1", "7123456789abcdef0123456789abcdef01234567", &[("author_offset", "NONE")]),
                "field `author_offset`",
                "a commit with no author offset",
            ),
            (
                commit("d2", "repository:r1", "8123456789abcdef0123456789abcdef01234567", &[("committer_offset", "NONE")]),
                "field `committer_offset`",
                "a commit with no committer offset",
            ),
            (
                commit("d3", "repository:r1", "9123456789abcdef0123456789abcdef01234567", &[("patch_id", "'abc12'")]),
                "field `patch_id`",
                "a patch id shorter than a hash",
            ),
            (commit_file(&[("path", "'b.rs'"), ("added_ignoring_space", "-1")]), "field `added_ignoring_space`", "negative lines added ignoring space"),
            (commit_file(&[("path", "'c.rs'"), ("removed_ignoring_space", "-1")]), "field `removed_ignoring_space`", "negative lines removed ignoring space"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_operation_git_does_not_have_a_non_hash_commit_or_onto_and_negative_rebase_counts_are_refused() {
    let db = provisioned().await;
    take(&db, &repositories()).await;
    refuse(
        &db,
        &[
            (checkout_state(&[("operations", "['squash']")]), "field `operations`", "an operation git does not have"),
            (checkout_state(&[("operation_commits", "['abc12']")]), "field `operation_commits`", "an operation's commit that is not a hash"),
            (checkout_state(&[("rebase_onto", "'abc12'")]), "field `rebase_onto`", "a rebase onto that is not a hash"),
            (checkout_state(&[("rebase_done", "-1")]), "field `rebase_done`", "a negative count of rebase commands done"),
            (checkout_state(&[("rebase_left", "-1")]), "field `rebase_left`", "a negative count of rebase commands left"),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_state_written_without_the_operations_columns_is_taken() {
    let db = provisioned().await;
    let mut rows = repositories();
    rows.push(checkout_state(&[]));
    take(&db, &rows).await;
}
