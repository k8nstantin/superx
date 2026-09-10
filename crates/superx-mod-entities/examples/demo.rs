//! A dev harness: the entities service on a throwaway in-memory
//! database, so the dashboard can be driven without a provisioned
//! instance.
//!
//! `cargo run -p superx-mod-entities --example demo [port] [--empty]`
//!
//! It seeds the operator's own example — a DBA role with a vocabulary,
//! fields and a four-deep graph — so there is something real to look at
//! the moment the page loads. `--empty` skips the seed: a freshly
//! provisioned instance has no vocabulary at all, and the way the UI
//! bootstraps one is a thing that has to be seen working.

use superx_mod_entities::attribute::{self, Write};
use superx_mod_entities::author::Author;
use superx_mod_entities::{edge, entity, SCHEMA_DDL};

/// Deliberately NOT the module's own port: this harness must never be
/// able to answer for a real instance.
const DEMO_PORT: u16 = 5199; // skill-allow: §9-const — a dev harness's own default, overridable by argument

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let db = surrealdb::engine::any::connect("mem://").await?;
    db.use_ns("superx").use_db("entities").await?;
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "demo")).await?.check()?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--empty") {
        seed(&db).await?;
    }

    let kernel = superx_kernel::Kernel::from_db(db.clone());
    // Never assume the module's real port is free: an instance may
    // already own it, and binding over a live service is not something a
    // demo should be able to attempt.
    let port: u16 = args
        .iter()
        .find_map(|p| p.parse().ok())
        .unwrap_or(DEMO_PORT);
    superx_mod_entities::serve(&kernel, db, port).await?;
    println!("entities dashboard: http://127.0.0.1:{port}");
    // Park. The harness is killed from outside.
    std::future::pending::<()>().await;
    Ok(())
}

async fn seed(db: &superx_kernel::Db) -> Result<(), Box<dyn std::error::Error>> {
    let op = Author::operator();
    // `label` carries itself: an entity that has `label` attached is a
    // label, and the word is no exception — which is also what the UI
    // writes when it bootstraps the vocabulary on a fresh instance.
    let label = entity::create(db, "label", &op).await?;
    entity::declare(db, &label, "is", std::slice::from_ref(&label), &op).await?;
    let make_label = |name: &'static str| {
        let db = db.clone();
        let label = label.clone();
        let op = op.clone();
        async move {
            let e = entity::create(&db, name, &op).await?;
            entity::declare(&db, &e, "is", std::slice::from_ref(&label), &op).await?;
            Ok::<_, Box<dyn std::error::Error>>(e)
        }
    };
    let role = make_label("role").await?;
    let mandate = make_label("mandate").await?;
    let contains = make_label("contains").await?;
    let consults = make_label("consults").await?;

    attribute::add(
        db,
        &role,
        Write {
            name: "treatment",
            datatype: "text",
            content: Some(superx_kernel::types::Value::String(
                "<p>This is who you are. Act within it and nothing beyond it.</p>".into(),
            )),
            labels: &[],
            options: None,
        },
        &op,
    )
    .await?;

    let dba = entity::create(db, "DBA", &op).await?;
    entity::declare(db, &dba, "is", std::slice::from_ref(&role), &op).await?;
    attribute::add(
        db,
        &dba,
        Write {
            name: "house_rules",
            datatype: "text",
            content: Some(superx_kernel::types::Value::String(
                "<p>Never drop a table without a snapshot.</p>".into(),
            )),
            labels: std::slice::from_ref(&mandate),
            options: None,
        },
        &op,
    )
    .await?;
    attribute::add(
        db,
        &dba,
        Write {
            name: "replicas",
            datatype: "number",
            content: Some(superx_kernel::types::Value::Number(3.into())),
            labels: &[],
            options: None,
        },
        &op,
    )
    .await?;

    let backups = entity::create(db, "Backups", &op).await?;
    let nightly = entity::create(db, "Nightly verify", &op).await?;
    let checksum = entity::create(db, "Checksum step", &op).await?;
    let runbooks = entity::create(db, "Runbooks", &op).await?;
    let c = std::slice::from_ref(&contains);
    edge::link(db, &dba, &backups, "runs backups", c, &op).await?;
    edge::link(db, &backups, &nightly, "its task", c, &op).await?;
    edge::link(db, &nightly, &checksum, "then", c, &op).await?;
    edge::link(db, &dba, &runbooks, "reads", std::slice::from_ref(&consults), &op).await?;
    Ok(())
}
