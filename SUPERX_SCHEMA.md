# SUPERX SCHEMA

> **v0.1 single-deployment model.** SuperX is one global agentic OS — one
> installation, one operator, telemetry global across everything. No
> multi-tenant isolation at the data layer. Earlier drafts of this doc
> carried a `tenant: record<entity>` FK on every table plus a
> `node_substrate` metamodel concept; both were removed in the
> "drop multi-tenant scaffolding" operator-approved schema amendment.
> If multi-customer SaaS isolation is added in a future release, it
> lands as a separate schema PR with its own operator approval.

## Universal contract (binding on every table)

Every row in every table carries:

| Field | Type | Why |
|---|---|---|
| `id` | `uuidv7` | Explicit UUIDv7 set by the kernel at insert time. Lex-sortable by creation time. Externally portable — referenceable from Iceberg lakes, federation peers, audit trails, downstream pipelines, any system that consumes SuperX exports. Never auto-generated. |
| `valid_from` | `datetime` | When this row was created. The substrate is **append-only + insert-only** — every state change is a fresh row; the "current" version of a chain is recovered by `ORDER BY valid_from DESC LIMIT 1` against the chain key. There is **no `is_current` flag**, **no `valid_to` close timestamp**, **no in-place mutation** — those would drift from the truth the moment the model couldn't or didn't update them. The only temporal field is `valid_from`. |

**Canonical row reference format:** `<table>:<uuidv7>` — portable across SuperX nodes, downstream warehouses, federation peers, every external consumer.

## Database users + access

The substrate has two accounts. The operator owns root; the model uses a service account with no UPDATE / DELETE / schema-write privileges.

```sql
-- Operator: root account is the SurrealDB built-in root user.
-- Used only by the operator, only to apply schema changes.
-- Not represented here as a DEFINE USER — it is the engine's root.

-- Model: service account named `superx`.
-- EDITOR role at user level, narrowed by per-table PERMISSIONS so the
-- effective grant is SELECT + CREATE only (no UPDATE, no DELETE).
DEFINE USER IF NOT EXISTS superx ON DATABASE
    PASSWORD $superx_service_password
    ROLES EDITOR
    DURATION FOR SESSION 1h, FOR TOKEN 1h;
```

**Credentials** (also recorded in `.claude/skills/zero-trust-execution/SKILL.md §13`):
- Login: `superx`
- Password: `superx-v01-dev-x9KmP2nQ7tR3vW8y` (v0.1 dev default; operator overrides via `SUPERX_SERVICE_PASSWORD` env)

Per [SurrealDB's `DEFINE USER` docs](https://surrealdb.com/docs/surrealql/statements/define/user), only the built-in `OWNER` / `EDITOR` / `VIEWER` roles are available at the user level. To enforce **SELECT + CREATE only** at the substrate, every table additionally carries:

```sql
DEFINE TABLE <table> SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;
```

`FOR update NONE` and `FOR delete NONE` document the intended write surface (SELECT + CREATE only). Per SKILL.md §10 the EDITOR role does not honour those clauses for system users; the append-only invariant is actually enforced by **kernel-verb discipline** — no kernel verb emits an UPDATE or DELETE statement, ever. The `FOR update / FOR delete NONE` lines are kept as in-schema documentation of intent, not as the enforcement layer.

The password is bound from the `SUPERX_SERVICE_PASSWORD` environment variable at bootstrap time. The operator sets this once; the model never sees it in plain text after handover.

## Cross-reference contract

Every field that holds an id of another row uses SurrealDB's `record<table>` type — a typed foreign-key reference. The same `<table>:<uuidv7>` string surfaces in the substrate, on the wire to downstream systems, and in any audit export. Joins are first-class; integrity is at the engine level.

Three special entity types underpin the cross-reference graph:

| Entity type | Purpose | Referenced by |
|---|---|---|
| `node_run` | Workflow run identity | every `run` field on `execution_params`, `schedule`, `telemetry_stream` |
| `node_agent` | Agent identity | `execution_params.agent` |
| `node_source` | Telemetry capture source (file path, OTLP endpoint, …) | `cursor.subject` when `cursor_type.category = 'telemetry'` |

All three are seeded by `seed_metamodel` on first deploy.

---

## type_definition

| Field | Type |
|---|---|
| id | uuidv7 |
| uid | string |
| category | string |
| is_acyclic | bool |
| sch_json | option<string> |
| memory_tier | string |
| valid_from | datetime |

## entity

| Field | Type |
|---|---|
| id | uuidv7 |
| type | record<type_definition> |
| role | string |
| valid_from | datetime |

## relation

| Field | Type |
|---|---|
| id | uuidv7 |
| in | record<entity> |
| out | record<entity> |
| type | record<type_definition> |
| is_acyclic | bool |
| valid_from | datetime |

## state_ledger

| Field | Type |
|---|---|
| id | uuidv7 |
| target | record<entity> |
| type | record<type_definition> |
| payload | object |
| valid_from | datetime |

## cursor

| Field | Type |
|---|---|
| id | uuidv7 |
| subject | record<entity> |
| cursor_type | record<cursor_type> |
| last_processed | option<string> |
| metadata | option<object> |
| valid_from | datetime |

## cursor_type

| Field | Type |
|---|---|
| id | uuidv7 |
| uid | string |
| category | string |
| description | string |
| sch_json | option<string> |
| valid_from | datetime |

## execution_params

| Field | Type |
|---|---|
| id | uuidv7 |
| run | record<entity> |
| agent | record<entity> |
| params_json | object |
| valid_from | datetime |

## schedule

| Field | Type |
|---|---|
| id | uuidv7 |
| run | record<entity> |
| kind | string |
| target | record<entity> |
| due_at | datetime |
| status | string |
| attempt | int |
| depends_on | array<record<schedule>> |
| metadata | object |
| valid_from | datetime |

## telemetry_stream

| Field | Type |
|---|---|
| id | uuidv7 |
| lifecycle_event | string |
| payload | any |
| run | option<record<entity>> |
| valid_from | datetime |

---

## Cross-reference graph (every typed FK with its engine-enforced constraint)

Every FK is `record<table>` (typed reference) plus an `ASSERT` clause that enforces type-of-target. The substrate refuses any insert that points at the wrong type. Constraints below are live in `apply_substrate_schema`.

| Table | FK field | Type | ASSERT |
|---|---|---|---|
| `entity` | `type` | `record<type_definition>` | `$value.category = 'node'` |
| `relation` | `in` | `record<entity>` | — (any entity) |
| `relation` | `out` | `record<entity>` | — (any entity) |
| `relation` | `type` | `record<type_definition>` | `$value.category = 'edge'` |
| `state_ledger` | `target` | `record<entity>` | — (any entity) |
| `state_ledger` | `type` | `record<type_definition>` | `$value.category = 'attribute'` |
| `cursor` | `subject` | `record<entity>` | — (polymorphic: `node_run` for workload, `node_source` for telemetry) |
| `cursor` | `cursor_type` | `record<cursor_type>` | — (table is self-discriminating) |
| `execution_params` | `run` | `record<entity>` | `$value.type.uid = 'node_run'` |
| `execution_params` | `agent` | `record<entity>` | `$value.type.uid = 'node_agent'` |
| `schedule` | `run` | `record<entity>` | `$value.type.uid = 'node_run'` |
| `schedule` | `target` | `record<entity>` | — (any DAG-node entity: product, component, task, …) |
| `schedule` | `depends_on[]` | `array<record<schedule>>` | — (each element points at a `schedule` row) |
| `telemetry_stream` | `run` | `option<record<entity>>` | `$value = NONE OR $value.type.uid = 'node_run'` |

Additional enum-style ASSERTs (not FKs but value-domain constraints):

| Table | Field | Type | ASSERT |
|---|---|---|---|
| `type_definition` | `memory_tier` | `string` | `$value INSIDE ['core', 'working', 'archival', 'recall']` |
| `entity` | `role` | `string` | `$value INSIDE ['user', 'admin']` |
| `cursor_type` | `category` | `string` | `$value INSIDE ['workload', 'telemetry']` |
| `schedule` | `status` | `string` | `$value INSIDE ['waiting', 'scheduled', 'running', 'completed', 'failed', 'awaiting_human']` |
| `schedule` | `attempt` | `int` | `$value >= 0` |

## NOT NULL contract

Every field is NOT NULL **by default** in SurrealDB unless explicitly declared `option<...>`. Fields that are nullable:

| Table | Nullable field | Why |
|---|---|---|
| `type_definition` | `sch_json: option<string>` | Many types have no JSON Schema (e.g. `edge_owns`, `node_agent`) |
| `cursor` | `last_processed: option<string>` | A freshly-enqueued cursor has no progress yet |
| `cursor` | `metadata: option<object>` | Optional envelope |
| `cursor_type` | `sch_json: option<string>` | Most cursor types have no metadata schema yet |
| `telemetry_stream` | `run: option<record<entity>>` | Some events (e.g. `system_bootstrap`) have no run context |

Every other field on every table is **NOT NULL** at the engine level. Attempts to insert without these fields fail at the substrate.

---

## Why UUIDv7 specifically

| Property | What UUIDv7 gives us | Why it matters for an agentic OS |
|---|---|---|
| Lex-sortable by time | 48-bit ms-since-epoch prefix | Range queries on `id` alone; recent-first ordering for free |
| Globally unique | 128 bits, RFC 9562 | Multi-node federation without coordination |
| Externally portable | Standard UUID format | Iceberg / ClickHouse / Postgres / Parquet / OTel / Kafka understand it natively |
| Universal | Any language / tool can parse | `grep`, `jq`, ad-hoc queries from any environment |
| Temporal | Insertion order recoverable from the id itself | Audit reconstruction without auxiliary state |

---

## "Current" semantics — query, not column

The substrate has **no `is_current` flag** and **no `valid_to` close timestamp**. Both were dropped for one reason: the model's service account cannot UPDATE rows after insert, so any close-prior pattern (set `is_current = false` on the old row) is engine-refused. A column that the application can write at insert but never update will drift from reality the first time the model can't or won't update it, and that drift is the worst kind — silent.

Instead, every "find current" query takes the form:

```sql
SELECT * FROM <table>
  WHERE <chain_key>
  ORDER BY valid_from DESC
  LIMIT 1;
```

For each table, the chain key is the business identity:

| Table | Chain key (`WHERE …`) |
|---|---|
| `state_ledger` | `target = $entity AND type = $attr_type` |
| `execution_params` | `run = $run AND agent = $agent` |
| `schedule` | `id = $schedule_chain_root` *(latest in the chain)* |
| `cursor` | `subject = $subject AND cursor_type = $cursor_type` |
| `entity` | `id = $entity` *(latest version of an identity, if entities ever supersede)* |
| `relation` | `in = $a AND out = $b AND type = $edge_type` |
| `type_definition` | `uid = $uid` |
| `cursor_type` | `uid = $uid` |
| `telemetry_stream` | event-stream — every row is its own row; "latest" is a recency query, not a chain |

History is the full SELECT without `LIMIT 1`, ordered ASC. Time-travel is filtering by `valid_from <= $t`. Audit is the immutable log.

---

## Changes from existing schema (today)

| Table | Change |
|---|---|
| all tables | explicit UUIDv7 row id contract (was implicit auto-id on some tables) |
| all tables | only `valid_from` as the temporal column. `is_current` and `valid_to` are **dropped** (model is insert-only; mutable advisory columns would drift). |
| all tables | tenant FK dropped — v0.1 is a single global agentic OS, no multi-tenant isolation at the data layer |
| all tables | `PERMISSIONS FOR update NONE; FOR delete NONE;` as documentation of intent (kernel-verb discipline is the actual enforcement, per SKILL.md §10) |
| `type_definition` | + `uid string` (human-readable identifier separated from row id; previously row id was the named string) |
| `entity` | − `is_deleted` (drop; superseding entities create a new row with a later `valid_from`) |
| `execution_cursor` → `cursor` | renamed; `subject: record<entity>` replaces `run_id: string`; `cursor_type` is `record<cursor_type>` FK; − `updated_at` (replaced by `valid_from`) |
| `cursor_type` | **new** — categorization of cursor kinds (`workload` / `telemetry` / …) |
| `execution_params` | `run_id: string` → `run: record<entity>`; `agent_id: string` → `agent: record<entity>` |
| `schedule` | `run_id: string` → `run: record<entity>`; `target_entity: string` → `target: record<entity>`; `depends_on: array<string>` → `array<record<schedule>>` |
| `telemetry_stream` | `run_id: option<string>` → `run: option<record<entity>>`; − `timestamp` (replaced by `valid_from`) |
| **new** | `DEFINE USER service_account ROLES EDITOR` for the model |

## Metamodel additions

| Type | Status | Purpose |
|---|---|---|
| `node_agent` | seeded | agent identity; `execution_params.agent` FK |
| `node_run` | seeded | workflow-run identity; every `run` FK across substrate points here. Enables per-run JOINs, lifecycle tracking, and lineage queries. |
| `node_source` | seeded | telemetry capture source (Claude Code session file, Gemini transcript dir, OTLP endpoint, …); `cursor.subject` FK when `cursor_type.category = 'telemetry'`. Makes capture sources queryable in the substrate. |

## Implied kernel-verb changes (design-level only; gated by operator authorization)

| Verb | Change |
|---|---|
| `set_session_auth` | `db.signin(…)` against the `superx` service account (EDITOR role). No session variables, no `db.set("session_tenant", …)` — v0.1 has no tenant scoping. |
| `supersede_state` | drops the `BEGIN TRANSACTION; UPDATE prior; CREATE new; COMMIT;` pattern. Becomes pure `CREATE state_ledger CONTENT {…};`. "Current" is recovered via `ORDER BY valid_from DESC LIMIT 1` filtered by `(target, type)`. |
| `set_execution_params` | same — pure INSERT, no close-prior |
| `transition_schedule_status` | same |
| `checkpoint_execution` | renamed to `write_cursor` (or similar) — accepts `Thing` for `subject`, not a string; pure INSERT |
| `enqueue_schedule_item` | accepts `Thing` for `run` and `target`, not strings |
| `log_telemetry` | accepts `Option<Thing>` for `run`, not `Option<String>` |
| PERMISSIONS clauses on every table | `FOR select FULL FOR create FULL FOR update NONE FOR delete NONE` (intent-doc; kernel-verb discipline is the actual enforcement) |

These are gated by explicit operator authorization per skill §10.A. The model does not modify any of them without per-change sign-off.

## Why this matters for the agentic OS

Once all FK fields are `record<table>` typed and kernel-verb discipline refuses update/delete, the substrate becomes a **fully joinable, append-only graph that cannot be coerced into a corrupt state by application code**:

- Every cursor knows its subject; every schedule item knows its run + target entity; every telemetry event optionally knows its run; every state-ledger write knows its target entity.
- Cross-referenceable end-to-end — no opaque string keys, no string-to-id parsing at the verb boundary, no downstream consumer having to reconstruct relationships from naming conventions.
- The model cannot silently corrupt history — every state change is a new row with its own UUIDv7, and kernel-verb discipline refuses to delete or mutate prior rows.
- Bugs surface as substrate constraint violations within milliseconds (§12), not as silent downstream drift.
