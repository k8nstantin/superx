//! The watcher (#573): recon's second part, tested on SurrealDB's in-memory
//! engine with lines written as the Claude Code adapter writes them, at the
//! times each test gives; every look is given the time it judges at, so no
//! test waits for a bound.

pub mod common;

use std::path::{Path, PathBuf};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use common::*;
use serde_json::{json, Value as Json};
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::Kernel;
use superx_mod_recon::instance::Instance;
use superx_mod_recon::params::{Params, WATCH_PLANS_PARAM};
use superx_mod_recon::server;
use superx_mod_recon::watcher::alerts::Kind;
use superx_mod_recon::watcher::{plan, Snapshot, Watched, Watcher};
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

    async fn turn_end(&mut self, after: i64) -> DateTime<Utc> {
        self.event(json!({"type": "system", "subtype": "turn_duration", "durationMs": 5}), after).await
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
    watcher.snapshot(p, false)
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
    let now = s.sessions.iter().find(|x| x.name == "claude_code/long").expect("live");
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
    let now = s.sessions.iter().find(|x| x.name == "claude_code/approve").expect("live");
    assert_eq!(now.state, "waiting on your approval");
    assert!(s.alerts.is_empty(), "{:?}", s.alerts);
    agent.result("tf", false, "Apply complete", 20 * 60).await;
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let approval: i64 = s.hours.iter().map(|h| h.at.split.approval).sum();
    assert_eq!(approval, 20 * 60, "the wait was the operator's approval, not the tool's run");
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
    let s = look(&mut w, &kernel, &p, agent.at).await;
    let mut t = superx_mod_recon::watcher::track::Split::default();
    for h in &s.hours {
        t.plus(&h.at.split);
    }
    assert_eq!(t.thinking, 5);
    assert_eq!(t.writing, 10 + 2 + 3 + 2 + 4, "words and calls");
    assert_eq!(t.tools, 30 + 1, "the test's run, and the job's start");
    assert_eq!(t.jobs, 600 + 3000, "your question while the job ran, and the job to its notice");
    assert_eq!(t.operator, 300 + 120 + 3600, "your next message, your answer, the session left");
    assert_eq!(t.approval, 0);
    let models = superx_mod_recon::watcher::measure::models(&s);
    assert_eq!(models[0].model, "claude-opus-5-5");
    assert_eq!(models[0].counts.prompts, 3);
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
    let s = look(&mut w, &kernel, &p, agent.at + TimeDelta::minutes(1)).await;
    let watched = Watched::default();
    watched.publish(s);
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
    assert_eq!(v["sessions"][0]["state"], "job running");
    assert!(v["sessions"][0]["jobs"][0]["expected_end"].is_string(), "when the same launch ended last time: {}", v["sessions"][0]);
    assert_eq!(v["runs"].as_array().map(Vec::len), Some(2), "{}", v["runs"]);
    assert_eq!(v["models"][0]["model"], "claude-opus-5-5");
    assert!(v["days"].as_array().is_some_and(|d| !d.is_empty() && d[0]["day"].as_str().is_some_and(|x| x.len() == 10)), "{}", v["days"]);
    let r = app.oneshot(ask("/api/watch?offset=100000")).await.expect("answer");
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}
