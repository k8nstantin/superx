//! Reading git out of shell calls, on the shapes the design and build
//! sessions actually use (gryphon-data-lake, 2026-10-09 to 10-13).

use superx_mod_specs::git::{self, BranchEvent};

fn events(command: &str) -> Vec<(String, &'static str)> {
    git::activity(command).branches.into_iter().map(|BranchEvent { branch, event, .. }| (branch, event)).collect()
}

#[test]
fn branch_events_are_read_from_the_commands_that_make_them() {
    assert_eq!(
        events("cd /r && git checkout -b ENG-8071-source-v3-haiku-5-5-max && git branch --show-current"),
        [("ENG-8071-source-v3-haiku-5-5-max".to_string(), "created")]
    );
    assert_eq!(
        events("set -uo pipefail; cd /r && git push origin --delete ENG-8072-destination-v5-sonnet-5-5-max ENG-8072-destination-v6-sonnet-5-5-max 2>&1 | tail -3"),
        [
            ("ENG-8072-destination-v5-sonnet-5-5-max".to_string(), "deleted-remote"),
            ("ENG-8072-destination-v6-sonnet-5-5-max".to_string(), "deleted-remote"),
        ]
    );
    assert_eq!(
        events("git branch -D ENG-8071-source-v3-haiku-5-5-max 2>/dev/null; git push -q origin --delete ENG-8071-source-v3-haiku-5-5-max > /dev/null"),
        [
            ("ENG-8071-source-v3-haiku-5-5-max".to_string(), "deleted"),
            ("ENG-8071-source-v3-haiku-5-5-max".to_string(), "deleted-remote"),
        ],
        "a redirection's file is no branch"
    );
    assert_eq!(events("git push -u origin ENG-1-tool-v3-x"), [("ENG-1-tool-v3-x".to_string(), "pushed")]);
    assert_eq!(events("git push origin :old-branch"), [("old-branch".to_string(), "deleted-remote")]);
    assert_eq!(events("git merge --no-ff -m \"merge it\" ENG-8071-source-v5-sonnet-5-5-max"), [(
        "ENG-8071-source-v5-sonnet-5-5-max".to_string(),
        "merged"
    )]);
    assert_eq!(events("git -C /r worktree add -b feat/x /tmp/w origin/main"), [("feat/x".to_string(), "created")]);
    assert!(events("for b in A B; do git branch -D $b; done").is_empty(), "a name built at run time is not guessed");
    assert!(events("git branch -a && git log --oneline -5").is_empty(), "a listing is no event");
    assert!(events("git push origin HEAD").is_empty(), "HEAD names no branch");
    assert!(events("git merge --no-commit --no-ff origin/ENG-8071-source-opus-5-5-max").is_empty(), "a trial merge merges nothing");
    assert_eq!(
        events("git merge --no-ff -q -m \"Merge branch 'x' into sandbox\" origin/ENG-8071-source-v5-sonnet-5-5-max"),
        [("origin/ENG-8071-source-v5-sonnet-5-5-max".to_string(), "merged")],
        "the ref as written; the ledger matches it to its branch"
    );
}

#[test]
fn text_inside_a_here_document_is_never_a_command() {
    let call = "cat > $S/msg.txt <<'EOF'\nENG-7811 spec v0: ENG-8072 file version 7\n\ngit branch -D not-a-command\nEOF\ngit commit -q -F $S/msg.txt";
    let a = git::activity(call);
    assert!(a.branches.is_empty(), "{:?}", a.branches);
    assert_eq!(a.commits.len(), 1);
    assert_eq!(a.commits[0].message_file.as_deref(), Some("$S/msg.txt"));
    let written = git::written_files(call);
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].0, "$S/msg.txt");
    assert!(written[0].1.starts_with("ENG-7811 spec v0: ENG-8072 file version 7"));
}

#[test]
fn a_commit_carries_the_message_it_was_given() {
    let a = git::activity("git add a && git commit -q -m \"ENG-8071 source v3: the door\" -m \"body\"");
    assert_eq!(a.commits[0].message, "ENG-8071 source v3: the door\n\nbody");

    let a = git::activity("git commit -q -F - <<'EOF'\nENG-7811 spec v0: ENG-8072 file version 1\nEOF");
    assert_eq!(a.commits[0].message, "ENG-7811 spec v0: ENG-8072 file version 1", "a here-document fed to -F -");
    assert_eq!(a.commits[0].message_file, None);
}

#[test]
fn a_revision_is_a_message_naming_a_tasks_file_version() {
    let r = git::revisions(
        "ENG-7811 spec v0: ENG-8072 file version 6 from the review of its v5 build: an exception of tables() at step 1 \
         is the finding statements (the operator, 2026-10-12)",
    );
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].key.as_str(), r[0].version, r[0].reviewed), ("ENG-8072", 6, Some(5)));

    let r = git::revisions("ENG-7811 spec v0: ENG-8072 file version 7\n\nThe operator's rulings of 2026-10-13");
    assert_eq!((r[0].version, r[0].reviewed), (7, None));

    assert!(git::revisions("task t02, the destination tool. File version 7 (2026-10-13).").is_empty(), "a file's own header names no key");
    assert!(git::revisions("bump the file version 3").is_empty(), "no key, no revision");
}

#[test]
fn printed_commits_are_read_from_a_oneline_log() {
    let c = git::printed_commits("aef287d7 ENG-7811 spec v0: ENG-8072 file version 6\nwrote kn-specs/build_spec.html\n");
    assert_eq!(c, [("aef287d7".to_string(), "ENG-7811 spec v0: ENG-8072 file version 6".to_string())]);
}
