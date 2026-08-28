//! Does the database actually end up looking like the file?
//!
//! THIS IS THE TEST THAT WAS MISSING. The second attempt applied its DDL
//! and assumed it took. It did not: the tables were defined
//! `IF NOT EXISTS`, so on an instance that had run the shape before it
//! nothing was replaced — `entity` kept a required column the new code
//! never writes, and every create was refused on a database that
//! reported itself provisioned. The DDL ran without error the whole
//! time. Nothing compared the result to the intent.
//!
//! So: apply, read the schema back OUT of the engine, and diff it
//! against this module's own file. Every table it declares must be
//! there, and nothing it does not declare may be.

use std::collections::BTreeSet;

use superx_kernel::types::Value;
use superx_mod_entities::{author::Author, entity, SCHEMA_DDL};

/// The keys of an object the engine returned, or nothing.
fn keys_of(v: &Value, section: &str) -> BTreeSet<String> {
    let Value::Object(top) = v else { return BTreeSet::new() };
    match top.get(section) {
        Some(Value::Object(inner)) => inner.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

const PW: &str = "test-password";

async fn connect() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    db
}

async fn provision(db: &superx_kernel::Db) {
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", PW);
    db.query(ddl).await.expect("provision").check().expect("provision is clean");
}

/// What the ENGINE says is in the database, straight from `INFO`.
async fn tables_in_database(db: &superx_kernel::Db) -> BTreeSet<String> {
    let mut resp = db.query("INFO FOR DB").await.expect("info for db");
    let info: Value = resp.take(0).expect("info decodes");
    keys_of(&info, "tables")
}

async fn fields_in_table(db: &superx_kernel::Db, table: &str) -> BTreeSet<String> {
    let mut resp = db
        .query(format!("INFO FOR TABLE {table}"))
        .await
        .expect("info for table");
    let info: Value = resp.take(0).expect("info decodes");
    keys_of(&info, "fields")
}

/// What THIS FILE says should be in the database. Parsed from the DDL
/// rather than typed out again, so the expectation cannot drift away
/// from the thing it is checking.
fn declared() -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    let mut tables = BTreeSet::new();
    let mut fields = BTreeSet::new();
    for line in SCHEMA_DDL.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("DEFINE TABLE ") {
            if let Some(name) = rest.split_whitespace().next() {
                tables.insert(name.to_string());
                // A RELATION table gets its two endpoints from the
                // engine: `IN entity OUT entity` in the declaration IS
                // the definition of `in` and `out`, and INFO reports
                // them as fields.
                if rest.contains("TYPE RELATION") {
                    fields.insert((name.to_string(), "in".to_string()));
                    fields.insert((name.to_string(), "out".to_string()));
                }
            }
        }
        if let Some(rest) = line.strip_prefix("DEFINE FIELD ") {
            let mut it = rest.split_whitespace();
            if let (Some(field), Some("ON"), Some(kw), Some(table)) =
                (it.next(), it.next(), it.next(), it.next())
            {
                // `ON TABLE x` and `ON x` are both legal.
                let table = if kw == "TABLE" { table } else { kw };
                fields.insert((table.to_string(), field.to_string()));
                // THE ENGINE ADDS ITS OWN. A field declared as an array
                // gets a second definition for the element type,
                // `field.*`, which INFO reports and this file never
                // writes. Expect it, rather than filtering it out —
                // filtering would also swallow a genuine extra.
                if rest.contains("array") {
                    fields.insert((table.to_string(), format!("{field}.*")));
                }
            }
        }
    }
    (tables, fields)
}

/// THE DIFF. On a fresh database the engine's answer and the file's
/// claim must be the same set — no missing table, and no extra one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_leaves_the_database_exactly_as_this_file_describes() {
    let db = connect().await;
    provision(&db).await;

    let (want_tables, want_fields) = declared();
    let got_tables = tables_in_database(&db).await;

    assert_eq!(
        got_tables, want_tables,
        "the database does not match the schema file.\n  in the file, not the database: {:?}\n  in the database, not the file: {:?}",
        want_tables.difference(&got_tables).collect::<Vec<_>>(),
        got_tables.difference(&want_tables).collect::<Vec<_>>(),
    );

    for table in &want_tables {
        let got = fields_in_table(&db, table).await;
        let want: BTreeSet<String> = want_fields
            .iter()
            .filter(|(t, _)| t == table)
            .map(|(_, f)| f.clone())
            .collect();
        assert_eq!(
            got, want,
            "table `{table}` does not match the file.\n  in the file, not the database: {:?}\n  in the database, not the file: {:?}",
            want.difference(&got).collect::<Vec<_>>(),
            got.difference(&want).collect::<Vec<_>>(),
        );
    }
}

/// AND THE SAME ON A DATABASE THAT ALREADY HELD SOMETHING ELSE — which
/// is the case that broke. A column from an older shape surviving
/// underneath is exactly what this catches.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_shape_does_not_survive_underneath() {
    let db = connect().await;

    // The shape that shipped before, and some junk beside it.
    let v1 = include_str!("fixtures/v1-entities.surql").replace("$SUPERX_MODULE_PASSWORD", PW);
    db.query(v1).await.expect("v1").check().expect("v1 is clean");
    db.query("DEFINE TABLE leftover SCHEMAFULL; DEFINE FIELD junk ON TABLE leftover TYPE string;")
        .await
        .expect("junk")
        .check()
        .expect("junk defined");

    provision(&db).await;

    let (want_tables, _) = declared();
    let got_tables = tables_in_database(&db).await;
    assert_eq!(
        got_tables, want_tables,
        "an older shape survived provisioning.\n  left behind: {:?}",
        got_tables.difference(&want_tables).collect::<Vec<_>>(),
    );
    assert!(
        !fields_in_table(&db, "entity").await.contains("entity_type"),
        "the old required column is still on `entity` — every create will be refused"
    );

    // The proof that the diff is not just cosmetic.
    entity::create(&db, "DBA", &Author::operator())
        .await
        .expect("an entity must be creatable after provisioning over an older shape");
}
