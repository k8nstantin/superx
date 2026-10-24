-- =========================================================================
-- superx-mod-repos — the repository crawler's OWN schema (#466)
-- =========================================================================
--
-- THIS FILE IS THE SCHEMA (§11): the model changes here first, the
-- operator applies it, code follows. Applied into the module's own
-- database `superx/repos` by `superx-repos --initialize`. The
-- KERNEL schema is untouched (§7). Operator-approved 2026-10-21 (#466);
-- the crawl's telemetry, every git call, ref history, uncommitted work,
-- survival by file, how a commit changed a file and its whole message
-- operator-approved 2026-10-24 (#501).
--
--
-- WHAT THIS DATABASE HOLDS -------------------------------------------
--
-- What the crawler read from the git repositories the agents work in.
-- It finds them from the working directories in the sessions the kernel
-- captured, and from the roots the operator names (`attr_repos_include`).
-- It reads them with git, read-only: never a fetch, a write, a checkout,
-- or any program the repository names (`--no-textconv`, no optional
-- locks). Nothing here is judged. Whether work is rework, thrash or a
-- bulk drop is decided by whoever reads these facts.
--
--   repository      one row per repository, ever
--   checkout        one row per working tree of a repository, ever
--   checkout_state  one row per change in a checkout's branch or presence
--   directory       one row per change in which checkout holds a directory
--   head_move       one row per move of a checkout's HEAD, from its reflog
--   commit          one row per commit read, ever
--   commit_file     one row per file a commit changed, ever
--   placement       one row per change in where a commit sits
--   landed_via      one row per file an off-main-line commit landed through
--                   another commit (a squash or a replay)
--   survival        one row per change in the lines blame still credits to
--                   a commit at its main line's tip
--   survival_file   the same, per file at the tip
--   crawl           one row per crawl of one repository: what it scanned,
--                   what changed since the last one, what it cost
--   crawl_step      one row per step of a crawl
--   git_call        one row per git call a crawl made
--   ref_state       one row per change in a branch's or tag's tip
--   worktree_file   one row per change in a checkout's uncommitted file
--   sweep           one row per pass over the list
--
-- EVERY ROW IS DERIVED, from git and from the kernel's captured sessions,
-- with one exception: reflog entries and unreachable commits that git has
-- since expired or pruned are kept only here (`head_move`, and the
-- commits whose placement is `recovered`). Provisioning loses those.
--
--
-- HOW CHANGE WORKS ---------------------------------------------------
--
-- Nothing is overwritten and nothing is removed (§10). A repository, a
-- checkout and a commit each have one row for good; what changes about
-- them appends a row to a state table, and the newest `valid_from` is the
-- fact. A crawl appends a placement or survival row only when it differs
-- from the newest one. Row ids are UUIDv7, set by the module's code
-- (§11), never by the engine.
--
--
-- WHAT THE ENGINE ENFORCES, and the code never relaxes (§12) ---------
--
--   * Every link inside this database is a typed `record<…>` with an
--     ASSERT that the record exists.
--   * One row per repository (`common_dir`), checkout (`toplevel`),
--     commit (`repository, hash`), file of a commit (`commit, path`),
--     HEAD move (`checkout, at, branch`) and landing (`commit, path, via`):
--     UNIQUE indexes, held by the engine, not by care.
--   * Closed vocabularies are ASSERTed: how a main line was chosen, where
--     a commit sits, where a directory came from, how a crawl or a pass
--     ended, how a commit changed a file, a crawl's steps, a file's
--     uncommitted state.
--   * Counts are never negative; a pass's crawls all exist.
--   * `FOR update NONE / FOR delete NONE` does not bind a system user;
--     append-only is held by the code, which never emits UPDATE or
--     DELETE (§10).

-- THE WHOLE DATABASE GOES FIRST. Provisioning leaves the database in
-- exactly this shape; nothing that was there survives. Verified by a test
-- that diffs the engine's own schema against this file.

REMOVE DATABASE IF EXISTS repos;
DEFINE DATABASE repos;

DEFINE USER IF NOT EXISTS superx_mod_repos ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;


-- 1 ─── repository ─────────────────────── a repository the crawler found ───
-- Identified by its common git directory, so every worktree of one
-- repository is the same repository. `name` is the directory that holds
-- that `.git`.

DEFINE TABLE repository SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD common_dir ON TABLE repository TYPE string;
DEFINE FIELD name       ON TABLE repository TYPE string;
DEFINE FIELD found_at   ON TABLE repository TYPE datetime;
DEFINE FIELD valid_from ON TABLE repository TYPE datetime;

DEFINE INDEX idx_repository_dir ON TABLE repository FIELDS common_dir UNIQUE;


-- 2 ─── checkout ─────────────────────── one working tree of a repository ───
-- The repository's main working tree, or one of its worktrees, by its
-- own root.

DEFINE TABLE checkout SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository ON TABLE checkout TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD toplevel   ON TABLE checkout TYPE string;
DEFINE FIELD found_at   ON TABLE checkout TYPE datetime;
DEFINE FIELD valid_from ON TABLE checkout TYPE datetime;

DEFINE INDEX idx_checkout_top  ON TABLE checkout FIELDS toplevel UNIQUE;
DEFINE INDEX idx_checkout_repo ON TABLE checkout FIELDS repository;


-- 3 ─── checkout_state ─────────────────── a checkout's branch, as last read ───
-- `branch` is the branch HEAD is on now, NONE when it is detached.
-- `first_branch` is the branch before the oldest move its reflog still
-- holds: what the checkout was on before any recorded move. `present` is
-- whether its root still exists.

DEFINE TABLE checkout_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD checkout     ON TABLE checkout_state TYPE record<checkout>
    ASSERT record::exists($value);
DEFINE FIELD branch       ON TABLE checkout_state TYPE option<string>;
DEFINE FIELD first_branch ON TABLE checkout_state TYPE option<string>;
DEFINE FIELD present      ON TABLE checkout_state TYPE bool;
DEFINE FIELD valid_from   ON TABLE checkout_state TYPE datetime;

DEFINE INDEX idx_checkout_state ON TABLE checkout_state FIELDS checkout, valid_from;


-- 4 ─── directory ─────────────────── which checkout holds a directory ───
-- A working directory a session worked in (`source = 'session'`), or a
-- root the operator named (`'include'`). `checkout` is NONE when no
-- repository holds it. `exact` is false when the directory itself is gone
-- and only an ancestor was found: its repository is known, its branch is
-- not, because the ancestor's checkout may not be the one it belonged to.

DEFINE TABLE directory SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD path       ON TABLE directory TYPE string;
DEFINE FIELD source     ON TABLE directory TYPE string
    ASSERT $value INSIDE ['session', 'include'];
DEFINE FIELD checkout   ON TABLE directory TYPE option<record<checkout>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD exact      ON TABLE directory TYPE bool;
DEFINE FIELD valid_from ON TABLE directory TYPE datetime;

DEFINE INDEX idx_directory_path ON TABLE directory FIELDS path, valid_from;


-- 5 ─── head_move ─────────────────── one move of a checkout's HEAD ───
-- From the checkout's HEAD reflog: a checkout, a branch rename, a rebase
-- leaving or returning. `branch` is where HEAD landed, NONE when it was
-- left detached. Kept after git expires the reflog entry.

DEFINE TABLE head_move SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD checkout   ON TABLE head_move TYPE record<checkout>
    ASSERT record::exists($value);
DEFINE FIELD at         ON TABLE head_move TYPE datetime;
DEFINE FIELD branch     ON TABLE head_move TYPE option<string>;
DEFINE FIELD valid_from ON TABLE head_move TYPE datetime;

DEFINE INDEX idx_head_move ON TABLE head_move FIELDS checkout, at, branch UNIQUE;


-- 6 ─── commit ──────────────────────────────── one commit, as git has it ───
-- `authored_at` is when the work was done: author time, which a rebase
-- keeps and a squash's branch commits still carry. `committed_at` is when
-- it was committed. `author` is the author's email; `committer` is who
-- made the commit, which differs after a rebase or a squash on GitHub.
-- `subject` is the first line of the message, as written; `body` is the
-- rest, whole, trailers included (`Co-Authored-By: …`), NONE when there is
-- none. `parents` above one is a merge; a merge's lines are counted
-- against its first parent. `added` and `removed` sum its files.

DEFINE TABLE commit SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository   ON TABLE commit TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD hash         ON TABLE commit TYPE string
    ASSERT string::len($value) INSIDE [40, 64];
DEFINE FIELD authored_at  ON TABLE commit TYPE datetime;
DEFINE FIELD committed_at ON TABLE commit TYPE datetime;
DEFINE FIELD author       ON TABLE commit TYPE string;
DEFINE FIELD committer    ON TABLE commit TYPE string;
DEFINE FIELD subject      ON TABLE commit TYPE string;
DEFINE FIELD body         ON TABLE commit TYPE option<string>;
DEFINE FIELD parents      ON TABLE commit TYPE int;
DEFINE FIELD added        ON TABLE commit TYPE int;
DEFINE FIELD removed      ON TABLE commit TYPE int;
DEFINE FIELD valid_from   ON TABLE commit TYPE datetime;

DEFINE INDEX idx_commit_hash   ON TABLE commit FIELDS repository, hash UNIQUE;
DEFINE INDEX idx_commit_author ON TABLE commit FIELDS repository, authored_at;


-- 7 ─── commit_file ─────────────────────── one file a commit changed ───
-- Renames read as a delete and an add, so every path is one that exists
-- in some tree. `change` is how the commit changed the file: it created
-- it, modified it, deleted it, or changed its type (a file became a link,
-- or back). `binary` is a file git counted no lines in (a binary, or
-- anything `.gitattributes` marks `-diff`); its counts are zero.

DEFINE TABLE commit_file SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE commit_file TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE commit_file TYPE string;
DEFINE FIELD change     ON TABLE commit_file TYPE string
    ASSERT $value INSIDE ['added', 'modified', 'deleted', 'type_changed'];
DEFINE FIELD added      ON TABLE commit_file TYPE int;
DEFINE FIELD removed    ON TABLE commit_file TYPE int;
DEFINE FIELD binary     ON TABLE commit_file TYPE bool;
DEFINE FIELD valid_from ON TABLE commit_file TYPE datetime;

DEFINE INDEX idx_commit_file ON TABLE commit_file FIELDS commit, path UNIQUE;
DEFINE INDEX idx_file_path   ON TABLE commit_file FIELDS path;


-- 8 ─── placement ─────────────────────────── where a commit sits ───
-- `mainline`: its repository's main line contains it. `branch`: a branch
-- the main line does not contain holds it. `in_flight`: such a branch is
-- checked out right now, so the work is still going on. `recovered`: no
-- branch holds it; it was read from a reflog or from unreachable objects,
-- and is kept only for what of it landed. `first_parent` marks the main
-- line's first-parent history: how work landed on it, directly or as a
-- merge.

DEFINE TABLE placement SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit       ON TABLE placement TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD crawl        ON TABLE placement TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD place        ON TABLE placement TYPE string
    ASSERT $value INSIDE ['mainline', 'branch', 'in_flight', 'recovered'];
DEFINE FIELD first_parent ON TABLE placement TYPE bool;
DEFINE FIELD valid_from   ON TABLE placement TYPE datetime;

DEFINE INDEX idx_placement ON TABLE placement FIELDS commit, valid_from;


-- 9 ─── landed_via ──────── a file that landed through another commit ───
-- A commit off the main line wrote a version of `path` that the main line
-- carries; `via` is the main-line commit that wrote that version there,
-- by squash or by replay. That file's work landed, though the commit
-- itself never reached the main line.

DEFINE TABLE landed_via SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE landed_via TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE landed_via TYPE string;
DEFINE FIELD via        ON TABLE landed_via TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE landed_via TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD valid_from ON TABLE landed_via TYPE datetime;

DEFINE INDEX idx_landed_via     ON TABLE landed_via FIELDS commit, path, via UNIQUE;
DEFINE INDEX idx_landed_via_via ON TABLE landed_via FIELDS via;


-- 10 ── survival ─────────── what of a commit is still there ───
-- The lines `git blame` credits to the commit at its main line's tip,
-- never more than the commit added. Files git counts no lines in, and the
-- paths `attr_repos_survival_skip` names, are never blamed.

DEFINE TABLE survival SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE survival TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE survival TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD alive      ON TABLE survival TYPE int
    ASSERT $value >= 0;
DEFINE FIELD valid_from ON TABLE survival TYPE datetime;

DEFINE INDEX idx_survival ON TABLE survival FIELDS commit, valid_from;


-- 11 ── crawl ─────────────────────────── one crawl of one repository ───
-- `mainline` is the ref the crawl read as the main line and
-- `mainline_from` how it was chosen: the operator's override
-- (`attr_repos_mainline_refs`), what `origin/HEAD` points at, a local
-- `main` or `master`, or `HEAD`. `identity` is the email this machine
-- commits as there. `tip` is the main line's head as read, NONE when it
-- could not be. `since` is how far back it read, NONE for all history.
-- `files_left` counts files over the blame budget. `note` says, in git's
-- words, what could not be read.
--
-- What it scanned: the repository's checkouts, its refs, the file changes
-- of the commits read, HEAD reflog entries, unreachable commits, lines of
-- blame, and the bytes git returned. What changed since the previous
-- crawl: commits, their file changes and lines; refs that appeared, moved,
-- were rewritten (the new tip does not contain the old one: a rebase, an
-- amend, a force-push) or went; HEAD moves; placements, landings and
-- survival that changed, and the net change in surviving lines, negative
-- when later work rewrote or removed them. What it cost: git calls cut off
-- at `attr_repos_git_timeout_ms`, time spent waiting on git, rows written.
-- A crawl is written once its reads are done, before the rows that point
-- to it.

DEFINE TABLE crawl SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository    ON TABLE crawl TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD started_at    ON TABLE crawl TYPE datetime;
DEFINE FIELD finished_at   ON TABLE crawl TYPE datetime;
DEFINE FIELD mainline      ON TABLE crawl TYPE string;
DEFINE FIELD mainline_from ON TABLE crawl TYPE string
    ASSERT $value INSIDE ['override', 'origin_head', 'local_branch', 'head'];
DEFINE FIELD identity      ON TABLE crawl TYPE option<string>;
DEFINE FIELD tip           ON TABLE crawl TYPE option<string>;
DEFINE FIELD since         ON TABLE crawl TYPE option<datetime>;
DEFINE FIELD outcome       ON TABLE crawl TYPE string
    ASSERT $value INSIDE ['complete', 'partial', 'unreadable'];
DEFINE FIELD commits_read  ON TABLE crawl TYPE int;
DEFINE FIELD files_blamed  ON TABLE crawl TYPE int;
DEFINE FIELD files_left    ON TABLE crawl TYPE int;
DEFINE FIELD git_calls     ON TABLE crawl TYPE int;
DEFINE FIELD git_failed    ON TABLE crawl TYPE int;
DEFINE FIELD note          ON TABLE crawl TYPE option<string>;
DEFINE FIELD checkouts_read     ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD refs_read          ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD files_read         ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD reflog_read        ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD unreachable_read   ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD blame_lines_read   ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD bytes_read         ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD commits_new        ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD commit_files_new   ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD lines_added_new    ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD lines_removed_new  ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD refs_new           ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD refs_moved         ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD refs_gone          ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD refs_rewritten     ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD head_moves_new     ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD placements_changed ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD landed_new         ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD survival_changed   ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD survival_delta     ON TABLE crawl TYPE int;
DEFINE FIELD git_timeouts       ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD git_ms             ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD rows_written       ON TABLE crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from    ON TABLE crawl TYPE datetime;

DEFINE INDEX idx_crawl_repo ON TABLE crawl FIELDS repository, valid_from;


-- 12 ── survival_file ─── what of a commit is still there, file by file ───
-- The lines `git blame` credits to the commit in one file at its main
-- line's tip. `path` is the file at the tip that holds them, which can
-- differ from the file the commit wrote them in. A commit's counts add up
-- to its `survival.alive`. Appended when a count differs from the newest.

DEFINE TABLE survival_file SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE survival_file TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE survival_file TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE survival_file TYPE string;
DEFINE FIELD alive      ON TABLE survival_file TYPE int
    ASSERT $value >= 0;
DEFINE FIELD valid_from ON TABLE survival_file TYPE datetime;

DEFINE INDEX idx_survival_file ON TABLE survival_file FIELDS commit, path, valid_from;


-- 13 ── ref_state ───────────── a branch's or tag's tip, as last read ───
-- One row per change in a ref's tip: `name` is the full ref
-- (`refs/heads/…`, `refs/remotes/…`, `refs/tags/…`), `tip` the commit it
-- points at (a tag's commit), NONE once the ref is gone. `rewritten`: the
-- new tip does not contain the old one, so history was rewritten (a
-- rebase, an amend, a force-push). A branch thrown away stays on record.

DEFINE TABLE ref_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository ON TABLE ref_state TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE ref_state TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD name       ON TABLE ref_state TYPE string;
DEFINE FIELD tip        ON TABLE ref_state TYPE option<string>
    ASSERT $value == NONE OR string::len($value) INSIDE [40, 64];
DEFINE FIELD rewritten  ON TABLE ref_state TYPE bool;
DEFINE FIELD valid_from ON TABLE ref_state TYPE datetime;

DEFINE INDEX idx_ref_state ON TABLE ref_state FIELDS repository, name, valid_from;


-- 14 ── worktree_file ───────────────── a checkout's uncommitted file ───
-- What is changed in a checkout but not committed, file by file, as a
-- crawl found it. Git keeps no record of it: once committed or thrown
-- away, the work in progress is gone. `state` is how the file differs from
-- HEAD; `clean` closes a file that no longer does. `added` and `removed`
-- count lines against HEAD (an untracked file's are its lines). Read
-- without touching the checkout: no index refresh, no locks.

DEFINE TABLE worktree_file SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD checkout   ON TABLE worktree_file TYPE record<checkout>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE worktree_file TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE worktree_file TYPE string;
DEFINE FIELD from_path  ON TABLE worktree_file TYPE option<string>;
DEFINE FIELD state      ON TABLE worktree_file TYPE string
    ASSERT $value INSIDE ['modified', 'added', 'deleted', 'renamed', 'untracked', 'conflicted', 'clean'];
DEFINE FIELD staged     ON TABLE worktree_file TYPE bool;
DEFINE FIELD added      ON TABLE worktree_file TYPE int ASSERT $value >= 0;
DEFINE FIELD removed    ON TABLE worktree_file TYPE int ASSERT $value >= 0;
DEFINE FIELD binary     ON TABLE worktree_file TYPE bool;
DEFINE FIELD valid_from ON TABLE worktree_file TYPE datetime;

DEFINE INDEX idx_worktree_file ON TABLE worktree_file FIELDS checkout, path, valid_from;


-- 15 ── crawl_step ────────────────────── where one crawl's time went ───
-- One row per step of a crawl: when it began, how long it took, its git
-- calls, what it read and what it wrote.

DEFINE TABLE crawl_step SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD crawl        ON TABLE crawl_step TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD step         ON TABLE crawl_step TYPE string
    ASSERT $value INSIDE ['checkouts', 'refs', 'log', 'reflog', 'unreachable', 'placement', 'landed', 'blame', 'write'];
DEFINE FIELD started_at   ON TABLE crawl_step TYPE datetime;
DEFINE FIELD ms           ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD git_calls    ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD git_failed   ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD git_timeouts ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD objects_read ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD bytes_read   ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD rows_written ON TABLE crawl_step TYPE int ASSERT $value >= 0;
DEFINE FIELD note         ON TABLE crawl_step TYPE option<string>;
DEFINE FIELD valid_from   ON TABLE crawl_step TYPE datetime;

DEFINE INDEX idx_crawl_step ON TABLE crawl_step FIELDS crawl;


-- 16 ── git_call ────────────────────────── one git call a crawl made ───
-- Every git call: the step, the directory, the arguments exactly as run,
-- how long it took, its exit code (NONE when it was cut off at
-- `attr_repos_git_timeout_ms`), how much it returned, and what git wrote
-- to its error output, whole.

DEFINE TABLE git_call SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD crawl      ON TABLE git_call TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD step       ON TABLE git_call TYPE string
    ASSERT $value INSIDE ['checkouts', 'refs', 'log', 'reflog', 'unreachable', 'placement', 'landed', 'blame'];
DEFINE FIELD dir        ON TABLE git_call TYPE string;
DEFINE FIELD args       ON TABLE git_call TYPE array<string>;
DEFINE FIELD started_at ON TABLE git_call TYPE datetime;
DEFINE FIELD ms         ON TABLE git_call TYPE int ASSERT $value >= 0;
DEFINE FIELD exit       ON TABLE git_call TYPE option<int>;
DEFINE FIELD timed_out  ON TABLE git_call TYPE bool;
DEFINE FIELD bytes_out  ON TABLE git_call TYPE int ASSERT $value >= 0;
DEFINE FIELD lines_out  ON TABLE git_call TYPE int ASSERT $value >= 0;
DEFINE FIELD stderr     ON TABLE git_call TYPE option<string>;
DEFINE FIELD valid_from ON TABLE git_call TYPE datetime;

DEFINE INDEX idx_git_call        ON TABLE git_call FIELDS crawl;
DEFINE INDEX idx_git_call_failed ON TABLE git_call FIELDS timed_out, exit;


-- 17 ── sweep ──────────────────────────────── one pass over the list ───
-- What one pass did: the sessions and directories it read, the
-- repositories and checkouts it found new, gone (stale) and back, the
-- crawls it ran and how they ended, and its totals. Written once, when the
-- pass ends, with its crawls; `stopped` when the module was stopped
-- mid-pass.

DEFINE TABLE sweep SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD started_at         ON TABLE sweep TYPE datetime;
DEFINE FIELD finished_at        ON TABLE sweep TYPE datetime;
DEFINE FIELD interval_secs      ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD sessions_read      ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD directories_read   ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD directories_new    ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD directories_moved  ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD repositories_known ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD repositories_new   ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD repositories_stale ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD checkouts_known    ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD checkouts_new      ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD checkouts_gone     ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD checkouts_back     ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD excluded           ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD crawls             ON TABLE sweep TYPE array<record<crawl>>
    ASSERT array::all($value, |$c| record::exists($c));
DEFINE FIELD crawls_complete    ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD crawls_partial     ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD crawls_unreadable  ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD commits_new        ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD rows_written       ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD git_calls          ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD git_failed         ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD git_timeouts       ON TABLE sweep TYPE int ASSERT $value >= 0;
DEFINE FIELD outcome            ON TABLE sweep TYPE string
    ASSERT $value INSIDE ['complete', 'partial', 'stopped'];
DEFINE FIELD note               ON TABLE sweep TYPE option<string>;
DEFINE FIELD valid_from         ON TABLE sweep TYPE datetime;

DEFINE INDEX idx_sweep ON TABLE sweep FIELDS valid_from;
