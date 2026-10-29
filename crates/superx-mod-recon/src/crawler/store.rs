//! How the crawler writes: rows built a field at a time, held in order,
//! and created a few hundred statements per round trip. Every source
//! writes through here. Nothing here emits UPDATE or DELETE (§10). Row ids
//! are UUIDv7, set here (§11), never by the engine.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::{Array, Datetime, Number, Object, RecordId, Uuid, Value};
use superx_kernel::{Db, Result};

/// Statements in one write: a page of rows goes in a few round trips.
const STATEMENTS_PER_WRITE: usize = 200; // skill-allow: §9-const — write-path bound, not a policy tunable

/// Rows to create, in the order they must be created: a row's links name
/// rows created before it, and the engine checks each link exists.
#[derive(Debug, Default)]
pub struct Batch {
    rows: Vec<(RecordId, Object)>,
    /// Rows added, by table, since the pass began.
    pub added: BTreeMap<&'static str, i64>,
}

impl Batch {
    /// A row of `table`, its id set here, its `valid_from` now.
    pub fn create(&mut self, table: &'static str, mut content: Row) -> RecordId {
        let id = RecordId::new(table, Uuid::new_v7());
        content.0.insert("valid_from", Value::Datetime(Datetime::from(Utc::now())));
        self.rows.push((id.clone(), content.0));
        *self.added.entry(table).or_default() += 1;
        id
    }

    /// Rows waiting to be written.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.rows.len()
    }

    /// Rows added in all, since the pass began.
    #[must_use]
    pub fn total(&self) -> i64 {
        self.added.values().sum()
    }
}

/// Write every waiting row, in order.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the engine refuses a row: the rows
/// before it in its write are written, the rest are not.
pub async fn flush(db: &Db, batch: &mut Batch) -> Result<()> {
    for chunk in batch.rows.chunks(STATEMENTS_PER_WRITE) {
        let mut sql = String::new();
        let mut vars = Object::new();
        for (i, (id, content)) in chunk.iter().enumerate() {
            sql.push_str(&format!("CREATE $i{i} CONTENT $c{i};\n"));
            vars.insert(format!("i{i}"), Value::RecordId(id.clone()));
            vars.insert(format!("c{i}"), Value::Object(content.clone()));
        }
        db.query(sql).bind(vars).await?.check()?;
    }
    batch.rows.clear();
    Ok(())
}

/// A row's content, built a field at a time; a field with no value is left
/// out, which the schema reads as NONE.
#[derive(Debug, Default, Clone)]
pub struct Row(Object);

impl Row {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn text(mut self, key: &str, v: impl Into<String>) -> Self {
        self.0.insert(key, Value::String(v.into()));
        self
    }

    #[must_use]
    pub fn some_text(self, key: &str, v: Option<impl Into<String>>) -> Self {
        match v {
            Some(v) => self.text(key, v),
            None => self,
        }
    }

    #[must_use]
    pub fn int(mut self, key: &str, v: i64) -> Self {
        self.0.insert(key, Value::Number(Number::Int(v)));
        self
    }

    #[must_use]
    pub fn some_int(self, key: &str, v: Option<i64>) -> Self {
        match v {
            Some(v) => self.int(key, v),
            None => self,
        }
    }

    #[must_use]
    pub fn flag(mut self, key: &str, v: bool) -> Self {
        self.0.insert(key, Value::Bool(v));
        self
    }

    #[must_use]
    pub fn time(mut self, key: &str, v: DateTime<Utc>) -> Self {
        self.0.insert(key, Value::Datetime(Datetime::from(v)));
        self
    }

    #[must_use]
    pub fn uuid(mut self, key: &str, v: Uuid) -> Self {
        self.0.insert(key, Value::Uuid(v));
        self
    }

    #[must_use]
    pub fn some_uuid(self, key: &str, v: Option<Uuid>) -> Self {
        match v {
            Some(v) => self.uuid(key, v),
            None => self,
        }
    }

    #[must_use]
    pub fn link(mut self, key: &str, v: &RecordId) -> Self {
        self.0.insert(key, Value::RecordId(v.clone()));
        self
    }

    #[must_use]
    pub fn object(mut self, key: &str, v: Object) -> Self {
        self.0.insert(key, Value::Object(v));
        self
    }
}

impl Row {
    #[must_use]
    pub fn texts(mut self, key: &str, v: &[String]) -> Self {
        let items: Vec<Value> = v.iter().cloned().map(Value::String).collect();
        self.0.insert(key, Value::Array(Array::from(items)));
        self
    }

    #[must_use]
    pub fn links(mut self, key: &str, v: &[RecordId]) -> Self {
        let items: Vec<Value> = v.iter().cloned().map(Value::RecordId).collect();
        self.0.insert(key, Value::Array(Array::from(items)));
        self
    }

    #[must_use]
    pub fn counts(mut self, key: &str, v: &BTreeMap<&'static str, i64>) -> Self {
        let mut o = Object::new();
        for (name, n) in v {
            o.insert(*name, Value::Number(Number::Int(*n)));
        }
        self.0.insert(key, Value::Object(o));
        self
    }
}
