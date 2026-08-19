-- =========================================================================
-- superx-mod-ui — the UI module's OWN schema (superx/ui)
-- =========================================================================
-- Applied by `superx modules provision ui` under the operator path.
-- The kernel schema is untouched (§7).

DEFINE USER IF NOT EXISTS superx_mod_ui ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;

-- Console command history (append-only).
DEFINE TABLE command_history SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD argv       ON TABLE command_history TYPE string;
DEFINE FIELD is_error   ON TABLE command_history TYPE bool;
DEFINE FIELD valid_from ON TABLE command_history TYPE datetime;

-- Saved views / preferences (SCD-2 by name; latest wins).
DEFINE TABLE preference SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD name       ON TABLE preference TYPE string;
DEFINE FIELD value      ON TABLE preference TYPE object FLEXIBLE;
DEFINE FIELD valid_from ON TABLE preference TYPE datetime;
