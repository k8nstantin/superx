//! The watcher (#573): recon's second part, tested on SurrealDB's in-memory
//! engine with lines written as the Claude Code adapter writes them, at the
//! times each test gives; every look is given the time it judges at, so no
//! test waits for a bound.

pub mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, FixedOffset, SecondsFormat, TimeDelta, Utc};
use common::*;
use serde_json::{json, Value as Json};
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::Kernel;
use superx_mod_recon::instance::Instance;
use superx_mod_recon::params::{Params, WATCH_PLANS_PARAM};
use superx_mod_recon::server;
use superx_mod_recon::watcher::alerts::Kind;
use superx_mod_recon::watcher::command::{self, MergeCommit, Push};
use superx_mod_recon::watcher::found::Hidden;
use superx_mod_recon::watcher::track::{Split, StepKind};
use superx_mod_recon::watcher::{measure, plan, task, Snapshot, Watched, Watcher};
use tower::ServiceExt as _;

const PORT: u16 = 5152;
const JOB: &str = "gcloud run jobs execute kn-s6-load-iceberg-run --project p --region r --args stage,run,6,n1,n2,n3 --wait";
const VERIFY: &str = "gcloud run jobs execute kn-s6-load-iceberg-verify --project p --region r --args stage,verify,6,n1,n2,n3 --wait";

/// An id the kernel would give a line it captured at `at`.
fn id_at(table: &str, at: DateTime<Utc>) -> RecordId {
    let stamp = uuid::Timestamp::from_unix(uuid::NoContext, u64::try_from(at.timestamp()).expect("after 1970"), at.timestamp_subsec_nanos());
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::new_v7(stamp)))
}

/// A Claude Code session, its lines written as the adapter writes them: a
/// user or assistant line a message row, its text beside its parsed copy;
/// every other line a transcript_event, its payload whole.
struct Agent {
    kernel: Kernel,
    agent: RecordId,
    session: RecordId,
    key: String,
    cwd: String,
    at: DateTime<Utc>,
}

impl Agent {
    async fn new(kernel: &Kernel, key: &str, cwd: &Path, at: DateTime<Utc>) -> Self {
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let mut d = Object::new();
        d.insert("name".to_string(), Value::String("claude_code".to_string()));
        kernel.supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("agent descriptor");
        let session = kernel.create_entity("node_session").await.expect("session");
        let mut d = Object::new();
        d.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
        d.insert("session".to_string(), Value::String(key.to_string()));
        kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(d)).await.expect("session descriptor");
        Self { kernel: kernel.clone(), agent, session, key: key.to_string(), cwd: cwd.to_string_lossy().into_owned(), at }
    }

    /// Time passing with no line.
    fn wait(&mut self, secs: i64) {
        self.at += TimeDelta::seconds(secs);
    }

    fn stamp(&mut self, json: &mut Json, after: i64) -> DateTime<Utc> {
        self.at += TimeDelta::seconds(after);
        json["timestamp"] = Json::String(self.at.to_rfc3339());
        json["sessionId"] = Json::String(self.key.clone());
        json["cwd"] = Json::String(self.cwd.clone());
        self.at
    }

    /// A user or assistant line, `after` seconds after the one before, as
    /// `message` keeps it.
    async fn message(&mut self, mut json: Json, after: i64) -> DateTime<Utc> {
        let at = self.stamp(&mut json, after);
        self.kernel
            .db()
            .query(
                "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw: $raw, raw_text: $text, \
                 emitted_at: $at, valid_from: $at }",
            )
            .bind(("id", id_at("message", at)))
            .bind(("sess", self.session.clone()))
            .bind(("agent", self.agent.clone()))
            .bind(("raw", Value::Object(superx_kernel::message::json_to_object(&json))))
            .bind(("text", json.to_string()))
            .bind(("at", at))
            .await
            .expect("line")
            .check()
            .expect("line taken");
        at
    }

    /// Any other line, as a `transcript_event` keeps it whole.
    async fn event(&mut self, mut json: Json, after: i64) -> DateTime<Utc> {
        let at = self.stamp(&mut json, after);
        let payload = json!({ "session": self.key, "kind": json["type"], "file": "t.jsonl", "line": json, "raw_text": json.to_string() });
        self.kernel
            .db()
            .query("CREATE $id CONTENT { lifecycle_event: 'transcript_event', payload: $payload, agent: $agent, valid_from: $at }")
            .bind(("id", id_at("telemetry_stream", at)))
            .bind(("payload", superx_kernel::message::value_from_json(&payload)))
            .bind(("agent", self.agent.clone()))
            .bind(("at", at))
            .await
            .expect("event")
            .check()
            .expect("event taken");
        at
    }

    async fn typed(&mut self, text: &str, after: i64) -> DateTime<Utc> {
        self.message(json!({"type": "user", "origin": {"kind": "human"}, "message": {"role": "user", "content": text}}), after).await
    }

    async fn thought(&mut self, after: i64) -> DateTime<Utc> {
        self.message(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "thinking", "thinking": "hm"}]}}), after)
            .await
    }

    async fn said(&mut self, text: &str, after: i64) -> DateTime<Utc> {
        self.message(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "text", "text": text}]}}), after).await
    }

    async fn call(&mut self, id: &str, tool: &str, input: Json, after: i64) -> DateTime<Utc> {
        self.message(
            json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "tool_use", "id": id, "name": tool, "input": input}]}}),
            after,
        )
        .await
    }

    async fn result(&mut self, id: &str, failed: bool, content: &str, after: i64) -> DateTime<Utc> {
        self.message(
            json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "is_error": failed, "content": content}]}}),
            after,
        )
        .await
    }

    /// A command run in the background: the call, and Claude Code's answer
    /// that it runs.
    async fn job(&mut self, id: &str, command: &str, after: i64) -> DateTime<Utc> {
        let at = self.call(id, "Bash", json!({"command": command, "run_in_background": true}), after).await;
        self.result(id, false, &format!("Command running in background with ID: b{id}. Output is being written to: /tmp/{id}.output"), 1).await;
        at
    }

    /// A background job's notice that it ended.
    async fn notified(&mut self, call: &str, status: &str, after: i64) -> DateTime<Utc> {
        let text = format!("<task-notification>\n<task-id>b{call}</task-id>\n<tool-use-id>{call}</tool-use-id>\n<status>{status}</status>\n</task-notification>");
        self.message(json!({"type": "user", "origin": {"kind": "task-notification"}, "message": {"role": "user", "content": text}}), after).await
    }

    /// A request Claude Code sends the API: building on `base` and adding
    /// a message, or sending `base` again with nothing added.
    async fn asked(&mut self, id: &str, base: Option<&str>, again: bool, after: i64) -> DateTime<Utc> {
        let tail: Vec<&str> = if again { Vec::new() } else { vec!["blob"] };
        let json = json!({"type": "api-request", "id": id, "base": base, "keep": 3, "tail": tail,
            "params": {"model": "claude-sonnet-5-5", "output_config": {"effort": "max"}}});
        self.event(json, after).await
    }

    /// Claude Code giving up on the API.
    async fn api_error(&mut self, text: &str, after: i64) -> DateTime<Utc> {
        let json = json!({"type": "assistant", "isApiErrorMessage": true, "apiError": "connection_failed", "error": "server_error",
            "message": {"model": "<synthetic>", "stop_reason": "stop_sequence", "content": [{"type": "text", "text": text}]}});
        self.message(json, after).await
    }

    async fn turn_end(&mut self, after: i64) -> DateTime<Utc> {
        self.event(json!({"type": "system", "subtype": "turn_duration", "durationMs": 5}), after).await
    }

    /// The context compacted: the summary Claude Code writes in its place.
    async fn compacted(&mut self, after: i64) -> DateTime<Utc> {
        self.message(
            json!({"type": "user", "isCompactSummary": true, "message": {"role": "user", "content": "This session is being continued from a previous conversation."}}),
            after,
        )
        .await
    }
}

async fn setup(key: &str) -> (Kernel, PathBuf, Agent, DateTime<Utc>) {
    let kernel = kernel().await;
    let dir = scratch();
    let t0 = Utc::now() - TimeDelta::hours(30);
    let agent = Agent::new(&kernel, key, &dir, t0).await;
    (kernel, dir, agent, t0)
}

async fn look(watcher: &mut Watcher, kernel: &Kernel, p: &Params, now: DateTime<Utc>) -> Snapshot {
    watcher.look(kernel, p, now).await.expect("a look");
    watcher.snapshot()
}

/// Every session's time summed, over every row the measure gives.
fn total(w: &Watcher, keep: impl Fn(&measure::TimeRow) -> bool) -> (Split, u32) {
    let time = measure::time(w.tracks(), FixedOffset::east_opt(0).expect("utc"));
    let mut split = Split::default();
    let mut calls = 0;
    for r in time.rows.iter().filter(|r| keep(r)) {
        split.plus(&r.split);
        calls += r.calls;
    }
    (split, calls)
}

fn of(s: &Snapshot, kind: Kind) -> Vec<&superx_mod_recon::watcher::alerts::Alert> {
    s.alerts.iter().filter(|a| a.kind == kind).collect()
}

#[test]
fn the_watcher_is_its_own_part_it_reaches_neither_the_crawler_nor_the_results_and_every_query_it_sends_is_a_select() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = |dir: &Path| -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("a folder") {
                let p = e.expect("an entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).expect("a file");
                    out.push((p, text));
                }
            }
        }
        out
    };
    let watcher = files(&src.join("watcher"));
    assert!(watcher.len() >= 7, "the watcher's own files: {}", watcher.len());
    for (p, text) in &watcher {
        assert!(!text.contains("crate::crawler") && !text.contains("crate::results"), "{} reaches the crawler", p.display());
        for verb in ["CREATE ", "UPDATE ", "UPSERT ", "DELETE ", "INSERT ", "RELATE ", "DEFINE ", "REMOVE ", "ALTER "] {
            assert!(!text.contains(verb), "{} writes with {verb}", p.display());
        }
    }
    for part in ["crawler", "results"] {
        for (p, text) in files(&src.join(part)) {
            assert!(!text.contains("crate::watcher"), "{} reaches the watcher", p.display());
        }
    }
}

#[test]
fn a_launch_is_the_job_command_in_a_command_as_written_whatever_runs_around_it_and_a_plan_counts_the_same_job_launched_on_any_nodes() {
    let jobs = params(&[]).watch_jobs;
    let g4 = format!("date -u '+G4 started: %H:%M'; {JOB}");
    let g6 = format!("date -u '+G6 run started: %H:%M' && {JOB} 2>&1 | tail -5");
    assert_eq!(plan::launch(&g4, &jobs), plan::launch(&g6, &jobs), "the same launch behind different words");
    assert_eq!(
        plan::launch(&g4, &jobs).as_deref(),
        Some("gcloud run jobs execute kn-s6-load-iceberg-run --project p --region r --args stage,run,6,n1,n2,n3"),
        "--wait set aside"
    );
    assert_eq!(plan::launch("FOO=1 terraform apply plan.tfplan", &jobs).as_deref(), Some("terraform apply plan.tfplan"));
    assert_eq!(plan::launch("echo 'gcloud run jobs execute x'", &jobs), None, "a quoted mention is no launch");
    assert_eq!(plan::launch("git status", &jobs), None);
    assert_eq!(
        plan::launch("gcloud builds submit --tag x . > /tmp/out.txt 2>&1 < /dev/null", &jobs).as_deref(),
        Some("gcloud builds submit --tag x ."),
        "redirections set aside"
    );
    assert_eq!(plan::label(plan::launch(&g4, &jobs).as_deref().expect("a launch"), &jobs), "kn-s6-load-iceberg-run stage,run,6,n1,n2,n3");

    let xml = r#"<task><gates>
      <gate id="G4" kind="door" proves="steps 1 to 8: every table reloaded">
        <cmd>gcloud run jobs execute &lt;door.stage_jobs.6.run&gt; --project &lt;p&gt; --args stage,run,6,&lt;nodes&gt; --wait</cmd>
      </gate>
      <gate id="G5" kind="door" proves="step 9: verified"><cmd>gcloud run jobs execute &lt;door.stage_jobs.6.verify&gt; --project &lt;p&gt; --args stage,verify,6,&lt;nodes&gt; --wait</cmd></gate>
      <gate id="G6" kind="door" proves="a run again replaces the rows">
        <cmd>gcloud run jobs execute &lt;door.stage_jobs.6.run&gt; --project &lt;p&gt; --args stage,run,6,&lt;nodes&gt; --wait</cmd>
        <cmd>gcloud run jobs execute &lt;door.stage_jobs.6.verify&gt; --project &lt;p&gt; --args stage,verify,6,&lt;nodes&gt; --wait</cmd>
      </gate>
      <gate id="G7" kind="door" proves="a table name is refused"><cmd>gcloud run jobs execute &lt;door.stage_jobs.6.run&gt; --project &lt;p&gt; --args stage,run,6,&lt;nodes&gt;,&lt;the name of a table&gt; --wait</cmd></gate>
      <gate id="G8" kind="door" proves="every archive"><cmd>gcloud run jobs execute &lt;door.stage_jobs.6.run&gt; --project &lt;p&gt; --args 'stage,run,6,*' --wait</cmd></gate>
    </gates></task>"#;
    let repeats = plan::repeats(xml, &jobs);
    let times: Vec<(String, usize)> = repeats.iter().map(|r| (plan::label(&r.job, &jobs), r.times)).collect();
    assert_eq!(times, [("<door.stage_jobs.6.run> stage,run,6".to_string(), 4), ("<door.stage_jobs.6.verify> stage,verify,6".to_string(), 2)]);
    assert!(repeats[0].by.iter().any(|g| g.starts_with("G6 (a run again replaces the rows)")), "{:?}", repeats[0].by);
    assert!(plan::repeats("<task><gates><gate id=\"G1\"><cmd>python -m unittest</cmd></gate></gates></task>", &jobs).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finished_work_launched_again_is_one_alert_at_the_relaunch_costed_by_what_the_first_run_took() {
    let (kernel, _dir, mut agent, t0) = setup("again").await;
    let p = params(&[]);
    agent.typed("run the gates", 0).await;
    let first = agent.job("g4", &format!("date -u '+G4 started'; {JOB}"), 2).await;
    agent.notified("g4", "completed", 64 * 60).await;
    agent.job("g5", &format!("date -u '+G5 started'; {VERIFY}"), 30).await;
    agent.notified("g5", "completed", 80 * 60).await;
    // A launch that took a minute, run again: not costly enough to raise.
    agent.call("v1", "Bash", json!({"command": "gcloud run jobs execute kn --args door,version --wait"}), 5).await;
    agent.result("v1", false, "done", 60).await;
    agent.call("v2", "Bash", json!({"command": "gcloud run jobs execute kn --args door,version --wait"}), 5).await;
    agent.result("v2", false, "done", 60).await;
    let again = agent.job("g6", &format!("date -u '+G6 run started'; {JOB} 2>&1"), 30).await;

    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    let alerts = of(&s, Kind::RunAgain);
    assert_eq!(alerts.len(), 1, "one alert, at the costly relaunch alone: {:?}", s.alerts);
    let a = alerts[0];
    assert_eq!(a.at, again, "raised when it was launched again");
    assert_eq!(a.cost_secs, Some(64 * 60 + 1), "what the first run took, from its launch to its notice");
    assert_eq!(a.since, Some(first), "the first run's start, for the page to show");
    assert!(a.what.starts_with("kn-s6-load-iceberg-run stage,run,6,n1,n2,n3 launched again"), "{}", a.what);
    assert!(a.evidence[0].starts_with("gcloud run jobs execute kn-s6-load-iceberg-run"), "{:?}", a.evidence);

    // Another look raises it no second time.
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(2)).await;
    assert_eq!(of(&s, Kind::RunAgain).len(), 1);
    assert!(t0 < again);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_background_job_past_the_long_bound_is_one_alert_at_the_crossing_and_past_twice_the_run_it_repeats_sooner() {
    let (kernel, _dir, mut agent, _t0) = setup("long").await;
    let p = params(&[]);
    let start = agent.job("load", &format!("date; {JOB}"), 0).await;
    agent.typed("status", 70 * 60).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let long = of(&s, Kind::LongRun);
    assert_eq!(long.len(), 1, "{:?}", s.alerts);
    assert_eq!(long[0].at, start + TimeDelta::hours(1), "raised when it crossed the hour");
    let now = s.now.iter().find(|x| x.name == "claude_code/long").expect("live");
    assert_eq!(now.state, "job running");
    assert_eq!(now.jobs.len(), 1);
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(5)).await;
    assert_eq!(of(&s, Kind::LongRun).len(), 1, "once");

    // A job of 10 minutes, launched again and still running at 21: past
    // twice the run it repeats, long before the hour.
    agent.notified("load", "completed", 1).await;
    agent.job("short", "./wrapper.sh --all", 5).await;
    agent.notified("short", "completed", 10 * 60).await;
    let second = agent.job("short2", "./wrapper.sh --all", 5).await;
    agent.typed("status", 21 * 60).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let twice: Vec<_> = of(&s, Kind::LongRun).into_iter().filter(|a| a.id.ends_with("/short2")).collect();
    assert_eq!(twice.len(), 1, "{:?}", s.alerts);
    assert_eq!(twice[0].at, second + TimeDelta::seconds(2 * (10 * 60 + 1)));
    assert!(twice[0].what.contains("twice the 10 min the same launch took before"), "{}", twice[0].what);
    assert_eq!(of(&s, Kind::RunAgain).len(), 1, "a run of 600 s or more, launched again: {:?}", s.alerts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreground_call_waiting_past_the_command_limit_waits_on_your_approval_and_raises_nothing() {
    let (kernel, _dir, mut agent, _t0) = setup("approve").await;
    let p = params(&[]);
    agent.typed("deploy", 0).await;
    agent.call("tf", "Bash", json!({"command": "terraform apply plan.tf"}), 3).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(20)).await;
    let now = s.now.iter().find(|x| x.name == "claude_code/approve").expect("live");
    assert_eq!(now.state, "waiting on your approval");
    assert!(s.alerts.is_empty(), "{:?}", s.alerts);
    agent.result("tf", false, "Apply complete", 20 * 60).await;
    look(&mut w, &kernel, &p, agent.at).await;
    let (split, _) = total(&w, |r| r.tool == "Bash" && r.target == "terraform apply plan.tf");
    assert_eq!(split.approval, 20 * 60, "the wait was the operator's approval, not the tool's run, and it was on the apply");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn your_stop_then_a_file_written_or_a_job_launched_before_its_words_is_one_alert_and_nothing_else_is() {
    let (kernel, _dir, mut agent, _t0) = setup("stop").await;
    let p = params(&[]);
    agent.typed("stop, do not touch the schema", 0).await;
    let wrote = agent.call("e1", "Edit", json!({"file_path": "/w/schema/a.sql", "old_string": "a", "new_string": "b"}), 4).await;
    agent.result("e1", false, "ok", 1).await;
    agent.said("done", 2).await;
    // A stop the agent answers with words first.
    agent.typed("STOP", 30).await;
    agent.said("Stopped. Nothing runs.", 3).await;
    agent.call("e2", "Edit", json!({"file_path": "/w/b.rs", "old_string": "a", "new_string": "b"}), 2).await;
    // "wait" asks; it does not stop.
    agent.typed("wait why is this here?", 30).await;
    agent.call("e3", "Write", json!({"file_path": "/w/c.rs", "content": "x"}), 2).await;
    // Reading after a stop is not acting on it.
    agent.typed("don't change anything", 30).await;
    agent.call("r1", "Read", json!({"file_path": "/w/c.rs"}), 2).await;
    agent.call("b1", "Bash", json!({"command": "git status && ls"}), 2).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    let stops = of(&s, Kind::StopNotFollowed);
    assert_eq!(stops.len(), 1, "{:?}", s.alerts);
    assert_eq!(stops[0].at, wrote);
    assert_eq!(stops[0].what, "after your stop it wrote /w/schema/a.sql before answering you");
    assert_eq!(stops[0].evidence[0], "stop, do not touch the schema");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_read_or_written_that_launches_the_same_job_more_than_once_is_one_alert_per_plan() {
    let (kernel, dir, mut agent, _t0) = setup("plan").await;
    let p = params(&[]);
    let tasks = dir.join("kn-specs/tasks");
    std::fs::create_dir_all(&tasks).expect("tasks");
    let file = tasks.join("ENG-1.xml");
    let xml = "<task><gates>\n<gate id=\"G4\" proves=\"the load\"><cmd>gcloud run jobs execute &lt;j&gt; --args stage,run,6,&lt;nodes&gt; --wait</cmd></gate>\n\
               <gate id=\"G6\" proves=\"a run again\"><cmd>gcloud run jobs execute &lt;j&gt; --args stage,run,6,&lt;nodes&gt; --wait</cmd></gate>\n</gates></task>";
    std::fs::write(&file, xml).expect("plan");
    let numbered: String = xml.lines().enumerate().map(|(i, l)| format!("{:>6}\t{l}\n", i + 1)).collect();
    agent.typed("Build kn-specs/tasks/ENG-1.xml", 0).await;
    agent.call("r", "Read", json!({"file_path": file.to_string_lossy()}), 3).await;
    let read = agent.result("r", false, &numbered, 1).await;
    agent.call("r2", "Read", json!({"file_path": file.to_string_lossy()}), 30).await;
    agent.result("r2", false, &numbered, 1).await;
    // Edited on disk: the plan as it now stands is read from the file.
    agent.call("e", "Edit", json!({"file_path": file.to_string_lossy(), "old_string": "x", "new_string": "y"}), 30).await;
    agent.result("e", false, "ok", 1).await;
    // A file of no plan's path is never read.
    agent.call("o", "Read", json!({"file_path": dir.join("other.xml").to_string_lossy()}), 30).await;
    agent.result("o", false, &numbered, 1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    let plans = of(&s, Kind::PlanRepeats);
    assert_eq!(plans.len(), 1, "once for the plan as it stands: {:?}", s.alerts);
    assert_eq!(plans[0].at, read, "raised when the session read it");
    assert_eq!(plans[0].what, "the plan in ENG-1.xml launches <j> stage,run,6 2 times");
    assert!(plans[0].evidence[0].contains("G4 (the load); G6 (a run again)"), "{:?}", plans[0].evidence);

    // A plan path set elsewhere by the operator.
    let p = params(&[(WATCH_PLANS_PARAM, Value::Array(vec![Value::String("/nowhere/".to_string())].into()))]);
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    assert!(of(&s, Kind::PlanRepeats).is_empty(), "{:?}", s.alerts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_gap_goes_to_what_ended_it_thinking_writing_tools_jobs_your_answer_and_your_approval() {
    let (kernel, _dir, mut agent, _t0) = setup("time").await;
    let p = params(&[]);
    agent.typed("go", 0).await;
    agent.thought(5).await;
    agent.said("on it", 10).await;
    agent.call("c", "Bash", json!({"command": "cargo test"}), 2).await;
    agent.result("c", false, "ok", 30).await;
    agent.said("done", 3).await;
    agent.turn_end(0).await;
    agent.typed("next", 300).await;
    agent.job("j", "./long.sh", 2).await;
    agent.typed("status", 600).await;
    agent.notified("j", "completed", 3000).await;
    agent.call("q", "AskUserQuestion", json!({"questions": []}), 4).await;
    agent.result("q", false, "answered", 120).await;
    // A gap longer than any reply: the session was left.
    agent.wait(3600);
    agent.said("back", 0).await;
    let mut w = Watcher::new();
    look(&mut w, &kernel, &p, agent.at).await;
    let (t, _) = total(&w, |_| true);
    assert_eq!(t.thinking, 5);
    assert_eq!(t.writing, 10 + 2 + 3 + 2 + 4, "words and calls");
    assert_eq!(t.tools, 30 + 1, "the test's run, and the job's start");
    assert_eq!(t.jobs, 600 + 3000, "your question while the job ran, and the job to its notice");
    assert_eq!(t.operator, 300 + 120 + 3600, "your next message, your answer, the session left");
    assert_eq!(t.approval, 0);
    let time = measure::time(w.tracks(), FixedOffset::east_opt(0).expect("utc"));
    assert!(time.rows.iter().all(|r| r.model.as_deref() == Some("claude-opus-5-5")), "every second under the model at work");
    assert_eq!(time.hours.iter().map(|h| h.counts.prompts).sum::<u32>(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_you_say_raises_nothing_status_asks_questions_frustration_and_estimates_are_no_alerts() {
    let (kernel, _dir, mut agent, _t0) = setup("noise").await;
    let p = params(&[]);
    for text in ["status", "what's going on???", "why is this taking so long, fuck", "ETA?", "is it ready?", "no no"] {
        agent.typed(text, 60).await;
        agent.said("About 5 hours more.", 5).await;
        agent.call(&format!("r{}", text.len()), "Read", json!({"file_path": "/w/a.rs"}), 2).await;
    }
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    assert!(s.alerts.is_empty(), "{:?}", s.alerts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_watch_answers_the_alerts_the_sessions_the_runs_and_the_measures_by_model_and_by_your_days() {
    let (kernel, dir, mut agent, _t0) = setup("api").await;
    let p = params(&[]);
    agent.typed("run", 0).await;
    agent.job("g4", JOB, 2).await;
    agent.notified("g4", "completed", 65 * 60).await;
    agent.job("g6", JOB, 10).await;
    let mut w = Watcher::new();
    look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    let watched = Watched::new(w);
    let instance = Instance {
        home: dir.clone(),
        endpoint: "mem://".to_string(),
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        data_dir: dir.join("db"),
        log_filter: "info".to_string(),
    };
    let app = server::app(kernel.clone(), instance, PORT, superx_mod_recon::crawler::progress::Progress::default(), watched);
    let ask = |uri: &str| {
        Request::builder().uri(uri).header("host", format!("127.0.0.1:{PORT}")).body(Body::empty()).expect("request")
    };
    let r = app.clone().oneshot(ask("/api/watch?offset=-240")).await.expect("answer");
    assert_eq!(r.status(), StatusCode::OK);
    let v: Json = serde_json::from_slice(&to_bytes(r.into_body(), usize::MAX).await.expect("body")).expect("json");
    assert_eq!(v["alerts"][0]["kind"], "run_again", "{v}");
    assert_eq!(v["alerts"].as_array().map(Vec::len), Some(2), "run again, and the first run past the hour: {}", v["alerts"]);
    assert_eq!(v["now"][0]["state"], "job running");
    assert!(v["now"][0]["jobs"][0]["expected_end"].is_string(), "when the same launch ended last time: {}", v["now"][0]);
    assert_eq!(v["runs"].as_array().map(Vec::len), Some(2), "{}", v["runs"]);
    let session = v["now"][0]["id"].as_str().expect("the session").to_string();
    let get = |uri: String| {
        let app = app.clone();
        async move {
            let r = app.oneshot(ask(&uri)).await.expect("answer");
            let status = r.status();
            let v: Json = serde_json::from_slice(&to_bytes(r.into_body(), usize::MAX).await.expect("body")).expect("json");
            (status, v)
        }
    };
    let (status, time) = get("/api/watch/time?offset=-240".to_string()).await;
    assert_eq!(status, StatusCode::OK);
    let rows = time["rows"].as_array().expect("rows");
    assert!(rows.iter().any(|r| r["tool"] == "Bash" && r["split"]["jobs"].as_i64().is_some_and(|j| j > 3_600)), "the hour on the job, under the job: {time}");
    assert!(rows.iter().all(|r| r["day"].as_str().is_some_and(|d| d.len() == 10)), "{time}");
    let (status, _) = get("/api/watch/time?offset=100000".to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, found) = get("/api/watch/found".to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(found.is_array(), "{found}");
    let span = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let (from, to) = (span(agent.at - TimeDelta::hours(3)), span(agent.at + TimeDelta::hours(1)));
    let (status, lines) = get(format!("/api/watch/lines?session={session}&from={from}&to={to}")).await;
    assert_eq!(status, StatusCode::OK, "{lines}");
    let calls: Vec<&Json> = lines["lines"].as_array().expect("lines").iter().filter(|l| l["kind"] == "call").collect();
    assert_eq!(calls.len(), 2, "both launches: {lines}");
    assert_eq!(calls[0]["background"], true);
    assert!(calls[0]["secs"].as_i64().is_some_and(|s| s >= 65 * 60), "the first job's run, by its notice: {}", calls[0]);
    assert_eq!(lines["lines"][0]["kind"], "you", "the operator's word first: {lines}");
    let (status, _) = get(format!("/api/watch/lines?session=nobody&from={from}&to={to}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, calls) = get("/api/watch/calls?tool=Bash".to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(calls["total"], 2, "{calls}");
}

#[test]
fn a_command_is_read_for_its_work_its_sleep_what_it_changes_and_what_it_pushes() {
    assert_eq!(command::label("cd /w && cargo test -p x 2>&1 | tail -5"), "cargo test");
    assert_eq!(command::label("git -C /r status --short | head"), "git status");
    assert_eq!(command::label("python3 tools/skill_audit.py"), "python3 skill_audit.py");
    assert_eq!(command::label("python3 -m unittest discover -s tests"), "python3 -m unittest");
    assert_eq!(command::label("python3 - <<'EOF'\nprint(1)\nEOF"), "python3 stdin");
    assert_eq!(command::label("TOKEN=$(gcloud auth print-access-token) && curl -s x"), "gcloud auth");
    assert_eq!(command::label("nice -n 19 cargo build --release"), "cargo build");
    assert_eq!(command::label("echo '--- x' && git log --oneline -3"), "git log");
    assert_eq!(command::label("for f in a b; do wc -l $f; done"), "wc");
    assert!(command::sleeps("sleep 300; gcloud run jobs executions describe x"));
    assert!(!command::sleeps("grep -n sleep notes.txt"));
    assert_eq!(command::changed("git add gryphon-kn/schema && git commit -m x"), ["gryphon-kn/schema"]);
    assert_eq!(command::changed("sed -i '' 's/a/b/' src/a.rs"), ["src/a.rs"]);
    assert_eq!(command::changed("cat a > /w/out.txt 2>&1"), ["/w/out.txt"]);
    assert!(command::changed("grep -rn 'ALTER TABLE' src").is_empty(), "a search changes nothing");
    assert!(command::changed("bq query --use_legacy_sql=false 'ALTER TABLE t ADD COLUMN c INT64'")[0].starts_with("DDL: "));
    assert!(command::changed("gcloud dataproc batches submit spark-sql /s/alter-relationships.sql --project p")[0].starts_with("DDL: "));
    assert_eq!(
        command::pushes("git push --force-with-lease=sandbox:0276e913 origin 97749550:refs/heads/sandbox"),
        [Push { branch: "sandbox".to_string(), force: true }]
    );
    assert_eq!(command::pushes("git push -q -u origin ENG-8079-task-file 2>&1 | grep -v x"), [Push { branch: "ENG-8079-task-file".to_string(), force: false }]);
    assert!(command::pushes("git push -q origin --delete feat/x").is_empty(), "a deletion pushes no work");
    assert_eq!(command::pushes("git push 2>&1 | tail -1")[0].branch, "");
    assert_eq!(command::pushes("git push -u origin HEAD")[0].branch, "", "HEAD is the branch checked out");
    assert_eq!(command::new_branch("git switch -c ENG-8078-task-file-v2 origin/sandbox 2>&1 | tail -1").as_deref(), Some("ENG-8078-task-file-v2"));
    assert_eq!(command::merges("gh pr merge 449 --squash --admin"), Some(Some("449".to_string())));
    assert_eq!(command::merges("gh pr merge --squash"), Some(None));
    assert_eq!(command::merges("git status"), None);
    assert_eq!(
        command::merge_commits("3cf1a23 Merged in ENG-8080-annotate-design (pull request #109)\n9a1b2c3 annotate"),
        [MergeCommit { branch: "ENG-8080-annotate-design".to_string(), pull: 109, commit: Some("3cf1a23".to_string()) }]
    );
    assert_eq!(
        command::merge_commits("Merge pull request #12 from k8nstantin/feat/573-x"),
        [MergeCommit { branch: "feat/573-x".to_string(), pull: 12, commit: None }]
    );
    assert_eq!(
        command::forced_updates("lease ok\n + 1a347c3b...3cf1a23a 3cf1a23af13a44865fef27537c6a4aca4c35dbac -> sandbox (forced update)"),
        [("sandbox".to_string(), "1a347c3b".to_string(), "3cf1a23a".to_string())]
    );
    assert!(command::same_commit("1a347c3b628434c0fd1cc41c5910b2a5ef3ed802", "1a347c3b") && !command::same_commit("1a347c3", "1a347c4"));
    assert_eq!(task::keys("feat/573-watcher-v2-opus-5-5-max"), ["#573"], "a branch named for an issue carries it");
    assert!(task::keys("fix/view-repeats").is_empty());
    assert!(command::merge_commits("it merged in the end (pull request soon)").is_empty());
    assert!(command::merge_commits("Merged in X (pull request #)").is_empty());
    // A here-document's body is the command's input, not commands.
    assert_eq!(
        command::simple_commands("python3 -I - <<'EOF'\nimport os\nif x > 3: os.system('git push --force origin main')\nEOF\necho done"),
        ["python3 -I - <<'EOF'", "echo done"]
    );
    assert!(command::changed("cd kn && python3 - <<EOF\nif a > 3: pass\nEOF").is_empty());
    assert!(command::pushes("python3 - <<-'PY'\nrun('git push origin main')\n\tPY\ngit status").is_empty());
    assert_eq!(command::changed("cat <<EOF > out.txt\nhello\nEOF"), ["out.txt"]);
    assert_eq!(task::stem("ENG-8078-task-file-v2"), "ENG-8078-task-file");
    assert_eq!(task::stem("feat/573-watcher-v2-opus-5-5-max"), "feat/573-watcher");
    assert_eq!(task::stem("ENG-7811-revert-eng8071-v8-eng8072-v9-builds"), "ENG-7811-revert-eng8071-v8-eng8072", "its own version is the last");
    assert_eq!(task::stem("ENG-8080-annotate-design"), "ENG-8080-annotate-design");
    assert_eq!(task::stem("fix/view-v8x"), "fix/view-v8x");
    assert_eq!(command::undoes("git reset --hard HEAD~1").as_deref(), Some("git reset --hard HEAD~1"));
    assert_eq!(command::undoes("git reset HEAD a.rs"), None);
    assert_eq!(task::estimate("I'm fixing the start rules on a sandbox branch, about 15 minutes"), Some(900));
    assert_eq!(task::estimate("Running the harness now: about 30 to 40 minutes."), Some(2_400));
    assert_eq!(task::estimate("That is about 4 to 6 hours"), Some(21_600));
    assert_eq!(task::estimate("The ETA is about 3.5 hours"), Some(12_600));
    assert_eq!(task::estimate("it took about 12 minutes last time"), None, "a measure, not a promise");
    assert_eq!(task::estimate("pauses within ~2s"), None);
    assert_eq!(task::estimate("At about 20 seconds per call"), None);
    assert_eq!(task::keys("ENG-8078-task-file-v2"), ["ENG-8078"]);
    assert_eq!(task::keys("feat/573-watcher"), ["#573"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_second_goes_to_the_tool_and_what_it_was_called_on_thinking_to_what_came_after_it() {
    let (kernel, _dir, mut agent, _t0) = setup("on").await;
    let p = params(&[]);
    agent.typed("fix the parser", 0).await;
    agent.thought(20).await;
    agent.call("r", "Read", json!({"file_path": "/w/src/parse.rs"}), 4).await;
    agent.result("r", false, "fn parse() {}", 2).await;
    agent.thought(30).await;
    agent.call("e", "Edit", json!({"file_path": "/w/src/parse.rs", "old_string": "a", "new_string": "b"}), 10).await;
    agent.result("e", false, "ok", 1).await;
    agent.call("c", "Bash", json!({"command": "cd /w && cargo test -p parser 2>&1 | tail -20"}), 3).await;
    agent.result("c", true, "test failed", 90).await;
    agent.said("Fixed.", 5).await;
    agent.turn_end(0).await;
    agent.typed("thanks", 600).await;
    let mut w = Watcher::new();
    look(&mut w, &kernel, &p, agent.at).await;
    let on = |tool: &'static str, target: &'static str| total(&w, move |r| r.tool == tool && r.target == target);
    let (read, reads) = on("Read", "/w/src/parse.rs");
    assert_eq!((read.thinking, read.writing, read.tools, reads), (20, 4, 2, 1), "the thought before the read is the read's");
    let (edit, _) = on("Edit", "/w/src/parse.rs");
    assert_eq!((edit.thinking, edit.writing, edit.tools), (30, 10, 1));
    let (test, tests) = on("Bash", "cargo test");
    assert_eq!((test.writing, test.tools, tests), (3, 90, 1));
    let time = measure::time(w.tracks(), FixedOffset::east_opt(0).expect("utc"));
    assert_eq!(time.rows.iter().filter(|r| r.target == "cargo test").map(|r| r.failed).sum::<u32>(), 1, "the failed run counted");
    assert_eq!(on("(its words)", "").0.writing, 5);
    assert_eq!(on("(your next message)", "").0.operator, 600);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_shows_nowhere_is_found_held_asleep_refused_protected_thrash_read_again_flooded_undone_compacted_and_what_you_said() {
    let (kernel, _dir, mut agent, _t0) = setup("finds").await;
    let p = params(&[]);
    agent.typed("build the stage", 0).await;
    agent.call("h", "Bash", json!({"command": "terraform apply plan.tf"}), 3).await;
    agent.result("h", false, "Apply complete", 20 * 60).await;
    agent.job("j", "./long.sh", 5).await;
    agent.call("s", "Bash", json!({"command": "sleep 300; tail -5 /w/j.out"}), 3).await;
    agent.result("s", false, "...", 300).await;
    for x in ["x1", "x2"] {
        agent.call(x, "Bash", json!({"command": "rm -rf build"}), 3).await;
        agent.result(x, true, "Permission for this action was denied by the Claude Code auto mode classifier.", 1).await;
    }
    agent.call("p1", "Bash", json!({"command": "git add gryphon-kn/schema && git commit -m x"}), 3).await;
    agent.result("p1", false, "", 1).await;
    for k in 0..10 {
        agent.call(&format!("t{k}"), "Edit", json!({"file_path": "/w/a.rs", "old_string": "a", "new_string": "b"}), 20).await;
        agent.result(&format!("t{k}"), false, "ok", 1).await;
    }
    for r in ["r1", "r2"] {
        agent.call(r, "Read", json!({"file_path": "/w/b.rs"}), 3).await;
        agent.result(r, false, "x", 1).await;
    }
    agent.call("f", "Bash", json!({"command": "cat big.log"}), 3).await;
    agent.result("f", false, &"x".repeat(50_000), 1).await;
    agent.call("u", "Bash", json!({"command": "git reset --hard HEAD~1"}), 3).await;
    agent.result("u", false, "", 1).await;
    agent.compacted(30).await;
    agent.call("r3", "Read", json!({"file_path": "/w/b.rs"}), 3).await;
    agent.result("r3", false, "x", 1).await;
    agent.typed("status?", 30).await;
    agent.typed("wrong, i told you not to touch the schema", 30).await;
    agent.typed("[Request interrupted by user]", 5).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let found = w.found();
    let kinds: BTreeSet<Hidden> = found.iter().map(|f| f.kind).collect();
    for k in [
        Hidden::Held,
        Hidden::Slept,
        Hidden::Refused,
        Hidden::Protected,
        Hidden::Thrash,
        Hidden::Reread,
        Hidden::Flood,
        Hidden::Undone,
        Hidden::Compacted,
        Hidden::Asked,
        Hidden::Corrected,
        Hidden::Interrupted,
    ] {
        assert!(kinds.contains(&k), "{k:?} not found in {found:#?}");
    }
    let one = |k: Hidden, call: &str| found.iter().find(|f| f.kind == k && f.call.as_deref() == Some(call)).unwrap_or_else(|| panic!("{k:?} of {call}"));
    assert_eq!(one(Hidden::Held, "h").secs, Some(20 * 60));
    assert!(one(Hidden::Slept, "s").what.contains("while its job"), "{}", one(Hidden::Slept, "s").what);
    assert_eq!(one(Hidden::Refused, "x2").times, 2, "the same call refused twice");
    assert!(found.iter().any(|f| f.kind == Hidden::Protected && f.target.as_deref() == Some("/schema")), "{found:#?}");
    assert_eq!(found.iter().find(|f| f.kind == Hidden::Thrash).map(|f| f.times), Some(10));
    assert_eq!(found.iter().find(|f| f.kind == Hidden::Compacted).map(|f| f.times), Some(1), "b.rs read again after it");
    assert!(s.alerts.is_empty(), "none of it is an alert: {:?}", s.alerts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_merged_then_pushed_again_or_made_anew_is_one_alert_kept_up_to_date_and_its_words_are_its_record() {
    let (kernel, _dir, mut agent, _t0) = setup("tailspin").await;
    let p = params(&[]);
    agent.typed("write the ENG-8080 xml", 0).await;
    agent.call("b", "Bash", json!({"command": "git switch -c ENG-8080-annotate-design origin/sandbox"}), 3).await;
    agent.result("b", false, "Switched to a new branch", 1).await;
    agent.said("Writing it now, about 2 hours.", 2).await;
    agent.call("p", "Bash", json!({"command": "git push -q -u origin ENG-8080-annotate-design"}), 60).await;
    agent.result("p", false, "", 2).await;
    agent.said("Everything on the branch is done, so your merge means done.", 2).await;
    agent.typed("merged", 120).await;
    agent.call("f", "Bash", json!({"command": "git fetch -q origin && git log --oneline -2 origin/sandbox"}), 5).await;
    agent.result("f", false, "3cf1a23 Merged in ENG-8080-annotate-design (pull request #108)\n9a1b2c3 ENG-8080 annotate", 1).await;
    agent.said("PR #108 is merged. One step remains before you start the agent: its build folder.", 10).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert!(of(&s, Kind::EndlessTask).is_empty(), "words raise nothing: {:?}", s.alerts);
    let task = s.tasks.iter().find(|t| t.branch == "ENG-8080-annotate-design").expect("the task");
    assert_eq!(task.not_done.len(), 1, "{:?}", task.not_done);
    assert!(task.not_done[0].1.starts_with("PR #108 is merged. One step remains"), "{:?}", task.not_done);
    agent.typed("why", 30).await;
    agent.call("p2", "Bash", json!({"command": "git push 2>&1 | tail -1"}), 600).await;
    agent.result("p2", false, "", 2).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let endless = of(&s, Kind::EndlessTask);
    assert_eq!(endless.len(), 1, "{:?}", s.alerts);
    assert!(endless[0].what.starts_with("ENG-8080 (ENG-8080-annotate-design) never ends"), "{}", endless[0].what);
    assert!(endless[0].what.contains("reopened 1 times; the newest: pushed ENG-8080-annotate-design again"), "{}", endless[0].what);
    agent.call("p3", "Bash", json!({"command": "git push -q origin ENG-8080-annotate-design"}), 60).await;
    agent.result("p3", false, "", 2).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let endless = of(&s, Kind::EndlessTask);
    assert_eq!(endless.len(), 1, "once, kept up to date: {:?}", s.alerts);
    assert!(endless[0].what.contains("reopened 2 times"), "{}", endless[0].what);
    // The same merge read again is one merge; the next pull request merged is another.
    agent.call("f2", "Bash", json!({"command": "git log --oneline -1 origin/sandbox"}), 30).await;
    agent.result("f2", false, "3cf1a23 Merged in ENG-8080-annotate-design (pull request #108)", 1).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let task = s.tasks.iter().find(|t| t.branch == "ENG-8080-annotate-design").expect("the task");
    assert_eq!(task.tickets, ["ENG-8080"]);
    assert_eq!(task.merges.len(), 1, "read again, merged once");
    assert_eq!(task.estimates.first().map(|e| e.secs), Some(7_200));
    agent.call("f3", "Bash", json!({"command": "git log --oneline -2 origin/sandbox"}), 30).await;
    agent.result("f3", false, "77aa001 Merged in ENG-8080-annotate-design (pull request #109)\n3cf1a23 Merged in ENG-8080-annotate-design (pull request #108)", 1).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let task = s.tasks.iter().find(|t| t.branch == "ENG-8080-annotate-design").expect("the task");
    assert_eq!(task.merges.len(), 2, "pushed again and merged again");
    // A new version of a task merged is the task again, reopened; its first
    // push is the same reopening.
    agent.call("v", "Bash", json!({"command": "git switch -c ENG-8080-annotate-design-v2 origin/sandbox"}), 60).await;
    agent.result("v", false, "", 1).await;
    agent.call("p4", "Bash", json!({"command": "git push -u origin ENG-8080-annotate-design-v2"}), 60).await;
    agent.result("p4", false, "", 1).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let endless = of(&s, Kind::EndlessTask);
    assert_eq!(endless.len(), 1, "{:?}", s.alerts);
    assert!(endless[0].what.contains("reopened 3 times; the newest: made ENG-8080-annotate-design-v2, a new version of it"), "{}", endless[0].what);
    let task = s.tasks.iter().find(|t| t.branch == "ENG-8080-annotate-design").expect("the task");
    assert_eq!(task.versions, ["ENG-8080-annotate-design-v2"]);
    assert_eq!((task.merges[1].pull, task.merges[1].commit.as_deref()), (Some(109), Some("77aa001")));
    // The shared branch put back from a merge's commit undid that merge.
    agent.call("u", "Bash", json!({"command": "git push --force-with-lease=refs/heads/sandbox:77aa001 origin 3cf1a23:refs/heads/sandbox"}), 60).await;
    agent.result("u", false, " + 77aa001...3cf1a23 3cf1a23 -> sandbox (forced update)", 1).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let endless = of(&s, Kind::EndlessTask);
    assert!(endless[0].what.contains("the newest: undid its merge #109: sandbox put back from 77aa001 to 3cf1a23"), "{}", endless[0].what);

    // Your word alone is no merge: the merge you speak of may be another
    // session's. Nor is what comes after the agent's own merge a reopening.
    let (kernel, _dir, mut agent, _t0) = setup("relayed").await;
    agent.typed("build the watcher", 0).await;
    agent.call("b", "Bash", json!({"command": "git switch -c feat/573-watcher main"}), 3).await;
    agent.result("b", false, "", 1).await;
    agent.call("p", "Bash", json!({"command": "git push -u origin HEAD"}), 3).await;
    agent.result("p", false, "", 1).await;
    agent.typed("GAVE TO ME AS READY I MERGED AND NOW A WHOLE NEW LOOP OF ISSUES", 60).await;
    agent.said("The new alert fires when a task you took as done is reopened; one step remains: the deploy.", 10).await;
    agent.call("p2", "Bash", json!({"command": "git push origin HEAD"}), 30).await;
    agent.result("p2", false, "", 1).await;
    agent.call("o", "Bash", json!({"command": "gh pr create --title x --body-file b.md"}), 3).await;
    agent.result("o", false, "https://github.com/k8nstantin/superx/pull/574", 1).await;
    agent.typed("merge it", 30).await;
    agent.call("m", "Bash", json!({"command": "gh pr merge 574 --squash --admin"}), 3).await;
    agent.result("m", false, "✓ Squashed and merged pull request #574", 1).await;
    agent.said("Merged. The deploy remains.", 2).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert!(of(&s, Kind::EndlessTask).is_empty(), "{:?}", s.alerts);
    let task = s.tasks.iter().find(|t| t.branch == "feat/573-watcher").expect("HEAD is the branch checked out");
    assert_eq!(task.merges.len(), 1, "the agent's merge alone");
    assert!(task.not_done.is_empty(), "{:?}", task.not_done);

    // A merge call that only offers to merge later merged nothing.
    let (kernel, _dir, mut agent, _t0) = setup("offered").await;
    agent.typed("go", 0).await;
    agent.call("b", "Bash", json!({"command": "git switch -c feat/1-x main"}), 3).await;
    agent.result("b", false, "", 1).await;
    agent.call("p", "Bash", json!({"command": "git push -u origin feat/1-x"}), 3).await;
    agent.result("p", false, "", 1).await;
    agent.call("m", "Bash", json!({"command": "gh pr merge --squash"}), 3).await;
    agent.result("m", false, "To have the pull request merged after all the requirements have been met, add the `--auto` flag.", 1).await;
    agent.call("p2", "Bash", json!({"command": "git push origin feat/1-x"}), 30).await;
    agent.result("p2", false, "", 1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert!(of(&s, Kind::EndlessTask).is_empty(), "{:?}", s.alerts);
}

/// A task file in the form the specs write it: two local gates, two door
/// gates that each expect `exit 0`, what the build owns and what it may not
/// touch, its verbs forbidden from the checkout and the bucket forbidden.
const TASK_FILE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<task id="t10" name="stage 8 relationships" jira="ENG-9999" command="python -m kn stage run 8 &lt;archive&gt;; python -m kn stage verify 8 &lt;node&gt;"
      builds="gryphon-kn/kn/stages/s08_relationships.py gryphon-kn/tests/test_s08_relationships.py">
  <start>Cut the branch ENG-9999-stage-8-v0-&lt;model&gt;-&lt;effort&gt; from the head of sandbox. Every commit message starts with ENG-9999 and carries no attribution trailer. Build what this file says, run its gates, push the branch, and hand the operator the PR into sandbox; origin is Bitbucket, and the operator opens the PR.</start>
  <gates>
    <gate id="Lint" kind="local" proves="the rules"><cmd>python -m kn.lint --check</cmd><expect>the last line is "0 violations"</expect></gate>
    <gate id="Writing relationships" kind="door" proves="steps 1 to 6">
      <cmd>gcloud run jobs execute &lt;door.stage_jobs.8.run&gt; --project &lt;project.gcp_project&gt; --args stage,run,8,&lt;nodes&gt; --wait</cmd>
      <cmd>gcloud storage cat 'gs://&lt;bucket.root&gt;/runs/&lt;run id&gt;/*.log'</cmd>
      <expect>one batch log, no finding, and ends with `exit 0`.</expect>
    </gate>
    <gate id="Verifying relationships" kind="door" proves="step 7">
      <cmd>gcloud run jobs execute &lt;door.stage_jobs.8.verify&gt; --project &lt;project.gcp_project&gt; --args stage,verify,8,&lt;nodes&gt; --wait</cmd>
      <expect>one `&lt;node&gt;: ok` line per node, and ends with `exit 0`.</expect>
    </gate>
  </gates>
  <owns>gryphon-kn/kn/stages/s08_relationships.py; gryphon-kn/kn/core/door.py.</owns>
  <forbid>gryphon-kn/schema/**; every other file of gryphon-kn/kn/** and of gryphon-kn/tests/**; kn-specs/**. No run of run or verify from the checkout, and no call from it that reaches the bucket: the agent writes, copies and removes nothing in the bucket itself.</forbid>
</task>"#;

/// A file as Claude Code's Read gives it: each line numbered.
fn numbered(text: &str) -> String {
    text.lines().enumerate().map(|(i, l)| format!("{:>6}\t{l}", i + 1)).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn a_build_that_breaks_its_task_file_is_one_alert_per_break_and_one_that_keeps_it_is_none() {
    let (kernel, dir, mut agent, _t0) = setup("build").await;
    let p = params(&[]);
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    let door = |args: &str| format!("gcloud run jobs execute kn-s8 --project p --args {args} --wait");
    agent.typed("Build kn-specs/tasks/ENG-9999.xml", 0).await;
    agent.call("r", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml")}), 2).await;
    agent.result("r", false, &numbered(TASK_FILE), 1).await;
    // Its start's git: the branch it names, its commits, the PR yours.
    agent.call("g1", "Bash", json!({"command": "git switch -c ENG-9999-stage-8-v0-sonnet-5-5-max"}), 5).await;
    agent.result("g1", false, "", 1).await;
    agent.call("g2", "Bash", json!({"command": "python -m kn.lint --check; echo exit=$?"}), 5).await;
    agent.result("g2", false, "0 violations\nexit=0", 1).await;
    agent.call("g3", "Bash", json!({"command": "git commit -q -m 'fix the door'"}), 5).await;
    agent.result("g3", false, "", 1).await;
    agent.call("g4", "Bash", json!({"command": "git commit -q -F - <<'EOF'\nENG-9999 step 8\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nEOF"}), 5).await;
    agent.result("g4", false, "", 1).await;
    agent.call("g5", "Bash", json!({"command": "gh pr create --base sandbox --title x"}), 5).await;
    agent.result("g5", false, "https://bitbucket.org/x/y/pull-requests/7", 1).await;
    agent.call("g6", "Bash", json!({"command": "git switch -c fix/door"}), 5).await;
    agent.result("g6", false, "", 1).await;
    agent.call("e1", "Edit", json!({"file_path": at("gryphon-kn/kn/stages/s08_relationships.py"), "old_string": "a", "new_string": "b"}), 30).await;
    agent.result("e1", false, "ok", 1).await;
    agent.call("e2", "Edit", json!({"file_path": at("gryphon-kn/schema/relationships.sql"), "old_string": "a", "new_string": "b"}), 30).await;
    agent.result("e2", false, "ok", 1).await;
    agent.call("s", "Bash", json!({"command": "cd gryphon-kn && sed -i '' 's/a/b/' kn/core/config.py"}), 30).await;
    agent.result("s", false, "", 1).await;
    // Python's caches and a script's own text change no file of the repository.
    agent.call("c1", "Bash", json!({"command": "cd gryphon-kn && rm kn/__pycache__/door.cpython-314.pyc"}), 30).await;
    agent.result("c1", false, "", 1).await;
    agent.call("c2", "Bash", json!({"command": "cd gryphon-kn/schema && python3 -I - <<'EOF'\nif len(x) > 3: print(x)\nEOF"}), 30).await;
    agent.result("c2", false, "", 1).await;
    // A launch that started no execution ran nothing: its relaunch is the gate's run.
    agent.call("w0", "Bash", json!({"command": door("stage,run,8,n1")}), 30).await;
    agent.result("w0", true, "ERROR: (gcloud.run.jobs.execute) There was a problem refreshing your current auth tokens", 1).await;
    agent.call("w1", "Bash", json!({"command": door("stage,run,8,n1")}), 30).await;
    agent.result("w1", false, "Creating execution...done\nExecution [kn-x1] has successfully completed.", 600).await;
    agent.call("w2", "Bash", json!({"command": door("stage,run,8,n1")}), 30).await;
    agent.result("w2", false, "Creating execution...done\nExecution [kn-x2] has successfully completed.", 600).await;
    agent.call("v1", "Bash", json!({"command": format!("{} 2>&1 | tail -3", door("stage,verify,8,n1"))}), 30).await;
    agent.result("v1", false, "Creating execution...done\nERROR: (gcloud.run.jobs.execute) The execution failed.", 300).await;
    agent.call("e3", "Edit", json!({"file_path": at("gryphon-kn/kn/stages/s08_relationships.py"), "old_string": "b", "new_string": "c"}), 30).await;
    agent.result("e3", false, "ok", 1).await;
    agent.typed("fix it", 30).await;
    agent.call("e4", "Edit", json!({"file_path": at("gryphon-kn/kn/stages/s08_relationships.py"), "old_string": "c", "new_string": "d"}), 30).await;
    agent.result("e4", false, "ok", 1).await;
    agent.call("x", "Bash", json!({"command": "python3 -m kn stage run 8 n1 2>&1 | tail -5"}), 30).await;
    agent.result("x", false, "exit 0", 1).await;
    agent.call("c", "Bash", json!({"command": "gcloud storage cp out.json gs://bucket/runs/x.json"}), 30).await;
    agent.result("c", false, "", 1).await;
    agent.call("l", "Bash", json!({"command": "gcloud storage cat 'gs://bucket/runs/r1/*.log'"}), 30).await;
    agent.result("l", false, "exit 0", 1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let broken: Vec<&str> = of(&s, Kind::TaskFileBroken).iter().map(|a| a.what.as_str()).collect();
    assert_eq!(broken.len(), 8, "{broken:#?}");
    assert!(broken.iter().any(|w| w.starts_with("cut the branch fix/door, not the one ENG-9999.xml names: ENG-9999-stage-8-v0-<model>-<effort>")), "{broken:#?}");
    assert!(broken.iter().any(|w| w.starts_with("a commit whose message carries an attribution trailer")), "the newest commit's break, one alert: {broken:#?}");
    assert!(broken.iter().any(|w| w.starts_with("opened or merged a pull request itself")), "{broken:#?}");
    assert!(
        broken.iter().any(|w| w.starts_with("changed 2 files: gryphon-kn/schema/relationships.sql, gryphon-kn/kn/core/config.py, which ENG-9999.xml forbids")),
        "one alert for its forbidden changes, a shell's from the folder it went to: {broken:#?}"
    );
    assert!(broken.iter().any(|w| w.starts_with("launched the door gate «Writing relationships» again")), "{broken:#?}");
    assert!(
        broken.iter().any(|w| w.starts_with("went on after the door gate «Verifying relationships» failed: changed gryphon-kn/kn/stages/s08_relationships.py")),
        "{broken:#?}"
    );
    assert!(broken.iter().any(|w| w.starts_with("ran «python -m kn stage run 8» from the checkout")), "{broken:#?}");
    assert!(broken.iter().any(|w| w.starts_with("wrote to the bucket, which ENG-9999.xml forbids")), "{broken:#?}");
    assert!(of(&s, Kind::RunAgain).is_empty(), "the gate run again is one alert: {:?}", s.alerts);
    let build = s.builds.first().expect("the build, as the page lists it");
    assert_eq!((build.file.as_str(), build.ticket.as_deref(), build.stopped), ("ENG-9999.xml", Some("ENG-9999"), None));
    assert_eq!(build.forbidden, ["gryphon-kn/schema/relationships.sql", "gryphon-kn/kn/core/config.py"]);
    assert_eq!(build.written, ["gryphon-kn/kn/stages/s08_relationships.py"]);
    let gates: Vec<(&str, Option<bool>)> = build.gates.iter().map(|g| (g.gate.as_str(), g.ended_well)).collect();
    assert_eq!(gates, [("Writing relationships", Some(true)), ("Verifying relationships", Some(false))]);
    let locals: Vec<(&str, Option<bool>)> = build.locals.iter().map(|g| (g.gate.as_str(), g.passed)).collect();
    assert_eq!(locals, [("Lint", Some(true))]);
    assert_eq!(build.branch.as_deref(), Some("fix/door"));
    let commits: Vec<(&str, bool)> = build.commits.iter().map(|c| (c.line.as_str(), c.ok)).collect();
    assert_eq!(commits, [("fix the door", false), ("ENG-9999 step 8", false)]);

    // A build that hands you its turn before it wrote what its task file
    // builds stopped short, once; the task merged with that file, in the
    // session that wrote it, is reopened by it.
    let (kernel, dir, mut spec, t0) = setup("spec-merged").await;
    spec.typed("write the ENG-9999 task file", 0).await;
    spec.call("b", "Bash", json!({"command": "git switch -c ENG-9999-task-file origin/sandbox"}), 3).await;
    spec.result("b", false, "", 1).await;
    spec.call("w", "Write", json!({"file_path": dir.join("kn-specs/tasks/ENG-9999.xml").to_string_lossy(), "content": TASK_FILE}), 30).await;
    spec.result("w", false, "File created", 1).await;
    spec.call("p", "Bash", json!({"command": "git push -u origin ENG-9999-task-file"}), 60).await;
    spec.result("p", false, "", 1).await;
    spec.typed("merged", 60).await;
    spec.call("f", "Bash", json!({"command": "git fetch -q origin && git log --oneline -1 origin/sandbox"}), 5).await;
    spec.result("f", false, "1a347c3 Merged in ENG-9999-task-file (pull request #9)", 1).await;
    let mut build = Agent::new(&kernel, "build-stopped", &dir, t0 + TimeDelta::hours(1)).await;
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    build.typed("Build kn-specs/tasks/ENG-9999.xml", 0).await;
    build.call("r", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml")}), 2).await;
    build.result("r", false, &numbered(TASK_FILE), 1).await;
    build.call("e", "Edit", json!({"file_path": at("gryphon-kn/kn/core/door.py"), "old_string": "a", "new_string": "b"}), 300).await;
    build.result("e", false, "ok", 1).await;
    build.said("The build stopped on silences in the task file. The branch has step 8 only.", 30).await;
    build.turn_end(1).await;
    build.typed("ok", 60).await;
    build.said("Waiting for the revised file.", 5).await;
    build.turn_end(1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, build.at).await;
    let stopped = of(&s, Kind::BuildStopped);
    assert_eq!(stopped.len(), 1, "once a build: {:?}", s.alerts);
    assert_eq!(
        stopped[0].what,
        "the build of ENG-9999.xml handed you its turn before it wrote gryphon-kn/kn/stages/s08_relationships.py and gryphon-kn/tests/test_s08_relationships.py, which it builds"
    );
    assert!(stopped[0].evidence.first().is_some_and(|e| e.starts_with("The build stopped on silences")), "{:?}", stopped[0].evidence);
    assert!(s.builds.iter().any(|b| b.stopped == Some(stopped[0].at)), "{:?}", s.builds);
    let endless = of(&s, Kind::EndlessTask);
    assert_eq!(endless.len(), 1, "{:?}", s.alerts);
    assert!(endless[0].what.starts_with("ENG-9999 (ENG-9999-task-file) never ends"), "{}", endless[0].what);
    assert!(endless[0].what.contains("the newest: its build stopped short in"), "{}", endless[0].what);
    // Its task file changed and no gate passed after: merged unproven.
    let unproven = of(&s, Kind::UnprovenMerge);
    assert_eq!(unproven.len(), 1, "{:?}", s.alerts);
    assert!(unproven[0].what.starts_with("merged #9 of ENG-9999 (ENG-9999-task-file) after its task file changed"), "{}", unproven[0].what);
    let task = s.tasks.iter().find(|t| t.branch == "ENG-9999-task-file").expect("the spec's task");
    assert_eq!(task.merges[0].proven, Some(false));

    // A gate that passed after the change proves the merge.
    let (kernel, _dir, mut spec, _t0) = setup("spec-proven").await;
    spec.typed("write the ENG-9998 task file", 0).await;
    spec.call("b", "Bash", json!({"command": "git switch -c ENG-9998-task-file origin/sandbox"}), 3).await;
    spec.result("b", false, "", 1).await;
    spec.call("e", "Bash", json!({"command": "python3 -I fix.py && git add kn-specs/tasks/ENG-9998.xml && git commit -q -m 'ENG-9998 v1'"}), 30).await;
    spec.result("e", false, "", 1).await;
    spec.call("h", "Bash", json!({"command": "python3 ~/models_run/harness/ledger23.py check . 7d9f01a0 | tail -2"}), 600).await;
    spec.result("h", false, "1707 rows, 193 cases, 0 open, 0 defects\nGATE: PASSED", 1).await;
    spec.call("p", "Bash", json!({"command": "git push -u origin ENG-9998-task-file"}), 30).await;
    spec.result("p", false, "", 1).await;
    spec.call("f", "Bash", json!({"command": "git fetch -q origin && git log --oneline -1 origin/sandbox"}), 600).await;
    spec.result("f", false, "7d9f01a Merged in ENG-9998-task-file (pull request #12)", 1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, spec.at).await;
    assert!(of(&s, Kind::UnprovenMerge).is_empty(), "{:?}", s.alerts);
    let task = s.tasks.iter().find(|t| t.branch == "ENG-9998-task-file").expect("the spec's task");
    assert_eq!(task.merges[0].proven, Some(true));

    // A task file read in parts makes its contract once the parts are whole.
    let (kernel, dir, mut agent, _t0) = setup("build-parts").await;
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    let lines: Vec<&str> = TASK_FILE.lines().collect();
    let half = lines.len() / 2;
    let number = |from: usize, part: &[&str]| part.iter().enumerate().map(|(i, l)| format!("{:>6}\t{l}", from + i + 1)).collect::<Vec<_>>().join("\n");
    agent.typed("Build kn-specs/tasks/ENG-9999.xml", 0).await;
    agent.call("r1", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml"), "limit": half}), 2).await;
    agent.result("r1", false, &number(0, &lines[..half]), 1).await;
    agent.call("r2", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml"), "offset": half}), 2).await;
    agent.result("r2", false, &number(half - 1, &lines[half - 1..]), 1).await;
    agent.call("e", "Edit", json!({"file_path": at("gryphon-kn/schema/relationships.sql"), "old_string": "a", "new_string": "b"}), 30).await;
    agent.result("e", false, "ok", 1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert_eq!(of(&s, Kind::TaskFileBroken).len(), 1, "its forbid, from the second part: {:?}", s.alerts);
    let build = s.builds.first().expect("the build");
    assert_eq!(build.builds.len(), 2, "its head, from the first part: {build:?}");
    assert_eq!(build.owns, ["gryphon-kn/kn/stages/s08_relationships.py", "gryphon-kn/kn/core/door.py"], "a line both parts hold, once");

    // A build waiting on its own job has not stopped.
    let (kernel, dir, mut agent, _t0) = setup("build-waits").await;
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    agent.typed("Build kn-specs/tasks/ENG-9999.xml", 0).await;
    agent.call("r", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml")}), 2).await;
    agent.result("r", false, &numbered(TASK_FILE), 1).await;
    agent.job("j", "python -m unittest discover -s tests", 30).await;
    agent.said("The tests run in the background; I'll pick up when they end.", 2).await;
    agent.turn_end(1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert!(of(&s, Kind::BuildStopped).is_empty(), "{:?}", s.alerts);

    // A session that writes the task file, not one that builds from it, is
    // judged by none of it.
    let (kernel, dir, mut agent, _t0) = setup("spec").await;
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    agent.typed("write the ENG-9999 task file", 0).await;
    agent.call("r", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml")}), 2).await;
    agent.result("r", false, &numbered(TASK_FILE), 1).await;
    agent.call("e", "Edit", json!({"file_path": at("gryphon-kn/schema/relationships.sql"), "old_string": "a", "new_string": "b"}), 30).await;
    agent.result("e", false, "ok", 1).await;
    // The order to build is a session's first message; later it is talk of it.
    agent.typed("Build kn-specs/tasks/ENG-9999.xml is the line you will give it", 30).await;
    agent.call("r2", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml")}), 2).await;
    agent.result("r2", false, &numbered(TASK_FILE), 1).await;
    agent.call("e2", "Edit", json!({"file_path": at("gryphon-kn/schema/relationships.sql"), "old_string": "b", "new_string": "c"}), 30).await;
    agent.result("e2", false, "ok", 1).await;
    agent.said("Done.", 2).await;
    agent.turn_end(1).await;
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert!(of(&s, Kind::TaskFileBroken).is_empty() && of(&s, Kind::BuildStopped).is_empty(), "{:?}", s.alerts);

    // A door gate that gives no command of its own, `G5 repeats G3 on
    // purpose`, takes one launch of the file's work.
    let (kernel, dir, mut agent, _t0) = setup("build-repeats").await;
    let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    let repeats = TASK_FILE.replace(
        "</gates>",
        r#"<gate id="Writing again" kind="door" proves="the run is repeatable"><cmd>diff first.log second.log</cmd><expect>no difference, exit 0</expect></gate></gates>"#,
    );
    agent.typed("Build kn-specs/tasks/ENG-9999.xml", 0).await;
    agent.call("r", "Read", json!({"file_path": at("kn-specs/tasks/ENG-9999.xml")}), 2).await;
    agent.result("r", false, &numbered(&repeats), 1).await;
    for id in ["w1", "w2"] {
        agent.call(id, "Bash", json!({"command": door("stage,run,8,n1")}), 30).await;
        agent.result(id, false, "Creating execution...done\nExecution [kn-x] has successfully completed.", 60).await;
    }
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, agent.at).await;
    assert!(of(&s, Kind::TaskFileBroken).is_empty(), "{:?}", s.alerts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_api_leaving_a_session_without_an_answer_is_one_alert_that_says_it_all_and_its_time_is_the_apis() {
    // 2026-11-05: a build waiting on one answer the operator watched as
    // "thinking"; the network went, the request was sent again, the
    // operator interrupted and asked again, and Claude Code gave up.
    let p = params(&[]);
    let (kernel, dir, mut build, t0) = setup("api-build").await;
    let error = "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)";
    build.typed("Build it", 0).await;
    build.asked("r1", None, false, 1).await;
    build.call("c1", "Read", json!({"file_path": "/x/a.py"}), 5).await;
    build.result("c1", false, "a", 1).await;
    build.asked("r2", Some("r1"), false, 1).await;
    // Six minutes on one answer with nothing sent again is the model at work:
    // its card says so, and no alert.
    build.wait(360);
    let mut w = Watcher::new();
    let s = look(&mut w, &kernel, &p, build.at).await;
    assert!(of(&s, Kind::ApiStalled).is_empty(), "{:?}", s.alerts);
    let now = s.now.iter().find(|n| n.name == "claude_code/api-build").expect("the build, now");
    assert_eq!(now.state, "waiting on the API");
    let waits = now.api.as_ref().expect("its wait");
    assert_eq!((waits.resent, waits.model.as_deref(), waits.effort.as_deref()), (0, Some("claude-sonnet-5-5"), Some("max")));
    // The connection gone: sent again, the operator's stop and question,
    // sent again, and Claude Code gives up.
    build.wait(300);
    build.asked("r3", Some("r2"), true, 0).await;
    build.asked("r4", Some("r3"), true, 75).await;
    build.message(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}}), 21).await;
    build.typed("status", 0).await;
    build.asked("r5", Some("r4"), false, 1).await;
    build.asked("r6", Some("r5"), true, 75).await;
    build.asked("r7", Some("r6"), true, 30).await;
    build.api_error(error, 40).await;
    build.turn_end(1).await;
    // Another session the API left without an answer in the same minutes.
    let mut other = Agent::new(&kernel, "api-other", &dir, t0 + TimeDelta::seconds(700)).await;
    other.typed("go on", 0).await;
    other.asked("o1", None, false, 1).await;
    other.asked("o2", Some("o1"), true, 75).await;
    other.api_error(error, 60).await;
    other.turn_end(1).await;
    // A long answer with nothing sent again was the model's work, and its
    // time thinking, past the quarter hour a reply is trusted to take.
    let mut long = Agent::new(&kernel, "api-long", &dir, t0 + TimeDelta::hours(2)).await;
    long.typed("think hard", 0).await;
    long.asked("l1", None, false, 1).await;
    long.thought(16 * 60).await;
    long.said("Here.", 5).await;
    long.turn_end(0).await;
    // A request sent again at once and answered: no alert.
    let mut blip = Agent::new(&kernel, "api-blip", &dir, t0 + TimeDelta::hours(3)).await;
    blip.typed("go", 0).await;
    blip.asked("b1", None, false, 1).await;
    blip.asked("b2", Some("b1"), true, 3).await;
    blip.said("Done.", 4).await;
    blip.turn_end(0).await;
    // Claude Code giving up twice within minutes, an answer between: one alert.
    let login = "Login expired · Please run /login";
    let mut twice = Agent::new(&kernel, "api-twice", &dir, t0 + TimeDelta::hours(4)).await;
    twice.typed("go", 0).await;
    twice.asked("t1", None, false, 1).await;
    twice.api_error(login, 2).await;
    twice.turn_end(1).await;
    twice.typed("again", 20).await;
    twice.asked("t2", Some("t1"), false, 1).await;
    twice.said("On it.", 3).await;
    twice.turn_end(0).await;
    twice.typed("more", 30).await;
    twice.asked("t3", Some("t2"), false, 1).await;
    twice.api_error(login, 2).await;
    twice.turn_end(1).await;
    // A request sent again two hours on: Claude Code was not running (the
    // machine slept), the time is the operator's and the silence starts over.
    let mut slept = Agent::new(&kernel, "api-slept", &dir, t0 + TimeDelta::hours(5)).await;
    slept.typed("go", 0).await;
    slept.asked("z1", None, false, 1).await;
    slept.asked("z2", Some("z1"), true, 7200).await;
    slept.said("Done.", 5).await;
    slept.turn_end(0).await;
    // A request sent again at once and then nothing for two hours: the
    // session was left; the request after starts over and is answered.
    let mut left = Agent::new(&kernel, "api-left", &dir, t0 + TimeDelta::hours(6)).await;
    left.typed("look", 0).await;
    left.asked("y1", None, false, 1).await;
    left.asked("y2", Some("y1"), true, 1).await;
    left.typed("Ok lets build", 7200).await;
    left.asked("y3", Some("y2"), false, 1).await;
    left.said("Building.", 15).await;
    left.turn_end(1).await;
    let end = left.at;
    let s = look(&mut w, &kernel, &p, end).await;
    let api = of(&s, Kind::ApiStalled);
    let named = |n: &str| api.iter().filter(|a| a.name == n).copied().collect::<Vec<_>>();
    let b = named("claude_code/api-build");
    assert_eq!(b.len(), 1, "one alert for the stretch: {api:?}");
    assert_eq!(
        b[0].what,
        format!(
            "No answer from the API for 15 min: Claude Code sent its request again 4 times, you interrupted it, Claude Code gave up: «{error}». \
             1 other session had no answer in the same minutes: the connection or the API, not the agent."
        )
    );
    assert_eq!(b[0].evidence, [error]);
    let u = &b[0].stretches[..];
    assert_eq!(u.len(), 1);
    assert_eq!((u[0].sent, u[0].resent, u[0].failures), (6, 4, 1), "every request of the stretch, the ones sent again, the giving up");
    assert!(u[0].interrupted.is_some() && u[0].turn_ended.is_some() && u[0].answered.is_none(), "{:?}", u[0]);
    assert_eq!(b[0].alongside.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["claude_code/api-other"]);
    let o = named("claude_code/api-other");
    assert!(o.len() == 1 && o[0].what.contains("1 other session had no answer in the same minutes"), "{o:?}");
    for quiet in ["claude_code/api-long", "claude_code/api-blip", "claude_code/api-slept", "claude_code/api-left"] {
        assert!(named(quiet).is_empty(), "{quiet}: {api:?}");
    }
    let t = named("claude_code/api-twice");
    assert_eq!(t.len(), 1, "{api:?}");
    assert_eq!(t[0].stretches.len(), 2);
    assert_eq!(t[0].what, format!("No answer from the API 2 times in 1 min: Claude Code gave up 2 times, the last: «{login}»."));
    assert!(of(&s, Kind::BuildStopped).is_empty(), "a turn the API ended is not the agent stopping: {:?}", s.alerts);
    // The build's wait was the API's, not the operator's; the long answer
    // was thinking; the machine asleep was the operator's.
    let session = |name: &str| s.sessions.iter().find(|t| t.name == name).map(|t| t.id.clone()).expect("a session");
    let (build_split, _) = total(&w, |r| r.session == session("claude_code/api-build"));
    assert_eq!(build_split.api, 903, "from the line before the wait to Claude Code giving up: {build_split:?}");
    assert_eq!(build_split.operator, 0, "{build_split:?}");
    let (long_split, _) = total(&w, |r| r.session == session("claude_code/api-long"));
    assert_eq!((long_split.thinking, long_split.operator, long_split.api), (16 * 60 + 1, 0, 0), "{long_split:?}");
    let (slept_split, _) = total(&w, |r| r.session == session("claude_code/api-slept"));
    assert_eq!((slept_split.api, slept_split.operator), (0, 7201), "{slept_split:?}");
    let (left_split, _) = total(&w, |r| r.session == session("claude_code/api-left"));
    assert_eq!((left_split.api, left_split.operator), (2, 7200), "the two seconds to its request sent again, then yours: {left_split:?}");
    let steps = w.lines(&session("claude_code/api-build"), t0, end).expect("its lines");
    assert_eq!(steps.lines.iter().filter(|l| l.step.kind == StepKind::Api).count(), 5, "four sent again and the giving up");
}
