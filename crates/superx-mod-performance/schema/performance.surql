-- =========================================================================
-- superx-mod-performance — the measurement module's ONE schema (#466)
-- =========================================================================
--
-- THIS FILE IS THE SCHEMA (§11): the model changes here first, the
-- operator applies it, code follows. Applied into the module's own
-- database `superx/performance` by `superx-performance --initialize`.
-- The KERNEL schema is untouched (§7).
--
-- One module, one schema, one crawler (operator, 2026-10-25). The
-- measurement tab's design was approved on 2026-10-24 ("yes"), and the
-- telemetry tables (1–15) on 2026-10-24 ("sche,a looks good"). The
-- repositories' tables (16–31) were approved for superx-repos on
-- 2026-10-21 (#466) and 2026-10-24 (#501); they join this database with
-- their definitions unchanged. The two modules' pass records become one
-- crawler's (32, 33).
--
--
-- WHAT THIS DATABASE HOLDS -------------------------------------------
--
-- What the agents did and what the operator asked, and what became of the
-- work in the git repositories the agents worked in, so the models, the
-- agents, thinking effort and the operator can be measured: now, and
-- against any earlier period.
--
-- ONE CRAWLER fills it, pass after pass, through its sources, run at
-- once (`pass_source`). It reads the lines the kernel captured since the
-- last pass, with SELECT only, a bounded page at a time in line-id order.
-- The sessions' working directories lead it to their repositories, and a
-- commit or a push a session printed sends it to crawl that repository in
-- the same pass. It reads the repositories with git, read-only: never a fetch, a
-- write, a checkout, or any program a repository names
-- (`--no-textconv`, no optional locks). A commit it reads is tied back to
-- the session that made it, in this one database.
--
-- Every row read from a kernel line names it (`kernel_message`, the
-- line's UUIDv7), so what a table does not copy, such as a tool call's
-- whole input, stays one read away. Nothing here is judged. Which work
-- the operator directed, which came from a spec, which an agent did
-- alone, what counts as a correction, how work is split into sittings,
-- and whether work is rework, thrash or a bulk drop are parameters,
-- applied when a chart is drawn.
--
-- Claude Code and Gemini CLI write conversations. Claude Desktop keeps no
-- conversation on this machine, so it adds no rows here.
--
--   From the captured lines
--   session        one row per captured session, ever
--   session_place  one row per change in a session's working directory
--                  or branch
--   prompt         one row per message from the operator
--   reply          one row per model reply, counted once
--   reply_state    one row per change in a reply's counts as its lines
--                  are read
--   thinking       one row per thinking span
--   tool_call      one row per tool call
--   tool_result    one row per tool call's result
--   edit           one row per file a file tool changed
--   git_event      one row per git or GitHub action an agent's command
--                  performed
--   test_run       one row per command that ran tests
--   build_run      one row per command that built code
--   turn           one row per turn Claude Code timed
--   compaction     one row per compaction
--   away           one row per recap written for the operator's return
--
--   From the repositories
--   repository     one row per repository, ever
--   checkout       one row per working tree of a repository, ever
--   checkout_state one row per change in a checkout's branch or presence
--   directory      one row per change in which checkout holds a directory
--   head_move      one row per move of a checkout's HEAD, from its reflog
--   commit         one row per commit read, ever
--   commit_file    one row per file a commit changed, ever
--   placement      one row per change in where a commit sits
--   landed_via     one row per file an off-main-line commit landed through
--                  another commit (a squash or a replay)
--   survival       one row per change in the lines blame still credits to
--                  a commit at its main line's tip
--   survival_file  the same, per file at the tip
--   crawl          one row per crawl of one repository: what it scanned,
--                  what changed since the last one, what it cost
--   ref_state      one row per change in a branch's or tag's tip
--   worktree_file  one row per change in a checkout's uncommitted file
--   crawl_step     one row per step of a crawl
--   git_call       one row per git call a crawl made
--
--   The crawler's own record
--   pass           one row per pass of the crawler
--   pass_source    one row per source the crawler ran in a pass
--
-- EVERY ROW IS DERIVED, from the kernel's captured lines and from git,
-- with one exception: reflog entries and unreachable commits that git has
-- since expired or pruned are kept only here (`head_move`, and the
-- commits whose placement is `recovered`). Provisioning loses those.
--
--
-- HOW CHANGE WORKS ---------------------------------------------------
--
-- Nothing is overwritten and nothing is removed (§10). A session, a
-- prompt, a reply, a tool call, a repository, a checkout and a commit
-- each have one row for good. What changes about them appends a row: a
-- reply's counts that grow while it is read append a `reply_state`; a
-- checkout's branch, a commit's placement or its surviving lines append a
-- row to their state table, only when it differs from the newest. The
-- newest `valid_from` is the fact. Row ids are UUIDv7, set by the
-- module's code (§11), never by the engine.
--
--
-- WHAT THE ENGINE ENFORCES, and the code never relaxes (§12) ---------
--
--   * Every link inside this database is a typed `record<…>` with an
--     ASSERT that the record exists. Links to the kernel are its UUIDv7
--     ids, typed `uuid`: a record link cannot cross databases.
--   * One row per session (`kernel_session`), per line read as a prompt,
--     turn, compaction or recap (`kernel_message`), per reply
--     (`session, agent_id`), per thinking span (`reply, position`), per
--     tool call (`reply, call_id`), per result (`tool_call`), per file an
--     edit changed (`tool_call, path`), per test or build command
--     (`tool_call`); per repository (`common_dir`), checkout
--     (`toplevel`), commit (`repository, hash`), file of a commit
--     (`commit, path`), HEAD move (`checkout, at, branch`) and landing
--     (`commit, path, via`): UNIQUE indexes, held by the engine, not by
--     care.
--   * Closed vocabularies are ASSERTed: what kind of prompt a line is,
--     how a tool call ended, how a file was changed and how its lines
--     were counted, which git action an agent took, which tool ran tests
--     or a build, what started a compaction; how a main line was chosen,
--     where a commit sits, where a directory came from, how a commit
--     changed a file, a crawl's steps, a file's uncommitted state, how a
--     crawl, a source or a pass ended, which sources the crawler has.
--   * Counts are never negative; a commit hash is 7 to 64 characters
--     where an agent printed it and 40 or 64 where git gave it whole; a
--     pass's crawls all exist.
--   * `FOR update NONE / FOR delete NONE` does not bind a system user;
--     append-only is held by the code, which never emits UPDATE or
--     DELETE (§10).

-- THE WHOLE DATABASE GOES FIRST. Provisioning leaves the database in
-- exactly this shape; nothing that was there survives. Every row is
-- derived (but for the reflog entries and unreachable commits above), so
-- a pass rebuilds it. Verified by a test that diffs the engine's own
-- schema against this file.

REMOVE DATABASE IF EXISTS performance;
DEFINE DATABASE performance;

DEFINE USER IF NOT EXISTS superx_mod_performance ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;


-- 1 ─── session ─────────────────────────── a session the kernel captured ───
-- `kernel_session` is the kernel's session entity. `agent` is the agent's
-- name as the kernel registered it (`claude_code`, `gemini_cli`).
-- `agent_id` is the session's id in the agent's own files. `entrypoint` is
-- how Claude Code was started (`cli`, an SDK, …) and `client_version` the
-- agent's version, both as its first line has them. `first_at` is its
-- first line.

DEFINE TABLE session SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD kernel_session ON TABLE session TYPE uuid;
DEFINE FIELD agent          ON TABLE session TYPE string;
DEFINE FIELD agent_id       ON TABLE session TYPE option<string>;
DEFINE FIELD entrypoint     ON TABLE session TYPE option<string>;
DEFINE FIELD client_version ON TABLE session TYPE option<string>;
DEFINE FIELD first_at       ON TABLE session TYPE datetime;
DEFINE FIELD valid_from     ON TABLE session TYPE datetime;

DEFINE INDEX idx_session_kernel ON TABLE session FIELDS kernel_session UNIQUE;
DEFINE INDEX idx_session_first  ON TABLE session FIELDS first_at;


-- 2 ─── session_place ───────────────── where a session was working ───
-- One row each time a session's working directory or branch changes, at
-- the first line that shows the new pair. `cwd` follows the agent's `cd`;
-- `branch` is Claude Code's `gitBranch`, which is the branch of the
-- directory the session was started in, not of `cwd`. NONE when the
-- agent records no branch.

DEFINE TABLE session_place SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session    ON TABLE session_place TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD cwd        ON TABLE session_place TYPE string;
DEFINE FIELD branch     ON TABLE session_place TYPE option<string>;
DEFINE FIELD at         ON TABLE session_place TYPE datetime;
DEFINE FIELD valid_from ON TABLE session_place TYPE datetime;

DEFINE INDEX idx_session_place ON TABLE session_place FIELDS session, at;


-- 3 ─── prompt ──────────────────────────── a message from the operator ───
-- `kind`: `typed`, sent while the agent waited for it; `queued`, typed
-- while the agent was working and handed to it at its next step (Claude
-- Code's `queued_command`); `command`, a slash command; `shell`, a command
-- the operator ran in the agent's shell mode (`!`); `interrupt`, the
-- operator stopping the agent. Claude Code marks who wrote a line
-- (`origin.kind`): task notifications, hook output, reminders, compaction
-- summaries and the instructions an agent gives its own subagent arrive
-- on the same kind of line and are not prompts. `text` is the message,
-- whole; `images` counts the images pasted with it.

DEFINE TABLE prompt SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE prompt TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE prompt TYPE uuid;
DEFINE FIELD at             ON TABLE prompt TYPE datetime;
DEFINE FIELD kind           ON TABLE prompt TYPE string
    ASSERT $value INSIDE ['typed', 'queued', 'command', 'shell', 'interrupt'];
DEFINE FIELD text           ON TABLE prompt TYPE string;
DEFINE FIELD chars          ON TABLE prompt TYPE int ASSERT $value >= 0;
DEFINE FIELD images         ON TABLE prompt TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE prompt TYPE datetime;

DEFINE INDEX idx_prompt_line    ON TABLE prompt FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_prompt_session ON TABLE prompt FIELDS session, at;
DEFINE INDEX idx_prompt_at      ON TABLE prompt FIELDS at;


-- 4 ─── reply ─────────────────────────────────────── one model reply ───
-- One API response, however many lines it was written across: Claude Code
-- writes a line per content block, each repeating the reply's usage;
-- Gemini re-emits a reply as it streams. `agent_id` is Claude's
-- `message.id` or Gemini's record `id`. `kernel_message` and `at` are its first line.
-- `model` is the model that answered; `requested_model` the one the client
-- asked for and `advisor_model` the advisor it names (Claude Code).
-- `effort` is the thinking effort on the reply's line and `turn_effort`
-- Claude Code's `perTurnEffort`. `stop_reason` is why it stopped
-- (`end_turn`, `tool_use`, `refusal`, …). `subagent`: the reply is a
-- subagent's (Claude Code's `isSidechain`).

DEFINE TABLE reply SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session         ON TABLE reply TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD agent_id        ON TABLE reply TYPE string;
DEFINE FIELD request_id      ON TABLE reply TYPE option<string>;
DEFINE FIELD kernel_message  ON TABLE reply TYPE uuid;
DEFINE FIELD at              ON TABLE reply TYPE datetime;
DEFINE FIELD model           ON TABLE reply TYPE string;
DEFINE FIELD requested_model ON TABLE reply TYPE option<string>;
DEFINE FIELD advisor_model   ON TABLE reply TYPE option<string>;
DEFINE FIELD effort          ON TABLE reply TYPE option<string>;
DEFINE FIELD turn_effort     ON TABLE reply TYPE option<string>;
DEFINE FIELD stop_reason     ON TABLE reply TYPE option<string>;
DEFINE FIELD subagent        ON TABLE reply TYPE bool;
DEFINE FIELD valid_from      ON TABLE reply TYPE datetime;

DEFINE INDEX idx_reply_agent   ON TABLE reply FIELDS session, agent_id UNIQUE;
DEFINE INDEX idx_reply_session ON TABLE reply FIELDS session, at;
DEFINE INDEX idx_reply_at      ON TABLE reply FIELDS at;
DEFINE INDEX idx_reply_model   ON TABLE reply FIELDS model, at;


-- 5 ─── reply_state ───────────────────── a reply's counts, as last read ───
-- Appended when a reply's counts differ from its newest row: once for a
-- Claude reply, whose usage is final on its first line, again as its
-- answer's text arrives on later lines; once per growth for a Gemini
-- reply. `output_tokens` is everything the model generated, thinking
-- included (Claude's `output_tokens`; Gemini's `output` plus `thoughts`);
-- `thinking_tokens` is the thinking part. `input_tokens` is the input not
-- served from cache (Gemini's `input` less `cached`). `cache_write_tokens`
-- is what was written to the cache (Claude). `tool_tokens` is Gemini's
-- `tool`. `web_searches` and `web_fetches` are Claude's server tool use.
-- `text_chars` is the length of the answer's text read so far. `usage` is
-- the whole usage object as captured, so a counter an agent adds later is
-- kept.

DEFINE TABLE reply_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD reply              ON TABLE reply_state TYPE record<reply>
    ASSERT record::exists($value);
DEFINE FIELD input_tokens       ON TABLE reply_state TYPE int ASSERT $value >= 0;
DEFINE FIELD cache_read_tokens  ON TABLE reply_state TYPE int ASSERT $value >= 0;
DEFINE FIELD cache_write_tokens ON TABLE reply_state TYPE int ASSERT $value >= 0;
DEFINE FIELD output_tokens      ON TABLE reply_state TYPE int ASSERT $value >= 0;
DEFINE FIELD thinking_tokens    ON TABLE reply_state TYPE int ASSERT $value >= 0;
DEFINE FIELD tool_tokens        ON TABLE reply_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD web_searches       ON TABLE reply_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD web_fetches        ON TABLE reply_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD service_tier       ON TABLE reply_state TYPE option<string>;
DEFINE FIELD text_chars         ON TABLE reply_state TYPE int ASSERT $value >= 0;
DEFINE FIELD usage              ON TABLE reply_state TYPE object FLEXIBLE;
DEFINE FIELD valid_from         ON TABLE reply_state TYPE datetime;

DEFINE INDEX idx_reply_state ON TABLE reply_state FIELDS reply, valid_from;


-- 6 ─── thinking ─────────────────────────────────── one thinking span ───
-- `position` is the span's place in its reply. `ms` is how long the model
-- thought (Claude Code's `thinkingDurationMs`), NONE where the agent
-- records no length (Gemini's thoughts carry only their time). `chars` is
-- the length of the thinking text as recorded, 0 when only its signature
-- is kept. `display` is Claude Code's `thinkingDisplay`.

DEFINE TABLE thinking SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD reply          ON TABLE thinking TYPE record<reply>
    ASSERT record::exists($value);
DEFINE FIELD position       ON TABLE thinking TYPE int ASSERT $value >= 0;
DEFINE FIELD kernel_message ON TABLE thinking TYPE uuid;
DEFINE FIELD at             ON TABLE thinking TYPE datetime;
DEFINE FIELD ms             ON TABLE thinking TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD chars          ON TABLE thinking TYPE int ASSERT $value >= 0;
DEFINE FIELD display        ON TABLE thinking TYPE option<string>;
DEFINE FIELD valid_from     ON TABLE thinking TYPE datetime;

DEFINE INDEX idx_thinking    ON TABLE thinking FIELDS reply, position UNIQUE;
DEFINE INDEX idx_thinking_at ON TABLE thinking FIELDS at;


-- 7 ─── tool_call ───────────────────────────────────── one tool call ───
-- `call_id` is the agent's id for the call. `tool` is its name as the
-- agent gave it (`Bash`, `Edit`, `mcp__<server>__<tool>`, …). What it
-- acted on, when the input names it: `path` (the file tools), `command`
-- (a shell command, whole), `description`, `pattern` (search tools), `url`
-- or `query` (web tools), `subagent_type` (a subagent it started), and
-- `background`, a command started to run on its own. `input_bytes` is the
-- size of the whole input, which stays in the kernel line.

DEFINE TABLE tool_call SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD reply          ON TABLE tool_call TYPE record<reply>
    ASSERT record::exists($value);
DEFINE FIELD call_id        ON TABLE tool_call TYPE string;
DEFINE FIELD kernel_message ON TABLE tool_call TYPE uuid;
DEFINE FIELD at             ON TABLE tool_call TYPE datetime;
DEFINE FIELD tool           ON TABLE tool_call TYPE string;
DEFINE FIELD path           ON TABLE tool_call TYPE option<string>;
DEFINE FIELD command        ON TABLE tool_call TYPE option<string>;
DEFINE FIELD description    ON TABLE tool_call TYPE option<string>;
DEFINE FIELD pattern        ON TABLE tool_call TYPE option<string>;
DEFINE FIELD url            ON TABLE tool_call TYPE option<string>;
DEFINE FIELD query          ON TABLE tool_call TYPE option<string>;
DEFINE FIELD subagent_type  ON TABLE tool_call TYPE option<string>;
DEFINE FIELD background     ON TABLE tool_call TYPE bool;
DEFINE FIELD input_bytes    ON TABLE tool_call TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE tool_call TYPE datetime;

DEFINE INDEX idx_tool_call      ON TABLE tool_call FIELDS reply, call_id UNIQUE;
DEFINE INDEX idx_tool_call_at   ON TABLE tool_call FIELDS at;
DEFINE INDEX idx_tool_call_tool ON TABLE tool_call FIELDS tool, at;


-- 8 ─── tool_result ───────────────────────── how one tool call ended ───
-- `outcome`: `ok`; `error`, the tool reported a failure; `denied`, the
-- operator refused the call; `interrupted`, the operator stopped it;
-- `cancelled`, the agent's own cancel (Gemini); `timed_out`, a command
-- cut off at its time limit. `ms` is the agent's own measure of the call,
-- NONE when it records none: the time between the call and its result is
-- read from `at`. `bytes` is the size of what the tool returned.

DEFINE TABLE tool_result SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call      ON TABLE tool_result TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE tool_result TYPE uuid;
DEFINE FIELD at             ON TABLE tool_result TYPE datetime;
DEFINE FIELD outcome        ON TABLE tool_result TYPE string
    ASSERT $value INSIDE ['ok', 'error', 'denied', 'interrupted', 'cancelled', 'timed_out'];
DEFINE FIELD ms             ON TABLE tool_result TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD bytes          ON TABLE tool_result TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE tool_result TYPE datetime;

DEFINE INDEX idx_tool_result    ON TABLE tool_result FIELDS tool_call UNIQUE;
DEFINE INDEX idx_tool_result_at ON TABLE tool_result FIELDS at;


-- 9 ─── edit ──────────────────────────── a file a file tool changed ───
-- Written from a successful result, so a call that changed nothing has no
-- row. `kind`: the tool created the file, or modified one that existed.
-- `added` and `removed` count lines, `counted_from` says how: the
-- structured patch the agent recorded (`patch`), the old and new text the
-- call carried (`strings`), or the whole content it wrote (`content`).
-- `operator_changed`: the operator altered the change before accepting it
-- (Claude Code's `userModified`). Changes made through a shell command are
-- not counted here; the repositories count every committed line.

DEFINE TABLE edit SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call        ON TABLE edit TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD path             ON TABLE edit TYPE string;
DEFINE FIELD kind             ON TABLE edit TYPE string
    ASSERT $value INSIDE ['created', 'modified'];
DEFINE FIELD added            ON TABLE edit TYPE int ASSERT $value >= 0;
DEFINE FIELD removed          ON TABLE edit TYPE int ASSERT $value >= 0;
DEFINE FIELD counted_from     ON TABLE edit TYPE string
    ASSERT $value INSIDE ['patch', 'strings', 'content'];
DEFINE FIELD operator_changed ON TABLE edit TYPE bool;
DEFINE FIELD valid_from       ON TABLE edit TYPE datetime;

DEFINE INDEX idx_edit      ON TABLE edit FIELDS tool_call, path UNIQUE;
DEFINE INDEX idx_edit_path ON TABLE edit FIELDS path;


-- 10 ── git_event ──────────── a git or GitHub action an agent performed ───
-- Read from a shell command and what it printed, or from Claude Code's
-- own `gitOperation` note on the result. `action` is what happened;
-- `dir` the directory the command ran in. `hash` is the commit as printed
-- (a commit's short hash; a merge's, revert's or cherry-pick's new
-- commit), `branch` the branch it named, `subject` the commit's first
-- line, `pr_number` and `url` the pull request. A commit seen here is the
-- exact tie between a session and the repository's commit.

DEFINE TABLE git_event SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call  ON TABLE git_event TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD at         ON TABLE git_event TYPE datetime;
DEFINE FIELD action     ON TABLE git_event TYPE string
    ASSERT $value INSIDE ['commit', 'push', 'merge', 'rebase', 'revert', 'cherry_pick', 'reset', 'branch_deleted', 'pr_created', 'pr_merged'];
DEFINE FIELD dir        ON TABLE git_event TYPE option<string>;
DEFINE FIELD hash       ON TABLE git_event TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD branch     ON TABLE git_event TYPE option<string>;
DEFINE FIELD subject    ON TABLE git_event TYPE option<string>;
DEFINE FIELD pr_number  ON TABLE git_event TYPE option<int>
    ASSERT $value == NONE OR $value > 0;
DEFINE FIELD url        ON TABLE git_event TYPE option<string>;
DEFINE FIELD valid_from ON TABLE git_event TYPE datetime;

DEFINE INDEX idx_git_event      ON TABLE git_event FIELDS tool_call;
DEFINE INDEX idx_git_event_hash ON TABLE git_event FIELDS hash;
DEFINE INDEX idx_git_event_at   ON TABLE git_event FIELDS action, at;


-- 11 ── test_run ─────────────────────── a command that ran tests ───
-- The counts the test runner printed, summed over every suite the command
-- ran. `runner` is the tool that printed them.

DEFINE TABLE test_run SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call  ON TABLE test_run TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD at         ON TABLE test_run TYPE datetime;
DEFINE FIELD runner     ON TABLE test_run TYPE string
    ASSERT $value INSIDE ['cargo', 'pytest', 'jest', 'vitest', 'go', 'node'];
DEFINE FIELD passed     ON TABLE test_run TYPE int ASSERT $value >= 0;
DEFINE FIELD failed     ON TABLE test_run TYPE int ASSERT $value >= 0;
DEFINE FIELD ignored    ON TABLE test_run TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from ON TABLE test_run TYPE datetime;

DEFINE INDEX idx_test_run    ON TABLE test_run FIELDS tool_call UNIQUE;
DEFINE INDEX idx_test_run_at ON TABLE test_run FIELDS at;


-- 12 ── build_run ────────────────────── a command that built code ───
-- `ok`: the build finished. `errors` and `warnings` are what the compiler
-- printed.

DEFINE TABLE build_run SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call  ON TABLE build_run TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD at         ON TABLE build_run TYPE datetime;
DEFINE FIELD builder    ON TABLE build_run TYPE string
    ASSERT $value INSIDE ['cargo', 'tsc', 'vite', 'npm', 'go'];
DEFINE FIELD ok         ON TABLE build_run TYPE bool;
DEFINE FIELD errors     ON TABLE build_run TYPE int ASSERT $value >= 0;
DEFINE FIELD warnings   ON TABLE build_run TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from ON TABLE build_run TYPE datetime;

DEFINE INDEX idx_build_run    ON TABLE build_run FIELDS tool_call UNIQUE;
DEFINE INDEX idx_build_run_at ON TABLE build_run FIELDS at;


-- 13 ── turn ──────────────────────────────── a turn Claude Code timed ───
-- From Claude Code's `turn_duration` line: how long the turn took, from
-- the prompt to the agent's last word, and how many lines it wrote.

DEFINE TABLE turn SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE turn TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE turn TYPE uuid;
DEFINE FIELD at             ON TABLE turn TYPE datetime;
DEFINE FIELD ms             ON TABLE turn TYPE int ASSERT $value >= 0;
DEFINE FIELD lines          ON TABLE turn TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE turn TYPE datetime;

DEFINE INDEX idx_turn_line    ON TABLE turn FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_turn_session ON TABLE turn FIELDS session, at;


-- 14 ── compaction ───────────────────────────────── one compaction ───
-- From Claude Code's `compact_boundary` line: what started it, the
-- context before and after in tokens, the tokens dropped so far in the
-- session, and how long it took.

DEFINE TABLE compaction SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE compaction TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE compaction TYPE uuid;
DEFINE FIELD at             ON TABLE compaction TYPE datetime;
DEFINE FIELD trigger        ON TABLE compaction TYPE string
    ASSERT $value INSIDE ['auto', 'manual'];
DEFINE FIELD tokens_before  ON TABLE compaction TYPE int ASSERT $value >= 0;
DEFINE FIELD tokens_after   ON TABLE compaction TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD tokens_dropped ON TABLE compaction TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD ms             ON TABLE compaction TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD valid_from     ON TABLE compaction TYPE datetime;

DEFINE INDEX idx_compaction_line    ON TABLE compaction FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_compaction_session ON TABLE compaction FIELDS session, at;


-- 15 ── away ──────────────────────── a recap for the operator's return ───
-- From Claude Code's `away_summary` line: written when the operator comes
-- back, saying what happened meanwhile. `text` is the recap, whole.

DEFINE TABLE away SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE away TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE away TYPE uuid;
DEFINE FIELD at             ON TABLE away TYPE datetime;
DEFINE FIELD text           ON TABLE away TYPE string;
DEFINE FIELD valid_from     ON TABLE away TYPE datetime;

DEFINE INDEX idx_away_line    ON TABLE away FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_away_session ON TABLE away FIELDS session, at;


-- 16 ── repository ─────────────────────── a repository the crawler found ───
-- Identified by its common git directory, so every worktree of one
-- repository is the same repository. `name` is the directory that holds
-- that `.git`.

DEFINE TABLE repository SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD common_dir ON TABLE repository TYPE string;
DEFINE FIELD name       ON TABLE repository TYPE string;
DEFINE FIELD found_at   ON TABLE repository TYPE datetime;
DEFINE FIELD valid_from ON TABLE repository TYPE datetime;

DEFINE INDEX idx_repository_dir ON TABLE repository FIELDS common_dir UNIQUE;


-- 17 ── checkout ─────────────────────── one working tree of a repository ───
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


-- 18 ── checkout_state ─────────────────── a checkout's branch, as last read ───
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


-- 19 ── directory ─────────────────── which checkout holds a directory ───
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


-- 20 ── head_move ─────────────────── one move of a checkout's HEAD ───
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


-- 21 ── commit ──────────────────────────────── one commit, as git has it ───
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


-- 22 ── commit_file ─────────────────────── one file a commit changed ───
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


-- 23 ── placement ─────────────────────────── where a commit sits ───
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


-- 24 ── landed_via ──────── a file that landed through another commit ───
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


-- 25 ── survival ─────────── what of a commit is still there ───
-- The lines `git blame` credits to the commit at its main line's tip,
-- never more than the commit added. Files git counts no lines in, and the
-- paths `attr_performance_survival_skip` names, are never blamed.

DEFINE TABLE survival SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE survival TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE survival TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD alive      ON TABLE survival TYPE int
    ASSERT $value >= 0;
DEFINE FIELD valid_from ON TABLE survival TYPE datetime;

DEFINE INDEX idx_survival ON TABLE survival FIELDS commit, valid_from;


-- 26 ── crawl ─────────────────────────── one crawl of one repository ───
-- `mainline` is the ref the crawl read as the main line and
-- `mainline_from` how it was chosen: the operator's override
-- (`attr_performance_mainline_refs`), what `origin/HEAD` points at, a local
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
-- at `attr_performance_git_timeout_ms`, time spent waiting on git, rows written.
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


-- 27 ── survival_file ─── what of a commit is still there, file by file ───
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


-- 28 ── ref_state ───────────── a branch's or tag's tip, as last read ───
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


-- 29 ── worktree_file ───────────────── a checkout's uncommitted file ───
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


-- 30 ── crawl_step ────────────────────── where one crawl's time went ───
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


-- 31 ── git_call ────────────────────────── one git call a crawl made ───
-- Every git call: the step, the directory, the arguments exactly as run,
-- how long it took, its exit code (NONE when it was cut off at
-- `attr_performance_git_timeout_ms`), how much it returned, and what git wrote
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


-- 32 ── pass ──────────────────────────────── one pass of the crawler ───
-- One crawler fills this database, pass after pass. A pass runs every
-- source the crawler has, as many at once, and each source as much of its
-- work at once, as their parameters allow. `rows_written` is what the
-- pass added in all; what each source did is in `pass_source`. Written
-- once, when the pass ends: `partial` when a source could not read
-- everything, `stopped` when the module was stopped mid-pass.

DEFINE TABLE pass SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD started_at   ON TABLE pass TYPE datetime;
DEFINE FIELD finished_at  ON TABLE pass TYPE datetime;
DEFINE FIELD rows_written ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD outcome      ON TABLE pass TYPE string
    ASSERT $value INSIDE ['complete', 'partial', 'stopped'];
DEFINE FIELD note         ON TABLE pass TYPE option<string>;
DEFINE FIELD valid_from   ON TABLE pass TYPE datetime;

DEFINE INDEX idx_pass ON TABLE pass FIELDS valid_from;


-- 33 ── pass_source ───────────────────── what one source did in a pass ───
-- A source is the part of the crawler that reads one kind of thing:
-- `lines`, the lines the kernel captured; `repos`, the git repositories
-- the lines lead to. A source reads what the others wrote here: `repos`
-- follows the sessions' working directories and the commits and pushes
-- they printed. A source added later (the GitHub and Bitbucket hosts,
-- and others) adds its name to this list and its own tables of facts, by
-- a schema change, and needs no change to the pass record.
--
-- `items` counts what it worked through (lines, repositories), `unread`
-- those in a shape it does not know, so nothing goes unaccounted. `calls`
-- counts the calls it made outside the database (git, a host's API),
-- `failed` and `timed_out` those that did not end well. `rows` is the
-- rows it added, by table; `counts` what else it counts, by name (for
-- `repos`: directories followed and new, repositories and checkouts new,
-- stale, gone and back, and those excluded). `from_id` and `to_id` are
-- where a source that reads in order began and stopped (for `lines`, the
-- kernel lines' UUIDv7s): its next pass starts after `to_id`, reading a
-- little before it again so a line captured late is never missed.
-- `crawls` are the repository crawls it ran. `outcome`: `complete`;
-- `partial`, it could not read everything; `failed`, it stopped on an
-- error, said in `note`; `stopped`, the module was stopped.

DEFINE TABLE pass_source SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pass        ON TABLE pass_source TYPE record<pass>
    ASSERT record::exists($value);
DEFINE FIELD source      ON TABLE pass_source TYPE string
    ASSERT $value INSIDE ['lines', 'repos'];
DEFINE FIELD started_at  ON TABLE pass_source TYPE datetime;
DEFINE FIELD finished_at ON TABLE pass_source TYPE datetime;
DEFINE FIELD items       ON TABLE pass_source TYPE int ASSERT $value >= 0;
DEFINE FIELD unread      ON TABLE pass_source TYPE int ASSERT $value >= 0;
DEFINE FIELD calls       ON TABLE pass_source TYPE int ASSERT $value >= 0;
DEFINE FIELD failed      ON TABLE pass_source TYPE int ASSERT $value >= 0;
DEFINE FIELD timed_out   ON TABLE pass_source TYPE int ASSERT $value >= 0;
DEFINE FIELD rows        ON TABLE pass_source TYPE object FLEXIBLE
    ASSERT array::all(object::values($value), |$n| type::is_int($n) AND $n >= 0);
DEFINE FIELD counts      ON TABLE pass_source TYPE object FLEXIBLE
    ASSERT array::all(object::values($value), |$n| type::is_int($n) AND $n >= 0);
DEFINE FIELD from_id     ON TABLE pass_source TYPE option<uuid>;
DEFINE FIELD to_id       ON TABLE pass_source TYPE option<uuid>;
DEFINE FIELD crawls      ON TABLE pass_source TYPE array<record<crawl>>
    ASSERT array::all($value, |$c| record::exists($c));
DEFINE FIELD outcome     ON TABLE pass_source TYPE string
    ASSERT $value INSIDE ['complete', 'partial', 'failed', 'stopped'];
DEFINE FIELD note        ON TABLE pass_source TYPE option<string>;
DEFINE FIELD valid_from  ON TABLE pass_source TYPE datetime;

DEFINE INDEX idx_pass_source        ON TABLE pass_source FIELDS pass;
DEFINE INDEX idx_pass_source_source ON TABLE pass_source FIELDS source, valid_from;
