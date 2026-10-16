-- =========================================================================
-- superx-mod-specs — the specs module's OWN schema (#446)
-- =========================================================================
--
-- THIS FILE IS THE SCHEMA (§11): the model changes here first, the
-- operator applies it, code follows. Applied into the module's own
-- database `superx/specs` by `superx modules provision specs`. The
-- KERNEL schema is untouched (§7). Operator-approved 2026-10-13.
--
--
-- WHAT THIS DATABASE HOLDS -------------------------------------------
--
-- What an agent session was told and what it did, both read from the
-- sessions the kernel already captured. Nothing here is read from a
-- repository, and nothing here is judged at write time beyond what one
-- check says about one run: whether a build failed because its spec was
-- wrong or because the agent did not follow it is computed when it is
-- read, from these facts.
--
--   spec_doc       one row per distinct spec file, as a session read it
--   spec_clause    one row per checkable part of a spec file
--   spec_run       one row per build: a session opened with a kickoff
--                  line naming a task file
--   spec_finding   one row per check of a run against a clause
--   spec_revision  one row per spec version a commit in a session named
--   branch_event   one row per branch a session's git created, pushed,
--                  merged, reverted the merge of, or deleted
--
-- EVERY ROW IS DERIVED. Each one is computed from the kernel's captured
-- sessions, so provisioning over an existing database loses nothing an
-- ingest cannot rebuild.
--
--
-- HOW CHANGE WORKS ---------------------------------------------------
--
-- Nothing is overwritten and nothing is removed (§10). A spec file is
-- identified by the hash of its content, so a second read of the same
-- bytes finds the row the first read made. A run whose session has
-- grown appends a new row for the same session; the newest `valid_from`
-- is the run.
--
--
-- WHAT THE ENGINE ENFORCES, and the code never relaxes (§12) ---------
--
--   * `record<…>` is a TYPE check, not a foreign key — hence the ASSERTs
--     on every link inside this database.
--   * `spec_run.session` is the KERNEL's session id. It lives in the
--     kernel's database, so this database can only check its type.
--   * `spec_doc.sha256` is UNIQUE: one row per content, held by the
--     engine, not by care.
--   * `FOR update NONE / FOR delete NONE` does not bind a system user;
--     append-only is held by the code, which never emits UPDATE or
--     DELETE (§10).

-- THE WHOLE DATABASE GOES FIRST. Provisioning leaves the database in
-- exactly this shape; nothing that was there survives. Safe to re-run,
-- and destructive every time — and lossless, because every row is
-- derived. Verified by `provisioning_leaves_the_database_exactly_as_this
-- _file_describes`, which diffs the engine's own schema against this file.

REMOVE DATABASE IF EXISTS specs;
DEFINE DATABASE specs;

DEFINE USER IF NOT EXISTS superx_mod_specs ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;


-- 1 ─── spec_doc ───────────────────────── a spec file, as it was read ───
-- The content is the bytes the Read tool returned, from a whole read.
-- `kind` is the file's document element. A task file carries its key,
-- its name and its file version; root and rules carry their version.

DEFINE TABLE spec_doc SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD sha256        ON TABLE spec_doc TYPE string;
DEFINE FIELD kind          ON TABLE spec_doc TYPE string
    ASSERT $value INSIDE ['root', 'rules', 'task'];
DEFINE FIELD key           ON TABLE spec_doc TYPE option<string>;
DEFINE FIELD name          ON TABLE spec_doc TYPE option<string>;
DEFINE FIELD version       ON TABLE spec_doc TYPE option<int>;
DEFINE FIELD path          ON TABLE spec_doc TYPE string;
DEFINE FIELD content       ON TABLE spec_doc TYPE string;
DEFINE FIELD first_read_at ON TABLE spec_doc TYPE datetime;
DEFINE FIELD valid_from    ON TABLE spec_doc TYPE datetime;

DEFINE INDEX idx_doc_sha ON TABLE spec_doc FIELDS sha256 UNIQUE;


-- 2 ─── spec_clause ─────────────────── a checkable part of a spec file ───
-- One per method step of a root, rule of a rules file, and reads file,
-- owns, forbid, agent-decides and gate of a task file, in document order.
-- `ident` is the clause's own name where the file gives one: a rule's
-- id, a gate's id, a method step's element name, a read file's path.

DEFINE TABLE spec_clause SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD doc        ON TABLE spec_clause TYPE record<spec_doc>
    ASSERT record::exists($value);
DEFINE FIELD kind       ON TABLE spec_clause TYPE string;
DEFINE FIELD ident      ON TABLE spec_clause TYPE option<string>;
DEFINE FIELD position   ON TABLE spec_clause TYPE int;
DEFINE FIELD text       ON TABLE spec_clause TYPE string;
DEFINE FIELD valid_from ON TABLE spec_clause TYPE datetime;

DEFINE INDEX idx_clause_doc ON TABLE spec_clause FIELDS doc, position;


-- 3 ─── spec_run ───────────────────────────────────────── one build ───
-- A session whose first human message is a kickoff line naming a task
-- file. `reads` are the paths the session read, in order, before its
-- first write (`first_write`); `label` is what the session's build
-- branch says after `<key>-<name>-v<version>-`; `model` is the model
-- that wrote most of the session's replies.

DEFINE TABLE spec_run SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session     ON TABLE spec_run TYPE record<entity>;
DEFINE FIELD session_key ON TABLE spec_run TYPE string;
DEFINE FIELD kickoff     ON TABLE spec_run TYPE string;
DEFINE FIELD task_path   ON TABLE spec_run TYPE string;
DEFINE FIELD task_doc    ON TABLE spec_run TYPE option<record<spec_doc>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD root_doc    ON TABLE spec_run TYPE option<record<spec_doc>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD rules_doc   ON TABLE spec_run TYPE option<record<spec_doc>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD key         ON TABLE spec_run TYPE option<string>;
DEFINE FIELD version     ON TABLE spec_run TYPE option<int>;
DEFINE FIELD branch      ON TABLE spec_run TYPE option<string>;
DEFINE FIELD label       ON TABLE spec_run TYPE option<string>;
DEFINE FIELD model       ON TABLE spec_run TYPE option<string>;
DEFINE FIELD reads       ON TABLE spec_run TYPE array<string>;
DEFINE FIELD first_write ON TABLE spec_run TYPE option<string>;
DEFINE FIELD started_at  ON TABLE spec_run TYPE datetime;
DEFINE FIELD valid_from  ON TABLE spec_run TYPE datetime;

DEFINE INDEX idx_run_session ON TABLE spec_run FIELDS session, valid_from;


-- 4 ─── spec_finding ───────────────────── one check of a run, one clause ───
-- `check` names what was checked; `clause` is the spec text it was
-- checked against, absent when the run never read the file that holds
-- it. `evidence` are the tool call ids the result rests on.

DEFINE TABLE spec_finding SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD run        ON TABLE spec_finding TYPE record<spec_run>
    ASSERT record::exists($value);
DEFINE FIELD clause     ON TABLE spec_finding TYPE option<record<spec_clause>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD check      ON TABLE spec_finding TYPE string;
DEFINE FIELD result     ON TABLE spec_finding TYPE string
    ASSERT $value INSIDE ['complied', 'breached', 'no-evidence'];
DEFINE FIELD note       ON TABLE spec_finding TYPE string;
DEFINE FIELD evidence   ON TABLE spec_finding TYPE array<string>;
DEFINE FIELD valid_from ON TABLE spec_finding TYPE datetime;

DEFINE INDEX idx_finding_run ON TABLE spec_finding FIELDS run;


-- 5 ─── spec_revision ───────────────── a spec version, as a session committed it ───
-- A successful `git commit` in any session whose message names
-- `<KEY> file version <N>` — how the design sessions commit a spec
-- file's new version. `reviewed` is the build whose review it came from
-- (`… of its v5 build`); `line` the message line naming it; `commit` the
-- hash the session printed for it, when it printed one. `model` is the
-- model that made the call; `call` its tool call id.

DEFINE TABLE spec_revision SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session     ON TABLE spec_revision TYPE record<entity>;
DEFINE FIELD session_key ON TABLE spec_revision TYPE string;
DEFINE FIELD model       ON TABLE spec_revision TYPE option<string>;
DEFINE FIELD key         ON TABLE spec_revision TYPE string;
DEFINE FIELD version     ON TABLE spec_revision TYPE int;
DEFINE FIELD reviewed    ON TABLE spec_revision TYPE option<int>;
DEFINE FIELD line        ON TABLE spec_revision TYPE string;
DEFINE FIELD message     ON TABLE spec_revision TYPE string;
DEFINE FIELD commit      ON TABLE spec_revision TYPE option<string>;
DEFINE FIELD at          ON TABLE spec_revision TYPE datetime;
DEFINE FIELD call        ON TABLE spec_revision TYPE string;
DEFINE FIELD valid_from  ON TABLE spec_revision TYPE datetime;

DEFINE INDEX idx_revision_call ON TABLE spec_revision FIELDS call, key, version UNIQUE;
DEFINE INDEX idx_revision_key  ON TABLE spec_revision FIELDS key, version;


-- 6 ─── branch_event ─────────────────── what a session's git did to a branch ───
-- Read from the text of the session's shell calls: what the command
-- says, never inferred. `reverted` is a `git revert` of a commit whose
-- printed subject is `Merge branch '<branch>'` (operator-approved
-- 2026-10-16). (No `committed`: a line's `gitBranch` is the
-- branch of the directory the session was launched in, not of the
-- worktree a call may have changed into.) `ok` is whether the call succeeded,
-- absent when its result was not captured. A branch's fate is its
-- newest event; one no session touched is unknown.

DEFINE TABLE branch_event SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session     ON TABLE branch_event TYPE record<entity>;
DEFINE FIELD session_key ON TABLE branch_event TYPE string;
DEFINE FIELD model       ON TABLE branch_event TYPE option<string>;
DEFINE FIELD branch      ON TABLE branch_event TYPE string;
DEFINE FIELD event       ON TABLE branch_event TYPE string
    ASSERT $value INSIDE ['created', 'pushed', 'merged', 'reverted', 'deleted', 'deleted-remote'];
DEFINE FIELD command     ON TABLE branch_event TYPE string;
DEFINE FIELD ok          ON TABLE branch_event TYPE option<bool>;
DEFINE FIELD at          ON TABLE branch_event TYPE datetime;
DEFINE FIELD call        ON TABLE branch_event TYPE string;
DEFINE FIELD valid_from  ON TABLE branch_event TYPE datetime;

DEFINE INDEX idx_branch_call ON TABLE branch_event FIELDS call, branch, event UNIQUE;
DEFINE INDEX idx_branch_name ON TABLE branch_event FIELDS branch, at;
