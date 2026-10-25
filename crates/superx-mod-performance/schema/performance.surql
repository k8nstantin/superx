-- =========================================================================
-- superx-mod-performance — the measurement module's OWN schema (#466)
-- =========================================================================
--
-- THIS FILE IS THE SCHEMA (§11): the model changes here first, the
-- operator applies it, code follows. Applied into the module's own
-- database `superx/performance` by `superx-performance --initialize`.
-- The KERNEL schema is untouched (§7). The measurement tab's design was
-- approved by the operator on 2026-10-24 ("yes").
--
--
-- WHAT THIS DATABASE HOLDS -------------------------------------------
--
-- What the agents did and what the operator asked, read from every line
-- the kernel captured, so the models, the agents, thinking effort and the
-- operator can be measured: now, and against any earlier period. The
-- module reads the kernel's `message` rows with SELECT only, a bounded
-- page at a time in line-id order, and never reads git: the repositories'
-- facts come from superx-repos, in a later schema change of their own.
--
-- Every row names the kernel line it was read from (`kernel_message`, the
-- line's UUIDv7), so what a table does not copy, such as a tool call's
-- whole input or a file's whole content, stays one read away. Nothing
-- here is judged. Which work you directed, which came from a spec, which
-- an agent did alone, what counts as a correction, and how work is split
-- into sittings are parameters, applied when a chart is drawn.
--
-- Claude Code and Gemini CLI write conversations. Claude Desktop keeps no
-- conversation on this machine, so it adds no rows here.
--
--   session       one row per captured session, ever
--   session_place one row per change in a session's working directory
--                 or branch
--   prompt        one row per message from the operator
--   reply         one row per model reply, counted once
--   reply_state   one row per change in a reply's counts as its lines
--                 are read
--   thinking      one row per thinking span
--   tool_call     one row per tool call
--   tool_result   one row per tool call's result
--   edit          one row per file a file tool changed
--   git_event     one row per git or GitHub action an agent's command
--                 performed
--   test_run      one row per command that ran tests
--   build_run     one row per command that built code
--   turn          one row per turn Claude Code timed
--   compaction    one row per compaction
--   away          one row per recap written for the operator's return
--   pass          one row per pass of the reader
--
--
-- HOW CHANGE WORKS ---------------------------------------------------
--
-- Nothing is overwritten and nothing is removed (§10). A session, a
-- prompt, a reply and a tool call each have one row for good; a reply's
-- counts that grow while it is read append a `reply_state` row, and the
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
--     (`session, agent_id`), per thinking span (`reply, position`), per tool call
--     (`reply, call_id`), per result (`tool_call`), per file an edit
--     changed (`tool_call, path`), per test or build command
--     (`tool_call`): UNIQUE indexes, held by the engine, not by care.
--   * Closed vocabularies are ASSERTed: what kind of prompt a line is,
--     how a tool call ended, how a file was changed and how its lines
--     were counted, which git action an agent took, which tool ran tests
--     or a build, what started a compaction, how a pass ended.
--   * Counts are never negative; a commit hash is 7 to 64 characters.
--   * `FOR update NONE / FOR delete NONE` does not bind a system user;
--     append-only is held by the code, which never emits UPDATE or
--     DELETE (§10).

-- THE WHOLE DATABASE GOES FIRST. Provisioning leaves the database in
-- exactly this shape; nothing that was there survives. Every row here is
-- derived from the kernel's lines, so a pass rebuilds it all. Verified by
-- a test that diffs the engine's own schema against this file.

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


-- 16 ── pass ───────────────────────────────── one pass of the reader ───
-- What one pass read and wrote. `from_line` and `to_line` are the first
-- and last kernel lines it read, NONE when it read none; the next pass
-- starts after `to_line`, reading a little before it again so a line
-- captured late is never missed. `lines_unread` counts lines in a shape
-- the reader does not know, so no line goes unaccounted. Written once,
-- when the pass ends; `stopped` when the module was stopped mid-pass.

DEFINE TABLE pass SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD started_at       ON TABLE pass TYPE datetime;
DEFINE FIELD finished_at      ON TABLE pass TYPE datetime;
DEFINE FIELD from_line        ON TABLE pass TYPE option<uuid>;
DEFINE FIELD to_line          ON TABLE pass TYPE option<uuid>;
DEFINE FIELD lines_read       ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD lines_unread     ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD sessions_new     ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD places_new       ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD prompts_new      ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD replies_new      ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD reply_states_new ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD thinking_new     ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD tool_calls_new   ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD tool_results_new ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD edits_new        ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD git_events_new   ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD test_runs_new    ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD build_runs_new   ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD turns_new        ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD compactions_new  ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD aways_new        ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD rows_written     ON TABLE pass TYPE int ASSERT $value >= 0;
DEFINE FIELD outcome          ON TABLE pass TYPE string
    ASSERT $value INSIDE ['complete', 'partial', 'stopped'];
DEFINE FIELD note             ON TABLE pass TYPE option<string>;
DEFINE FIELD valid_from       ON TABLE pass TYPE datetime;

DEFINE INDEX idx_pass ON TABLE pass FIELDS valid_from;
