//! Replay transcript files through the module, on in-memory engines, and
//! print what it would record. Reads the files it is given and nothing
//! else; touches no running instance.
//!
//!   cargo run -p superx-mod-specs --example replay -- <transcript.jsonl>…
//!
//! Lines are stored the way the kernel's capture stores them: the role
//! the adapter gives (`user` for a human prompt, `tool` for a tool
//! result, `assistant`), the text, and the whole line as `raw`.

use std::collections::HashMap;

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, NewMessage, SCHEMA_DDL as KERNEL_DDL};
use superx_mod_specs::{ingest, render_runs, store, SCHEMA_DDL};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let kdb = surrealdb::engine::any::connect("mem://").await.expect("mem");
    kdb.use_ns("superx").use_db("kernel").await.expect("nsdb");
    kdb.query(KERNEL_DDL.replace("$SUPERX_KERNEL_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");
    let kernel = Kernel::from_db(kdb);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await.expect("seed");
    }
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");

    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let mut sessions: HashMap<String, superx_kernel::types::RecordId> = HashMap::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read transcript");
        for line in text.lines() {
            let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let kind = json.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if kind != "user" && kind != "assistant" {
                continue;
            }
            let key = json.get("sessionId").and_then(|s| s.as_str()).unwrap_or(file).to_string();
            let session = match sessions.get(&key) {
                Some(s) => s.clone(),
                None => {
                    let id = kernel.create_entity("node_session").await.expect("session");
                    let mut desc = Object::new();
                    desc.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
                    desc.insert("session".to_string(), Value::String(key.clone()));
                    desc.insert("locator".to_string(), Value::String(file.clone()));
                    kernel.supersede_state(id.clone(), "attr_session_descriptor", Value::Object(desc)).await.expect("desc");
                    sessions.insert(key.clone(), id.clone());
                    id
                }
            };
            let content = json.pointer("/message/content");
            let role = if kind == "assistant" {
                "assistant"
            } else if json.pointer("/origin/kind").and_then(|k| k.as_str()) == Some("human") {
                "user"
            } else if content.and_then(|c| c.as_array()).is_some_and(|b| {
                b.iter().any(|x| x.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            }) {
                "tool"
            } else {
                "system"
            };
            let text = content.and_then(|c| c.as_str()).unwrap_or("").to_string();
            let emitted_at = json
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&chrono::Utc));
            kernel
                .log_message(NewMessage {
                    session,
                    agent: agent.clone(),
                    role: role.to_string(),
                    content: text,
                    raw: Some(superx_kernel::message::json_to_object(&json)),
                    seq: None,
                    emitted_at,
                })
                .await
                .expect("message");
        }
    }
    let report = ingest::ingest(&kernel, &db).await.expect("ingest");
    // The builds' repositories, read-only, as last fetched.
    let crawled = superx_mod_specs::crawl::crawl(&kernel, &db).await.expect("crawl");
    println!("{crawled:?}");
    println!("{report:?}\n");
    print!("{}", render_runs(&store::current_runs(&db).await.expect("runs")));
    println!();
    print!("{}", superx_mod_specs::ledger::render(&db, None).await.expect("ledger"));
}
