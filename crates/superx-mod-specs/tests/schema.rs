//! Does the database end up looking like the file? Apply the schema,
//! read it back OUT of the engine, and diff it against the file: every
//! table and field it declares, and nothing else (the check the entities
//! module learned it needed, epic #353).

use std::collections::BTreeSet;

use superx_kernel::types::Value;
use superx_mod_specs::SCHEMA_DDL;

const PW: &str = "test-password";

fn keys_of(v: &Value, section: &str) -> BTreeSet<String> {
    let Value::Object(top) = v else { return BTreeSet::new() };
    match top.get(section) {
        Some(Value::Object(inner)) => inner.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

async fn provisioned() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", PW);
    db.query(ddl).await.expect("provision").check().expect("provision is clean");
    db
}

/// What the file declares, parsed from the DDL so the expectation cannot
/// drift from the thing it checks. An array field gets a second
/// definition from the engine, `field.*`, which INFO reports.
fn declared() -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    let mut tables = BTreeSet::new();
    let mut fields = BTreeSet::new();
    for line in SCHEMA_DDL.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("DEFINE TABLE ") {
            if let Some(name) = rest.split_whitespace().next() {
                tables.insert(name.to_string());
            }
        }
        if let Some(rest) = line.strip_prefix("DEFINE FIELD ") {
            let mut it = rest.split_whitespace();
            if let (Some(field), Some("ON"), Some("TABLE"), Some(table)) = (it.next(), it.next(), it.next(), it.next()) {
                fields.insert((table.to_string(), field.to_string()));
                if rest.contains("array") {
                    fields.insert((table.to_string(), format!("{field}.*")));
                }
            }
        }
    }
    (tables, fields)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_leaves_the_database_exactly_as_this_file_describes() {
    let db = provisioned().await;
    let (want_tables, want_fields) = declared();

    let info: Value = db.query("INFO FOR DB").await.expect("info").take(0).expect("decodes");
    let got_tables = keys_of(&info, "tables");
    assert_eq!(got_tables, want_tables, "the database's tables are not the file's");

    for table in &want_tables {
        let info: Value = db.query(format!("INFO FOR TABLE {table}")).await.expect("info").take(0).expect("decodes");
        let got = keys_of(&info, "fields");
        let want: BTreeSet<String> = want_fields.iter().filter(|(t, _)| t == table).map(|(_, f)| f.clone()).collect();
        assert_eq!(
            got, want,
            "table `{table}` is not the file's.\n  in the file only: {:?}\n  in the database only: {:?}",
            want.difference(&got).collect::<Vec<_>>(),
            got.difference(&want).collect::<Vec<_>>(),
        );
    }
}

/// Provisioning over a database that already holds something leaves
/// nothing of it behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_shape_does_not_survive_underneath() {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    db.query("DEFINE TABLE leftover SCHEMAFULL; DEFINE FIELD junk ON TABLE leftover TYPE string;")
        .await
        .expect("junk")
        .check()
        .expect("junk defined");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", PW);
    db.query(ddl).await.expect("provision").check().expect("provision is clean");

    let info: Value = db.query("INFO FOR DB").await.expect("info").take(0).expect("decodes");
    assert_eq!(keys_of(&info, "tables"), declared().0, "an older table survived provisioning");
}

/// The engine holds the constraints the code relies on: one row per
/// content, links that point at rows that exist, results from the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_refuses_what_the_schema_forbids() {
    let db = provisioned().await;
    let doc = "CREATE spec_doc CONTENT { sha256: 'a', kind: 'task', path: 'p', content: 'c', \
               first_read_at: time::now(), valid_from: time::now() }";
    db.query(doc).await.expect("first").check().expect("the first row is taken");
    let dup = db.query(doc).await.expect("second").check();
    assert!(dup.is_err(), "a second row with the same content hash must be refused");

    let kind = db
        .query("CREATE spec_doc CONTENT { sha256: 'b', kind: 'design', path: 'p', content: 'c', \
                first_read_at: time::now(), valid_from: time::now() }")
        .await
        .expect("query")
        .check();
    assert!(kind.is_err(), "a kind outside root, rules, task must be refused");

    let dangling = db
        .query("CREATE spec_clause CONTENT { doc: spec_doc:nothing, kind: 'rule', position: 0, text: 't', \
                valid_from: time::now() }")
        .await
        .expect("query")
        .check();
    assert!(dangling.is_err(), "a clause of a spec file that does not exist must be refused");
}
