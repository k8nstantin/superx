use chrono::{DateTime, Utc};
use surrealdb::engine::any::{connect, Any};
use surrealdb::opt::auth::Database;
use surrealdb::types::{RecordId, SurrealValue};
use surrealdb::Surreal;
use uuid::Uuid;

use crate::error::{KernelError, Result};

/// Service-account username — the only credential the model is permitted
/// to use after schema deploy. Locked by SKILL.md §13.
const SERVICE_USERNAME: &str = "superx";

/// Env var the operator sets to provision the service-account password.
const SERVICE_PASSWORD_ENV: &str = "SUPERX_SERVICE_PASSWORD";

/// v0.1 dev default published in SKILL.md §13 + SUPERX_SCHEMA.md
/// "Database users + access". Operator overrides via the env var above.
/// Documented in the skill so the literal here is the skill, not magic.
const SERVICE_PASSWORD_DEV_DEFAULT: &str = "superx-v01-dev-x9KmP2nQ7tR3vW8y";

/// The v0.1 metamodel — the minimum set of `type_definition` rows the
/// substrate's typed FK ASSERTs reference. Created by
/// [`Kernel::seed_metamodel`] once per substrate, idempotent on repeat
/// invocation.
const METAMODEL_TYPES: &[MetamodelType] = &[
    MetamodelType { uid: "node_run",    category: "node", memory_tier: "core" },
    MetamodelType { uid: "node_agent",  category: "node", memory_tier: "core" },
    MetamodelType { uid: "node_source", category: "node", memory_tier: "core" },
];

struct MetamodelType {
    uid: &'static str,
    category: &'static str,
    memory_tier: &'static str,
}

/// The substrate kernel — the model's only access path to SurrealDB.
///
/// A `Kernel` owns one [`Surreal<Any>`] connection that is signed in as
/// the `superx` EDITOR service account against an operator-provisioned
/// substrate (where `scripts/deploy-schema.sh` has already applied
/// the locked schema). Every typed verb on this struct issues only
/// `CREATE` or `SELECT` SQL — there is no `UPDATE`-emitting and no
/// `DELETE`-emitting method by design.
pub struct Kernel {
    db: Surreal<Any>,
}

impl Kernel {
    /// Connect to an operator-provisioned SurrealDB and sign in as
    /// the `superx` service account.
    ///
    /// `endpoint` is a SurrealDB connection URL — `ws://host:port`,
    /// `wss://host:port`, `http://host:port`, `https://host:port`, or
    /// (in tests) `mem://`. `namespace` and `database` select the
    /// SuperX deployment within the engine.
    ///
    /// The service-account password is read from the
    /// `SUPERX_SERVICE_PASSWORD` env var; if absent it falls back to
    /// the v0.1 dev default documented in SKILL.md §13.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::Db`] if the engine refuses the
    /// connection, the namespace/database selection, or the signin —
    /// e.g. when the schema has not been deployed yet or the password
    /// does not match the one bound at schema-deploy time.
    pub async fn connect_service(
        endpoint: &str,
        namespace: &str,
        database: &str,
    ) -> Result<Self> {
        let password = std::env::var(SERVICE_PASSWORD_ENV)
            .unwrap_or_else(|_| SERVICE_PASSWORD_DEV_DEFAULT.to_string());

        let db = connect(endpoint).await?;
        db.use_ns(namespace).use_db(database).await?;
        db.signin(Database {
            namespace: namespace.to_string(),
            database: database.to_string(),
            username: SERVICE_USERNAME.to_string(),
            password,
        })
        .await?;

        Ok(Self { db })
    }

    /// Wrap an already-authenticated [`Surreal<Any>`] connection.
    ///
    /// Intended for integration tests that hand-construct the
    /// substrate (`mem://` engine, schema applied, signed in) and want
    /// to exercise the kernel verbs against it. Production code paths
    /// go through [`Kernel::connect_service`].
    #[must_use]
    pub fn from_db(db: Surreal<Any>) -> Self {
        Self { db }
    }

    /// Underlying SurrealDB handle.
    #[must_use]
    pub fn db(&self) -> &Surreal<Any> {
        &self.db
    }

    /// Idempotently CREATE the v0.1 metamodel rows that the substrate's
    /// typed FK ASSERTs reference (`node_run`, `node_agent`,
    /// `node_source`). Safe to call repeatedly — only missing types are
    /// inserted; existing ones are left alone (the substrate is
    /// append-only, so creating a second row for the same uid would
    /// just produce a chain whose latest entry wins by `valid_from`,
    /// but that's wasteful and would break the "one row per uid"
    /// invariant the metamodel relies on).
    ///
    /// Emits one `metamodel_seeded` telemetry event listing the uids
    /// that were created (empty list if the metamodel was already
    /// fully seeded).
    ///
    /// # Errors
    ///
    /// Surfaces engine refusals verbatim via [`KernelError::Db`].
    pub async fn seed_metamodel(&self) -> Result<()> {
        let mut created: Vec<String> = Vec::with_capacity(METAMODEL_TYPES.len());

        for spec in METAMODEL_TYPES {
            if self.find_type_opt(spec.uid).await?.is_some() {
                continue;
            }
            let id = RecordId::new(
                "type_definition",
                surrealdb::types::Uuid::from(Uuid::now_v7()),
            );
            let row = TypeDefinitionRow {
                uid: spec.uid.to_string(),
                category: spec.category.to_string(),
                is_acyclic: true,
                sch_json: None,
                memory_tier: spec.memory_tier.to_string(),
                valid_from: Utc::now(),
            };
            let _: Option<TypeDefinitionRow> =
                self.db.create(id).content(row).await?;
            created.push(spec.uid.to_string());
        }

        let mut payload = surrealdb::types::Object::new();
        let array_items: Vec<surrealdb::types::Value> = created
            .into_iter()
            .map(surrealdb::types::Value::String)
            .collect();
        payload.insert(
            "created_uids".to_string(),
            surrealdb::types::Value::Array(surrealdb::types::Array::from(array_items)),
        );
        self.log_telemetry(
            "metamodel_seeded",
            surrealdb::types::Value::Object(payload),
            None,
        )
        .await?;

        Ok(())
    }

    /// Look up the [`RecordId`] of the latest `type_definition` row
    /// whose `uid` matches the argument. Returns [`KernelError::NotFound`]
    /// if no such row exists.
    ///
    /// # Errors
    ///
    /// [`KernelError::NotFound`] when no row with the given uid is
    /// present; [`KernelError::Db`] for engine errors.
    pub async fn find_type(&self, uid: &str) -> Result<RecordId> {
        self.find_type_opt(uid)
            .await?
            .ok_or_else(|| KernelError::NotFound(format!("type_definition with uid '{uid}'")))
    }

    async fn find_type_opt(&self, uid: &str) -> Result<Option<RecordId>> {
        #[derive(SurrealValue)]
        struct IdOnly {
            id: RecordId,
        }

        // `seed_metamodel` guarantees one row per uid; LIMIT 1 alone
        // is sufficient. ORDER BY valid_from would require projecting
        // valid_from too in SurrealDB 3.x, which is dead weight while
        // the uniqueness invariant holds.
        let rows: Vec<IdOnly> = self
            .db
            .query("SELECT id FROM type_definition WHERE uid = $uid LIMIT 1")
            .bind(("uid", uid.to_string()))
            .await?
            .take(0)?;

        Ok(rows.into_iter().next().map(|r| r.id))
    }

    /// CREATE one row in `entity` with an explicit UUIDv7 id (§11) and
    /// a typed FK to the named `type_definition` row.
    ///
    /// Emits one `entity_created` telemetry event with the new entity's
    /// id and type uid in the payload.
    ///
    /// # Errors
    ///
    /// [`KernelError::NotFound`] if `type_uid` doesn't resolve;
    /// [`KernelError::Db`] for engine errors (e.g. role outside the
    /// engine-enforced `INSIDE ['user', 'admin']` ASSERT).
    pub async fn create_entity(&self, type_uid: &str, role: &str) -> Result<RecordId> {
        let type_id = self.find_type(type_uid).await?;
        let id = RecordId::new("entity", surrealdb::types::Uuid::from(Uuid::now_v7()));
        let row = EntityRow {
            r#type: type_id,
            role: role.to_string(),
            valid_from: Utc::now(),
        };

        let _: Option<EntityRow> = self.db.create(id.clone()).content(row).await?;

        let mut payload = surrealdb::types::Object::new();
        payload.insert(
            "entity_id".to_string(),
            surrealdb::types::Value::RecordId(id.clone()),
        );
        payload.insert(
            "type_uid".to_string(),
            surrealdb::types::Value::String(type_uid.to_string()),
        );
        self.log_telemetry(
            "entity_created",
            surrealdb::types::Value::Object(payload),
            None,
        )
        .await?;

        Ok(id)
    }

    /// Append one row to `telemetry_stream` with an explicit UUIDv7 id.
    ///
    /// Telemetry is global across the entire OS (v0.1 single-deployment
    /// model — no tenant scoping). `run` may reference a `node_run`
    /// entity or be `None` for events with no run context (e.g.
    /// bootstrap, system_*).
    ///
    /// # Errors
    ///
    /// Surfaces engine refusals verbatim via [`KernelError::Db`].
    pub async fn log_telemetry(
        &self,
        lifecycle_event: &str,
        payload: surrealdb::types::Value,
        run: Option<RecordId>,
    ) -> Result<RecordId> {
        let id = RecordId::new(
            "telemetry_stream",
            surrealdb::types::Uuid::from(Uuid::now_v7()),
        );
        let row = TelemetryRow {
            lifecycle_event: lifecycle_event.to_string(),
            payload,
            run,
            valid_from: Utc::now(),
        };

        let _: Option<TelemetryRow> = self.db.create(id.clone()).content(row).await?;
        Ok(id)
    }
}

#[derive(Debug, SurrealValue)]
struct TypeDefinitionRow {
    uid: String,
    category: String,
    is_acyclic: bool,
    sch_json: Option<String>,
    memory_tier: String,
    valid_from: DateTime<Utc>,
}

#[derive(Debug, SurrealValue)]
struct EntityRow {
    #[surreal(rename = "type")]
    r#type: RecordId,
    role: String,
    valid_from: DateTime<Utc>,
}

#[derive(Debug, SurrealValue)]
struct TelemetryRow {
    lifecycle_event: String,
    payload: surrealdb::types::Value,
    run: Option<RecordId>,
    valid_from: DateTime<Utc>,
}
