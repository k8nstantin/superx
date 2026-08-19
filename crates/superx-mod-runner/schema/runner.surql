-- =========================================================================
-- superx-mod-runner — the scheduler's OWN schema (epic #189, S1 verbatim)
-- =========================================================================
-- Applied into superx/runner by `superx modules provision runner` under
-- the operator's one-shot root session. Kernel schema untouched (§7).
-- IF NOT EXISTS everywhere: provisioning re-applies this file verbatim.
--
-- Contract (D23): two tables, append-only SCD-2 chains, uuid7 record
-- ids AND chain ids, current = ORDER BY valid_from DESC LIMIT 1.
-- The schedule row is DUMB (§8/§9): time + entity ref + recurrence.
-- What the work is and what order it runs in live in the entity graph.

DEFINE USER IF NOT EXISTS superx_mod_runner ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;

-- The INTENT: "at run_at, kick entity". Nothing else.
DEFINE TABLE IF NOT EXISTS schedule SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid        ON TABLE schedule TYPE string;
DEFINE FIELD IF NOT EXISTS entity     ON TABLE schedule TYPE string;
DEFINE FIELD IF NOT EXISTS run_at     ON TABLE schedule TYPE datetime;
DEFINE FIELD IF NOT EXISTS recurrence ON TABLE schedule TYPE string;
DEFINE FIELD IF NOT EXISTS status     ON TABLE schedule TYPE string
    ASSERT $value INSIDE ['scheduled', 'fired', 'cancelled'];
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE schedule TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_schedule_chain ON TABLE schedule FIELDS uid, valid_from;
DEFINE INDEX IF NOT EXISTS idx_schedule_due   ON TABLE schedule FIELDS run_at, valid_from;

-- The FACTS: one chain per (firing, task); statuses are appended rows.
DEFINE TABLE IF NOT EXISTS run SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid              ON TABLE run TYPE string;
DEFINE FIELD IF NOT EXISTS schedule         ON TABLE run TYPE string;
DEFINE FIELD IF NOT EXISTS firing           ON TABLE run TYPE string;
DEFINE FIELD IF NOT EXISTS task             ON TABLE run TYPE string;
DEFINE FIELD IF NOT EXISTS status           ON TABLE run TYPE string
    ASSERT $value INSIDE ['dispatched', 'done', 'failed', 'cancelled'];
DEFINE FIELD IF NOT EXISTS instruct_version ON TABLE run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS output_ref       ON TABLE run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS detail           ON TABLE run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS valid_from       ON TABLE run TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_run_chain    ON TABLE run FIELDS uid, valid_from;
DEFINE INDEX IF NOT EXISTS idx_run_schedule ON TABLE run FIELDS schedule, valid_from;
