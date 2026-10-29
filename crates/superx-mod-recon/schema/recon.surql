-- =========================================================================
-- superx-mod-recon — the recon module's ONE schema: the structure
-- =========================================================================
--
-- THIS FILE IS THE STRUCTURE (§11): the model changes here first, with the
-- operator, who approves it change by change; the operator applies it, and
-- code follows. Applied into the module's own database `superx/recon` by
-- `superx-recon --initialize`. The KERNEL schema is untouched (§7). The
-- build spec, specs/recon/, names it as the authority over every table.
--
-- One module, one schema, one crawler (operator, 2026-10-25). The
-- measurement tab's design was approved on 2026-10-24 ("yes"), and the
-- telemetry tables (1–15) on 2026-10-24 ("sche,a looks good"). The
-- repositories' tables (16–31) were approved for superx-repos on
-- 2026-10-21 (#466) and 2026-10-24 (#501); they join this database with
-- their definitions unchanged. The two modules' pass records become one
-- crawler's (32, 33).
--
-- The deep crawl (operator, 2026-10-25: "we need deep info I can not
-- overstate it", "crawl not skim", "crawler means slow diligent
-- methodical") gives every field the agents write a place: a column, a
-- table, or the kernel line it stays in, said here. It adds the lines'
-- tables 34–53 and the repositories' 54–59, and changes tables 3–9, 21,
-- 30 and 31.
--
-- Recon (operator, 2026-10-26): the module is superx-mod-recon. Its
-- crawler fetches into mirror clones of its own, and reads the hosts the
-- repositories live on: everything their read APIs return, every answer
-- kept whole beside typed tables for what is measured ("the more info it
-- has the better"; less data is more assumption, and the crawler is there
-- for the facts the operator does not know). It adds the hosts' and the
-- mirrors' tables 60–82, and changes tables 26, 28, 30, 31, 33, 55 and 59.
--
-- Git's facts (operator, 2026-10-26: "1 -9 yes change the schema for more
-- data"), from git's own documentation read whole (git 2.50.1): where
-- each commit was made in local time, files copied beside files moved,
-- how each merge's conflicts were resolved, the commits that make the
-- same change, operations a checkout is in the middle of, each branch's
-- upstream and how far it is ahead and behind, line counts blind to
-- whitespace, annotated tags and signatures, and commit trailers. It adds
-- tables 83–85, and changes tables 18, 21, 22, 28, 30, 31 and 56.
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
-- the same pass. It reads the operator's repositories with git, read-only:
-- never a fetch into them, a write, a checkout, or any program a repository
-- names (`--no-textconv`, no optional locks). It fetches only into its own
-- mirror clones, one per repository on a host, in the repos folder of its
-- module folder: every ref the host advertises, never pruned and never
-- collected. It reads the hosts (GitHub, Bitbucket) through their APIs,
-- GET only, with the operator's read-only tokens, one call at a time per
-- host, and keeps every answer whole. A commit it reads is tied back to the
-- session that made it, and to the pull request it went through, in this
-- one database.
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
--   trigger        one row per line that set an agent working without the
--                  operator: a task notification, a peer, a continuation
--   said           one row per block of text a model wrote, as it grew
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
--   line           one row per line the crawler read, and what it became
--   line_field     one row per field seen on a kind of line, and per change
--                  in whether the crawler stores it
--   request        one row per request Claude Code sent to a model
--   request_part   one row per distinct piece of a request, by its hash
--   request_shape  one row per system prompt and tool set, by its hash
--   context        one row per attachment that put something before the
--                  model: instructions, skills, tools, hooks, files
--   hook_run       one row per hook a session ran
--   session_state  one row per change in a session's title or modes
--   session_cost   one row per change in what a session cost
--   model_cost     the same, per model
--   session_link   one row per pull request, page or issue a session
--                  links to
--   git_state      one row per change in the git state a session saw
--   notice         one row per notice the client gave in place of a reply
--   question       one row per question an agent asked the operator
--   task_event     one row per change an agent made to its own task list
--   plan           one row per plan an agent put to the operator
--   subagent_run   one row per subagent an agent started
--   queue_event    one row per change to the operator's queued messages
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
--   remote_state   one row per change in a repository's remotes
--   ref_move       one row per reflog entry of any ref, ever
--   commit_rename  one row per file a commit moved or copied
--   tip_file       one row per change in a file at the main line's tip
--   repository_state one row per change in how a repository is set up
--   commit_session one row per commit tied to the session that made it
--   merge_resolution one row per file a merge resolved differently from
--                  git's own merge of its parents
--   tag            one row per annotated tag, ever
--   commit_trailer one row per trailer of a commit's message
--
--   From the hosts and the mirrors
--   host_repository one row per repository on a host, ever
--   host_object    one row per object a host's answer held, and per change
--                  in it, whole
--   host_repository_state one row per change in how a host keeps a
--                  repository
--   host_user      one row per account the hosts name, ever
--   host_crawl     one row per crawl of one repository's host
--   host_call      one row per call to a host
--   mirror         one row per mirror clone the crawler keeps, ever
--   pull_request   one row per pull request, ever
--   pull_request_state one row per change in a pull request
--   pull_request_commit one row per change in whether a pull request holds
--                  a commit
--   pull_request_file one row per change in a file a pull request changes
--   pull_request_review one row per review, and per change in one
--   pull_request_event one row per event of a pull request's timeline
--   issue          one row per issue, ever
--   issue_state    one row per change in an issue
--   host_comment   one row per comment on a pull request or an issue, and
--                  per edit
--   ci_run         one row per CI run, job, step, check or status, ever
--   ci_run_state   one row per change in one
--   ci_log         one row per log of a finished job or step, whole
--   release        one row per release, and per change in one
--   host_rule      one row per change in a rule a host enforces
--   deployment     one row per deployment, ever
--   deployment_state one row per change in a deployment
--
--   The crawler's own record
--   pass           one row per pass of the crawler
--   pass_source    one row per source the crawler ran in a pass
--
-- EVERY ROW IS DERIVED, from the kernel's captured lines and from git,
-- with one exception: reflog entries and unreachable commits that git has
-- since expired or pruned are kept only here (`head_move`, `ref_move`, and
-- the commits whose placement is `recovered`). Provisioning loses those.
-- The mirrors are files of the module's folder, not of this database: a
-- provision keeps them, and with them every ref the hosts ever showed. What
-- a host has deleted since the crawler read it (a comment, a pull request's
-- earlier commits) is kept only here, in `host_object` and the tables read
-- from it; a provision loses that too.
--
-- WHAT STAYS IN THE KERNEL LINE. A row names its line, so the line's
-- other fields are one read away. Whole tool inputs and outputs, the text
-- of request pieces and system prompts, images, and Claude Code's own
-- bookkeeping (file backups, artifact ledgers) stay there; `line_field`
-- lists every field the crawler does not store, so none is left out unseen.
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
--     trigger, turn, compaction, recap, request, attachment, notice or
--     queue change, and per line read at all (`kernel_message`), per reply
--     (`session, agent_id`), per thinking span (`reply, position`), per
--     tool call (`reply, call_id`), per result (`tool_call`), per file an
--     edit changed (`tool_call, path`), per test or build command, task
--     change, plan and subagent (`tool_call`), per question
--     (`tool_call, position`), per hook (`kernel_message, position`), per
--     request piece (`hash`) and shape (`shape_hash`), per session link
--     (`session, kind, url`), per model in a cost (`session_cost, model`);
--     per repository (`common_dir`), checkout (`toplevel`), commit
--     (`repository, hash`), file of a commit (`commit, path`), rename
--     (`commit, from_path, to_path`), HEAD move (`checkout, at, branch`),
--     reflog entry (`repository, checkout, ref, at, old, new, action`; a
--     mirror's: `mirror, ref, at, old, new, action`), landing (`commit,
--     path, via`) and commit tie (`commit, session, via`); per repository
--     on a host (`host, path`), host account (`host, host_id`), host object
--     and content (`host_repository, kind, key, hash`), mirror
--     (`host_repository`), pull request and issue (`host_repository,
--     number`), CI run (`host_repository, kind, host_id`), log (`ci_run`),
--     deployment (`host_repository, host_id`), timeline event and comment
--     (`object`); per file a merge resolved (`commit, path`), annotated tag
--     (`repository, object`) and trailer (`commit, position`): UNIQUE
--     indexes, held by the engine, not by care. With one
--     gap, the engine's: SurrealDB 3.1.4 holds no unique index for a row
--     whose indexed field is NONE (checked 2026-10-26), so a HEAD move to a
--     detached HEAD, and a reflog entry with no checkout, no old commit or
--     no new one, are held by the crawler, which reads what the table holds
--     before it writes.
--   * Closed vocabularies are ASSERTed where the words are this module's
--     own: what kind of prompt a line is, how a tool call ended, how a
--     file was changed and how its lines were counted, which git action an
--     agent took, which tool ran tests or a build, what started a
--     compaction, what a session links to, how an agent changed its task
--     list, the tables a line was read into; how a main line was chosen,
--     where a commit sits, where a directory came from, how a commit
--     changed a file, a crawl's steps, a file's uncommitted state, how a
--     commit was tied to a session, how a crawl, a source or a pass ended,
--     which sources the crawler has; the hosts it reads, the kinds of object
--     and of CI run, rule and comment it keeps, how a host crawl ended;
--     whether git found a file moved or copied, the operations a checkout
--     can be in the middle of, what an annotated tag points at. Words
--     an agent or a host writes (a model, an effort, a permission mode, an
--     error, an attachment's type; a pull request's state, a review's, a
--     check's conclusion, an event's kind) are kept as written: a new one is
--     a new value, not a refusal.
--   * Counts are never negative; a commit hash is 7 to 64 characters
--     where an agent printed it or a host gave it, and 40 or 64 where git
--     gave it whole; a host object's content hash is a SHA-256; a pass's
--     crawls all exist.
--   * `FOR update NONE / FOR delete NONE` does not bind a system user;
--     append-only is held by the code, which never emits UPDATE or
--     DELETE (§10).

-- THE WHOLE DATABASE GOES FIRST. Provisioning leaves the database in
-- exactly this shape; nothing that was there survives. Every row is
-- derived (but for the reflog entries and unreachable commits above), so
-- a pass rebuilds it. Verified by a test that diffs the engine's own
-- schema against this file.

REMOVE DATABASE IF EXISTS recon;
DEFINE DATABASE recon;

DEFINE USER IF NOT EXISTS superx_mod_recon ON DATABASE
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
-- on the same kind of line and are not prompts (they are `trigger` rows).
-- `text` is the message, whole; `images` counts the images pasted with it.
-- `prompt_id` is Claude Code's id for the prompt, which every line of the
-- turn it started carries. `source` is how Claude Code says it came in
-- (`typed`, `queued`, `suggestion_accepted`, …), `permission_mode` how tool
-- calls were being approved when it was sent (`default`, `acceptEdits`,
-- `plan`, `auto`, …), `turn_index` and `prompt_index` its place in the
-- session as Claude Code counts it, all as written.

DEFINE TABLE prompt SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session         ON TABLE prompt TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message  ON TABLE prompt TYPE uuid;
DEFINE FIELD at              ON TABLE prompt TYPE datetime;
DEFINE FIELD kind            ON TABLE prompt TYPE string
    ASSERT $value INSIDE ['typed', 'queued', 'command', 'shell', 'interrupt'];
DEFINE FIELD text            ON TABLE prompt TYPE string;
DEFINE FIELD chars           ON TABLE prompt TYPE int ASSERT $value >= 0;
DEFINE FIELD images          ON TABLE prompt TYPE int ASSERT $value >= 0;
DEFINE FIELD prompt_id       ON TABLE prompt TYPE option<string>;
DEFINE FIELD source          ON TABLE prompt TYPE option<string>;
DEFINE FIELD permission_mode ON TABLE prompt TYPE option<string>;
DEFINE FIELD turn_index      ON TABLE prompt TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD prompt_index    ON TABLE prompt TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD valid_from      ON TABLE prompt TYPE datetime;

DEFINE INDEX idx_prompt_line    ON TABLE prompt FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_prompt_session ON TABLE prompt FIELDS session, at;
DEFINE INDEX idx_prompt_at      ON TABLE prompt FIELDS at;
DEFINE INDEX idx_prompt_id      ON TABLE prompt FIELDS session, prompt_id;


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
--
-- `request` is the request it answered, as Claude Code recorded it
-- (`requestRef`), NONE where none was recorded. `prompt_id` is the prompt
-- whose turn it is in: the newest `promptId` on the session's lines before
-- it. `client_version` is the agent's version on its line. What steered it,
-- as Claude Code names it: the skill (`attributionSkill`), plugin, and MCP
-- server and tool. `refusal` and `refusal_text` are the category and the
-- explanation of a refusal. `cache_miss` is why the prompt cache was missed
-- and `cache_missed` the input tokens that missed it. `thinking_dropped`
-- counts the earlier thinking the request left out.

DEFINE TABLE reply SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session          ON TABLE reply TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD agent_id         ON TABLE reply TYPE string;
DEFINE FIELD request_id       ON TABLE reply TYPE option<string>;
DEFINE FIELD kernel_message   ON TABLE reply TYPE uuid;
DEFINE FIELD at               ON TABLE reply TYPE datetime;
DEFINE FIELD model            ON TABLE reply TYPE string;
DEFINE FIELD requested_model  ON TABLE reply TYPE option<string>;
DEFINE FIELD advisor_model    ON TABLE reply TYPE option<string>;
DEFINE FIELD effort           ON TABLE reply TYPE option<string>;
DEFINE FIELD turn_effort      ON TABLE reply TYPE option<string>;
DEFINE FIELD stop_reason      ON TABLE reply TYPE option<string>;
DEFINE FIELD subagent         ON TABLE reply TYPE bool;
DEFINE FIELD request          ON TABLE reply TYPE option<record<request>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD prompt_id        ON TABLE reply TYPE option<string>;
DEFINE FIELD client_version   ON TABLE reply TYPE option<string>;
DEFINE FIELD skill            ON TABLE reply TYPE option<string>;
DEFINE FIELD plugin           ON TABLE reply TYPE option<string>;
DEFINE FIELD mcp_server       ON TABLE reply TYPE option<string>;
DEFINE FIELD mcp_tool         ON TABLE reply TYPE option<string>;
DEFINE FIELD refusal          ON TABLE reply TYPE option<string>;
DEFINE FIELD refusal_text     ON TABLE reply TYPE option<string>;
DEFINE FIELD cache_miss       ON TABLE reply TYPE option<string>;
DEFINE FIELD cache_missed     ON TABLE reply TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD thinking_dropped ON TABLE reply TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from       ON TABLE reply TYPE datetime;

DEFINE INDEX idx_reply_agent   ON TABLE reply FIELDS session, agent_id UNIQUE;
DEFINE INDEX idx_reply_session ON TABLE reply FIELDS session, at;
DEFINE INDEX idx_reply_at      ON TABLE reply FIELDS at;
DEFINE INDEX idx_reply_model   ON TABLE reply FIELDS model, at;
DEFINE INDEX idx_reply_prompt  ON TABLE reply FIELDS session, prompt_id;
DEFINE INDEX idx_reply_skill   ON TABLE reply FIELDS skill, at;


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
-- kept. `cache_write_1h_tokens` and `cache_write_5m_tokens` split the
-- cache writes by how long they are kept, which prices them differently
-- (Claude).

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
DEFINE FIELD cache_write_1h_tokens ON TABLE reply_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD cache_write_5m_tokens ON TABLE reply_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD usage              ON TABLE reply_state TYPE object FLEXIBLE;
DEFINE FIELD valid_from         ON TABLE reply_state TYPE datetime;

DEFINE INDEX idx_reply_state ON TABLE reply_state FIELDS reply, valid_from;


-- 6 ─── thinking ─────────────────────────────────── one thinking span ───
-- `position` is the span's place in its reply. `ms` is how long the model
-- thought (Claude Code's `thinkingDurationMs`), NONE where the agent
-- records no length (Gemini's thoughts carry only their time). `chars` is
-- the length of the thinking text as recorded, 0 when only its signature
-- is kept. `display` is Claude Code's `thinkingDisplay`. `text` is the
-- thinking as recorded, whole: Claude's summary of it where the display
-- shows one, Gemini's thought; empty when only the signature is kept.
-- `subject` is a Gemini thought's subject. `signature_chars` is the length
-- of the signature Claude keeps in place of the thinking itself.

DEFINE TABLE thinking SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD reply           ON TABLE thinking TYPE record<reply>
    ASSERT record::exists($value);
DEFINE FIELD position        ON TABLE thinking TYPE int ASSERT $value >= 0;
DEFINE FIELD kernel_message  ON TABLE thinking TYPE uuid;
DEFINE FIELD at              ON TABLE thinking TYPE datetime;
DEFINE FIELD ms              ON TABLE thinking TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD chars           ON TABLE thinking TYPE int ASSERT $value >= 0;
DEFINE FIELD display         ON TABLE thinking TYPE option<string>;
DEFINE FIELD text            ON TABLE thinking TYPE string;
DEFINE FIELD subject         ON TABLE thinking TYPE option<string>;
DEFINE FIELD signature_chars ON TABLE thinking TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from      ON TABLE thinking TYPE datetime;

DEFINE INDEX idx_thinking    ON TABLE thinking FIELDS reply, position UNIQUE;
DEFINE INDEX idx_thinking_at ON TABLE thinking FIELDS at;


-- 7 ─── tool_call ───────────────────────────────────── one tool call ───
-- `call_id` is the agent's id for the call. `tool` is its name as the
-- agent gave it (`Bash`, `Edit`, `mcp__<server>__<tool>`, …). What it
-- acted on, when the input names it: `path` (the file tools), `command`
-- (a shell command, whole), `description`, `pattern` (search tools), `url`
-- or `query` (web tools), `subagent_type` (a subagent it started), and
-- `background`, a command started to run on its own. `input_bytes` is the
-- size of the whole input, which stays in the kernel line. `skill` is the
-- skill a skill call loads, `model` the model a subagent call asks for,
-- `sandbox_off` a command asked to run outside the sandbox, `timeout_ms`
-- the time limit the call set.

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
DEFINE FIELD skill          ON TABLE tool_call TYPE option<string>;
DEFINE FIELD model          ON TABLE tool_call TYPE option<string>;
DEFINE FIELD sandbox_off    ON TABLE tool_call TYPE bool;
DEFINE FIELD timeout_ms     ON TABLE tool_call TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
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
--
-- Who let the call run, as Claude Code records it (`permissionDecision`):
-- `decision` (`accept`, `reject`), `decided_by` (`classifier`, `rule`,
-- `mode`, `safetyCheck`, …), `decision_source` (`config`,
-- `user_temporary`, `user_reject`, …) and `decision_reason` (the rule, as
-- `rule:userSettings`). `denial` is how a refused call was refused
-- (`user-rejected`, `automode-blocked`, `permission-rule`, …) and
-- `operator_feedback` what the operator wrote when refusing it, whole.
-- `sandboxed` is whether the command ran in the sandbox and
-- `sandbox_hosts` the hosts the sandbox stopped it reaching.
-- `persisted_bytes` is the whole output's size where Claude Code saved it
-- to a file and showed the model a preview. `return_note` is Claude Code's
-- reading of a command's exit code (`No matches found`). `background_task`
-- is the task a command started in the background. `text` is what the tool
-- returned when it did not end `ok`, whole.

DEFINE TABLE tool_result SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call       ON TABLE tool_result TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message  ON TABLE tool_result TYPE uuid;
DEFINE FIELD at              ON TABLE tool_result TYPE datetime;
DEFINE FIELD outcome         ON TABLE tool_result TYPE string
    ASSERT $value INSIDE ['ok', 'error', 'denied', 'interrupted', 'cancelled', 'timed_out'];
DEFINE FIELD ms              ON TABLE tool_result TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD bytes           ON TABLE tool_result TYPE int ASSERT $value >= 0;
DEFINE FIELD decision        ON TABLE tool_result TYPE option<string>;
DEFINE FIELD decided_by      ON TABLE tool_result TYPE option<string>;
DEFINE FIELD decision_source ON TABLE tool_result TYPE option<string>;
DEFINE FIELD decision_reason ON TABLE tool_result TYPE option<string>;
DEFINE FIELD denial          ON TABLE tool_result TYPE option<string>;
DEFINE FIELD operator_feedback ON TABLE tool_result TYPE option<string>;
DEFINE FIELD sandboxed       ON TABLE tool_result TYPE option<bool>;
DEFINE FIELD sandbox_hosts   ON TABLE tool_result TYPE array<string>;
DEFINE FIELD persisted_bytes ON TABLE tool_result TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD return_note     ON TABLE tool_result TYPE option<string>;
DEFINE FIELD background_task ON TABLE tool_result TYPE option<string>;
DEFINE FIELD text            ON TABLE tool_result TYPE option<string>;
DEFINE FIELD valid_from      ON TABLE tool_result TYPE datetime;

DEFINE INDEX idx_tool_result    ON TABLE tool_result FIELDS tool_call UNIQUE;
DEFINE INDEX idx_tool_result_at ON TABLE tool_result FIELDS at;


-- 9 ─── edit ──────────────────────────── a file a file tool changed ───
-- Written from a successful result, so a call that changed nothing has no
-- row. `kind`: the tool created the file, modified one that existed, or
-- deleted it. `added` and `removed` count lines, `counted_from` says how:
-- the structured patch the agent recorded (`patch`), the old and new text
-- the call carried (`strings`), the whole content it wrote (`content`), or
-- the files Claude Code saw a shell command change, with their hunks
-- (`shell`, its `bashEditDiff`). `operator_changed`: the operator altered
-- the change before accepting it (Claude Code's `userModified`). The
-- repositories count every committed line, however it was written.

DEFINE TABLE edit SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call        ON TABLE edit TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD path             ON TABLE edit TYPE string;
DEFINE FIELD kind             ON TABLE edit TYPE string
    ASSERT $value INSIDE ['created', 'modified', 'deleted'];
DEFINE FIELD added            ON TABLE edit TYPE int ASSERT $value >= 0;
DEFINE FIELD removed          ON TABLE edit TYPE int ASSERT $value >= 0;
DEFINE FIELD counted_from     ON TABLE edit TYPE string
    ASSERT $value INSIDE ['patch', 'strings', 'content', 'shell'];
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
--
-- What git is in the middle of there, as its own state files say (the
-- ones `git status` reads): `operations` names each (`merge`, `am`,
-- `rebase`, `rebase_interactive`, `cherry_pick`, `revert`, `bisect`),
-- empty when none; NONE when they were not read. `operation_commits` are
-- the commits it brings in (`MERGE_HEAD`, `CHERRY_PICK_HEAD`,
-- `REVERT_HEAD`); `rebase_branch` and `rebase_onto` the branch a rebase
-- replays and the commit it replays onto; `rebase_done` and `rebase_left`
-- the commands of an interactive rebase done and left; `bisect_start` what
-- a bisect started from. A checkout an agent left mid-rebase stays on
-- record here.

DEFINE TABLE checkout_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD checkout     ON TABLE checkout_state TYPE record<checkout>
    ASSERT record::exists($value);
DEFINE FIELD branch       ON TABLE checkout_state TYPE option<string>;
DEFINE FIELD first_branch ON TABLE checkout_state TYPE option<string>;
DEFINE FIELD present      ON TABLE checkout_state TYPE bool;
DEFINE FIELD operations   ON TABLE checkout_state TYPE option<array<string>>
    ASSERT $value == NONE OR array::all($value, |$o| $o INSIDE ['merge', 'am', 'rebase', 'rebase_interactive', 'cherry_pick', 'revert', 'bisect']);
DEFINE FIELD operation_commits ON TABLE checkout_state TYPE option<array<string>>
    ASSERT $value == NONE OR array::all($value, |$h| string::len($h) INSIDE [40, 64]);
DEFINE FIELD rebase_branch ON TABLE checkout_state TYPE option<string>;
DEFINE FIELD rebase_onto   ON TABLE checkout_state TYPE option<string>
    ASSERT $value == NONE OR string::len($value) INSIDE [40, 64];
DEFINE FIELD rebase_done   ON TABLE checkout_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD rebase_left   ON TABLE checkout_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD bisect_start  ON TABLE checkout_state TYPE option<string>;
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
-- `author_name` and `committer_name` are the names beside those emails,
-- `parent_hashes` the parents themselves, first parent first, and `tree`
-- the tree it records: the history's shape and content stay readable here
-- when git has pruned a commit.
--
-- `author_offset` and `committer_offset` are the UTC offsets git recorded
-- beside each time, in minutes east of UTC (`-0400` is -240): the hour of
-- the day the work was done where it was done. `patch_id` is git's stable
-- patch id of the commit's change against its first parent (`git
-- patch-id --stable`): commits with the same one make the same change, so
-- a rebased or cherry-picked copy shares its original's; NONE for a merge
-- and for a commit that changes nothing. `signature` is the signature the
-- commit object carries (its `gpgsig` header), whole, NONE when unsigned:
-- kept, never verified, since verifying runs a program.

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
DEFINE FIELD author_name    ON TABLE commit TYPE string;
DEFINE FIELD committer_name ON TABLE commit TYPE string;
DEFINE FIELD parent_hashes  ON TABLE commit TYPE array<string>
    ASSERT array::all($value, |$h| string::len($h) INSIDE [40, 64]);
DEFINE FIELD tree           ON TABLE commit TYPE string
    ASSERT string::len($value) INSIDE [40, 64];
DEFINE FIELD author_offset    ON TABLE commit TYPE int;
DEFINE FIELD committer_offset ON TABLE commit TYPE int;
DEFINE FIELD patch_id       ON TABLE commit TYPE option<string>
    ASSERT $value == NONE OR string::len($value) INSIDE [40, 64];
DEFINE FIELD signature      ON TABLE commit TYPE option<string>;
DEFINE FIELD valid_from   ON TABLE commit TYPE datetime;

DEFINE INDEX idx_commit_hash   ON TABLE commit FIELDS repository, hash UNIQUE;
DEFINE INDEX idx_commit_author ON TABLE commit FIELDS repository, authored_at;
DEFINE INDEX idx_commit_patch  ON TABLE commit FIELDS patch_id;


-- 22 ── commit_file ─────────────────────── one file a commit changed ───
-- Renames read as a delete and an add, so every path is one that exists
-- in some tree. `change` is how the commit changed the file: it created
-- it, modified it, deleted it, or changed its type (a file became a link,
-- or back). `binary` is a file git counted no lines in (a binary, or
-- anything `.gitattributes` marks `-diff`); its counts are zero.
-- `added_ignoring_space` and `removed_ignoring_space` count the same
-- change with whitespace ignored (`--ignore-all-space`): lines changed
-- only in their spacing count in `added` and `removed` and not here.

DEFINE TABLE commit_file SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE commit_file TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE commit_file TYPE string;
DEFINE FIELD change     ON TABLE commit_file TYPE string
    ASSERT $value INSIDE ['added', 'modified', 'deleted', 'type_changed'];
DEFINE FIELD added      ON TABLE commit_file TYPE int;
DEFINE FIELD removed    ON TABLE commit_file TYPE int;
DEFINE FIELD added_ignoring_space   ON TABLE commit_file TYPE int ASSERT $value >= 0;
DEFINE FIELD removed_ignoring_space ON TABLE commit_file TYPE int ASSERT $value >= 0;
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
-- paths `attr_recon_survival_skip` names, are never blamed.

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
-- (`attr_recon_mainline_refs`), the default branch the repository's host
-- names (`host_default`), what `origin/HEAD` points at, a local `main` or
-- `master`, or `HEAD`. `identity` is the email this machine
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
-- at `attr_recon_git_timeout_ms`, time spent waiting on git, rows written.
-- A crawl is written once its reads are done, before the rows that point
-- to it.

DEFINE TABLE crawl SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository    ON TABLE crawl TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD started_at    ON TABLE crawl TYPE datetime;
DEFINE FIELD finished_at   ON TABLE crawl TYPE datetime;
DEFINE FIELD mainline      ON TABLE crawl TYPE string;
DEFINE FIELD mainline_from ON TABLE crawl TYPE string
    ASSERT $value INSIDE ['override', 'host_default', 'origin_head', 'local_branch', 'head'];
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
-- `mirror` names the crawler's own clone the ref is in, the host's branch
-- as the host had it; NONE for a ref of the repository on this machine.
-- `upstream` is the ref a local branch tracks (`%(upstream)`), and
-- `upstream_ahead` and `upstream_behind` how many commits the branch has
-- that it lacks and lacks that it has: work not pushed, and work not
-- pulled. `mainline_ahead` and `mainline_behind` are the same against the
-- crawl's main line (`%(ahead-behind:<main line>)`), for every branch. A
-- row is appended when the tip, the upstream or a count changes.

DEFINE TABLE ref_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository ON TABLE ref_state TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE ref_state TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD mirror     ON TABLE ref_state TYPE option<record<mirror>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD name       ON TABLE ref_state TYPE string;
DEFINE FIELD tip        ON TABLE ref_state TYPE option<string>
    ASSERT $value == NONE OR string::len($value) INSIDE [40, 64];
DEFINE FIELD rewritten  ON TABLE ref_state TYPE bool;
DEFINE FIELD upstream        ON TABLE ref_state TYPE option<string>;
DEFINE FIELD upstream_ahead  ON TABLE ref_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD upstream_behind ON TABLE ref_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD mainline_ahead  ON TABLE ref_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD mainline_behind ON TABLE ref_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD valid_from ON TABLE ref_state TYPE datetime;

DEFINE INDEX idx_ref_state ON TABLE ref_state FIELDS repository, mirror, name, valid_from;


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
-- calls, what it read and what it wrote. `setup` reads the remotes, the
-- hooks and whether the clone is shallow; `tip` reads every file at the
-- main line's tip; `fetch` fetches the repository's mirror; `hosts` reads
-- its host; `tags` reads its annotated tags; `merges` reads how its merges
-- were resolved; `patches` reads its commits' patch ids.

DEFINE TABLE crawl_step SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD crawl        ON TABLE crawl_step TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD step         ON TABLE crawl_step TYPE string
    ASSERT $value INSIDE ['setup', 'fetch', 'checkouts', 'refs', 'log', 'reflog', 'unreachable', 'placement', 'landed', 'blame', 'tip', 'tags', 'merges', 'patches', 'hosts', 'write'];
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
-- `attr_recon_git_timeout_ms`), how much it returned, and what git wrote
-- to its error output, whole.

DEFINE TABLE git_call SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD crawl      ON TABLE git_call TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD step       ON TABLE git_call TYPE string
    ASSERT $value INSIDE ['setup', 'fetch', 'checkouts', 'refs', 'log', 'reflog', 'unreachable', 'placement', 'landed', 'blame', 'tip', 'tags', 'merges', 'patches'];
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
-- A source is the part of the crawler that crawls one kind of thing:
-- `lines`, the lines the kernel captured; `repos`, the git repositories
-- the lines lead to, and their mirrors; `hosts`, the hosts those
-- repositories live on. A source reads what the others wrote here: `repos`
-- follows the sessions' working directories and the commits and pushes
-- they printed; `hosts` follows the repositories' remotes. A source added
-- later adds its name to this list and its own tables of facts, by a
-- schema change, and needs no change to the pass record.
--
-- `items` counts what it worked through (lines, repositories), `unread`
-- those in a shape it does not know, so nothing goes unaccounted. `calls`
-- counts the calls it made outside the database (git, a host's API),
-- `failed` and `timed_out` those that did not end well. `rows` is the
-- rows it added, by table; `counts` what else it counts, by name (for
-- `repos`: directories followed and new, repositories and checkouts new,
-- stale, gone and back, those excluded, and the commits git holds against
-- those read; for `lines`: the lines the kernel holds in the range read
-- against those read, the lines no table used, and the fields seen, new
-- and not stored). A source checks what it read against what its source
-- holds, every pass, and a pass that finds them apart is `partial`, with
-- the difference in `note`. `from_id` and `to_id` are
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
    ASSERT $value INSIDE ['lines', 'repos', 'hosts'];
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


-- 34 ── trigger ─────── a line that set an agent working without the operator ───
-- Claude Code marks who wrote each line that starts a turn (`origin.kind`):
-- a task notification (a background command, a monitor or a subagent
-- reporting back), a peer (another session or agent writing to this one),
-- an automatic continuation. These are not prompts; each is one row here.
-- `kind` is the agent's own word for it, `source` who sent it as the line
-- names it (`origin.from`, `origin.producer`), `prompt_id` the turn it
-- started. `text` is what it said, whole.

DEFINE TABLE trigger SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE trigger TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE trigger TYPE uuid;
DEFINE FIELD at             ON TABLE trigger TYPE datetime;
DEFINE FIELD kind           ON TABLE trigger TYPE string;
DEFINE FIELD source         ON TABLE trigger TYPE option<string>;
DEFINE FIELD prompt_id      ON TABLE trigger TYPE option<string>;
DEFINE FIELD text           ON TABLE trigger TYPE string;
DEFINE FIELD chars          ON TABLE trigger TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE trigger TYPE datetime;

DEFINE INDEX idx_trigger_line    ON TABLE trigger FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_trigger_session ON TABLE trigger FIELDS session, at;


-- 35 ── said ───────────────────────── what a model wrote to the operator ───
-- One row per block of text a model wrote in a reply, whole, as it was
-- read. Claude Code writes each block once, on its own line; Gemini writes
-- a reply again as it grows, and each longer text is a new row, the newest
-- being the text. `position` is the block's place in the reply.

DEFINE TABLE said SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD reply          ON TABLE said TYPE record<reply>
    ASSERT record::exists($value);
DEFINE FIELD position       ON TABLE said TYPE int ASSERT $value >= 0;
DEFINE FIELD kernel_message ON TABLE said TYPE uuid;
DEFINE FIELD at             ON TABLE said TYPE datetime;
DEFINE FIELD text           ON TABLE said TYPE string;
DEFINE FIELD chars          ON TABLE said TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE said TYPE datetime;

DEFINE INDEX idx_said    ON TABLE said FIELDS reply, position, valid_from;
DEFINE INDEX idx_said_at ON TABLE said FIELDS at;


-- 36 ── line ─────────────────────────────── every line the crawler read ───
-- One row per agent line the crawler read: the kernel's conversation rows,
-- the lines it keeps as telemetry (`transcript_event`, kept whole since
-- 2026-10-21 and by their kind only before), and Claude Desktop's log
-- lines. `kind` is the line's type as the agent wrote it, with its subtype
-- or attachment type (`assistant`, `user`, `system/turn_duration`,
-- `attachment/queued_command`, `api-request`, `gemini`, …). `read_as` is
-- the tables the crawler wrote from it, empty when it wrote nothing: a kind
-- of line read and never used is one query away.
--
-- `agent_uuid` and `parent_uuid` are the agent's own ids for the line and
-- for the one it follows (Claude Code's `uuid`, and `parentUuid`, or
-- `logicalParentUuid` across a compaction): a line with siblings is where
-- the conversation was rewound and taken another way. `prompt_id` is the
-- prompt whose turn the line is in, where the line names it. `sidechain`:
-- a subagent's line. `client_version` is the agent's version on the line,
-- `bytes` the line's size as captured.

DEFINE TABLE line SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE line TYPE option<record<session>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD kernel_message ON TABLE line TYPE uuid;
DEFINE FIELD at             ON TABLE line TYPE datetime;
DEFINE FIELD agent          ON TABLE line TYPE string;
DEFINE FIELD kind           ON TABLE line TYPE string;
DEFINE FIELD agent_uuid     ON TABLE line TYPE option<string>;
DEFINE FIELD parent_uuid    ON TABLE line TYPE option<string>;
DEFINE FIELD prompt_id      ON TABLE line TYPE option<string>;
DEFINE FIELD sidechain      ON TABLE line TYPE bool;
DEFINE FIELD client_version ON TABLE line TYPE option<string>;
DEFINE FIELD bytes          ON TABLE line TYPE int ASSERT $value >= 0;
DEFINE FIELD read_as        ON TABLE line TYPE array<string>
    ASSERT array::all($value, |$t| $t INSIDE ['session', 'session_place', 'prompt', 'trigger', 'said', 'reply', 'reply_state', 'thinking', 'tool_call', 'tool_result', 'edit', 'git_event', 'test_run', 'build_run', 'turn', 'compaction', 'away', 'request', 'request_part', 'request_shape', 'context', 'hook_run', 'session_state', 'session_cost', 'model_cost', 'session_link', 'git_state', 'notice', 'question', 'task_event', 'plan', 'subagent_run', 'queue_event']);
DEFINE FIELD valid_from     ON TABLE line TYPE datetime;

DEFINE INDEX idx_line         ON TABLE line FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_line_session ON TABLE line FIELDS session, at;
DEFINE INDEX idx_line_kind    ON TABLE line FIELDS kind, at;
DEFINE INDEX idx_line_parent  ON TABLE line FIELDS session, parent_uuid;


-- 37 ── line_field ───────────────────── every field the agents write ───
-- One row the first time the crawler sees a field on a kind of line, and
-- one each time whether the crawler stores it changes: a field an agent
-- adds in an update is a row on the day it appears. `path` is where the
-- field sits in the line (`message.usage.output_tokens`), with a content
-- block's type and a tool's name in brackets
-- (`message.content[tool_use:Bash].input.command`), and a key that is data
-- rather than a name (a file path, an id, a question) written `<k>`.
-- `stored` is whether the crawler writes it to a column. `first_line` is
-- the line it was first seen on.

DEFINE TABLE line_field SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD agent      ON TABLE line_field TYPE string;
DEFINE FIELD kind       ON TABLE line_field TYPE string;
DEFINE FIELD path       ON TABLE line_field TYPE string;
DEFINE FIELD first_at   ON TABLE line_field TYPE datetime;
DEFINE FIELD first_line ON TABLE line_field TYPE uuid;
DEFINE FIELD stored     ON TABLE line_field TYPE bool;
DEFINE FIELD valid_from ON TABLE line_field TYPE datetime;

DEFINE INDEX idx_line_field ON TABLE line_field FIELDS agent, kind, path, valid_from;


-- 38 ── request ────────────────────────── one request Claude Code sent ───
-- From Claude Code's `api-request` line, written as it sends a request to a
-- model: the model asked for, the thinking effort, the output limit, the
-- kind of thinking and how it is shown, the API features turned on
-- (`betas`), how the context is trimmed (`context_edits`), which part of
-- Claude Code sent it (`query_source`), the cache breakpoints it set, how
-- many of the conversation's messages it kept (`keep`), and the pieces it
-- sent last (`tail`, the hashes of `request_part` rows). `request_key` is
-- the line's own id, which the replies name. `shape_hash` names the system
-- prompt and tools it was sent with (`request_shape`), `digest` the whole
-- request. `params` is every request parameter as recorded, so one Claude
-- Code adds later is kept.

DEFINE TABLE request SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session           ON TABLE request TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message    ON TABLE request TYPE uuid;
DEFINE FIELD at                ON TABLE request TYPE datetime;
DEFINE FIELD request_key       ON TABLE request TYPE option<string>;
DEFINE FIELD model             ON TABLE request TYPE option<string>;
DEFINE FIELD effort            ON TABLE request TYPE option<string>;
DEFINE FIELD max_tokens        ON TABLE request TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD thinking          ON TABLE request TYPE option<string>;
DEFINE FIELD thinking_display  ON TABLE request TYPE option<string>;
DEFINE FIELD betas             ON TABLE request TYPE array<string>;
DEFINE FIELD context_edits     ON TABLE request TYPE array<string>;
DEFINE FIELD query_source      ON TABLE request TYPE option<string>;
DEFINE FIELD cache_breakpoints ON TABLE request TYPE int ASSERT $value >= 0;
DEFINE FIELD keep              ON TABLE request TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD tail              ON TABLE request TYPE array<string>;
DEFINE FIELD shape_hash        ON TABLE request TYPE option<string>;
DEFINE FIELD digest            ON TABLE request TYPE option<string>;
DEFINE FIELD params            ON TABLE request TYPE object FLEXIBLE;
DEFINE FIELD valid_from        ON TABLE request TYPE datetime;

DEFINE INDEX idx_request_line    ON TABLE request FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_request_key     ON TABLE request FIELDS request_key;
DEFINE INDEX idx_request_session ON TABLE request FIELDS session, at;
DEFINE INDEX idx_request_model   ON TABLE request FIELDS model, at;


-- 39 ── request_part ───────────────── one piece of what Claude Code sent ───
-- From Claude Code's `api-request-blob` lines: each distinct piece of a
-- request (a part of the system prompt, a message) once, by its hash, when
-- it was first sent. `role` is whose words it is (`system`, `user`,
-- `assistant`), `blocks` the kinds of content in it, `chars` its length.
-- Its text stays in its kernel line. A system prompt that changes is a new
-- hash on the day it changed.

DEFINE TABLE request_part SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD hash           ON TABLE request_part TYPE string;
DEFINE FIELD session        ON TABLE request_part TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE request_part TYPE uuid;
DEFINE FIELD at             ON TABLE request_part TYPE datetime;
DEFINE FIELD role           ON TABLE request_part TYPE option<string>;
DEFINE FIELD blocks         ON TABLE request_part TYPE array<string>;
DEFINE FIELD chars          ON TABLE request_part TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE request_part TYPE datetime;

DEFINE INDEX idx_request_part    ON TABLE request_part FIELDS hash UNIQUE;
DEFINE INDEX idx_request_part_at ON TABLE request_part FIELDS role, at;


-- 40 ── request_shape ─────────── the system prompt and tools a request had ───
-- From Claude Code's `api-request-shape` lines: per shape, once, the parts
-- of the system prompt and their length, and the tools the model was
-- given, by name. Their text stays in the kernel line.

DEFINE TABLE request_shape SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD shape_hash     ON TABLE request_shape TYPE string;
DEFINE FIELD session        ON TABLE request_shape TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE request_shape TYPE uuid;
DEFINE FIELD at             ON TABLE request_shape TYPE datetime;
DEFINE FIELD system_parts   ON TABLE request_shape TYPE int ASSERT $value >= 0;
DEFINE FIELD system_chars   ON TABLE request_shape TYPE int ASSERT $value >= 0;
DEFINE FIELD tools          ON TABLE request_shape TYPE array<string>;
DEFINE FIELD valid_from     ON TABLE request_shape TYPE datetime;

DEFINE INDEX idx_request_shape ON TABLE request_shape FIELDS shape_hash UNIQUE;


-- 41 ── context ───────────── what was put before the model, or told it ───
-- From Claude Code's `attachment` lines other than queued messages (those
-- are prompts and triggers): instruction files (CLAUDE.md, memory), the
-- skills listed and loaded, MCP servers and deferred tools added or lost,
-- hook output, the environment and git status, the date, files attached or
-- changed outside the agent, reminders of the tokens left, thinking left
-- out of a request, and the rest. `kind` is the attachment's own type, as
-- written. `names` is what it names (file paths, skills, servers, tools,
-- models), `removed` and `failed` what it says went away or failed to
-- start. `chars` is the text it put in the model's context. `changed` and
-- `reason` are its own (`compaction`). `tokens_left` is the context left
-- where it says.

DEFINE TABLE context SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE context TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE context TYPE uuid;
DEFINE FIELD at             ON TABLE context TYPE datetime;
DEFINE FIELD kind           ON TABLE context TYPE string;
DEFINE FIELD names          ON TABLE context TYPE array<string>;
DEFINE FIELD removed        ON TABLE context TYPE array<string>;
DEFINE FIELD failed         ON TABLE context TYPE array<string>;
DEFINE FIELD chars          ON TABLE context TYPE int ASSERT $value >= 0;
DEFINE FIELD changed        ON TABLE context TYPE option<bool>;
DEFINE FIELD reason         ON TABLE context TYPE option<string>;
DEFINE FIELD tokens_left    ON TABLE context TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD valid_from     ON TABLE context TYPE datetime;

DEFINE INDEX idx_context_line    ON TABLE context FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_context_session ON TABLE context FIELDS session, at;
DEFINE INDEX idx_context_kind    ON TABLE context FIELDS kind, at;


-- 42 ── hook_run ─────────────────────────── one hook a session ran ───
-- From Claude Code's `stop_hook_summary` lines (each hook a stop ran) and
-- `hook_additional_context` attachments (a hook adding to the context).
-- `position` is the hook's place in its line. `event` is the hook's event
-- (`Stop`, `UserPromptSubmit`, …), `command` what ran, `ms` how long it
-- took, `errors` the errors it reported, `prevented` whether it stopped the
-- agent carrying on, `chars` what it added to the context.

DEFINE TABLE hook_run SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE hook_run TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE hook_run TYPE uuid;
DEFINE FIELD position       ON TABLE hook_run TYPE int ASSERT $value >= 0;
DEFINE FIELD at             ON TABLE hook_run TYPE datetime;
DEFINE FIELD event          ON TABLE hook_run TYPE string;
DEFINE FIELD command        ON TABLE hook_run TYPE option<string>;
DEFINE FIELD ms             ON TABLE hook_run TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD errors         ON TABLE hook_run TYPE int ASSERT $value >= 0;
DEFINE FIELD prevented      ON TABLE hook_run TYPE bool;
DEFINE FIELD chars          ON TABLE hook_run TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE hook_run TYPE datetime;

DEFINE INDEX idx_hook_run         ON TABLE hook_run FIELDS kernel_message, position UNIQUE;
DEFINE INDEX idx_hook_run_session ON TABLE hook_run FIELDS session, at;


-- 43 ── session_state ─────── a session's title and modes, as last recorded ───
-- From Claude Code's `ai-title`, `permission-mode` and `mode` lines, which it
-- writes again each time a session resumes: one row when one of them
-- changes. `title` is the session's title as Claude Code wrote it,
-- `permission_mode` how tool calls were approved (`default`,
-- `acceptEdits`, `plan`, `auto`, …), `mode` its input mode, as written.

DEFINE TABLE session_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session         ON TABLE session_state TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message  ON TABLE session_state TYPE uuid;
DEFINE FIELD at              ON TABLE session_state TYPE datetime;
DEFINE FIELD title           ON TABLE session_state TYPE option<string>;
DEFINE FIELD permission_mode ON TABLE session_state TYPE option<string>;
DEFINE FIELD mode            ON TABLE session_state TYPE option<string>;
DEFINE FIELD valid_from      ON TABLE session_state TYPE datetime;

DEFINE INDEX idx_session_state ON TABLE session_state FIELDS session, valid_from;


-- 44 ── session_cost ────────────────── what a session cost, as last recorded ───
-- From Claude Code's `cost-state` lines: the session's cost in US dollars as
-- Claude Code prices it, the time spent waiting on the API (with and
-- without retries), running tools, and in all, and the lines Claude Code
-- counts as added and removed. `unknown_cost`: a model it cannot price was
-- used. One row when the figures change.

DEFINE TABLE session_cost SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session           ON TABLE session_cost TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message    ON TABLE session_cost TYPE uuid;
DEFINE FIELD at                ON TABLE session_cost TYPE datetime;
DEFINE FIELD usd               ON TABLE session_cost TYPE number ASSERT $value >= 0;
DEFINE FIELD api_ms            ON TABLE session_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD api_ms_no_retries ON TABLE session_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD tool_ms           ON TABLE session_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD wall_ms           ON TABLE session_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD lines_added       ON TABLE session_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD lines_removed     ON TABLE session_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD unknown_cost      ON TABLE session_cost TYPE bool;
DEFINE FIELD valid_from        ON TABLE session_cost TYPE datetime;

DEFINE INDEX idx_session_cost ON TABLE session_cost FIELDS session, valid_from;


-- 45 ── model_cost ──────────────────────────── the same, per model ───
-- What one model cost in that session, as the same line has it: dollars,
-- input, output, cache reads and writes, thinking, web searches.

DEFINE TABLE model_cost SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session_cost ON TABLE model_cost TYPE record<session_cost>
    ASSERT record::exists($value);
DEFINE FIELD model        ON TABLE model_cost TYPE string;
DEFINE FIELD usd          ON TABLE model_cost TYPE number ASSERT $value >= 0;
DEFINE FIELD input        ON TABLE model_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD output       ON TABLE model_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD cache_read   ON TABLE model_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD cache_write  ON TABLE model_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD thinking     ON TABLE model_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD web_searches ON TABLE model_cost TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from   ON TABLE model_cost TYPE datetime;

DEFINE INDEX idx_model_cost ON TABLE model_cost FIELDS session_cost, model UNIQUE;


-- 46 ── session_link ──────────────────────── what a session links to ───
-- `pr`: a pull request (Claude Code's `pr-link`); `artifact`: a page it
-- published (`frame-link`); `issue`: an issue it created or changed in a
-- tracker, from the tracker tool's own call and result. `number` is a pull
-- request's number, `key` an issue's key (`ENG-8074`), `repository` the
-- host's owner/name, `title` as recorded. `first_at` is the first line that
-- names it.

DEFINE TABLE session_link SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE session_link TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kind           ON TABLE session_link TYPE string
    ASSERT $value INSIDE ['pr', 'artifact', 'issue'];
DEFINE FIELD url            ON TABLE session_link TYPE string;
DEFINE FIELD number         ON TABLE session_link TYPE option<int>
    ASSERT $value == NONE OR $value > 0;
DEFINE FIELD key            ON TABLE session_link TYPE option<string>;
DEFINE FIELD repository     ON TABLE session_link TYPE option<string>;
DEFINE FIELD title          ON TABLE session_link TYPE option<string>;
DEFINE FIELD first_at       ON TABLE session_link TYPE datetime;
DEFINE FIELD kernel_message ON TABLE session_link TYPE uuid;
DEFINE FIELD valid_from     ON TABLE session_link TYPE datetime;

DEFINE INDEX idx_session_link     ON TABLE session_link FIELDS session, kind, url UNIQUE;
DEFINE INDEX idx_session_link_url ON TABLE session_link FIELDS kind, url;


-- 47 ── git_state ───────── the git state Claude Code saw where a session works ───
-- Claude Code reads the repository a session is in before it judges a tool
-- call (`serverClassifierContext.context.git_state` on the call's result):
-- the repository's root, the branch and the default branch, whether the
-- working tree is clean, how many files are modified, staged and
-- untracked, and the remote it pushes to (`remote_host`, `remote` as the
-- host's owner/name, its `visibility`). One row each time that differs
-- from the session's newest: the logs' own record of uncommitted work, as
-- it changes, before any crawl reads the repository.

DEFINE TABLE git_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE git_state TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE git_state TYPE uuid;
DEFINE FIELD at             ON TABLE git_state TYPE datetime;
DEFINE FIELD cwd            ON TABLE git_state TYPE string;
DEFINE FIELD root           ON TABLE git_state TYPE option<string>;
DEFINE FIELD branch         ON TABLE git_state TYPE option<string>;
DEFINE FIELD default_branch ON TABLE git_state TYPE option<string>;
DEFINE FIELD clean          ON TABLE git_state TYPE option<bool>;
DEFINE FIELD modified       ON TABLE git_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD staged         ON TABLE git_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD untracked      ON TABLE git_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD remote_host    ON TABLE git_state TYPE option<string>;
DEFINE FIELD remote         ON TABLE git_state TYPE option<string>;
DEFINE FIELD visibility     ON TABLE git_state TYPE option<string>;
DEFINE FIELD valid_from     ON TABLE git_state TYPE datetime;

DEFINE INDEX idx_git_state      ON TABLE git_state FIELDS session, valid_from;
DEFINE INDEX idx_git_state_root ON TABLE git_state FIELDS root, at;


-- 48 ── notice ───────────── what the client said in place of a model reply ───
-- Claude Code writes these itself: an API error, a rate or spend limit, a
-- stream cut off, a refusal with no fallback model, an informational line,
-- a local command's output. `kind` is the agent's own word for it
-- (`rate_limit`, `server_error`, `authentication_failed`,
-- `model_refusal_no_fallback`, `informational`, `local_command`, …),
-- `status` the HTTP status, `model` the model it concerns, `text` what it
-- said, whole. `limit` and `resets_at` are a rate limit's window
-- (`five_hour`, `seven_day`) and when it resets.

DEFINE TABLE notice SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE notice TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE notice TYPE uuid;
DEFINE FIELD at             ON TABLE notice TYPE datetime;
DEFINE FIELD kind           ON TABLE notice TYPE string;
DEFINE FIELD status         ON TABLE notice TYPE option<int>;
DEFINE FIELD model          ON TABLE notice TYPE option<string>;
DEFINE FIELD text           ON TABLE notice TYPE option<string>;
DEFINE FIELD limit          ON TABLE notice TYPE option<string>;
DEFINE FIELD resets_at      ON TABLE notice TYPE option<datetime>;
DEFINE FIELD valid_from     ON TABLE notice TYPE datetime;

DEFINE INDEX idx_notice_line ON TABLE notice FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_notice_kind ON TABLE notice FIELDS kind, at;


-- 49 ── question ──────────────── a question an agent asked the operator ───
-- From Claude Code's `AskUserQuestion` and Gemini's `ask_user` calls: each
-- question, its header, its options, whether several could be picked, and
-- the operator's answer and note, whole; NONE when it went unanswered.
-- `position` is its place in the call.

DEFINE TABLE question SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call  ON TABLE question TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD position   ON TABLE question TYPE int ASSERT $value >= 0;
DEFINE FIELD at         ON TABLE question TYPE datetime;
DEFINE FIELD header     ON TABLE question TYPE option<string>;
DEFINE FIELD question   ON TABLE question TYPE string;
DEFINE FIELD multi      ON TABLE question TYPE bool;
DEFINE FIELD options    ON TABLE question TYPE array<string>;
DEFINE FIELD answer     ON TABLE question TYPE option<string>;
DEFINE FIELD note       ON TABLE question TYPE option<string>;
DEFINE FIELD valid_from ON TABLE question TYPE datetime;

DEFINE INDEX idx_question    ON TABLE question FIELDS tool_call, position UNIQUE;
DEFINE INDEX idx_question_at ON TABLE question FIELDS at;


-- 50 ── task_event ─────────── a change an agent made to its own task list ───
-- Claude Code's `TaskCreate` and `TaskUpdate`, Gemini's `update_topic`: the
-- agent's own plan, as it went. `action`: `created`, `updated`, or `topic`
-- (Gemini naming what it is working on). `task_key` is the agent's id for
-- the task, `subject` and `status` as given, `text` the description or the
-- topic's summary and intent, whole.

DEFINE TABLE task_event SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call  ON TABLE task_event TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD at         ON TABLE task_event TYPE datetime;
DEFINE FIELD action     ON TABLE task_event TYPE string
    ASSERT $value INSIDE ['created', 'updated', 'topic'];
DEFINE FIELD task_key   ON TABLE task_event TYPE option<string>;
DEFINE FIELD subject    ON TABLE task_event TYPE option<string>;
DEFINE FIELD status     ON TABLE task_event TYPE option<string>;
DEFINE FIELD text       ON TABLE task_event TYPE option<string>;
DEFINE FIELD valid_from ON TABLE task_event TYPE datetime;

DEFINE INDEX idx_task_event    ON TABLE task_event FIELDS tool_call UNIQUE;
DEFINE INDEX idx_task_event_at ON TABLE task_event FIELDS at;


-- 51 ── plan ──────────────── a plan an agent put to the operator ───
-- From Claude Code's `ExitPlanMode`: the plan, whole, the file it was kept
-- in, and whether the operator edited it before it went ahead. Whether it
-- was accepted is its call's result.

DEFINE TABLE plan SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call  ON TABLE plan TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD at         ON TABLE plan TYPE datetime;
DEFINE FIELD text       ON TABLE plan TYPE string;
DEFINE FIELD chars      ON TABLE plan TYPE int ASSERT $value >= 0;
DEFINE FIELD file       ON TABLE plan TYPE option<string>;
DEFINE FIELD edited     ON TABLE plan TYPE bool;
DEFINE FIELD valid_from ON TABLE plan TYPE datetime;

DEFINE INDEX idx_plan ON TABLE plan FIELDS tool_call UNIQUE;


-- 52 ── subagent_run ────────────────────── a subagent an agent started ───
-- From the result of Claude Code's `Agent` call: the subagent's id and
-- type, the model asked for and the one that ran, its status as Claude
-- Code gives it (`completed`, `async_launched`, …), whether it ran in the
-- background, and, for one that finished, how long it ran, its tokens, its
-- tool calls, its tool counts and its usage, whole. The subagent's own
-- lines are in a transcript of its own.

DEFINE TABLE subagent_run SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD tool_call       ON TABLE subagent_run TYPE record<tool_call>
    ASSERT record::exists($value);
DEFINE FIELD at              ON TABLE subagent_run TYPE datetime;
DEFINE FIELD agent_key       ON TABLE subagent_run TYPE option<string>;
DEFINE FIELD agent_type      ON TABLE subagent_run TYPE option<string>;
DEFINE FIELD requested_model ON TABLE subagent_run TYPE option<string>;
DEFINE FIELD model           ON TABLE subagent_run TYPE option<string>;
DEFINE FIELD status          ON TABLE subagent_run TYPE option<string>;
DEFINE FIELD background      ON TABLE subagent_run TYPE bool;
DEFINE FIELD ms              ON TABLE subagent_run TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD tokens          ON TABLE subagent_run TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD tool_calls      ON TABLE subagent_run TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD tool_stats      ON TABLE subagent_run TYPE option<object> FLEXIBLE;
DEFINE FIELD usage           ON TABLE subagent_run TYPE option<object> FLEXIBLE;
DEFINE FIELD valid_from      ON TABLE subagent_run TYPE datetime;

DEFINE INDEX idx_subagent_run    ON TABLE subagent_run FIELDS tool_call UNIQUE;
DEFINE INDEX idx_subagent_run_at ON TABLE subagent_run FIELDS at;


-- 53 ── queue_event ───────── a change to the messages waiting for the agent ───
-- From Claude Code's `queue-operation` lines: a message the operator typed
-- while the agent worked was queued, taken up, absorbed into the running
-- turn, or the queue was emptied. `operation` and `reason` as written
-- (`enqueue`, `dequeue`, `remove`, `popAll`; `absorbed_mid_turn`), `chars`
-- the message's length.

DEFINE TABLE queue_event SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD session        ON TABLE queue_event TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD kernel_message ON TABLE queue_event TYPE uuid;
DEFINE FIELD at             ON TABLE queue_event TYPE datetime;
DEFINE FIELD operation      ON TABLE queue_event TYPE string;
DEFINE FIELD reason         ON TABLE queue_event TYPE option<string>;
DEFINE FIELD chars          ON TABLE queue_event TYPE int ASSERT $value >= 0;
DEFINE FIELD valid_from     ON TABLE queue_event TYPE datetime;

DEFINE INDEX idx_queue_event      ON TABLE queue_event FIELDS kernel_message UNIQUE;
DEFINE INDEX idx_queue_event_sess ON TABLE queue_event FIELDS session, at;


-- 54 ── remote_state ─────────────────────── where a repository is hosted ───
-- One row per change in one of a repository's remotes: its name, the URL
-- it fetches from and pushes to, the host and the path the host knows it by
-- (`k8nstantin/superx`). The tie between a repository on this machine and
-- its pull requests on GitHub or Bitbucket, and the sessions' `git_state`
-- and `session_link` rows. `url` is NONE once the remote is gone.

DEFINE TABLE remote_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository ON TABLE remote_state TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE remote_state TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD name       ON TABLE remote_state TYPE string;
DEFINE FIELD url        ON TABLE remote_state TYPE option<string>;
DEFINE FIELD push_url   ON TABLE remote_state TYPE option<string>;
DEFINE FIELD host       ON TABLE remote_state TYPE option<string>;
DEFINE FIELD path       ON TABLE remote_state TYPE option<string>;
DEFINE FIELD valid_from ON TABLE remote_state TYPE datetime;

DEFINE INDEX idx_remote_state      ON TABLE remote_state FIELDS repository, name, valid_from;
DEFINE INDEX idx_remote_state_path ON TABLE remote_state FIELDS host, path;


-- 55 ── ref_move ───────────────────────── one entry of any ref's reflog ───
-- Every reflog entry git keeps: each branch's, each remote-tracking ref's,
-- the stash's, and each checkout's HEAD (`checkout` names it; NONE for a
-- ref the checkouts share). `old` and `new` are the commits it moved
-- between, NONE for none; `identity` the email git recorded; `action` the
-- entry's message, whole (`commit (amend): …`, `rebase (finish): …`,
-- `reset: moving to HEAD~1`, `checkout: moving from main to …`): what was
-- done to the branch, and when. Kept after git expires the entry. `mirror`
-- names the crawler's own clone the entry is in: every move a fetch saw the
-- host's ref make.

DEFINE TABLE ref_move SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository ON TABLE ref_move TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD checkout   ON TABLE ref_move TYPE option<record<checkout>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD mirror     ON TABLE ref_move TYPE option<record<mirror>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD crawl      ON TABLE ref_move TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD ref        ON TABLE ref_move TYPE string;
DEFINE FIELD at         ON TABLE ref_move TYPE datetime;
DEFINE FIELD old        ON TABLE ref_move TYPE option<string>
    ASSERT $value == NONE OR string::len($value) INSIDE [40, 64];
DEFINE FIELD new        ON TABLE ref_move TYPE option<string>
    ASSERT $value == NONE OR string::len($value) INSIDE [40, 64];
DEFINE FIELD identity   ON TABLE ref_move TYPE option<string>;
DEFINE FIELD action     ON TABLE ref_move TYPE string;
DEFINE FIELD valid_from ON TABLE ref_move TYPE datetime;

DEFINE INDEX idx_ref_move    ON TABLE ref_move FIELDS repository, checkout, ref, at, old, new, action UNIQUE;
DEFINE INDEX idx_ref_move_mirror ON TABLE ref_move FIELDS mirror, ref, at, old, new, action UNIQUE;
DEFINE INDEX idx_ref_move_at ON TABLE ref_move FIELDS repository, at;


-- 56 ── commit_rename ───────────────────────── a file a commit moved or copied ───
-- Git's rename and copy detection, kept beside the commit's files (which
-- read a rename as a delete and an add, and a copy as an add): `kind`
-- says whether git found the file `rename`d or `copy`d, from the old path
-- to the new path, and `similarity` how similar it found the two, as a
-- percentage. Code copied into a new file is told apart from new code.

DEFINE TABLE commit_rename SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE commit_rename TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD kind       ON TABLE commit_rename TYPE string
    ASSERT $value INSIDE ['rename', 'copy'];
DEFINE FIELD from_path  ON TABLE commit_rename TYPE string;
DEFINE FIELD to_path    ON TABLE commit_rename TYPE string;
DEFINE FIELD similarity ON TABLE commit_rename TYPE int
    ASSERT $value >= 0 AND $value <= 100;
DEFINE FIELD valid_from ON TABLE commit_rename TYPE datetime;

DEFINE INDEX idx_commit_rename ON TABLE commit_rename FIELDS commit, from_path, to_path UNIQUE;


-- 57 ── tip_file ───────────── one file at a repository's main line tip ───
-- What the main line holds, file by file, as each crawl read its tip: the
-- file's size in bytes and lines (NONE for a file git counts no lines in).
-- One row when a file appears, changes size, or leaves the tip (`present`
-- false). With `survival_file`, the share of what the main line holds that
-- each commit, and so each session and model, wrote.

DEFINE TABLE tip_file SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository ON TABLE tip_file TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD crawl      ON TABLE tip_file TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE tip_file TYPE string;
DEFINE FIELD bytes      ON TABLE tip_file TYPE int ASSERT $value >= 0;
DEFINE FIELD lines      ON TABLE tip_file TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD present    ON TABLE tip_file TYPE bool;
DEFINE FIELD valid_from ON TABLE tip_file TYPE datetime;

DEFINE INDEX idx_tip_file ON TABLE tip_file FIELDS repository, path, valid_from;


-- 58 ── repository_state ────────── how a repository is set up on this machine ───
-- One row per change: the branch `origin/HEAD` names, the hooks installed
-- (git's samples left out), the hooks directory git is told to use, and
-- whether the clone is shallow, in which case its oldest commit stands for
-- all the history before it.

DEFINE TABLE repository_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository  ON TABLE repository_state TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD crawl       ON TABLE repository_state TYPE record<crawl>
    ASSERT record::exists($value);
DEFINE FIELD origin_head ON TABLE repository_state TYPE option<string>;
DEFINE FIELD hooks       ON TABLE repository_state TYPE array<string>;
DEFINE FIELD hooks_path  ON TABLE repository_state TYPE option<string>;
DEFINE FIELD shallow     ON TABLE repository_state TYPE bool;
DEFINE FIELD valid_from  ON TABLE repository_state TYPE datetime;

DEFINE INDEX idx_repository_state ON TABLE repository_state FIELDS repository, valid_from;


-- 59 ── commit_session ─────────── a commit tied to the session that made it ───
-- `printed`: an agent's command in the session printed the commit's hash
-- as it made it (`git_event`, its `tool_call`). `pull_request`: the session
-- linked a pull request (`session_link`) that holds the commit
-- (`pull_request_commit`), or whose merge made it (`merge_sha`).

DEFINE TABLE commit_session SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE commit_session TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD session    ON TABLE commit_session TYPE record<session>
    ASSERT record::exists($value);
DEFINE FIELD tool_call  ON TABLE commit_session TYPE option<record<tool_call>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD via        ON TABLE commit_session TYPE string
    ASSERT $value INSIDE ['printed', 'pull_request'];
DEFINE FIELD pull_request ON TABLE commit_session TYPE option<record<pull_request>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD at         ON TABLE commit_session TYPE datetime;
DEFINE FIELD valid_from ON TABLE commit_session TYPE datetime;

DEFINE INDEX idx_commit_session      ON TABLE commit_session FIELDS commit, session, via UNIQUE;
DEFINE INDEX idx_commit_session_sess ON TABLE commit_session FIELDS session;


-- 60 ── host_repository ─────────────────── a repository on a host ───
-- A repository as a host knows it. `host` is the host the crawler reaches
-- (`github.com`, `bitbucket.org`), `path` the path the host knows it by
-- (`k8nstantin/superx`, a workspace and its repository slug), `host_id` the
-- host's own id for it (GitHub's number, Bitbucket's uuid), as the host
-- gives it. The repositories on this machine whose remotes name it are tied
-- to it by `remote_state` (`host`, `path`). `found_at` is when the crawler
-- first reached it.

DEFINE TABLE host_repository SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host       ON TABLE host_repository TYPE string
    ASSERT $value INSIDE ['github.com', 'bitbucket.org'];
DEFINE FIELD path       ON TABLE host_repository TYPE string;
DEFINE FIELD host_id    ON TABLE host_repository TYPE option<int | string>;
DEFINE FIELD found_at   ON TABLE host_repository TYPE datetime;
DEFINE FIELD valid_from ON TABLE host_repository TYPE datetime;

DEFINE INDEX idx_host_repository ON TABLE host_repository FIELDS host, path UNIQUE;


-- 61 ── host_object ───────────────── everything a host's answer held, whole ───
-- Every object an answer of a host held, whole, exactly as the host sent
-- it: one row per object, and one more each time its content changes. Every
-- other host table names the row it was read from (`object`), so what no
-- column holds is one read away, and a measure added later needs no second
-- crawl. `kind` is the crawler's word for what the object is; `key` is the
-- host's own id for it, or, for an object the host gives no id, what
-- identifies it: a commit's hash, a file's path, a status's commit and key.
-- `hash` is the SHA-256 of the object written with its keys in order, so a
-- change of content is a new row and an object read again unchanged is not.
-- `call` is the call whose answer first held this content.

DEFINE TABLE host_object SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE host_object TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD kind            ON TABLE host_object TYPE string
    ASSERT $value INSIDE ['repository', 'user', 'pull_request', 'pull_request_commit', 'pull_request_file', 'review', 'review_comment', 'issue', 'issue_comment', 'pull_request_comment', 'timeline_event', 'issue_event', 'pull_request_activity', 'pull_request_task', 'check_run', 'check_suite', 'commit_status', 'workflow_run', 'workflow_job', 'pipeline', 'pipeline_step', 'release', 'ruleset', 'branch_protection', 'branch_restriction', 'default_reviewer', 'deployment', 'deployment_status', 'environment'];
DEFINE FIELD key             ON TABLE host_object TYPE int | string;
DEFINE FIELD object          ON TABLE host_object TYPE object FLEXIBLE;
DEFINE FIELD hash            ON TABLE host_object TYPE string
    ASSERT string::len($value) == 64;
DEFINE FIELD call            ON TABLE host_object TYPE record<host_call>
    ASSERT record::exists($value);
DEFINE FIELD at              ON TABLE host_object TYPE datetime;
DEFINE FIELD valid_from      ON TABLE host_object TYPE datetime;

DEFINE INDEX idx_host_object      ON TABLE host_object FIELDS host_repository, kind, key, hash UNIQUE;
DEFINE INDEX idx_host_object_kind ON TABLE host_object FIELDS kind, at;


-- 62 ── host_repository_state ───────── how a host keeps a repository ───
-- One row per change in how the host keeps the repository: its default
-- branch (the main line as the host names it), whether it is private,
-- archived or a fork, and the repository it was forked from.

DEFINE TABLE host_repository_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE host_repository_state TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD default_branch  ON TABLE host_repository_state TYPE option<string>;
DEFINE FIELD private         ON TABLE host_repository_state TYPE option<bool>;
DEFINE FIELD archived        ON TABLE host_repository_state TYPE option<bool>;
DEFINE FIELD fork            ON TABLE host_repository_state TYPE option<bool>;
DEFINE FIELD parent          ON TABLE host_repository_state TYPE option<string>;
DEFINE FIELD object          ON TABLE host_repository_state TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE host_repository_state TYPE datetime;

DEFINE INDEX idx_host_repository_state ON TABLE host_repository_state FIELDS host_repository, valid_from;


-- 63 ── host_user ─────────────────────────── an account on a host ───
-- One row per account the hosts' answers name, ever: an author, a reviewer,
-- a commenter, whoever ran or merged something. `host_id` is the host's own
-- id (GitHub's number, Bitbucket's uuid); `login` the name it is known by
-- (GitHub's login, Bitbucket's nickname); `name` its display name; `kind`
-- what the host calls it (`User`, `Bot`, `Organization`; `user`, `team`,
-- `app_user`), as written: a bot and a person are told apart here.

DEFINE TABLE host_user SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host       ON TABLE host_user TYPE string
    ASSERT $value INSIDE ['github.com', 'bitbucket.org'];
DEFINE FIELD host_id    ON TABLE host_user TYPE int | string;
DEFINE FIELD login      ON TABLE host_user TYPE option<string>;
DEFINE FIELD name       ON TABLE host_user TYPE option<string>;
DEFINE FIELD kind       ON TABLE host_user TYPE option<string>;
DEFINE FIELD found_at   ON TABLE host_user TYPE datetime;
DEFINE FIELD valid_from ON TABLE host_user TYPE datetime;

DEFINE INDEX idx_host_user       ON TABLE host_user FIELDS host, host_id UNIQUE;
DEFINE INDEX idx_host_user_login ON TABLE host_user FIELDS host, login;


-- 64 ── host_crawl ───────────────── one crawl of one repository's host ───
-- What the crawler did on a host for one repository in one pass: its calls,
-- those the host answered unchanged (`not_modified`, which cost nothing of
-- the host's limit), those that failed, the times the host's rate limit made
-- it wait and for how long, the objects it found new and changed, the rows
-- it wrote. `crawl` is the repository's crawl it ran beside. `outcome`:
-- `complete`; `partial`, some answers could not be had; `unreadable`, the
-- host refused the repository; `no_credentials`, the credentials file names
-- no token for the host, so nothing was asked. Written once its calls are
-- done, before the rows that point to it.

DEFINE TABLE host_crawl SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE host_crawl TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD crawl           ON TABLE host_crawl TYPE option<record<crawl>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD started_at      ON TABLE host_crawl TYPE datetime;
DEFINE FIELD finished_at     ON TABLE host_crawl TYPE datetime;
DEFINE FIELD calls           ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD not_modified    ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD failed          ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD rate_waits      ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD waited_ms       ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD objects_new     ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD objects_changed ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD rows_written    ON TABLE host_crawl TYPE int ASSERT $value >= 0;
DEFINE FIELD outcome         ON TABLE host_crawl TYPE string
    ASSERT $value INSIDE ['complete', 'partial', 'unreadable', 'no_credentials'];
DEFINE FIELD note            ON TABLE host_crawl TYPE option<string>;
DEFINE FIELD valid_from      ON TABLE host_crawl TYPE datetime;

DEFINE INDEX idx_host_crawl ON TABLE host_crawl FIELDS host_repository, valid_from;


-- 65 ── host_call ──────────────────────────── one call to a host ───
-- Every call the crawler made to a host: GET only. `path` is what was asked,
-- with its query, never the host's name or a credential. `status` is the
-- HTTP status, NONE when no answer came; `not_modified` an answer that said
-- nothing changed since the `etag` sent. `etag` is the one the answer gave,
-- sent back next time. The host's rate limit as the answer reported it:
-- `rate_limit`, `rate_remaining`, `rate_reset`; `retry_after_ms` when the
-- host asked to wait. `objects` counts what the answer held; `next` says a
-- page followed. `error` is what went wrong, whole.

DEFINE TABLE host_call SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_crawl     ON TABLE host_call TYPE record<host_crawl>
    ASSERT record::exists($value);
DEFINE FIELD method         ON TABLE host_call TYPE string
    ASSERT $value INSIDE ['GET'];
DEFINE FIELD path           ON TABLE host_call TYPE string;
DEFINE FIELD started_at     ON TABLE host_call TYPE datetime;
DEFINE FIELD ms             ON TABLE host_call TYPE int ASSERT $value >= 0;
DEFINE FIELD status         ON TABLE host_call TYPE option<int>;
DEFINE FIELD not_modified   ON TABLE host_call TYPE bool;
DEFINE FIELD bytes          ON TABLE host_call TYPE int ASSERT $value >= 0;
DEFINE FIELD etag           ON TABLE host_call TYPE option<string>;
DEFINE FIELD rate_limit     ON TABLE host_call TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD rate_remaining ON TABLE host_call TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD rate_reset     ON TABLE host_call TYPE option<datetime>;
DEFINE FIELD retry_after_ms ON TABLE host_call TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD objects        ON TABLE host_call TYPE int ASSERT $value >= 0;
DEFINE FIELD next           ON TABLE host_call TYPE bool;
DEFINE FIELD error          ON TABLE host_call TYPE option<string>;
DEFINE FIELD valid_from     ON TABLE host_call TYPE datetime;

DEFINE INDEX idx_host_call      ON TABLE host_call FIELDS host_crawl;
DEFINE INDEX idx_host_call_path ON TABLE host_call FIELDS path, valid_from;


-- 66 ── mirror ──────────────────────── the crawler's own clone ───
-- One bare clone per repository on a host, kept by the crawler in the repos
-- folder of the module's folder and fetched with the operator's credentials:
-- every ref the host advertises, its pull requests' refs included, never
-- pruned and never collected, every ref's moves logged and never expired;
-- what the host once held stays here when the host rewrites or deletes it.
-- No one else writes it and no program it names is ever run. `url` is the
-- HTTPS URL it is fetched from, with no credential in it; `folder` its
-- folder under the repos folder. Its refs are `ref_state` rows and its
-- reflog entries `ref_move` rows that name it; its fetches are git calls of
-- the step `fetch`.

DEFINE TABLE mirror SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE mirror TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD url             ON TABLE mirror TYPE string;
DEFINE FIELD folder          ON TABLE mirror TYPE string;
DEFINE FIELD created_at      ON TABLE mirror TYPE datetime;
DEFINE FIELD valid_from      ON TABLE mirror TYPE datetime;

DEFINE INDEX idx_mirror ON TABLE mirror FIELDS host_repository UNIQUE;


-- 67 ── pull_request ──────────────────────── a pull request, ever ───
-- `number` is its number in its repository; `host_id` the host's own id.
-- `source_branch` and `target_branch` are the branches it asked to merge,
-- as it was opened; `source_path` the repository its branch lives in when
-- that is another (a fork). What changes about it is `pull_request_state`.

DEFINE TABLE pull_request SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE pull_request TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD number          ON TABLE pull_request TYPE int ASSERT $value > 0;
DEFINE FIELD host_id         ON TABLE pull_request TYPE int | string;
DEFINE FIELD author          ON TABLE pull_request TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD created_at      ON TABLE pull_request TYPE datetime;
DEFINE FIELD source_branch   ON TABLE pull_request TYPE string;
DEFINE FIELD target_branch   ON TABLE pull_request TYPE string;
DEFINE FIELD source_path     ON TABLE pull_request TYPE option<string>;
DEFINE FIELD url             ON TABLE pull_request TYPE string;
DEFINE FIELD object          ON TABLE pull_request TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE pull_request TYPE datetime;

DEFINE INDEX idx_pull_request         ON TABLE pull_request FIELDS host_repository, number UNIQUE;
DEFINE INDEX idx_pull_request_created ON TABLE pull_request FIELDS created_at;
DEFINE INDEX idx_pull_request_url     ON TABLE pull_request FIELDS url;


-- 68 ── pull_request_state ─────────── a pull request, as the host last said ───
-- One row per change. `state` as the host writes it (`open`, `closed`;
-- `OPEN`, `MERGED`, `DECLINED`, `SUPERSEDED`); `merged` whether it was
-- merged. `head_sha` and `base_sha` the commits it compared, `merge_sha`
-- the commit its merge made (a squash's new commit). Who merged or closed
-- it, when, and why it was declined (`reason`). The host's own counts, where
-- it gives them: lines added and removed, files, commits, comments, review
-- comments. `updated_at` is the host's own time of the change.

DEFINE TABLE pull_request_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pull_request    ON TABLE pull_request_state TYPE record<pull_request>
    ASSERT record::exists($value);
DEFINE FIELD state           ON TABLE pull_request_state TYPE string;
DEFINE FIELD merged          ON TABLE pull_request_state TYPE bool;
DEFINE FIELD draft           ON TABLE pull_request_state TYPE option<bool>;
DEFINE FIELD title           ON TABLE pull_request_state TYPE string;
DEFINE FIELD head_sha        ON TABLE pull_request_state TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD base_sha        ON TABLE pull_request_state TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD merge_sha       ON TABLE pull_request_state TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD merged_at       ON TABLE pull_request_state TYPE option<datetime>;
DEFINE FIELD merged_by       ON TABLE pull_request_state TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD closed_at       ON TABLE pull_request_state TYPE option<datetime>;
DEFINE FIELD closed_by       ON TABLE pull_request_state TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD reason          ON TABLE pull_request_state TYPE option<string>;
DEFINE FIELD additions       ON TABLE pull_request_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD deletions       ON TABLE pull_request_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD changed_files   ON TABLE pull_request_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD commits         ON TABLE pull_request_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD comments        ON TABLE pull_request_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD review_comments ON TABLE pull_request_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD updated_at      ON TABLE pull_request_state TYPE datetime;
DEFINE FIELD object          ON TABLE pull_request_state TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE pull_request_state TYPE datetime;

DEFINE INDEX idx_pull_request_state ON TABLE pull_request_state FIELDS pull_request, valid_from;
DEFINE INDEX idx_pull_request_merge ON TABLE pull_request_state FIELDS merge_sha;


-- 69 ── pull_request_commit ──────────── a commit a pull request held ───
-- One row the first time a pull request's commits list a commit, and one
-- each time it leaves or comes back (`present`): a commit pushed and then
-- forced away during review stays on record here. `position` is its place
-- in the list as read.

DEFINE TABLE pull_request_commit SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pull_request ON TABLE pull_request_commit TYPE record<pull_request>
    ASSERT record::exists($value);
DEFINE FIELD hash         ON TABLE pull_request_commit TYPE string
    ASSERT string::len($value) >= 7 AND string::len($value) <= 64;
DEFINE FIELD position     ON TABLE pull_request_commit TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD present      ON TABLE pull_request_commit TYPE bool;
DEFINE FIELD object       ON TABLE pull_request_commit TYPE option<record<host_object>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD valid_from   ON TABLE pull_request_commit TYPE datetime;

DEFINE INDEX idx_pull_request_commit      ON TABLE pull_request_commit FIELDS pull_request, hash, valid_from;
DEFINE INDEX idx_pull_request_commit_hash ON TABLE pull_request_commit FIELDS hash;


-- 70 ── pull_request_file ──────────── a file a pull request changes ───
-- One row per change in a file the pull request changes, read at its head
-- (`head_sha`): how (`status`, as written), the lines it adds and removes as
-- the host counts them, the path it had before a rename; `present` false
-- once the pull request no longer changes it.

DEFINE TABLE pull_request_file SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pull_request  ON TABLE pull_request_file TYPE record<pull_request>
    ASSERT record::exists($value);
DEFINE FIELD path          ON TABLE pull_request_file TYPE string;
DEFINE FIELD previous_path ON TABLE pull_request_file TYPE option<string>;
DEFINE FIELD status        ON TABLE pull_request_file TYPE option<string>;
DEFINE FIELD additions     ON TABLE pull_request_file TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD deletions     ON TABLE pull_request_file TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD head_sha      ON TABLE pull_request_file TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD present       ON TABLE pull_request_file TYPE bool;
DEFINE FIELD object        ON TABLE pull_request_file TYPE option<record<host_object>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD valid_from    ON TABLE pull_request_file TYPE datetime;

DEFINE INDEX idx_pull_request_file ON TABLE pull_request_file FIELDS pull_request, path, valid_from;


-- 71 ── pull_request_review ─────────── a review of a pull request ───
-- A GitHub review, or a Bitbucket approval or request for changes. `state`
-- as the host writes it (`APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`,
-- `DISMISSED`; `approved`, `changes_requested`); `commit_sha` the commit it
-- reviewed; `body` what the reviewer wrote, whole. One row per review, and
-- one more when the host changes it (a dismissal).

DEFINE TABLE pull_request_review SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pull_request ON TABLE pull_request_review TYPE record<pull_request>
    ASSERT record::exists($value);
DEFINE FIELD host_id      ON TABLE pull_request_review TYPE option<int | string>;
DEFINE FIELD reviewer     ON TABLE pull_request_review TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD state        ON TABLE pull_request_review TYPE string;
DEFINE FIELD submitted_at ON TABLE pull_request_review TYPE option<datetime>;
DEFINE FIELD commit_sha   ON TABLE pull_request_review TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD body         ON TABLE pull_request_review TYPE option<string>;
DEFINE FIELD object       ON TABLE pull_request_review TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from   ON TABLE pull_request_review TYPE datetime;

DEFINE INDEX idx_pull_request_review ON TABLE pull_request_review FIELDS pull_request, valid_from;


-- 72 ── pull_request_event ───────── what happened to a pull request ───
-- Every event of a pull request's timeline (GitHub) or activity
-- (Bitbucket): reviewed, committed, head_ref_force_pushed, merged, closed,
-- reopened, renamed, review_requested, ready_for_review; update, approval,
-- comment, changes_requested. `kind` as the host writes it; `actor` who did
-- it; `commit_sha` the commit it names. The rest of it is its object.

DEFINE TABLE pull_request_event SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pull_request ON TABLE pull_request_event TYPE record<pull_request>
    ASSERT record::exists($value);
DEFINE FIELD kind         ON TABLE pull_request_event TYPE string;
DEFINE FIELD actor        ON TABLE pull_request_event TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD at           ON TABLE pull_request_event TYPE datetime;
DEFINE FIELD commit_sha   ON TABLE pull_request_event TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD object       ON TABLE pull_request_event TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from   ON TABLE pull_request_event TYPE datetime;

DEFINE INDEX idx_pull_request_event      ON TABLE pull_request_event FIELDS pull_request, at;
DEFINE INDEX idx_pull_request_event_kind ON TABLE pull_request_event FIELDS kind, at;
DEFINE INDEX idx_pull_request_event_obj  ON TABLE pull_request_event FIELDS object UNIQUE;


-- 73 ── issue ─────────────────────────────── an issue, ever ───
-- An issue of a host's tracker that is not a pull request. What changes
-- about it is `issue_state`.

DEFINE TABLE issue SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE issue TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD number          ON TABLE issue TYPE int ASSERT $value > 0;
DEFINE FIELD host_id         ON TABLE issue TYPE int | string;
DEFINE FIELD author          ON TABLE issue TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD created_at      ON TABLE issue TYPE datetime;
DEFINE FIELD url             ON TABLE issue TYPE string;
DEFINE FIELD object          ON TABLE issue TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE issue TYPE datetime;

DEFINE INDEX idx_issue ON TABLE issue FIELDS host_repository, number UNIQUE;


-- 74 ── issue_state ──────────────────── an issue, as the host last said ───
-- One row per change: its state and why it closed (`state_reason`), its
-- title, labels, assignees and type, who closed it and when, its comments.

DEFINE TABLE issue_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD issue        ON TABLE issue_state TYPE record<issue>
    ASSERT record::exists($value);
DEFINE FIELD state        ON TABLE issue_state TYPE string;
DEFINE FIELD state_reason ON TABLE issue_state TYPE option<string>;
DEFINE FIELD title        ON TABLE issue_state TYPE string;
DEFINE FIELD labels       ON TABLE issue_state TYPE array<string>;
DEFINE FIELD assignees    ON TABLE issue_state TYPE array<string>;
DEFINE FIELD type         ON TABLE issue_state TYPE option<string>;
DEFINE FIELD closed_at    ON TABLE issue_state TYPE option<datetime>;
DEFINE FIELD closed_by    ON TABLE issue_state TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD comments     ON TABLE issue_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD updated_at   ON TABLE issue_state TYPE datetime;
DEFINE FIELD object       ON TABLE issue_state TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from   ON TABLE issue_state TYPE datetime;

DEFINE INDEX idx_issue_state ON TABLE issue_state FIELDS issue, valid_from;


-- 75 ── host_comment ─────────────── a comment, on a pull request or an issue ───
-- One row per comment and one more per edit (`updated_at`). `kind`:
-- `inline`, on a line or a file of a diff; `general`, on the conversation.
-- `path` and `line` place an inline one; `in_reply_to` the comment it
-- answers; `review` the review it belongs to; `deleted` one the host keeps
-- as removed. `body` is what was written, whole.

DEFINE TABLE host_comment SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD pull_request ON TABLE host_comment TYPE option<record<pull_request>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD issue        ON TABLE host_comment TYPE option<record<issue>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD host_id      ON TABLE host_comment TYPE int | string;
DEFINE FIELD kind         ON TABLE host_comment TYPE string
    ASSERT $value INSIDE ['inline', 'general'];
DEFINE FIELD author       ON TABLE host_comment TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD created_at   ON TABLE host_comment TYPE datetime;
DEFINE FIELD updated_at   ON TABLE host_comment TYPE datetime;
DEFINE FIELD path         ON TABLE host_comment TYPE option<string>;
DEFINE FIELD line         ON TABLE host_comment TYPE option<int>;
DEFINE FIELD in_reply_to  ON TABLE host_comment TYPE option<int | string>;
DEFINE FIELD review       ON TABLE host_comment TYPE option<int | string>;
DEFINE FIELD deleted      ON TABLE host_comment TYPE bool;
DEFINE FIELD body         ON TABLE host_comment TYPE string;
DEFINE FIELD object       ON TABLE host_comment TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from   ON TABLE host_comment TYPE datetime;

DEFINE INDEX idx_host_comment_pr    ON TABLE host_comment FIELDS pull_request, created_at;
DEFINE INDEX idx_host_comment_issue ON TABLE host_comment FIELDS issue, created_at;
DEFINE INDEX idx_host_comment_obj   ON TABLE host_comment FIELDS object UNIQUE;


-- 76 ── ci_run ───────────────────────────── one CI run, ever ───
-- A check run or suite, a workflow run or one of its jobs, a commit status
-- (GitHub); a pipeline, one of its steps, a commit status (Bitbucket).
-- `kind` is the crawler's word for which; `name` the check's, workflow's,
-- job's, step's or status context's name; `commit_sha` and `ref` what it
-- ran on; `parent` the run it is part of (a job's workflow run, a step's
-- pipeline, a check run's suite); `pull_request` the one it ran for, where
-- the host names it. What changes about it is `ci_run_state`.

DEFINE TABLE ci_run SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE ci_run TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD kind            ON TABLE ci_run TYPE string
    ASSERT $value INSIDE ['check_run', 'check_suite', 'workflow_run', 'workflow_job', 'commit_status', 'pipeline', 'pipeline_step'];
DEFINE FIELD host_id         ON TABLE ci_run TYPE int | string;
DEFINE FIELD name            ON TABLE ci_run TYPE option<string>;
DEFINE FIELD commit_sha      ON TABLE ci_run TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD ref             ON TABLE ci_run TYPE option<string>;
DEFINE FIELD parent          ON TABLE ci_run TYPE option<record<ci_run>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD pull_request    ON TABLE ci_run TYPE option<record<pull_request>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD url             ON TABLE ci_run TYPE option<string>;
DEFINE FIELD created_at      ON TABLE ci_run TYPE option<datetime>;
DEFINE FIELD object          ON TABLE ci_run TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE ci_run TYPE datetime;

DEFINE INDEX idx_ci_run        ON TABLE ci_run FIELDS host_repository, kind, host_id UNIQUE;
DEFINE INDEX idx_ci_run_commit ON TABLE ci_run FIELDS commit_sha;


-- 77 ── ci_run_state ─────────────────── a CI run, as the host last said ───
-- One row per change: `status` and `conclusion` as the host writes them
-- (`queued`, `in_progress`, `completed`; `success`, `failure`, `cancelled`,
-- `timed_out`; `PENDING`, `IN_PROGRESS`, `COMPLETED` with `SUCCESSFUL`,
-- `FAILED`, `STOPPED`, `ERROR`; `INPROGRESS`, `SUCCESSFUL`, `FAILED`,
-- `STOPPED`), when it started and finished, its attempt and its number as
-- the host counts them, who started it, the runner it ran on.

DEFINE TABLE ci_run_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD ci_run       ON TABLE ci_run_state TYPE record<ci_run>
    ASSERT record::exists($value);
DEFINE FIELD status       ON TABLE ci_run_state TYPE option<string>;
DEFINE FIELD conclusion   ON TABLE ci_run_state TYPE option<string>;
DEFINE FIELD started_at   ON TABLE ci_run_state TYPE option<datetime>;
DEFINE FIELD completed_at ON TABLE ci_run_state TYPE option<datetime>;
DEFINE FIELD attempt      ON TABLE ci_run_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD number       ON TABLE ci_run_state TYPE option<int>
    ASSERT $value == NONE OR $value >= 0;
DEFINE FIELD actor        ON TABLE ci_run_state TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD runner       ON TABLE ci_run_state TYPE option<string>;
DEFINE FIELD object       ON TABLE ci_run_state TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from   ON TABLE ci_run_state TYPE datetime;

DEFINE INDEX idx_ci_run_state ON TABLE ci_run_state FIELDS ci_run, valid_from;


-- 78 ── ci_log ──────────────────────────── what a CI run printed ───
-- The log of a finished workflow job or pipeline step, whole: where a
-- failure says what failed. `bytes` is its size; `call` the call it was read
-- with. One row per run, read once it finished.

DEFINE TABLE ci_log SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD ci_run     ON TABLE ci_log TYPE record<ci_run>
    ASSERT record::exists($value);
DEFINE FIELD call       ON TABLE ci_log TYPE record<host_call>
    ASSERT record::exists($value);
DEFINE FIELD bytes      ON TABLE ci_log TYPE int ASSERT $value >= 0;
DEFINE FIELD text       ON TABLE ci_log TYPE string;
DEFINE FIELD valid_from ON TABLE ci_log TYPE datetime;

DEFINE INDEX idx_ci_log ON TABLE ci_log FIELDS ci_run UNIQUE;


-- 79 ── release ─────────────────────────────── a release, ever ───
-- A GitHub release: its tag, who made it and when. What changes about it
-- (published, renamed, rewritten) is a new row of its object.

DEFINE TABLE release SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE release TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD host_id         ON TABLE release TYPE int | string;
DEFINE FIELD tag             ON TABLE release TYPE string;
DEFINE FIELD author          ON TABLE release TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD created_at      ON TABLE release TYPE datetime;
DEFINE FIELD published_at    ON TABLE release TYPE option<datetime>;
DEFINE FIELD draft           ON TABLE release TYPE bool;
DEFINE FIELD prerelease      ON TABLE release TYPE bool;
DEFINE FIELD object          ON TABLE release TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE release TYPE datetime;

DEFINE INDEX idx_release ON TABLE release FIELDS host_repository, host_id, valid_from;


-- 80 ── host_rule ─────────────────────── a rule a host enforces ───
-- One row per change in a rule over a repository's branches: a GitHub
-- ruleset or branch protection, a Bitbucket branch restriction or default
-- reviewer. `kind` is the crawler's word for which; `name` its name or the
-- branch pattern it covers; `enforcement` as written; `present` false once
-- the host no longer has it. The rule itself is its object.

DEFINE TABLE host_rule SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE host_rule TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD kind            ON TABLE host_rule TYPE string
    ASSERT $value INSIDE ['ruleset', 'branch_protection', 'branch_restriction', 'default_reviewer'];
DEFINE FIELD host_id         ON TABLE host_rule TYPE int | string;
DEFINE FIELD name            ON TABLE host_rule TYPE option<string>;
DEFINE FIELD enforcement     ON TABLE host_rule TYPE option<string>;
DEFINE FIELD present         ON TABLE host_rule TYPE bool;
DEFINE FIELD object          ON TABLE host_rule TYPE option<record<host_object>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD valid_from      ON TABLE host_rule TYPE datetime;

DEFINE INDEX idx_host_rule ON TABLE host_rule FIELDS host_repository, kind, host_id, valid_from;


-- 81 ── deployment ──────────────────────────── a deployment, ever ───
-- A deployment the host recorded: the environment, the commit and ref it
-- deployed, who made it and when. What became of it is `deployment_state`.

DEFINE TABLE deployment SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD host_repository ON TABLE deployment TYPE record<host_repository>
    ASSERT record::exists($value);
DEFINE FIELD host_id         ON TABLE deployment TYPE int | string;
DEFINE FIELD environment     ON TABLE deployment TYPE option<string>;
DEFINE FIELD commit_sha      ON TABLE deployment TYPE option<string>
    ASSERT $value == NONE OR (string::len($value) >= 7 AND string::len($value) <= 64);
DEFINE FIELD ref             ON TABLE deployment TYPE option<string>;
DEFINE FIELD creator         ON TABLE deployment TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD created_at      ON TABLE deployment TYPE option<datetime>;
DEFINE FIELD object          ON TABLE deployment TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from      ON TABLE deployment TYPE datetime;

DEFINE INDEX idx_deployment ON TABLE deployment FIELDS host_repository, host_id UNIQUE;


-- 82 ── deployment_state ─────────────── a deployment, as the host last said ───
-- One row per change: `state` as written (`success`, `failure`,
-- `in_progress`; `COMPLETED`, `IN_PROGRESS`, `UNDEPLOYED`), when, by whom.

DEFINE TABLE deployment_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD deployment ON TABLE deployment_state TYPE record<deployment>
    ASSERT record::exists($value);
DEFINE FIELD state      ON TABLE deployment_state TYPE string;
DEFINE FIELD at         ON TABLE deployment_state TYPE option<datetime>;
DEFINE FIELD actor      ON TABLE deployment_state TYPE option<record<host_user>>
    ASSERT $value == NONE OR record::exists($value);
DEFINE FIELD object     ON TABLE deployment_state TYPE record<host_object>
    ASSERT record::exists($value);
DEFINE FIELD valid_from ON TABLE deployment_state TYPE datetime;

DEFINE INDEX idx_deployment_state ON TABLE deployment_state FIELDS deployment, valid_from;


-- 83 ── merge_resolution ─────────── a file a merge resolved by hand ───
-- Where a merge commit's result differs from git's own merge of its two
-- parents (`--remerge-diff`): one row per file. `added` and `removed`
-- count the lines the merge's result has that git's merge did not and
-- lacks that it had; `conflicted` says git's merge left conflict markers
-- in the file, so the difference is how the conflict was resolved; and
-- `binary` that git counted no lines in it. A merge git would have made
-- the same way has no row. Git's merge is made in a folder of the
-- crawler's own, never in the repository.

DEFINE TABLE merge_resolution SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE merge_resolution TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD path       ON TABLE merge_resolution TYPE string;
DEFINE FIELD added      ON TABLE merge_resolution TYPE int ASSERT $value >= 0;
DEFINE FIELD removed    ON TABLE merge_resolution TYPE int ASSERT $value >= 0;
DEFINE FIELD conflicted ON TABLE merge_resolution TYPE bool;
DEFINE FIELD binary     ON TABLE merge_resolution TYPE bool;
DEFINE FIELD valid_from ON TABLE merge_resolution TYPE datetime;

DEFINE INDEX idx_merge_resolution ON TABLE merge_resolution FIELDS commit, path UNIQUE;


-- 84 ── tag ────────────────────────────────── an annotated tag, ever ───
-- An annotated tag object, read whole: `name` is the tag's own name as the
-- object holds it, `ref` the ref it was read through, `object` the tag
-- object's hash, `target` and `target_type` what it tags. Who tagged it,
-- when, and at what UTC offset (minutes east of UTC); `message` what it
-- says, whole, without its signature; `signature` the signature it
-- carries, whole, NONE when unsigned: kept, never verified. A lightweight
-- tag is only a ref (`ref_state`).

DEFINE TABLE tag SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD repository    ON TABLE tag TYPE record<repository>
    ASSERT record::exists($value);
DEFINE FIELD name          ON TABLE tag TYPE string;
DEFINE FIELD ref           ON TABLE tag TYPE option<string>;
DEFINE FIELD object        ON TABLE tag TYPE string
    ASSERT string::len($value) INSIDE [40, 64];
DEFINE FIELD target        ON TABLE tag TYPE string
    ASSERT string::len($value) INSIDE [40, 64];
DEFINE FIELD target_type   ON TABLE tag TYPE string
    ASSERT $value INSIDE ['commit', 'tree', 'blob', 'tag'];
DEFINE FIELD tagger        ON TABLE tag TYPE option<string>;
DEFINE FIELD tagger_name   ON TABLE tag TYPE option<string>;
DEFINE FIELD tagged_at     ON TABLE tag TYPE option<datetime>;
DEFINE FIELD tagger_offset ON TABLE tag TYPE option<int>;
DEFINE FIELD message       ON TABLE tag TYPE option<string>;
DEFINE FIELD signature     ON TABLE tag TYPE option<string>;
DEFINE FIELD valid_from    ON TABLE tag TYPE datetime;

DEFINE INDEX idx_tag        ON TABLE tag FIELDS repository, object UNIQUE;
DEFINE INDEX idx_tag_target ON TABLE tag FIELDS repository, target;


-- 85 ── commit_trailer ─────────────────── a trailer of a commit's message ───
-- Each trailer line git finds at the end of a commit's message (`git
-- interpret-trailers`: `Co-Authored-By: …`, `Signed-off-by: …`,
-- `Reviewed-by: …`), unfolded: `key` and `value` as written, `position`
-- its place among the commit's trailers. The message itself stays whole in
-- `commit.body`; these rows join a commit to who it names.

DEFINE TABLE commit_trailer SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD commit     ON TABLE commit_trailer TYPE record<commit>
    ASSERT record::exists($value);
DEFINE FIELD position   ON TABLE commit_trailer TYPE int ASSERT $value >= 0;
DEFINE FIELD key        ON TABLE commit_trailer TYPE string;
DEFINE FIELD value      ON TABLE commit_trailer TYPE string;
DEFINE FIELD valid_from ON TABLE commit_trailer TYPE datetime;

DEFINE INDEX idx_commit_trailer     ON TABLE commit_trailer FIELDS commit, position UNIQUE;
DEFINE INDEX idx_commit_trailer_key ON TABLE commit_trailer FIELDS key;
