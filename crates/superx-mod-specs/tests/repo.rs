//! The read-only crawler: what a build branch's repository shows of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, NewMessage, SCHEMA_DDL as KERNEL_DDL};
use superx_mod_specs::repo::{host_of, merge_subject};
use superx_mod_specs::{crawl, ledger, store, SCHEMA_DDL};

#[test]
fn a_host_is_read_from_the_remote_whatever_its_form() {
    assert_eq!(host_of("git@bitbucket.org:gryphonnetworks/gryphon-data-lake.git").as_deref(), Some("bitbucket.org"));
    assert_eq!(host_of("https://github.com/k8nstantin/superx.git").as_deref(), Some("github.com"));
    assert_eq!(host_of("ssh://git@git.example.com:2222/o/r.git").as_deref(), Some("git.example.com"));
    assert_eq!(host_of(""), None);
}

#[test]
fn a_merge_subject_names_its_branch_and_its_pull_request() {
    assert_eq!(
        merge_subject("Merged in ENG-7811-spec-v0-harness-fixes (pull request #58)"),
        Some(("ENG-7811-spec-v0-harness-fixes".to_string(), Some("#58".to_string()))),
        "Bitbucket"
    );
    assert_eq!(
        merge_subject("Merge pull request #12 from k8nstantin/feat/446-ledger"),
        Some(("feat/446-ledger".to_string(), Some("#12".to_string()))),
        "GitHub"
    );
    assert_eq!(
        merge_subject("Merge branch 'ENG-8071-source-v5-sonnet-5-5-max' into sandbox: the source tool"),
        Some(("ENG-8071-source-v5-sonnet-5-5-max".to_string(), None))
    );
    assert_eq!(merge_subject("Merge remote-tracking branch 'origin/ENG-1-x' into main"), Some(("ENG-1-x".to_string(), None)));
    assert_eq!(merge_subject("ENG-7811 spec v0: ENG-8072 file version 7"), None);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn snapshot(root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in std::fs::read_dir(root).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            snapshot(&path, out);
        } else {
            out.insert(path.clone(), std::fs::read(&path).expect("read file"));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_crawler_reads_a_merge_on_the_host_and_its_revert_and_writes_nothing() {
    // A repository and its remote: the build branch merged by a pull
    // request on the host (Bitbucket's subject), then the merge reverted.
    let base = std::env::temp_dir().join(format!(
        "superx-crawl-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let origin = base.join("origin.git");
    let work = base.join("work");
    std::fs::create_dir_all(&base).expect("base");
    git(&base, &["init", "-q", "--bare", origin.to_str().expect("path")]);
    git(&base, &["clone", "-q", origin.to_str().expect("path"), work.to_str().expect("path")]);
    std::fs::write(work.join("a.txt"), "one\n").expect("write");
    git(&work, &["add", "a.txt"]);
    git(&work, &["commit", "-q", "-m", "first"]);
    git(&work, &["push", "-q", "origin", "main"]);
    // What a clone of a non-empty remote has: origin/HEAD, the mainline
    // when attr_specs_mainline_refs is unset.
    git(&work, &["remote", "set-head", "origin", "main"]);
    let branch = "ENG-1-the-tool-v3-sonnet-5-5-max";
    git(&work, &["checkout", "-q", "-b", branch]);
    std::fs::write(work.join("tool.py"), "x\n").expect("write");
    git(&work, &["add", "tool.py"]);
    git(&work, &["commit", "-q", "-m", "ENG-1 the tool"]);
    git(&work, &["push", "-q", "origin", branch]);
    git(&work, &["checkout", "-q", "main"]);
    git(&work, &["merge", "-q", "--no-ff", "-m", &format!("Merged in {branch} (pull request #7)"), branch]);
    let merge = git(&work, &["rev-parse", "HEAD"]);
    git(&work, &["revert", "-m", "1", "--no-edit", &merge]);
    git(&work, &["push", "-q", "origin", "main"]);
    // A branch cut after the merge holds it too, and is no mainline.
    git(&work, &["checkout", "-q", "-b", "later-work"]);
    git(&work, &["push", "-q", "origin", "later-work"]);
    git(&work, &["checkout", "-q", "main"]);

    // The build, as the ledger knows it: its kickoff line was sent from the
    // repository.
    let kdb = surrealdb::engine::any::connect("mem://").await.expect("mem");
    kdb.use_ns("superx").use_db("kernel").await.expect("nsdb");
    kdb.query(KERNEL_DDL.replace("$SUPERX_KERNEL_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");
    let kernel = Kernel::from_db(kdb);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await.expect("seed");
    }
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let mut desc = Object::new();
    desc.insert("name".to_string(), Value::String("claude_code/b1".to_string()));
    desc.insert("session".to_string(), Value::String("b1".to_string()));
    kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(desc)).await.expect("desc");
    let kickoff = "Build specs/tasks/ENG-1.xml";
    kernel
        .log_message(NewMessage {
            session: session.clone(),
            agent,
            role: "user".to_string(),
            content: kickoff.to_string(),
            raw: Some(superx_kernel::message::json_to_object(&json!({
                "type": "user", "origin": {"kind": "human"}, "cwd": work.to_string_lossy(),
                "message": {"role": "user", "content": kickoff}}))),
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("kickoff");
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");
    store::create_run(
        &db,
        &store::Run {
            session,
            session_key: "b1".to_string(),
            kickoff: kickoff.to_string(),
            task_path: "specs/tasks/ENG-1.xml".to_string(),
            task_doc: None,
            root_doc: None,
            rules_doc: None,
            key: Some("ENG-1".to_string()),
            version: Some(3),
            branch: Some(branch.to_string()),
            label: Some("sonnet-5-5-max".to_string()),
            model: Some("claude-sonnet-5-5".to_string()),
            reads: Vec::new(),
            first_write: None,
            started_at: chrono::Utc::now(),
        },
    )
    .await
    .expect("run");

    // Crawl, and prove it wrote nothing in the repository.
    let mut before = BTreeMap::new();
    snapshot(&work, &mut before);
    let report = crawl::crawl(&kernel, &db).await.expect("crawl");
    let mut after = BTreeMap::new();
    snapshot(&work, &mut after);
    assert!(before == after, "the crawl changed the repository");
    assert_eq!((report.repos, report.branches, report.written, report.unreadable), (1, 1, 1, 0), "{report:?}");

    let tasks = ledger::build(&db, Some("ENG-1")).await.expect("ledger");
    let b = &tasks[0].versions[0].builds[0];
    let repo = b.repo.as_ref().expect("the repository's view");
    assert_eq!(repo.pull_request.as_deref(), Some("#7"));
    assert_eq!(repo.merge_commit.as_deref(), Some(merge.as_str()));
    assert_eq!(repo.merged_into, ["origin/main"], "into the mainline only, not every branch that holds it");
    assert!(repo.revert_commit.is_some(), "the revert names the merge it reverts");
    assert!(repo.text.contains("merged by pull request #7 into origin/main"), "{}", repo.text);
    assert!(repo.text.contains("then reverted"), "{}", repo.text);
    assert!(!b.merged, "a merge the repository reverted is not merged");
    assert_eq!(tasks[0].summary.merged, 0);

    // Nothing changed: a second crawl writes nothing.
    let again = crawl::crawl(&kernel, &db).await.expect("crawl again");
    assert_eq!((again.written, again.unchanged), (0, 1), "{again:?}");
    let _ = std::fs::remove_dir_all(&base);
}
