//! The spec format and the read-order rule, on fixtures in the shape of
//! the build spec format.

use chrono::Utc;
use superx_mod_specs::read_order::{self, Context, Step};
use superx_mod_specs::session::Read;
use superx_mod_specs::spec::{self, Kind};

const ROOT: &str = include_str!("fixtures/root.xml");
const RULES: &str = include_str!("fixtures/rules.xml");
const TASK: &str = include_str!("fixtures/task.xml");

#[test]
fn a_spec_file_is_known_by_its_document_element() {
    let root = spec::parse(ROOT).expect("root parses");
    assert_eq!(root.kind, Kind::Root);
    assert_eq!(root.platform.as_deref(), Some("app/core/"));
    assert!(root.method_step("read").expect("read step").contains("in this order"));
    assert!(root.clauses.iter().all(|c| c.ident.as_deref() != Some("note")), "a note is no step");
    assert_eq!(
        root.method_step("ownership"),
        Some("Create, modify and touch only the paths the task file lists under owns ."),
        "an inline element's text is part of the step's words"
    );

    let rules = spec::parse(RULES).expect("rules parse");
    assert_eq!(rules.kind, Kind::Rules);
    let ids: Vec<_> = rules.clauses.iter().map(|c| c.ident.as_deref().unwrap_or("")).collect();
    assert_eq!(ids, ["kickoff", "one-door"]);
    assert_eq!(rules.clauses[1].text, "Every command runs through the one door.", "a rule's words are its text");

    let task = spec::parse(TASK).expect("task parses");
    assert_eq!(task.kind, Kind::Task);
    assert_eq!((task.key.as_deref(), task.name.as_deref(), task.version), (Some("ENG-1"), Some("the tool"), Some(3)));
    assert_eq!(task.named, ["design/a.json", "config/"]);
    let kinds: Vec<_> = task.clauses.iter().map(|c| (c.kind, c.ident.as_deref())).collect();
    assert_eq!(
        kinds,
        [
            ("start", None),
            ("reflects", Some("design/a.json")),
            ("reads", Some("config/")),
            ("owns", None),
            ("forbid", None),
            ("agent-decides", None),
            ("gate", Some("G1")),
            ("gate", Some("G2")),
            ("done", None),
        ]
    );

    assert!(spec::parse("<project><x/></project>").is_none(), "another XML file is not a spec file");
    assert!(spec::parse("not xml").is_none());
}

#[test]
fn a_kickoff_line_names_one_file_and_nothing_else() {
    assert_eq!(spec::kickoff_task_path("Build kn-specs/tasks/ENG-8072.xml").as_deref(), Some("kn-specs/tasks/ENG-8072.xml"));
    assert_eq!(spec::kickoff_task_path("Build kn-specs/tasks/ENG-8072.xml.").as_deref(), Some("kn-specs/tasks/ENG-8072.xml"));
    assert_eq!(spec::kickoff_task_path("  `tasks/ENG-1.xml`  ").as_deref(), Some("tasks/ENG-1.xml"));
    assert_eq!(spec::kickoff_task_path("lets continue"), None, "a conversation is no kickoff");
    assert_eq!(spec::kickoff_task_path("compare a.xml with b.xml"), None, "two files is no kickoff");
    assert_eq!(spec::kickoff_task_path("Build tasks/ENG-1.xml\nand also this"), None, "a kickoff is one line");
}

#[test]
fn the_order_is_the_roots_own_words() {
    let root = spec::parse(ROOT).expect("root");
    let steps = read_order::steps(root.method_step("read").expect("read"), root.platform.as_deref()).expect("order");
    assert_eq!(steps, [Step::Platform, Step::Root, Step::Rules, Step::Task, Step::Named]);

    let reordered = "Then, in this order: this root; rules.xml; your task file; the code under app/core.";
    assert_eq!(
        read_order::steps(reordered, Some("app/core/")).expect("order"),
        [Step::Root, Step::Rules, Step::Task, Step::Platform],
        "a root that reorders its list is judged by its own list"
    );
    assert_eq!(read_order::steps("Read whatever helps.", Some("app/core/")), None, "no list, no judgement");
}

fn read(path: &str, n: usize, whole: bool) -> Read {
    Read { path: path.to_string(), whole, content: None, tool_use_id: Some(format!("toolu_{n}")), at: Utc::now() }
}

/// Judge a sequence of reads against the fixture root's order.
fn judge(paths: &[(&str, bool)]) -> read_order::Judgement {
    let root = spec::parse(ROOT).expect("root");
    let task = spec::parse(TASK).expect("task");
    let steps = read_order::steps(root.method_step("read").expect("read"), root.platform.as_deref()).expect("order");
    let reads: Vec<Read> = paths.iter().enumerate().map(|(i, (p, whole))| read(p, i, *whole)).collect();
    let refs: Vec<&Read> = reads.iter().collect();
    let kind_of = |p: &str| {
        if p.ends_with("/root.xml") {
            Some(Kind::Root)
        } else if p.ends_with("/rules.xml") {
            Some(Kind::Rules)
        } else if p.contains("/tasks/") {
            Some(Kind::Task)
        } else {
            None
        }
    };
    let cx = Context { platform: root.platform.as_deref(), task_path: "specs/tasks/ENG-1.xml", named: &task.named, kind_of: &kind_of };
    read_order::judge(&refs, &steps, true, &cx)
}

const TASK_FILE: (&str, bool) = ("/r/specs/tasks/ENG-1.xml", true);
const PLATFORM: (&str, bool) = ("/r/app/core/door.py", true);
const ROOT_FILE: (&str, bool) = ("/r/specs/root.xml", true);
const RULES_FILE: (&str, bool) = ("/r/specs/rules.xml", true);
const NAMED: (&str, bool) = ("/r/config/app.json", true);

#[test]
fn reading_in_the_roots_order_complies() {
    let j = judge(&[TASK_FILE, PLATFORM, ("/r/app/core/run.py", true), ROOT_FILE, RULES_FILE, TASK_FILE, ("/r/README.md", true), NAMED]);
    assert_eq!(j.result, "complied", "{}", j.note);
    assert!(j.evidence.is_empty());
}

#[test]
fn the_platform_after_the_spec_breaks_the_order() {
    // The haiku ENG-8071 v3 shape: root, rules, task, then the platform —
    // and no second pass.
    let j = judge(&[ROOT_FILE, RULES_FILE, TASK_FILE, PLATFORM, NAMED]);
    assert_eq!(j.result, "breached");
    assert!(j.note.contains("did not read, in order: root.xml, rules.xml, the task file"), "{}", j.note);
}

#[test]
fn starting_again_from_the_first_file_mends_the_order() {
    let j = judge(&[TASK_FILE, ROOT_FILE, PLATFORM, ROOT_FILE, RULES_FILE, TASK_FILE, NAMED]);
    assert_eq!(j.result, "complied", "{}", j.note);
    assert!(j.note.contains("after starting again"), "{}", j.note);
}

#[test]
fn an_earlier_step_after_a_later_one_breaks_the_order() {
    let j = judge(&[PLATFORM, RULES_FILE, ROOT_FILE, TASK_FILE, NAMED]);
    assert_eq!(j.result, "breached");
    assert!(j.note.contains("specs/root.xml after rules.xml, out of the root's order"), "{}", j.note);
    assert!(!j.note.contains("did not read"), "after a break, what follows is not counted, so nothing is listed missing: {}", j.note);
    assert_eq!(j.evidence, ["toolu_2"], "the read that broke the order is the evidence");
}

#[test]
fn another_tasks_spec_and_a_partial_read_are_breaches() {
    let j = judge(&[PLATFORM, ROOT_FILE, RULES_FILE, ("/r/specs/tasks/ENG-2.xml", true), TASK_FILE, NAMED]);
    assert_eq!(j.result, "breached");
    assert!(j.note.contains("another task's spec"), "{}", j.note);

    let j = judge(&[PLATFORM, ("/r/specs/root.xml", false), RULES_FILE, TASK_FILE, NAMED]);
    assert_eq!(j.result, "breached");
    assert!(j.note.contains("read part of a spec file"), "{}", j.note);
}

#[test]
fn no_reads_is_no_evidence() {
    assert_eq!(judge(&[]).result, "no-evidence");
}
