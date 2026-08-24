-- =========================================================================
-- superx-mod-entities — the product graph substrate's OWN schema
-- =========================================================================
-- Applied into the module's OWN database (superx/entities) by
--   superx modules provision entities
-- under the operator's one-shot root session. The KERNEL schema is
-- untouched (§7). Every DEFINE carries IF NOT EXISTS because module
-- provisioning re-applies this file verbatim (no tolerant path).
--
-- Contract (epic #166; D19/D20/D22):
--   * append-only — every change is an INSERT; "current" is computed as
--     ORDER BY valid_from DESC LIMIT 1 per chain; no is_current, no
--     valid_to, ever
--   * entity rows are IMMUTABLE ANCHORS (one row per entity, forever)
--     so native graph edges always point at a stable record
--   * edges are SurrealDB native relations written by RELATE (creation
--     only, D19); unlink appends a new row on the same edge_uid with
--     active = false — the full link history is the table
--   * uuid7 record ids everywhere — time-ordered, the historical log

DEFINE USER IF NOT EXISTS superx_mod_entities ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;

-- ---------------------------------------------------------------- types --
-- Types-as-data: node kinds (category 'entity') and edge kinds (category
-- 'relation') are ROWS, extensible at runtime with no DDL. Create-once
-- rows guarded by the UNIQUE name index.
DEFINE TABLE IF NOT EXISTS entity_type SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS name        ON TABLE entity_type TYPE string;
DEFINE FIELD IF NOT EXISTS category    ON TABLE entity_type TYPE string
    ASSERT $value INSIDE ['entity', 'relation'];
DEFINE FIELD IF NOT EXISTS description ON TABLE entity_type TYPE option<string>;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE entity_type TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_entity_type_name ON TABLE entity_type FIELDS name UNIQUE;

-- -------------------------------------------------------------- anchors --
-- One immutable row per entity, forever (D20). The uuid7 record id is
-- the entity's identity; everything that can change lives in
-- entity_state chains. Anchors are what edges point at.
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity_type ON TABLE entity TYPE string;
DEFINE FIELD IF NOT EXISTS created_at  ON TABLE entity TYPE datetime;

-- --------------------------------------------------------------- states --
-- SCD-2 version rows: one chain per entity. `name` is the one-line
-- label lists and trees render from; `content` is the unbounded text
-- payload of content-bearing nodes (chiefly type 'text' — D22);
-- `attributes` is the FLEXIBLE per-type JSON bag (SCHEMAFULL would
-- otherwise demand a DEFINE FIELD per nested key).
DEFINE TABLE IF NOT EXISTS entity_state SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity     ON TABLE entity_state TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name       ON TABLE entity_state TYPE string;
DEFINE FIELD IF NOT EXISTS content    ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS attributes ON TABLE entity_state TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE entity_state TYPE datetime;
-- Hidden, not erased: archiving takes a thing out of the lists without
-- taking it out of the record. Append-only, so archiving is a NEW
-- version with archived = true and un-archiving is another — which is
-- why this is not a mutable flag and why the decision is dated and
-- attributed like every other.
--
-- Distinct from `note.active`, which says "no longer stands". A thing
-- can be archived and still true; a note can be retracted and still
-- visible. Two decisions, and the schema keeps them apart. The note
-- table's own comment has referred to "the entity-level `archived`"
-- since #268; this is the column it was talking about.
--
-- option<> because every row written before today has no value, and
-- append-only forbids backfilling them. Absent reads as not archived.
DEFINE FIELD IF NOT EXISTS archived   ON TABLE entity_state TYPE option<bool>;
-- Authorship is not only provenance — it is the subject of
-- authorization. "A role may not edit another role's mandate" is
-- enforceable precisely because every write records who wrote it.
-- `via_uid` is the role context, which yields "agent X, acting as role
-- Y". Plain uuid strings, never record<> FKs: an author may live in
-- another database entirely.
--
-- Every other versioned table in this file already carries these three.
-- `entity_state` did not, so who renamed a thing, who changed its
-- attributes, and who archived it were all unanswerable.
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_state TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_entity_state_chain ON TABLE entity_state FIELDS entity, valid_from;

-- ---------------------------------------------------------------- edges --
-- Native graph relation (D19): the engine stores record pointers, so
-- traversal (->edge->entity) costs a node's degree, never the table
-- size. ENFORCED: edges to non-existent anchors are refused. Link =
-- RELATE with a fresh uuid7 edge_uid; unlink = a NEW row, same
-- edge_uid, active = false; current edge state = latest per edge_uid.
DEFINE TABLE IF NOT EXISTS edge SCHEMAFULL TYPE RELATION IN entity OUT entity ENFORCED
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS edge_uid   ON TABLE edge TYPE string;
DEFINE FIELD IF NOT EXISTS rel_type   ON TABLE edge TYPE string;
DEFINE FIELD IF NOT EXISTS active     ON TABLE edge TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE edge TYPE datetime;
-- THE MOST SECURITY-RELEVANT COLUMNS IN THE SYSTEM, and until now the
-- only versioned table without them.
--
-- Granting reach IS an edge: a role linked to a grant linked to a
-- target. So "who linked this role to this grant" is not metadata, it
-- is the authorization record itself — and an unlink is equally a
-- decision somebody made. Without these, a role acquiring reach is a
-- fact with no author, which is the one shape an audit cannot work
-- with.
--
-- Same contract as everywhere else in this file: plain uuid strings,
-- `via_uid` for the role context, option<> because rows written before
-- today have no value and append-only forbids inventing one.
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE edge TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_edge_chain ON TABLE edge FIELDS edge_uid, valid_from;

-- ----------------------------------------------------------- dictionary --
-- The dictionary (issue #266). Types say what a thing IS; labels say what
-- a TERM MEANS. A fact without a dimension is uninterpretable, which is
-- why an edge needs a label for the same reason an entity needs a type.
--
-- One vocabulary, two kinds:
--   * slot labels — what an entity carries  (description, spec, mandate)
--   * link labels — how entities connect    (contains, depends_on, granted)
--
-- Versioned from the first row, unlike entity_type: the chain is
-- (key, label_kind) and current is the latest valid_from, so changing what
-- a label MEANS is a diffable, attributed event rather than a silent edit.
-- There is deliberately NO unique index on key — a unique index would make
-- rows create-once and forbid the version chain.
DEFINE TABLE IF NOT EXISTS label SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS key             ON TABLE label TYPE string;
DEFINE FIELD IF NOT EXISTS label_kind      ON TABLE label TYPE string
    ASSERT $value INSIDE ['slot', 'link'];
DEFINE FIELD IF NOT EXISTS display         ON TABLE label TYPE string;
-- How a reader must TREAT it. Closed vocabularies, because code acts on
-- this and free text cannot be acted on reliably.
--   slot: binding | directive | context | guidance | dialogue | data | secret
--   link: composition | ordering | sequence | reach | reference | governance
DEFINE FIELD IF NOT EXISTS semantics       ON TABLE label TYPE string;
DEFINE FIELD IF NOT EXISTS description     ON TABLE label TYPE option<string>;
-- Prose for the model, injected when the field is presented.
DEFINE FIELD IF NOT EXISTS agent_note      ON TABLE label TYPE option<string>;
-- Slot labels only. `value_kind` decides STORAGE: prose kinds live as note
-- chains keyed by the label; value kinds live in the attributes bag.
DEFINE FIELD IF NOT EXISTS value_kind      ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS cardinality     ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS allowed_sources ON TABLE label TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS writable_by     ON TABLE label TYPE option<string>;
-- Link labels only. Endpoint types are what make a nonsense edge
-- impossible: without them anything links to anything.
DEFINE FIELD IF NOT EXISTS source_types    ON TABLE label TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS target_types    ON TABLE label TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS inverse         ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS acyclic         ON TABLE label TYPE option<bool>;
-- Extensible like every registry row: you cannot enumerate in advance what
-- a future label needs.
DEFINE FIELD IF NOT EXISTS attributes      ON TABLE label TYPE option<object> FLEXIBLE;
-- Hidden from default views, still real and still readable. Nothing is
-- ever deleted, so a superseded label must be hideable or the dictionary
-- becomes a graveyard.
DEFINE FIELD IF NOT EXISTS archived        ON TABLE label TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from      ON TABLE label TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind     ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid      ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid         ON TABLE label TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_label_chain ON TABLE label FIELDS key, label_kind, valid_from;

-- Which types carry which slot labels, and in what order. `active = false`
-- retires the binding without erasing that it existed.
DEFINE TABLE IF NOT EXISTS type_label SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity_type       ON TABLE type_label TYPE string;
DEFINE FIELD IF NOT EXISTS label             ON TABLE type_label TYPE string;
DEFINE FIELD IF NOT EXISTS required          ON TABLE type_label TYPE bool;
DEFINE FIELD IF NOT EXISTS display_order     ON TABLE type_label TYPE int;
-- `description` is context on a product and directive on a task: the same
-- label treated differently by what carries it.
DEFINE FIELD IF NOT EXISTS semantics_override ON TABLE type_label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS active            ON TABLE type_label TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from        ON TABLE type_label TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind       ON TABLE type_label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid        ON TABLE type_label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid           ON TABLE type_label TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_type_label_chain
    ON TABLE type_label FIELDS entity_type, label, valid_from;

-- The dictionary's cache key. Per-row valid_from cannot answer "has
-- anything changed?", so every dictionary write appends a revision here;
-- a reader caches by it and revalidates with one cheap read. A run pins
-- the revision it interpreted under, so a semantics change between a run
-- and its audit is visible rather than mysterious.
DEFINE TABLE IF NOT EXISTS dictionary SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS revision   ON TABLE dictionary TYPE int;
DEFINE FIELD IF NOT EXISTS reason     ON TABLE dictionary TYPE option<string>;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE dictionary TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_dictionary_rev ON TABLE dictionary FIELDS valid_from;

-- Registry rows get the model's discipline: extensible, archivable,
-- attributed. Additive and option<> throughout — rows written before
-- these fields genuinely do not have them, and pretending otherwise
-- would be a lie in the data.
DEFINE FIELD IF NOT EXISTS attributes  ON TABLE entity_type TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS archived    ON TABLE entity_type TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_type TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_type TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_type TYPE option<string>;

-- ---------------------------------------------------------------- notes --
-- Prose (issue #268). A description, a spec, a mandate, a playbook and a
-- comment are the SAME mechanism with a different LABEL — the dictionary
-- entry (#266) decides how each is treated, so a new kind of attached prose
-- is a label, not a migration. One level down from types-as-data.
--
-- Prose does not belong in `entity_state`: SCD-2 copies the whole row, so a
-- 35KB description would be rewritten on every unrelated attribute tweak.
-- And it does not belong in the graph as an entity, which is what it is
-- today: a product's description is currently a node in its own product
-- graph, and every reader has to know that some nodes are not real nodes.
--
-- Append-only, one chain per `uid`; current is the latest `(valid_from, id)`.
-- `active = false` retracts a note without erasing that it was said.
DEFINE TABLE IF NOT EXISTS note SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

-- The note's identity ACROSS versions — the chain key. The row id changes
-- every version; this does not.
DEFINE FIELD IF NOT EXISTS uid         ON TABLE note TYPE string;
-- What it is attached to. A typed record link, so a note pointing at an
-- entity that does not exist is refused by the engine rather than found
-- later (§12).
DEFINE FIELD IF NOT EXISTS entity      ON TABLE note TYPE record<entity>;
-- The dictionary slot label: description | spec | mandate | playbook |
-- comments | … . This is the whole difference between a paragraph for a
-- human and the contract the work is judged by.
DEFINE FIELD IF NOT EXISTS label       ON TABLE note TYPE string;
DEFINE FIELD IF NOT EXISTS body        ON TABLE note TYPE string;
-- Threading: a comment on a comment. Edges have no fields, which is why
-- this could not exist while prose was an entity joined by an edge.
DEFINE FIELD IF NOT EXISTS parent_uid  ON TABLE note TYPE option<string>;
-- Retracted, not erased. Distinct from the entity-level `archived`: this
-- says "no longer stands", not "hide it from the list".
DEFINE FIELD IF NOT EXISTS active      ON TABLE note TYPE bool;
DEFINE FIELD IF NOT EXISTS attributes  ON TABLE note TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE note TYPE datetime;
-- Authorship is not only provenance — it is the subject of authorization.
-- "A role may not edit another role's mandate" is enforceable precisely
-- because every write records who wrote it. `via_uid` is the role context,
-- which yields "agent X, acting as role Y". Plain uuid strings, never
-- record<> FKs: an author may live in another database entirely.
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE note TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE note TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE note TYPE option<string>;

-- The version chain of one note.
DEFINE INDEX IF NOT EXISTS idx_note_chain  ON TABLE note FIELDS uid, valid_from;
-- Every note on one entity, which is the read a detail page makes.
DEFINE INDEX IF NOT EXISTS idx_note_entity ON TABLE note FIELDS entity, valid_from;

-- ------------------------------------------------- content on anything --
-- Notes and attachments are ONE IDEA IN TWO SHAPES (issue #296, spec §3):
-- content that belongs to something and is never a node. One is inline
-- text, the other a file; identical discipline — chained, versioned,
-- authored, retractable.
--
-- `target_kind` is 'entity', 'type' or 'label', because a TYPE is exactly
-- the thing people argue about and needs a thread of its own. The cost,
-- stated rather than discovered: a polymorphic target cannot be a typed
-- `record<>` link, so the engine's referential-integrity check does not
-- cover that column and the verbs carry it instead.
DEFINE FIELD IF NOT EXISTS target_kind ON TABLE note TYPE option<string>
    ASSERT $value = NONE OR $value INSIDE ['entity', 'type', 'label'];
DEFINE FIELD IF NOT EXISTS target_uid  ON TABLE note TYPE option<string>;

-- `entity` was REQUIRED, which is what stopped a note attaching to a type
-- or a label at all. It is relaxed here, and OVERWRITE is deliberate: this
-- file is re-applied verbatim on every provision, and `IF NOT EXISTS` on
-- an existing field is ACCEPTED AND DOES NOT AMEND — probed on this
-- engine, not assumed. Left as `option<record<entity>>` rather than
-- dropped, so every row written before this keeps its typed link and old
-- readers keep working.
DEFINE FIELD OVERWRITE entity ON TABLE note TYPE option<record<entity>>;

DEFINE INDEX IF NOT EXISTS idx_note_target ON TABLE note FIELDS target_uid, valid_from;

-- Files, with the same discipline. The bytes live under the module's own
-- directory; this records where and what it MEANS.
DEFINE TABLE IF NOT EXISTS attachment SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid         ON TABLE attachment TYPE string;
DEFINE FIELD IF NOT EXISTS target_kind ON TABLE attachment TYPE string
    ASSERT $value INSIDE ['entity', 'type', 'label'];
DEFINE FIELD IF NOT EXISTS target_uid  ON TABLE attachment TYPE string;
-- The label is what makes a PDF *the mandate* rather than a file next to
-- one: source is separate from meaning (§5.4).
DEFINE FIELD IF NOT EXISTS label       ON TABLE attachment TYPE string;
DEFINE FIELD IF NOT EXISTS filename    ON TABLE attachment TYPE string;
DEFINE FIELD IF NOT EXISTS mime        ON TABLE attachment TYPE string;
DEFINE FIELD IF NOT EXISTS size        ON TABLE attachment TYPE int;
-- Relative to the module's own directory, never absolute: an instance
-- that moves must not take a dead path with it.
DEFINE FIELD IF NOT EXISTS path        ON TABLE attachment TYPE string;
DEFINE FIELD IF NOT EXISTS active      ON TABLE attachment TYPE bool;
DEFINE FIELD IF NOT EXISTS attributes  ON TABLE attachment TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE attachment TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE attachment TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE attachment TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE attachment TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_attachment_chain  ON TABLE attachment FIELDS uid, valid_from;
DEFINE INDEX IF NOT EXISTS idx_attachment_target ON TABLE attachment FIELDS target_uid, valid_from;
