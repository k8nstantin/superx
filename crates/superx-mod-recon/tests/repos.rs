//! The repositories crawled in depth (#538, steps 1 to 14), end to end:
//! repositories made with git in a scratch folder, a kernel holding the
//! sessions' lines, and the module's own database on the in-memory engine.
//! What a pass writes is read back from the database. One test per item of
//! the task's list, named by the item it proves.

pub mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::*;
use superx_kernel::types::Value;
use superx_mod_recon::crawler::Crawler;
use superx_mod_recon::params::{CRAWL_SECS_PARAM, EXCLUDE_PARAM, INCLUDE_PARAM, REPOS_AT_ONCE_PARAM};

/// How many crawls each repository has, by its name.
async fn crawls(db: &superx_kernel::Db) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for r in rows(db, "SELECT repository.name AS name FROM crawl").await {
        *out.entry(text(&r, "name").expect("a name")).or_default() += 1;
    }
    out
}

fn by_name(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
    pairs.iter().map(|(n, c)| ((*n).to_string(), *c)).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_crawls_in_depth_every_repository_at_a_full_pass_and_at_another_pass_the_new_ones_and_those_a_git_event_of_the_passs_lines_names_by_its_dir_and_no_other_an_excluded_repository_never()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (a, b, d, e) = (root.join("a"), root.join("b"), root.join("d"), root.join("e"));
    for r in [&a, &b, &d, &e] {
        repo(r);
        commit(r, &[("f.txt", "one\n")], "first");
    }
    let excluded = (EXCLUDE_PARAM, Value::Array(vec![Value::String("e".to_string())].into()));
    let hour = (CRAWL_SECS_PARAM, number(3600));
    let p = params(&[(INCLUDE_PARAM, strings(&[&a, &b, &e])), excluded.clone(), hour.clone()]);
    let mut crawler = Crawler::default();

    // A full pass: every repository on the list.
    let first = pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(first.outcome, "complete", "{first:?}");
    assert_eq!(crawls(&db).await, by_name(&[("a", 1), ("b", 1)]), "{first:?}");
    assert_eq!(first.source("repos").map(|s| s.crawls.len()), Some(2));

    // Another pass: nothing new, no git event, no crawl.
    let second = pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(second.source("repos").map(|s| s.crawls.len()), Some(0), "{second:?}");
    assert_eq!(crawls(&db).await, by_name(&[("a", 1), ("b", 1)]));

    // A repository new to the list: it alone.
    let p = params(&[(INCLUDE_PARAM, strings(&[&a, &b, &d, &e])), excluded.clone(), hour.clone()]);
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(crawls(&db).await, by_name(&[("a", 1), ("b", 1), ("d", 1)]));

    // A git event the pass's lines wrote in b's directory: b alone.
    let mut session = Capture::new(&kernel, "s1").await;
    session.bash(&b, "git commit -m x", "[main abc1234] x\n 1 file changed").await;
    let fourth = pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "git_event").await, 1, "{fourth:?}");
    assert_eq!(crawls(&db).await, by_name(&[("a", 1), ("b", 2), ("d", 1)]), "{fourth:?}");

    // A session working in a repository without a git event leads to no
    // crawl, and a full pass, here after a restart, crawls every one again;
    // the excluded repository never.
    session.worked_in(&a).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(crawls(&db).await, by_name(&[("a", 1), ("b", 2), ("d", 1)]));
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(crawls(&db).await, by_name(&[("a", 2), ("b", 3), ("d", 2)]));
    assert_eq!(count(&db, "repository").await, 3, "the excluded repository is never on the list");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The facts a pass wrote, without ids or times of writing: what two
/// crawlers must agree on.
async fn facts(db: &superx_kernel::Db) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (table, sql) in [
        ("commit", "SELECT repository.name AS r, hash, author, committer, author_name, committer_name, authored_at, committed_at, author_offset, committer_offset, subject, body, parents, parent_hashes, tree, added, removed, patch_id, signature FROM commit"),
        ("commit_file", "SELECT commit.hash AS c, path, change, added, removed, added_ignoring_space, removed_ignoring_space, binary FROM commit_file"),
        ("placement", "SELECT commit.hash AS c, place, first_parent FROM placement"),
        ("landed_via", "SELECT commit.hash AS c, path, via.hash AS v FROM landed_via"),
        ("survival", "SELECT commit.hash AS c, alive FROM survival"),
        ("ref_move", "SELECT repository.name AS r, checkout.toplevel AS t, ref, at, old, new, identity, action FROM ref_move"),
        ("remote_state", "SELECT repository.name AS r, name, url, push_url, host, path FROM remote_state"),
        ("checkout_state", "SELECT checkout.toplevel AS t, branch, first_branch, present, operations FROM checkout_state"),
        ("crawl", "SELECT repository.name AS r, mainline, mainline_from, identity, tip, outcome, commits_read, files_blamed, files_left, refs_read, files_read, reflog_read, unreachable_read, blame_lines_read, commits_new, commit_files_new, lines_added_new, lines_removed_new, head_moves_new, placements_changed, landed_new, survival_changed, survival_delta, git_failed FROM crawl"),
    ] {
        for r in rows(db, sql).await {
            out.push(format!("{table} {}", written(&r)));
        }
    }
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_repositories_crawled_with_attr_recon_repos_at_once_1_and_at_its_fallback_write_the_same_facts_each_crawls_rows_written_after_its_crawl_row()
{
    let kernel = kernel().await;
    let root = scratch();
    let names = ["r1", "r2", "r3", "r4"];
    let dirs: Vec<std::path::PathBuf> = names.iter().map(|n| root.join(n)).collect();
    for (i, dir) in dirs.iter().enumerate() {
        repo(dir);
        commit(dir, &[("a.txt", "a\nb\n")], "first");
        git(dir, &["switch", "-q", "-c", "side"]);
        commit(dir, &[("b.txt", &format!("side {i}\n"))], "side");
        git(dir, &["switch", "-q", "main"]);
        commit(dir, &[("a.txt", "a\nB\nc\n")], "second");
        git(dir, &["remote", "add", "origin", &format!("https://example.org/acme/{}.git", names[i])]);
    }
    let paths: Vec<&Path> = dirs.iter().map(std::path::PathBuf::as_path).collect();
    let one = params(&[(INCLUDE_PARAM, strings(&paths)), (REPOS_AT_ONCE_PARAM, number(1))]);
    let all = params(&[(INCLUDE_PARAM, strings(&paths))]);
    let (db_one, db_all) = (own_db().await, own_db().await);
    let first = pass(&kernel, &db_one, &one, &mut Crawler::default()).await;
    let second = pass(&kernel, &db_all, &all, &mut Crawler::default()).await;
    assert_eq!((first.outcome, second.outcome), ("complete", "complete"), "{first:?} {second:?}");
    assert_eq!(first.source("repos").map(|s| s.crawls.len()), Some(4));
    let (one_facts, all_facts) = (facts(&db_one).await, facts(&db_all).await);
    assert!(one_facts.len() > 40, "{one_facts:?}");
    assert_eq!(one_facts, all_facts, "one at a time and all at once write the same facts");

    // Each crawl's rows are written after it: none is valid before it.
    for db in [&db_one, &db_all] {
        for table in ["git_call", "remote_state", "placement", "survival", "ref_move"] {
            let early = rows(db, &format!("SELECT count() AS n FROM {table} WHERE valid_from < crawl.valid_from GROUP ALL")).await;
            assert!(early.first().and_then(|r| int(r, "n")).unwrap_or(0) == 0, "{table}: {early:?}");
            let linked = rows(db, &format!("SELECT count() AS n FROM {table} WHERE record::exists(crawl) GROUP ALL")).await;
            assert_eq!(linked.first().and_then(|r| int(r, "n")), Some(count(db, table).await), "{table}");
        }
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repository_with_no_present_checkout_is_crawled_through_its_common_git_directory_and_one_whose_common_git_directory_is_gone_is_not_crawled_and_keeps_what_the_crawler_holds_of_it()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (source, bare, tree) = (root.join("source"), root.join("bare.git"), root.join("tree"));
    repo(&source);
    let first = commit(&source, &[("a.txt", "a\n")], "first");
    git(&root, &["clone", "-q", "--bare", &source.to_string_lossy(), &bare.to_string_lossy()]);
    git(&bare, &["worktree", "add", "-q", &tree.to_string_lossy(), "main"]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&tree]))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(crawls(&db).await.values().sum::<i64>(), 1);

    // Its only checkout is gone: crawled through its common git directory.
    std::fs::remove_dir_all(&tree).expect("remove the checkout");
    let second = pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let present = rows(&db, "SELECT present, valid_from FROM checkout_state ORDER BY valid_from DESC LIMIT 1").await;
    assert_eq!(flag(&present[0], "present"), Some(false), "{second:?}");
    assert_eq!(crawls(&db).await.values().sum::<i64>(), 2, "{second:?}");
    let newest = rows(&db, "SELECT outcome, tip, valid_from FROM crawl ORDER BY valid_from DESC LIMIT 1").await;
    assert_eq!((text(&newest[0], "outcome"), text(&newest[0], "tip")), (Some("complete".into()), Some(first.clone())));
    let git_dir = rows(&db, "SELECT VALUE dir FROM git_call").await;
    assert!(git_dir.iter().all(|d| matches!(d, Value::String(s) if s.starts_with(&bare.to_string_lossy().into_owned()))), "{git_dir:?}");

    // Its common git directory is gone too: not crawled, and what is held
    // of it stays.
    let held = (count(&db, "commit").await, count(&db, "placement").await, count(&db, "crawl").await);
    std::fs::remove_dir_all(&bare).expect("remove the repository");
    let third = pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(third.source("repos").map(|s| s.crawls.len()), Some(0), "{third:?}");
    assert_eq!((count(&db, "commit").await, count(&db, "placement").await, count(&db, "crawl").await), held);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The subcommand a `git_call` row ran: its first argument past the
/// overrides.
fn subcommand(args: &[String]) -> String {
    let mut i = 0;
    while i < args.len() && args[i] == "-c" {
        i += 2;
    }
    args.get(i).cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_git_call_in_an_operators_repository_carries_the_overrides_no_diff_driver_and_no_optional_lock_never_a_g_placeholder_a_fetch_or_a_write_and_is_a_git_call_row_with_its_step_dir_args_as_run_exit_timed_out_bytes_out_lines_out_and_stderr_whole()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "one\ntwo\n")], "first");
    commit(&dir, &[("a.txt", "one\nTWO\nthree\n"), ("b.txt", "b\n")], "second");
    git(&dir, &["branch", "side"]);
    // HEAD on a branch with no commit yet, its index naming the empty tree
    // the repository does not hold: fsck prints `missing tree` and exits 2.
    // What it writes on its error output is the git's own (2.50 a notice
    // of the unborn branch, 2.56 nothing), so the row is held to it.
    git(&dir, &["switch", "-q", "--orphan", "fresh"]);
    let index = dir.join(".git").join("index");
    let before = std::fs::read(&index).ok();
    let modified = std::fs::metadata(&index).and_then(|m| m.modified()).ok();
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir]))]);
    let done = pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(done.source("repos").map(|s| s.crawls.len()), Some(1), "{done:?}");
    assert_eq!(std::fs::read(&index).ok(), before, "nothing written: the index is as it was");
    assert_eq!(std::fs::metadata(&index).and_then(|m| m.modified()).ok(), modified);

    let common = dir.join(".git").to_string_lossy().into_owned();
    let calls = rows(&db, "SELECT step, dir, args, started_at, ms, exit, timed_out, bytes_out, lines_out, stderr FROM git_call").await;
    assert!(calls.len() > 10, "{calls:?}");
    let reads = ["rev-parse", "config", "for-each-ref", "merge-base", "fsck", "log", "cat-file", "diff-tree", "patch-id", "reflog", "rev-list", "ls-tree", "blame"];
    let steps = ["setup", "refs", "unreachable", "log", "patches", "reflog", "placement", "landed", "tip", "blame"];
    for call in &calls {
        let args = texts(call, "args").expect("its args");
        assert_eq!(&args[..4], ["-c", "log.showSignature=false", "-c", "core.fsmonitor=false"], "{args:?}");
        let sub = subcommand(&args);
        assert!(reads.contains(&sub.as_str()), "a read, never a fetch or a write: {args:?}");
        let at = args.iter().position(|a| *a == sub).expect("the subcommand");
        match sub.as_str() {
            "log" | "diff-tree" => assert_eq!(&args[at + 1..at + 3], ["--no-textconv", "--no-ext-diff"], "{args:?}"),
            "blame" => assert_eq!(args[at + 1], "--no-textconv", "{args:?}"),
            _ => {}
        }
        assert!(args.iter().all(|a| !a.contains("%G")), "no format that verifies a signature: {args:?}");
        assert!(steps.contains(&text(call, "step").expect("its step").as_str()), "{call:?}");
        assert_eq!(text(call, "dir").as_deref(), Some(common.as_str()), "{call:?}");
        let exit = if sub == "fsck" { 2 } else { 0 };
        assert_eq!((int(call, "exit"), flag(call, "timed_out")), (Some(exit), Some(false)), "{call:?}");
        assert!(int(call, "ms").is_some() && field(call, "started_at").is_some());
    }
    let shallow = calls
        .iter()
        .find(|c| texts(c, "args").is_some_and(|a| a.contains(&"--is-shallow-repository".to_string())))
        .expect("the shallow question");
    assert_eq!((text(shallow, "step").as_deref(), int(shallow, "bytes_out"), int(shallow, "lines_out")), (Some("setup"), Some(6), Some(1)));
    assert_eq!(text(shallow, "stderr"), None, "nothing on its error output");
    let fsck = calls.iter().find(|c| texts(c, "args").is_some_and(|a| subcommand(&a) == "fsck")).expect("fsck");
    assert_eq!(text(fsck, "step").as_deref(), Some("unreachable"));
    let own = std::process::Command::new("git")
        .arg("-C")
        .arg(&common)
        .args(["fsck", "--unreachable", "--connectivity-only", "--no-progress"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("fsck");
    assert_eq!(own.status.code(), Some(2), "this git's fsck fails here too");
    let said = String::from_utf8_lossy(&own.stderr).into_owned();
    assert_eq!(text(fsck, "stderr"), (!said.is_empty()).then(|| said.clone()), "whole, as this git writes it, NONE when nothing");
    let crawl = rows(&db, "SELECT git_calls, git_failed, git_timeouts, bytes_read, outcome, note FROM crawl").await;
    let bytes: i64 = calls.iter().filter_map(|c| int(c, "bytes_out")).sum();
    assert_eq!(
        (int(&crawl[0], "git_calls"), int(&crawl[0], "git_failed"), int(&crawl[0], "git_timeouts"), int(&crawl[0], "bytes_read")),
        (Some(i64::try_from(calls.len()).expect("a count")), Some(1), Some(0), Some(bytes))
    );
    assert_eq!(text(&crawl[0], "outcome").as_deref(), Some("partial"), "a call that failed");
    let named = format!("git fsck in {common}: {}", said.trim());
    assert!(text(&crawl[0], "note").is_some_and(|n| n.contains(&named)), "{named:?} in {crawl:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Each remote's newest row, by name: url, push url, host and path.
async fn remotes(db: &superx_kernel::Db) -> Vec<Vec<Option<String>>> {
    let mut out: Vec<Vec<Option<String>>> = rows(db, "SELECT name, url, push_url, host, path FROM remote_state")
        .await
        .iter()
        .map(|r| ["name", "url", "push_url", "host", "path"].iter().map(|k| text(r, k)).collect())
        .collect();
    out.sort();
    out
}

fn some(values: &[Option<&str>]) -> Vec<Option<String>> {
    values.iter().map(|v| v.map(str::to_string)).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_remotes_url_push_url_host_and_path_is_a_remote_state_row_when_it_changes_and_one_with_url_none_once_it_is_gone_the_https_ssh_and_scp_like_forms_give_host_and_path_lowercased_without_git_a_local_path_neither_identity_is_the_configured_email()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "a\n")], "first");
    for (name, url) in [
        ("origin", "https://Someone@GitHub.com:443/Acme/Widget.git/"),
        ("up", "ssh://git@Bitbucket.org:22/Team/Repo.git"),
        ("scp", "git@GitLab.Example:Group/Sub/Proj.git"),
        ("local", "/srv/elsewhere/repo"),
        ("file", "file:///srv/x.git"),
    ] {
        git(&dir, &["remote", "add", name, url]);
    }
    git(&dir, &["config", "remote.up.pushurl", "git@bitbucket.org:team/repo-push.git"]);
    git(&dir, &["config", "user.email", "dev@example.com"]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir]))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(
        remotes(&db).await,
        [
            some(&[Some("file"), Some("file:///srv/x.git"), None, None, None]),
            some(&[Some("local"), Some("/srv/elsewhere/repo"), None, None, None]),
            some(&[Some("origin"), Some("https://Someone@GitHub.com:443/Acme/Widget.git/"), None, Some("github.com"), Some("acme/widget")]),
            some(&[Some("scp"), Some("git@GitLab.Example:Group/Sub/Proj.git"), None, Some("gitlab.example"), Some("group/sub/proj")]),
            some(&[Some("up"), Some("ssh://git@Bitbucket.org:22/Team/Repo.git"), Some("git@bitbucket.org:team/repo-push.git"), Some("bitbucket.org"), Some("team/repo")]),
        ]
    );
    let crawl = rows(&db, "SELECT identity FROM crawl").await;
    assert_eq!(text(&crawl[0], "identity").as_deref(), Some("dev@example.com"));

    // Unchanged: no row. Changed: one. Gone: one with url NONE.
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(count(&db, "remote_state").await, 5, "nothing changed, nothing written");
    git(&dir, &["remote", "set-url", "origin", "https://github.com/acme/widget-two"]);
    git(&dir, &["remote", "remove", "local"]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(count(&db, "remote_state").await, 7);
    let newest = rows(&db, "SELECT name, url, host, path, valid_from FROM remote_state WHERE name INSIDE ['origin', 'local'] ORDER BY valid_from DESC LIMIT 2").await;
    let mut newest: Vec<Vec<Option<String>>> = newest.iter().map(|r| ["name", "url", "host", "path"].iter().map(|k| text(r, k)).collect()).collect();
    newest.sort();
    assert_eq!(
        newest,
        [
            some(&[Some("local"), None, None, None]),
            some(&[Some("origin"), Some("https://github.com/acme/widget-two"), Some("github.com"), Some("acme/widget-two")]),
        ]
    );
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shallow_repositorys_crawl_says_shallow_in_its_note_and_reads_the_commits_git_holds() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (source, shallow) = (root.join("source"), root.join("shallow"));
    repo(&source);
    commit(&source, &[("a.txt", "1\n")], "one");
    commit(&source, &[("a.txt", "1\n2\n")], "two");
    let tip = commit(&source, &[("a.txt", "1\n2\n3\n")], "three");
    git(&root, &["clone", "-q", "--depth", "1", &format!("file://{}", source.display()), &shallow.to_string_lossy()]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&shallow]))]);
    let done = pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let crawl = rows(&db, "SELECT note, outcome, commits_read FROM crawl").await;
    assert!(text(&crawl[0], "note").is_some_and(|n| n.contains("shallow")), "{crawl:?}");
    assert_eq!((text(&crawl[0], "outcome").as_deref(), int(&crawl[0], "commits_read")), (Some("complete"), Some(1)), "{done:?}");
    assert_eq!((counted(&done, "repos", "commits_held"), counted(&done, "repos", "commits_read")), (1, 1));
    let held = rows(&db, "SELECT hash, parents FROM commit").await;
    assert_eq!((text(&held[0], "hash"), int(&held[0], "parents")), (Some(tip), Some(1)), "the commit git holds, as its object says it");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Hosts of the test's own, GitHub answering the repository acme/widget.
async fn hosts_answering() -> (Fake, Fake) {
    let (github, bitbucket) = (Fake::start("").await, Fake::start("/2.0").await);
    github.json("/repos/acme/widget", &github_repository(77, "main"));
    (github, bitbucket)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_repositories_whose_remotes_name_one_repository_on_a_host_make_one_host_repository_crawled_once_a_pass() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (a, b) = (root.join("a"), root.join("b"));
    for dir in [&a, &b] {
        repo(dir);
        commit(dir, &[("a.txt", "a\n")], "first");
        git(dir, &["remote", "add", "origin", "https://github.com/Acme/Widget.git"]);
    }
    hosted(&root, "github.com", "acme/widget", &a);
    let (github, bitbucket) = hosts_answering().await;
    let own = places(&root, Some(credentials(&root, Some(TOKEN), None)), &github, &bitbucket);
    let p = params(&[(INCLUDE_PARAM, strings(&[&a, &b]))]);
    let first = pass(&kernel, &db, &p, &mut Crawler::new(own.clone())).await;
    assert_eq!((count(&db, "crawl").await, count(&db, "host_repository").await, count(&db, "host_crawl").await), (2, 1, 1), "{first:?}");
    let repository = rows(&db, "SELECT host, path, host_id FROM host_repository").await;
    assert_eq!(
        (text(&repository[0], "host"), text(&repository[0], "path"), int(&repository[0], "host_id")),
        (Some("github.com".into()), Some("acme/widget".into()), Some(77))
    );
    assert_eq!(github.paths().iter().filter(|p| *p == "/repos/acme/widget").count(), 1, "asked once: {:?}", github.paths());
    assert_eq!(first.source("hosts").map(|s| s.items), Some(1));

    // Another pass: the same host repository, crawled once more.
    pass(&kernel, &db, &p, &mut Crawler::new(own)).await;
    assert_eq!((count(&db, "crawl").await, count(&db, "host_repository").await, count(&db, "host_crawl").await), (4, 1, 2));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mirror_is_made_bare_in_the_repos_folder_and_fetched_with_the_refspec_the_settings_and_the_environment_step_4_names_no_token_in_any_argument_or_variable_a_ref_the_host_deletes_or_rewrites_keeps_in_the_mirror_what_was_fetched_without_a_token_or_with_the_repository_unanswered_no_mirror_is_made()
{
    use superx_mod_recon::crawler::hosts::Host;
    use superx_mod_recon::crawler::repos::mirror;

    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let work = root.join("work");
    repo(&work);
    commit(&work, &[("a.txt", "a\n")], "first");
    git(&work, &["branch", "gone"]);
    git(&work, &["branch", "rewritten"]);
    git(&work, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    let host_git = hosted(&root, "github.com", "acme/widget", &work);
    let (github, bitbucket) = hosts_answering().await;
    let own = places(&root, Some(credentials(&root, Some(TOKEN), None)), &github, &bitbucket);
    let p = params(&[(INCLUDE_PARAM, strings(&[&work]))]);
    pass(&kernel, &db, &p, &mut Crawler::new(own.clone())).await;

    let folder = root.join("repos").join("github.com").join("acme/widget.git");
    assert_eq!(git(&folder, &["rev-parse", "--is-bare-repository"]), "true");
    assert!(!folder.join("hooks").exists() && !folder.join("description").exists(), "made with no template");
    let made = rows(&db, "SELECT url, folder, created_at, host_repository.path AS path FROM mirror").await;
    assert_eq!(
        (text(&made[0], "url"), text(&made[0], "folder"), text(&made[0], "path")),
        (Some("https://github.com/acme/widget.git".into()), Some(folder.to_string_lossy().into_owned()), Some("acme/widget".into()))
    );
    let fetches = rows(&db, "SELECT args, dir FROM git_call WHERE step = 'fetch'").await;
    let fetch = fetches.iter().find_map(|c| texts(c, "args").filter(|a| a.contains(&"fetch".to_string()))).expect("a fetch");
    for setting in [
        "fetch.prune=false",
        "fetch.pruneTags=false",
        "gc.auto=0",
        "maintenance.auto=false",
        "core.logAllRefUpdates=always",
        "gc.reflogExpire=never",
        "gc.reflogExpireUnreachable=never",
        "core.hooksPath=/dev/null",
        "http.followRedirects=false",
        "credential.helper=",
        "credential.username=x-access-token",
        "http.lowSpeedLimit=1",
        "http.lowSpeedTime=60",
    ] {
        let at = fetch.iter().position(|a| a == setting).unwrap_or_else(|| panic!("{setting} in {fetch:?}"));
        assert_eq!(fetch[at - 1], "-c");
    }
    let sub = fetch.iter().position(|a| a == "fetch").expect("fetch");
    assert_eq!(
        &fetch[sub..],
        ["fetch", "--no-auto-maintenance", "--no-recurse-submodules", &host_git.to_string_lossy(), "+refs/*:refs/*"],
        "the refspec, its only one, no prune"
    );
    assert!(!fetch.iter().any(|a| a.contains("prune") && !a.ends_with("=false")));
    let with = mirror::with(Host::GitHub, &mirror::Places { repos: own.repos.clone(), home: own.home.clone(), askpass: own.askpass.clone() });
    let env: BTreeMap<String, String> = with.env.iter().cloned().collect();
    assert_eq!(env.get("GIT_CONFIG_GLOBAL").map(String::as_str), Some("/dev/null"));
    assert_eq!(env.get("GIT_CONFIG_NOSYSTEM").map(String::as_str), Some("1"));
    assert_eq!(env.get("GIT_TERMINAL_PROMPT").map(String::as_str), Some("0"));
    assert_eq!(env.get("SUPERX_RECON_ASKPASS").map(String::as_str), Some("github.com"));
    assert_eq!(env.get("GIT_ASKPASS").map(String::as_str), Some(env!("CARGO_BIN_EXE_superx-recon")));
    assert_eq!(env.get("SUPERX_HOME"), Some(&root.to_string_lossy().into_owned()));
    for call in rows(&db, "SELECT args FROM git_call").await {
        assert!(!written(&call).contains(TOKEN), "no token in an argument: {call:?}");
    }
    assert!(with.env.iter().all(|(k, v)| !k.contains(TOKEN) && !v.contains(TOKEN)), "nor in a variable");
    assert!(with.config.iter().all(|c| !c.contains(TOKEN)));
    let old = git(&folder, &["rev-parse", "refs/heads/rewritten"]);

    // The host deletes a branch and rewrites another: the mirror keeps what
    // it fetched.
    git(&host_git, &["branch", "-D", "gone"]);
    let tree = git(&host_git, &["rev-parse", "rewritten^{tree}"]);
    let new = git_at(&host_git, &["commit-tree", &tree, "-m", "rewritten"], Some("2026-10-02T12:00:00+00:00"));
    git(&host_git, &["update-ref", "refs/heads/rewritten", &new]);
    pass(&kernel, &db, &p, &mut Crawler::new(own)).await;
    assert_eq!(git(&folder, &["rev-parse", "refs/heads/gone"]).len(), 40, "never pruned");
    assert_eq!(git(&folder, &["rev-parse", "refs/heads/rewritten"]), new, "taken as the host has it");
    assert!(git_may(&folder, &["cat-file", "-e", &old]).0, "what was fetched stays");
    let logged = std::fs::read_to_string(folder.join("logs/refs/heads/rewritten")).expect("its moves logged");
    assert!(logged.contains(&old) && logged.contains(&new), "{logged}");

    // Without a token, and with the repository unanswered: no mirror.
    for (token, answered) in [(None, true), (Some(TOKEN), false)] {
        let other = scratch();
        let dir = other.join("work");
        repo(&dir);
        commit(&dir, &[("a.txt", "a\n")], "first");
        git(&dir, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
        let (github, bitbucket) = (Fake::start("").await, Fake::start("/2.0").await);
        if answered {
            github.json("/repos/acme/widget", &github_repository(77, "main"));
        }
        let theirs = places(&other, Some(credentials(&other, token, None)), &github, &bitbucket);
        let db = own_db().await;
        pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::new(theirs)).await;
        assert!(!other.join("repos").join("github.com").exists(), "no mirror: token {token:?}, answered {answered}");
        assert_eq!(count(&db, "mirror").await, 0);
        std::fs::remove_dir_all(&other).expect("clean up");
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The newest crawl of the repository named `name`.
async fn newest_crawl(db: &superx_kernel::Db, name: &str) -> Value {
    let found = rows(db, &format!("SELECT *, valid_from FROM crawl WHERE repository.name = '{name}' ORDER BY valid_from DESC LIMIT 1")).await;
    found.into_iter().next().unwrap_or_else(|| panic!("a crawl of {name}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refs_are_counted_new_moved_rewritten_and_gone_against_the_previous_crawl_and_not_compared_at_a_repositorys_first_crawl_since_start()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "a\n")], "first");
    for b in ["moves", "rewrites"] {
        git(&dir, &["branch", b]);
    }
    git(&dir, &["tag", "-a", "v1", "-m", "v1"]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir])), (CRAWL_SECS_PARAM, number(1))]);
    let mut crawler = Crawler::default();
    pass(&kernel, &db, &p, &mut crawler).await;
    let first = newest_crawl(&db, "work").await;
    assert_eq!(
        ["refs_read", "refs_new", "refs_moved", "refs_rewritten", "refs_gone"].map(|k| int(&first, k)),
        [Some(4), Some(0), Some(0), Some(0), Some(0)],
        "main, two branches and a tag; nothing compared: {first:?}"
    );
    assert!(text(&first, "note").is_some_and(|n| n.contains("compared with none")), "{first:?}");

    // One ref new, one moved on, one rewritten, one gone.
    git(&dir, &["branch", "fresh"]);
    git(&dir, &["switch", "-q", "moves"]);
    commit(&dir, &[("b.txt", "b\n")], "moved on");
    git(&dir, &["switch", "-q", "rewrites"]);
    git_at(&dir, &["commit", "-q", "--amend", "-m", "rewritten"], Some("2026-10-02T12:00:00+00:00"));
    git(&dir, &["switch", "-q", "main"]);
    git(&dir, &["tag", "-d", "v1"]);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let second = newest_crawl(&db, "work").await;
    assert_eq!(
        ["refs_read", "refs_new", "refs_moved", "refs_rewritten", "refs_gone"].map(|k| int(&second, k)),
        [Some(4), Some(1), Some(2), Some(1), Some(1)],
        "{second:?}"
    );
    assert!(!text(&second, "note").unwrap_or_default().contains("compared with none"));

    // After a restart, the refs are compared with none again.
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let third = newest_crawl(&db, "work").await;
    assert_eq!(["refs_new", "refs_moved", "refs_rewritten", "refs_gone"].map(|k| int(&third, k)), [Some(0); 4]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_main_line_is_chosen_override_then_host_default_from_the_mirrors_branch_or_else_origins_then_origin_head_local_branch_and_head_each_recorded_with_its_word_and_with_no_tip_nothing_is_placed_on_it()
{
    use superx_mod_recon::params::MAINLINE_REFS_PARAM;

    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let make = |name: &str, branch: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("dir");
        git(&dir, &["init", "-q", "-b", branch]);
        let c = commit(&dir, &[("a.txt", &format!("{name}\n"))], "first");
        (dir, c)
    };
    // override
    let (ov, _) = make("ov", "main");
    git(&ov, &["switch", "-q", "-c", "release"]);
    let release = commit(&ov, &[("r.txt", "r\n")], "release");
    git(&ov, &["switch", "-q", "main"]);
    // host_default, from the mirror's branch
    let (hd, _) = make("hd", "main");
    git(&hd, &["switch", "-q", "-c", "trunk"]);
    let trunk = commit(&hd, &[("t.txt", "t\n")], "trunk");
    git(&hd, &["switch", "-q", "main"]);
    git(&hd, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    hosted(&root, "github.com", "acme/widget", &hd);
    // host_default, from origin's branch: the mirror does not hold it
    let (ht, _) = make("ht", "main");
    let develop = commit(&ht, &[("d.txt", "d\n")], "develop");
    git(&ht, &["update-ref", "refs/remotes/origin/develop", &develop]);
    git(&ht, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&ht, &["remote", "add", "origin", "https://bitbucket.org/team/repo.git"]);
    hosted(&root, "bitbucket.org", "team/repo", &ht);
    // origin_head
    let (oh, stable) = make("oh", "work");
    git(&oh, &["remote", "add", "origin", "https://example.org/x.git"]);
    git(&oh, &["update-ref", "refs/remotes/origin/stable", &stable]);
    git(&oh, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/stable"]);
    // local_branch, main and master
    let (lb, main) = make("lb", "main");
    let (lm, master) = make("lm", "master");
    // head
    let (hh, head) = make("hh", "work");
    // no tip: HEAD on a branch with no commit, and no other main line
    let (nt, orphaned) = make("nt", "work");
    git(&nt, &["switch", "-q", "--orphan", "fresh"]);

    let (github, bitbucket) = (Fake::start("").await, Fake::start("/2.0").await);
    github.json("/repos/acme/widget", &github_repository(77, "trunk"));
    bitbucket.json("/repositories/team/repo", &serde_json::json!({ "uuid": "{b0b}", "full_name": "team/repo", "mainbranch": { "name": "develop" } }));
    let own = places(&root, Some(credentials(&root, Some(TOKEN), Some(BB_TOKEN))), &github, &bitbucket);
    let overrides = Value::Object({
        let mut o = superx_kernel::types::Object::new();
        o.insert("ov".to_string(), Value::String("release".to_string()));
        o
    });
    let p = params(&[(INCLUDE_PARAM, strings(&[&ov, &hd, &ht, &oh, &lb, &lm, &hh, &nt])), (MAINLINE_REFS_PARAM, overrides)]);
    pass(&kernel, &db, &p, &mut Crawler::new(own)).await;
    for (name, mainline, from, tip) in [
        ("ov", "refs/heads/release", "override", Some(release)),
        ("hd", "refs/heads/trunk", "host_default", Some(trunk)),
        ("ht", "refs/remotes/origin/develop", "host_default", Some(develop)),
        ("oh", "refs/remotes/origin/stable", "origin_head", Some(stable)),
        ("lb", "refs/heads/main", "local_branch", Some(main)),
        ("lm", "refs/heads/master", "local_branch", Some(master)),
        ("hh", "HEAD", "head", Some(head)),
        ("nt", "HEAD", "head", None),
    ] {
        let crawl = newest_crawl(&db, name).await;
        assert_eq!(
            (text(&crawl, "mainline").as_deref(), text(&crawl, "mainline_from").as_deref(), text(&crawl, "tip")),
            (Some(mainline), Some(from), tip),
            "{name}: {crawl:?}"
        );
    }
    let nt_crawl = newest_crawl(&db, "nt").await;
    assert!(text(&nt_crawl, "note").is_some_and(|n| n.contains("no main line tip")), "{nt_crawl:?}");
    let placed = rows(&db, &format!("SELECT place FROM placement WHERE commit.hash = '{orphaned}'")).await;
    assert_eq!(placed.iter().map(|r| text(r, "place")).collect::<Vec<_>>(), [Some("branch".to_string())], "nothing on a main line with no tip");
    let blamed = rows(&db, "SELECT count() AS n FROM survival WHERE commit.repository.name = 'nt' GROUP ALL").await;
    assert_eq!(blamed.first().and_then(|r| int(r, "n")).unwrap_or(0), 0, "nothing blamed");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_no_ref_and_no_reflog_reaches_is_read_and_placed_recovered_at_most_attr_recon_unreachable_commits_of_them() {
    use superx_mod_recon::params::UNREACHABLE_COMMITS_PARAM;

    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "a\n")], "first");
    git(&dir, &["switch", "-q", "-c", "lost"]);
    commit(&dir, &[("b.txt", "b\n")], "lost one");
    commit(&dir, &[("c.txt", "c\n")], "lost two");
    git(&dir, &["switch", "-q", "main"]);
    git(&dir, &["branch", "-D", "lost"]);
    git(&dir, &["reflog", "expire", "--expire=now", "--expire-unreachable=now", "--all"]);
    let named: Vec<String> = git(&dir, &["fsck", "--unreachable", "--connectivity-only", "--no-progress"])
        .lines()
        .filter_map(|l| l.strip_prefix("unreachable commit ").map(str::to_string))
        .collect();
    assert_eq!(named.len(), 2, "{named:?}");
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir])), (UNREACHABLE_COMMITS_PARAM, number(1))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(int(&crawl, "unreachable_read"), Some(2), "every one fsck named is counted");
    let read: Vec<String> = rows(&db, "SELECT VALUE hash FROM commit").await.iter().filter_map(|v| match v {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }).collect();
    assert!(read.contains(&named[0]) && !read.contains(&named[1]), "the first fsck names, no more: {read:?} {named:?}");
    let placed = rows(&db, &format!("SELECT place FROM placement WHERE commit.hash = '{}'", named[0])).await;
    assert_eq!(text(&placed[0], "place").as_deref(), Some("recovered"));
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A commit's patch id, as git gives it.
fn patch_id(dir: &Path, commit: &str) -> String {
    use std::io::Write as _;
    let diff = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["diff-tree", "-p", "--root", "--no-renames", commit])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("diff-tree");
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["patch-id", "--stable"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("patch-id");
    child.stdin.take().expect("input").write_all(&diff.stdout).expect("written");
    let out = child.wait_with_output().expect("ends");
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or_default().to_string()
}

async fn commit_row(db: &superx_kernel::Db, hash: &str) -> Value {
    rows(db, &format!("SELECT * FROM commit WHERE hash = '{hash}'")).await.into_iter().next().unwrap_or_else(|| panic!("a row for {hash}"))
}

fn time_of(v: &Value, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    match field(v, key) {
        Some(Value::Datetime(d)) => Some(chrono::DateTime::<chrono::Utc>::from(*d)),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_takes_its_hash_tree_parents_emails_names_times_offsets_in_minutes_subject_body_with_its_trailers_signature_whole_and_patch_id_a_cherry_picked_copy_sharing_its_originals_patch_id_and_a_merge_and_an_empty_commit_having_none_a_commit_only_a_mirror_holds_is_read_from_the_mirror_attr_recon_since_days_leaves_older_commits_unread()
{
    use chrono::TimeZone as _;
    use superx_mod_recon::params::SINCE_DAYS_PARAM;

    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    let message = "first subject\n\n\nbody line one\nbody line two\n\nCo-Authored-By: Pat <pat@example.com>\n";
    let first = commit_as(
        &dir,
        &[("a.txt", "one\n")],
        message,
        ("Ann Author", "ann@example.com", "2026-09-01T10:00:00+05:30"),
        ("Carl Committer", "carl@example.com", "2026-09-02T11:00:00-04:00"),
    );
    let tree = git(&dir, &["rev-parse", &format!("{first}^{{tree}}")]);
    let signature = "-----BEGIN PGP SIGNATURE-----\n\niQEzBAABCAAdFiEE\n=abcd\n-----END PGP SIGNATURE-----";
    let signed_object = format!(
        "tree {tree}\nparent {first}\nauthor Ann Author <ann@example.com> 1790000000 +0530\ncommitter Carl Committer <carl@example.com> 1790000100 -0400\ngpgsig {}\n\nsigned commit\n",
        signature.replace('\n', "\n ")
    );
    let signed = object(&dir, "commit", &signed_object);
    git(&dir, &["update-ref", "refs/heads/signed", &signed]);
    git(&dir, &["switch", "-q", "-c", "side"]);
    let picked = commit(&dir, &[("b.txt", "b\n")], "pick me");
    git(&dir, &["switch", "-q", "main"]);
    git(&dir, &["cherry-pick", picked.as_str()]);
    let copy = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["switch", "-q", "-c", "merged"]);
    commit(&dir, &[("m.txt", "m\n")], "to merge");
    git(&dir, &["switch", "-q", "main"]);
    git(&dir, &["merge", "-q", "--no-ff", "-m", "a merge", "merged"]);
    let merge = git(&dir, &["rev-parse", "HEAD"]);
    let empty = commit(&dir, &[], "nothing");
    // Only the mirror holds a commit the host has.
    git(&dir, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    let host_git = hosted(&root, "github.com", "acme/widget", &dir);
    let remote_only = git_at(&host_git, &["commit-tree", &tree, "-p", &first, "-m", "on the host only"], Some("2026-10-03T12:00:00+00:00"));
    git(&host_git, &["update-ref", "refs/heads/remote-only", &remote_only]);
    let (github, bitbucket) = hosts_answering().await;
    let own = places(&root, Some(credentials(&root, Some(TOKEN), None)), &github, &bitbucket);
    let done = pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::new(own)).await;

    let c = commit_row(&db, &first).await;
    assert_eq!(text(&c, "hash").as_deref(), Some(first.as_str()));
    assert_eq!(text(&c, "tree").as_deref(), Some(tree.as_str()));
    assert_eq!((texts(&c, "parent_hashes"), int(&c, "parents")), (Some(vec![]), Some(0)));
    assert_eq!(
        (text(&c, "author"), text(&c, "author_name"), text(&c, "committer"), text(&c, "committer_name")),
        (Some("ann@example.com".into()), Some("Ann Author".into()), Some("carl@example.com".into()), Some("Carl Committer".into()))
    );
    assert_eq!(time_of(&c, "authored_at"), chrono::Utc.with_ymd_and_hms(2026, 9, 1, 4, 30, 0).single());
    assert_eq!(time_of(&c, "committed_at"), chrono::Utc.with_ymd_and_hms(2026, 9, 2, 15, 0, 0).single());
    assert_eq!((int(&c, "author_offset"), int(&c, "committer_offset")), (Some(330), Some(-240)));
    assert_eq!(text(&c, "subject").as_deref(), Some("first subject"));
    assert_eq!(text(&c, "body").as_deref(), Some("body line one\nbody line two\n\nCo-Authored-By: Pat <pat@example.com>\n"));
    assert_eq!((text(&c, "signature"), text(&c, "patch_id")), (None, Some(patch_id(&dir, &first))));
    let s = commit_row(&db, &signed).await;
    assert_eq!(text(&s, "signature").as_deref(), Some(signature), "whole, never verified");
    assert_eq!((texts(&s, "parent_hashes"), int(&s, "parents"), text(&s, "body")), (Some(vec![first.clone()]), Some(1), None));
    let (p, q) = (commit_row(&db, &picked).await, commit_row(&db, &copy).await);
    assert!(text(&p, "patch_id").is_some() && text(&p, "patch_id") == text(&q, "patch_id"), "a cherry-picked copy shares it");
    assert_eq!(text(&commit_row(&db, &merge).await, "patch_id"), None, "a merge has none");
    assert_eq!(int(&commit_row(&db, &merge).await, "parents"), Some(2));
    assert_eq!(text(&commit_row(&db, &empty).await, "patch_id"), None, "a commit that changes nothing has none");
    let r = commit_row(&db, &remote_only).await;
    assert_eq!(text(&r, "subject").as_deref(), Some("on the host only"), "{done:?}");
    let mirror = root.join("repos/github.com/acme/widget.git").to_string_lossy().into_owned();
    let reads: Vec<Value> = rows(&db, "SELECT dir, args FROM git_call").await;
    assert!(
        reads.iter().any(|c| text(c, "dir").as_deref() == Some(mirror.as_str()) && texts(c, "args").is_some_and(|a| a.contains(&"cat-file".to_string()))),
        "read from the mirror"
    );
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(int(&crawl, "commits_read"), Some(8), "{crawl:?}");

    // attr_recon_since_days leaves older commits unread.
    let old = root.join("old");
    repo(&old);
    commit_at(&old, &[("a.txt", "a\n")], "long ago", "2001-01-01T00:00:00+00:00");
    let recent = (chrono::Utc::now() - chrono::TimeDelta::days(1)).to_rfc3339();
    let kept = commit_at(&old, &[("a.txt", "a\nb\n")], "lately", &recent);
    let db = own_db().await;
    let p = params(&[(INCLUDE_PARAM, strings(&[&old])), (SINCE_DAYS_PARAM, number(30))]);
    let done = pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let read: Vec<Value> = rows(&db, "SELECT VALUE hash FROM commit").await;
    assert_eq!(read, [Value::String(kept)], "{done:?}");
    assert_eq!((counted(&done, "repos", "commits_held"), counted(&done, "repos", "commits_read")), (1, 1));
    let crawl = newest_crawl(&db, "old").await;
    assert!(time_of(&crawl, "since").is_some_and(|s| s < chrono::Utc::now() - chrono::TimeDelta::days(29)), "{crawl:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A commit's files, by path: change, added, removed, the same ignoring
/// space, binary.
async fn files_of(db: &superx_kernel::Db, hash: &str) -> BTreeMap<String, (String, i64, i64, i64, i64, bool)> {
    rows(db, &format!("SELECT path, change, added, removed, added_ignoring_space, removed_ignoring_space, binary FROM commit_file WHERE commit.hash = '{hash}'"))
        .await
        .iter()
        .map(|f| {
            (
                text(f, "path").expect("path"),
                (
                    text(f, "change").expect("change"),
                    int(f, "added").expect("added"),
                    int(f, "removed").expect("removed"),
                    int(f, "added_ignoring_space").expect("added"),
                    int(f, "removed_ignoring_space").expect("removed"),
                    flag(f, "binary").expect("binary"),
                ),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commits_files_are_added_modified_deleted_and_type_changed_a_rename_a_delete_and_an_add_a_binarys_counts_0_and_the_counts_ignoring_space_beside_the_others_a_merges_against_its_first_parent_its_added_and_removed_their_sums()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "one\ntwo\nthree\n"), ("b.txt", "b\n"), ("c.txt", "c\n"), ("t.txt", "target\n")], "first");
    std::fs::write(dir.join("a.txt"), "one\ntwo  \nTHREE\nfour\n").expect("a");
    std::fs::remove_file(dir.join("b.txt")).expect("b");
    git(&dir, &["mv", "c.txt", "d.txt"]);
    std::fs::remove_file(dir.join("t.txt")).expect("t");
    std::os::unix::fs::symlink("a.txt", dir.join("t.txt")).expect("a link");
    std::fs::write(dir.join("bin.dat"), [0u8, 1, 2, 0, 255]).expect("binary");
    let second = commit(&dir, &[], "second");
    git(&dir, &["switch", "-q", "-c", "side"]);
    commit(&dir, &[("s.txt", "s\n")], "side");
    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, &[("a.txt", "one\n")], "third");
    git(&dir, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let merge = git(&dir, &["rev-parse", "HEAD"]);
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;

    let files = files_of(&db, &second).await;
    let types = files.get("t.txt").cloned().expect("t.txt");
    assert_eq!(types.0, "type_changed");
    let mut want: BTreeMap<String, (String, i64, i64, i64, i64, bool)> = BTreeMap::new();
    want.insert("a.txt".into(), ("modified".into(), 3, 2, 2, 1, false));
    want.insert("b.txt".into(), ("deleted".into(), 0, 1, 0, 1, false));
    want.insert("c.txt".into(), ("deleted".into(), 0, 1, 0, 1, false));
    want.insert("d.txt".into(), ("added".into(), 1, 0, 1, 0, false));
    want.insert("bin.dat".into(), ("added".into(), 0, 0, 0, 0, true));
    want.insert("t.txt".into(), types.clone());
    assert_eq!(files, want);
    let row = commit_row(&db, &second).await;
    let (added, removed) = files.values().fold((0, 0), |(a, r), f| (a + f.1, r + f.2));
    assert_eq!((int(&row, "added"), int(&row, "removed")), (Some(added), Some(removed)), "its sums");
    let merged = files_of(&db, &merge).await;
    assert_eq!(merged.keys().collect::<Vec<_>>(), ["s.txt"], "a merge against its first parent: {merged:?}");
    assert_eq!(merged.get("s.txt").map(|f| (f.0.as_str(), f.1, f.2)), Some(("added", 1, 0)));
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The lines of every reflog file under a git directory's `logs`.
fn reflog_lines(git_dir: &Path) -> usize {
    let mut n = 0;
    let mut folders = vec![git_dir.join("logs")];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else {
                n += std::fs::read_to_string(&path).map(|t| t.lines().filter(|l| !l.is_empty()).count()).unwrap_or(0);
            }
        }
    }
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_reflog_entry_of_a_branch_a_remote_tracking_ref_each_checkouts_head_the_stash_and_a_mirrors_ref_is_one_ref_move_row_with_its_old_new_identity_action_and_at_the_zero_id_none_never_written_twice_across_passes_and_still_there_after_git_expires_it()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (dir, wt, upstream) = (root.join("work"), root.join("work-wt"), root.join("upstream"));
    repo(&upstream);
    commit(&upstream, &[("u.txt", "u\n")], "upstream");
    repo(&dir);
    commit(&dir, &[("a.txt", "a\n")], "first");
    git(&dir, &["branch", "feat"]);
    git(&dir, &["worktree", "add", "-q", "-b", "wtb", &wt.to_string_lossy()]);
    commit(&wt, &[("w.txt", "w\n")], "in the worktree");
    std::fs::write(dir.join("a.txt"), "a\nchanged\n").expect("a change");
    git(&dir, &["stash", "push", "-q", "-m", "keep this"]);
    git(&dir, &["remote", "add", "up", &upstream.to_string_lossy()]);
    git(&dir, &["fetch", "-q", "up"]);
    git(&dir, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    hosted(&root, "github.com", "acme/widget", &dir);
    let (github, bitbucket) = hosts_answering().await;
    let own = places(&root, Some(credentials(&root, Some(TOKEN), None)), &github, &bitbucket);
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir, &wt]))]);
    pass(&kernel, &db, &p, &mut Crawler::new(own.clone())).await;

    let common = dir.join(".git");
    let mirror = root.join("repos/github.com/acme/widget.git");
    let expected = reflog_lines(&common) + reflog_lines(&common.join("worktrees/work-wt")) + reflog_lines(&mirror);
    let moves = rows(&db, "SELECT ref, at, old, new, identity, action, checkout.toplevel AS top, mirror.folder AS mirror FROM ref_move").await;
    assert_eq!(moves.len(), expected, "{moves:?}");
    let of = |name: &str| moves.iter().filter(|m| text(m, "ref").as_deref() == Some(name)).cloned().collect::<Vec<_>>();
    let feat: Vec<Value> = of("refs/heads/feat").into_iter().filter(|m| text(m, "mirror").is_none()).collect();
    assert_eq!(feat.len(), 1, "{moves:?}");
    assert_eq!(
        (text(&feat[0], "old"), text(&feat[0], "new").map(|h| h.len()), text(&feat[0], "identity"), text(&feat[0], "action")),
        (None, Some(40), Some("author@example.com".into()), Some("branch: Created from main".into())),
        "the zero id is none"
    );
    assert!(time_of(&feat[0], "at").is_some());
    let heads = of("HEAD");
    let tops: std::collections::BTreeSet<Option<String>> = heads.iter().filter(|m| text(m, "mirror").is_none()).map(|m| text(m, "top")).collect();
    assert_eq!(tops, [Some(dir.to_string_lossy().into_owned()), Some(wt.to_string_lossy().into_owned())].into_iter().collect(), "each checkout's HEAD");
    assert!(!of("refs/stash").is_empty() && of("refs/stash").iter().all(|m| text(m, "top").is_none()), "the stash's");
    assert!(!of("refs/remotes/up/main").is_empty(), "a remote-tracking ref's");
    let mirrored: Vec<&Value> = moves.iter().filter(|m| text(m, "mirror").is_some()).collect();
    assert!(!mirrored.is_empty() && mirrored.iter().all(|m| text(m, "mirror").as_deref() == Some(mirror.to_string_lossy().as_ref())), "a mirror's");
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(int(&crawl, "reflog_read"), Some(i64::try_from(expected).expect("a count")));

    // Never written twice, and still there once git expires them.
    pass(&kernel, &db, &p, &mut Crawler::new(own.clone())).await;
    assert_eq!(count(&db, "ref_move").await, i64::try_from(expected).expect("a count"));
    git(&dir, &["reflog", "expire", "--expire=now", "--all"]);
    pass(&kernel, &db, &p, &mut Crawler::new(own)).await;
    assert_eq!(count(&db, "ref_move").await, i64::try_from(expected).expect("a count"), "kept after git expires them");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repository_whose_refs_git_keeps_in_a_reftable_has_its_reflog_entries_read_with_git_reflog_show_old_none() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("table");
    std::fs::create_dir_all(&dir).expect("dir");
    git(&dir, &["init", "-q", "-b", "main", "--ref-format=reftable"]);
    commit(&dir, &[("a.txt", "a\n")], "first");
    commit(&dir, &[("a.txt", "a\nb\n")], "second");
    git(&dir, &["branch", "feat"]);
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;
    let moves = rows(&db, "SELECT ref, old, new, action FROM ref_move").await;
    let shown = git(&dir, &["reflog", "show", "--all"]).lines().count();
    assert!(moves.len() >= 3 && moves.len() == shown, "{moves:?} against {shown}");
    assert!(moves.iter().all(|m| text(m, "old").is_none() && text(m, "new").is_some()), "old none: {moves:?}");
    assert!(moves.iter().any(|m| text(m, "ref").as_deref() == Some("refs/heads/feat")));
    let shows = rows(&db, "SELECT args FROM git_call WHERE step = 'reflog'").await;
    assert!(shows.iter().any(|c| texts(c, "args").is_some_and(|a| a.windows(2).any(|w| w == ["reflog", "show"]))), "{shows:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Each commit's newest place, by hash: place and first_parent.
async fn places_of(db: &superx_kernel::Db) -> BTreeMap<String, (String, bool)> {
    let mut newest: BTreeMap<String, (chrono::DateTime<chrono::Utc>, String, bool)> = BTreeMap::new();
    for r in rows(db, "SELECT commit.hash AS hash, place, first_parent, valid_from FROM placement").await {
        let (hash, at) = (text(&r, "hash").expect("hash"), time_of(&r, "valid_from").expect("valid_from"));
        if newest.get(&hash).is_none_or(|(seen, _, _)| *seen <= at) {
            newest.insert(hash, (at, text(&r, "place").expect("place"), flag(&r, "first_parent").expect("first_parent")));
        }
    }
    newest.into_iter().map(|(h, (_, p, f))| (h, (p, f))).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commits_are_placed_mainline_in_flight_branch_and_recovered_with_first_parent_and_a_placement_row_is_written_only_when_it_changes()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (dir, wt) = (root.join("work"), root.join("work-feat"));
    repo(&dir);
    let m1 = commit(&dir, &[("a.txt", "1\n")], "m1");
    git(&dir, &["switch", "-q", "-c", "side"]);
    let s1 = commit(&dir, &[("s.txt", "s\n")], "s1");
    git(&dir, &["switch", "-q", "main"]);
    let m2 = commit(&dir, &[("a.txt", "1\n2\n")], "m2");
    git(&dir, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let merge = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["worktree", "add", "-q", "-b", "feat", &wt.to_string_lossy()]);
    let f1 = commit(&wt, &[("f.txt", "f\n")], "f1");
    git(&dir, &["switch", "-q", "-c", "other"]);
    let o1 = commit(&dir, &[("o.txt", "o\n")], "o1");
    git(&dir, &["switch", "-q", "-c", "tmp"]);
    let r1 = commit(&dir, &[("r.txt", "r\n")], "r1");
    git(&dir, &["switch", "-q", "main"]);
    git(&dir, &["branch", "-D", "tmp"]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir, &wt]))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let want = |pairs: &[(&String, &str, bool)]| -> BTreeMap<String, (String, bool)> {
        pairs.iter().map(|(h, p, f)| ((*h).clone(), ((*p).to_string(), *f))).collect()
    };
    assert_eq!(
        places_of(&db).await,
        want(&[
            (&m1, "mainline", true),
            (&s1, "mainline", false),
            (&m2, "mainline", true),
            (&merge, "mainline", true),
            (&f1, "in_flight", false),
            (&o1, "branch", false),
            (&r1, "recovered", false),
        ])
    );
    assert_eq!(count(&db, "placement").await, 7);

    // Nothing changed: no row.
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(count(&db, "placement").await, 7);

    // `other` lands on the main line: its commit's place changes.
    git(&dir, &["merge", "-q", "--no-ff", "-m", "merge other", "other"]);
    let merge_two = git(&dir, &["rev-parse", "HEAD"]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(count(&db, "placement").await, 9, "o1 moved, and the new merge");
    let placed = places_of(&db).await;
    assert_eq!((placed.get(&o1), placed.get(&merge_two)), (Some(&("mainline".to_string(), false)), Some(&("mainline".to_string(), true))));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_commits_of_a_squash_merged_branch_land_via_the_squash_commit_for_each_file_whose_version_it_carries_and_not_for_a_file_the_squash_wrote_otherwise()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("base.txt", "base\n")], "base");
    git(&dir, &["switch", "-q", "-c", "feat"]);
    let c1 = commit(&dir, &[("a.txt", "a v1\n")], "a first");
    let c2 = commit(&dir, &[("a.txt", "a v2\n"), ("b.txt", "b\n"), ("c.txt", "c as written\n")], "a second, b, c");
    git(&dir, &["switch", "-q", "main"]);
    git(&dir, &["merge", "-q", "--squash", "feat"]);
    std::fs::write(dir.join("c.txt"), "c otherwise\n").expect("c");
    let squash = commit(&dir, &[], "squash feat");
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;
    let mut landed: Vec<(String, String, String)> = rows(&db, "SELECT commit.hash AS c, path, via.hash AS v FROM landed_via")
        .await
        .iter()
        .map(|r| (text(r, "c").expect("c"), text(r, "path").expect("path"), text(r, "v").expect("v")))
        .collect();
    landed.sort();
    let mut want = vec![(c2.clone(), "a.txt".to_string(), squash.clone()), (c2.clone(), "b.txt".to_string(), squash.clone())];
    want.sort();
    assert_eq!(landed, want, "c1's a.txt was not the version carried; c.txt the squash wrote otherwise");
    assert!(!landed.iter().any(|(c, _, _)| *c == c1));
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(int(&crawl, "landed_new"), Some(2));
    // Read again: nothing written twice.
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;
    assert_eq!(count(&db, "landed_via").await, 2);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Each commit's newest alive, by hash.
async fn alive(db: &superx_kernel::Db) -> BTreeMap<String, i64> {
    let mut newest: BTreeMap<String, (chrono::DateTime<chrono::Utc>, i64)> = BTreeMap::new();
    for r in rows(db, "SELECT commit.hash AS hash, alive, valid_from FROM survival").await {
        let (hash, at) = (text(&r, "hash").expect("hash"), time_of(&r, "valid_from").expect("valid_from"));
        if newest.get(&hash).is_none_or(|(seen, _)| *seen <= at) {
            newest.insert(hash, (at, int(&r, "alive").expect("alive")));
        }
    }
    newest.into_iter().map(|(h, (_, a))| (h, a)).collect()
}

/// The paths a crawl blamed: its blame calls' last argument.
async fn blamed(db: &superx_kernel::Db, crawl: &Value) -> Vec<String> {
    let Some(Value::RecordId(id)) = field(crawl, "id") else { panic!("a crawl id") };
    let calls: Vec<Value> =
        db.query("SELECT args FROM git_call WHERE crawl = $c AND step = 'blame'").bind(("c", id.clone())).await.expect("q").take(0).expect("rows");
    let mut paths: Vec<String> = calls.iter().filter_map(|c| texts(c, "args")).filter_map(|a| a.last().cloned()).collect();
    paths.sort();
    paths
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn survival_credits_each_commit_the_lines_blame_gives_it_at_the_tip_never_above_its_added_binaries_skipped_paths_and_the_files_past_attr_recon_blame_files_are_not_blamed_the_last_counted_in_files_left_nothing_is_blamed_while_the_tip_stays_and_a_commit_no_longer_credited_takes_alive_0()
{
    use superx_mod_recon::params::{BLAME_FILES_PARAM, SURVIVAL_SKIP_PARAM};

    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    std::fs::create_dir_all(dir.join("gen")).expect("gen");
    std::fs::write(dir.join("bin.dat"), [0u8, 1, 2, 0]).expect("binary");
    let c0 = commit(&dir, &[("b.txt", "head\n"), ("gen/x.txt", "g\n"), ("z.txt", "z\n")], "c0");
    let c1 = commit(&dir, &[("a.txt", "1\n2\n3\n"), ("b.txt", "head\nalpha beta gamma\n")], "c1");
    let c2 = commit(&dir, &[("a.txt", "1\ntwo\n3\n")], "c2");
    // A commit blame is told to ignore: the lines it split are credited to
    // the one before it, more than that one added.
    let ignored = commit(&dir, &[("b.txt", "head\nalpha\nbeta\ngamma\n")], "split");
    let list = root.join("ignore-revs");
    std::fs::write(&list, format!("{ignored}\n")).expect("ignore list");
    git(&dir, &["config", "blame.ignoreRevsFile", &list.to_string_lossy()]);
    let skip = Value::Array(vec![Value::String("/gen/".to_string())].into());
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir])), (BLAME_FILES_PARAM, number(2)), (SURVIVAL_SKIP_PARAM, skip), (CRAWL_SECS_PARAM, number(1))]);
    let mut crawler = Crawler::default();
    pass(&kernel, &db, &p, &mut crawler).await;
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(blamed(&db, &crawl).await, ["a.txt", "b.txt"], "not the binary, not the skipped path, not past the budget");
    assert_eq!(
        ["files_blamed", "files_left", "blame_lines_read", "survival_changed", "survival_delta"].map(|k| int(&crawl, k)),
        [Some(2), Some(1), Some(7), Some(3), Some(6)],
        "{crawl:?}"
    );
    let mut want: BTreeMap<String, i64> = BTreeMap::new();
    want.insert(c0.clone(), 1);
    want.insert(c1.clone(), 4);
    want.insert(c2.clone(), 1);
    assert_eq!(alive(&db).await, want, "c1 is credited five lines and added four");

    // The tip stays: nothing blamed.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let again = newest_crawl(&db, "work").await;
    assert_eq!((blamed(&db, &again).await.len(), int(&again, "files_blamed"), count(&db, "survival").await), (0, Some(0), 3));

    // c2's one line rewritten: c2 takes alive 0.
    let c3 = commit(&dir, &[("a.txt", "1\nTWO\n3\n")], "c3");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let third = newest_crawl(&db, "work").await;
    want.insert(c2.clone(), 0);
    want.insert(c3.clone(), 1);
    assert_eq!(alive(&db).await, want);
    assert_eq!(["survival_changed", "survival_delta"].map(|k| int(&third, k)), [Some(2), Some(0)], "{third:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blame_that_fails_or_is_cut_off_writes_no_survival_row_from_its_crawl_which_is_partial_and_names_the_file_and_the_next_crawl_blames_the_same_tip_again()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "1\n2\n")], "first");
    let tip = commit(&dir, &[("a.txt", "1\ntwo\n")], "second");
    git(&dir, &["config", "blame.ignoreRevsFile", "/nonexistent/ignore-revs"]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir])), (CRAWL_SECS_PARAM, number(1))]);
    let mut crawler = Crawler::default();
    let done = pass(&kernel, &db, &p, &mut crawler).await;
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(text(&crawl, "outcome").as_deref(), Some("partial"), "{crawl:?}");
    let note = text(&crawl, "note").expect("a note");
    assert!(note.contains("git blame a.txt") && note.contains("could not open object name list"), "{note}");
    assert_eq!(["survival_changed", "survival_delta"].map(|k| int(&crawl, k)), [Some(0), Some(0)]);
    assert_eq!(count(&db, "survival").await, 0, "{done:?}");
    assert_eq!(done.source("repos").map(|s| s.outcome), Some(superx_mod_recon::crawler::Outcome::Partial));

    // Fixed: the same tip is blamed again.
    git(&dir, &["config", "--unset", "blame.ignoreRevsFile"]);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let again = newest_crawl(&db, "work").await;
    assert_eq!((text(&again, "tip"), text(&again, "outcome").as_deref()), (Some(tip), Some("complete")));
    assert_eq!(blamed(&db, &again).await, ["a.txt"]);
    assert_eq!(count(&db, "survival").await, 2);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A repository whose `main` and `side` each changed `f.txt`: `main`'s
/// tip and `side`'s.
fn diverged(dir: &Path) -> (String, String) {
    repo(dir);
    commit(dir, &[("f.txt", "base\n")], "base");
    git(dir, &["switch", "-q", "-c", "side"]);
    let side = commit(dir, &[("f.txt", "side\n")], "side");
    git(dir, &["switch", "-q", "main"]);
    let main = commit(dir, &[("f.txt", "main\n")], "main");
    (main, side)
}

/// A checkout's newest state, as its row holds it.
async fn state_of(db: &superx_kernel::Db, toplevel: &Path) -> Value {
    let found = rows(
        db,
        &format!("SELECT *, valid_from FROM checkout_state WHERE checkout.toplevel = '{}' ORDER BY valid_from DESC LIMIT 1", toplevel.display()),
    )
    .await;
    found.into_iter().next().unwrap_or_else(|| panic!("a state of {}", toplevel.display()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkout_mid_merge_mid_rebase_mid_interactive_rebase_with_its_commands_done_and_left_mid_cherry_pick_mid_revert_mid_bisect_and_mid_am_says_so_in_its_checkout_state_and_one_in_the_middle_of_nothing_has_empty_operations()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let names = ["merging", "rebasing", "interactive", "picking", "reverting", "bisecting", "applying", "idle"];
    let dirs: BTreeMap<&str, std::path::PathBuf> = names.iter().map(|n| (*n, root.join(n))).collect();

    let (_, side) = diverged(&dirs["merging"]);
    assert!(!git_may(&dirs["merging"], &["merge", "side"]).0, "a conflict");
    let merge_side = side;

    let (main, _) = diverged(&dirs["rebasing"]);
    git(&dirs["rebasing"], &["switch", "-q", "side"]);
    assert!(!git_may(&dirs["rebasing"], &["rebase", "--apply", "main"]).0, "a conflict");
    let rebase_onto = main;

    let interactive = &dirs["interactive"];
    repo(interactive);
    commit(interactive, &[("a.txt", "1\n")], "one");
    let onto = git(interactive, &["rev-parse", "HEAD"]);
    commit(interactive, &[("a.txt", "1\n2\n")], "two");
    commit(interactive, &[("a.txt", "1\n2\n3\n")], "three");
    let edited = std::process::Command::new("git")
        .arg("-C")
        .arg(interactive)
        .args(["-c", "user.name=Test Author", "-c", "user.email=author@example.com", "-c", "core.hooksPath=/dev/null", "rebase", "-i", &onto])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_SEQUENCE_EDITOR", "sed -i.bak s/^pick/edit/")
        .output()
        .expect("git runs");
    assert!(edited.status.success(), "{}", String::from_utf8_lossy(&edited.stderr));

    let (_, side) = diverged(&dirs["picking"]);
    assert!(!git_may(&dirs["picking"], &["cherry-pick", &side]).0, "a conflict");
    let picked = side;

    let reverting = &dirs["reverting"];
    repo(reverting);
    commit(reverting, &[("f.txt", "one\n")], "one");
    let reverted = commit(reverting, &[("f.txt", "two\n")], "two");
    commit(reverting, &[("f.txt", "three\n")], "three");
    assert!(!git_may(reverting, &["revert", "--no-edit", &reverted]).0, "a conflict");

    let bisecting = &dirs["bisecting"];
    repo(bisecting);
    commit(bisecting, &[("f.txt", "one\n")], "one");
    git(bisecting, &["bisect", "start"]);

    let (_, side) = diverged(&dirs["applying"]);
    let patch = root.join("side.patch");
    std::fs::write(&patch, git(&dirs["applying"], &["format-patch", "-1", &side, "--stdout"]) + "\n").expect("patch");
    assert!(!git_may(&dirs["applying"], &["am", &patch.to_string_lossy()]).0, "a conflict");

    repo(&dirs["idle"]);
    commit(&dirs["idle"], &[("f.txt", "one\n")], "one");

    let all: Vec<&Path> = dirs.values().map(std::path::PathBuf::as_path).collect();
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&all))]), &mut Crawler::default()).await;

    let s = state_of(&db, &dirs["merging"]).await;
    assert_eq!((texts(&s, "operations"), texts(&s, "operation_commits")), (Some(vec!["merge".to_string()]), Some(vec![merge_side])));
    let s = state_of(&db, &dirs["rebasing"]).await;
    assert_eq!(texts(&s, "operations"), Some(vec!["rebase".to_string()]), "{s:?}");
    assert_eq!((text(&s, "rebase_branch").as_deref(), text(&s, "rebase_onto")), (Some("side"), Some(rebase_onto)));
    let s = state_of(&db, interactive).await;
    assert_eq!(texts(&s, "operations"), Some(vec!["rebase_interactive".to_string()]), "{s:?}");
    assert_eq!(
        (text(&s, "rebase_branch").as_deref(), text(&s, "rebase_onto"), int(&s, "rebase_done"), int(&s, "rebase_left")),
        (Some("main"), Some(onto), Some(1), Some(1))
    );
    let s = state_of(&db, &dirs["picking"]).await;
    assert_eq!((texts(&s, "operations"), texts(&s, "operation_commits")), (Some(vec!["cherry_pick".to_string()]), Some(vec![picked])));
    let s = state_of(&db, reverting).await;
    assert_eq!((texts(&s, "operations"), texts(&s, "operation_commits")), (Some(vec!["revert".to_string()]), Some(vec![reverted])));
    let s = state_of(&db, bisecting).await;
    assert_eq!((texts(&s, "operations"), text(&s, "bisect_start").as_deref()), (Some(vec!["bisect".to_string()]), Some("main")));
    let s = state_of(&db, &dirs["applying"]).await;
    assert_eq!(texts(&s, "operations"), Some(vec!["am".to_string()]), "{s:?}");
    let s = state_of(&db, &dirs["idle"]).await;
    assert_eq!((texts(&s, "operations"), texts(&s, "operation_commits")), (Some(vec![]), None), "in the middle of nothing");
    assert_eq!((int(&s, "rebase_done"), text(&s, "bisect_start")), (None, None));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkout_not_present_has_operations_none() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let (dir, wt) = (root.join("work"), root.join("work-wt"));
    repo(&dir);
    commit(&dir, &[("a.txt", "a\n")], "first");
    git(&dir, &["worktree", "add", "-q", "-b", "wtb", &wt.to_string_lossy()]);
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir, &wt]))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(texts(&state_of(&db, &wt).await, "operations"), Some(vec![]));
    std::fs::remove_dir_all(&wt).expect("the checkout is gone");
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let s = state_of(&db, &wt).await;
    assert_eq!((flag(&s, "present"), texts(&s, "operations")), (Some(false), None), "{s:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_a_session_printed_by_a_hash_prefix_one_commit_holds_is_tied_printed_to_that_session_and_its_tool_call_an_ambiguous_prefix_ties_nothing_and_a_sessions_pull_request_ties_its_listed_commits_and_its_merge_commit_via_pull_request()
{
    use sha2::{Digest as _, Sha256};

    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();

    // Printed: a repository whose objects git names by SHA-256, holding two
    // commits whose hashes begin alike.
    let printed = root.join("printed");
    std::fs::create_dir_all(&printed).expect("dir");
    git(&printed, &["init", "-q", "-b", "main", "--object-format=sha256"]);
    commit(&printed, &[("a.txt", "a\n")], "base");
    let unique = commit(&printed, &[("a.txt", "a\nb\n")], "unique");
    let tree = git(&printed, &["rev-parse", &format!("{unique}^{{tree}}")]);
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let (first, second) = (0u64..)
        .find_map(|n| {
            let content = format!("tree {tree}\nparent {unique}\nauthor A <a@x> 1790000000 +0000\ncommitter A <a@x> 1790000000 +0000\n\ntry {n}\n");
            let hash: String = Sha256::digest(format!("commit {}\0{content}", content.len()).as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            seen.insert(hash[..7].to_string(), content.clone()).map(|earlier| (earlier, content))
        })
        .expect("two hashes that begin alike");
    let (one, two) = (object(&printed, "commit", &first), object(&printed, "commit", &second));
    assert_eq!(one[..7], two[..7]);
    git(&printed, &["update-ref", "refs/heads/one", &one]);
    git(&printed, &["update-ref", "refs/heads/two", &two]);
    let mut session = Capture::new(&kernel, "printing").await;
    session.bash(&printed, "git commit -m unique", &format!("[main {}] unique\n 1 file changed", &unique[..7])).await;
    session.bash(&printed, "git commit -m try", &format!("[main {}] try\n 1 file changed", &one[..7])).await;

    // A pull request: a GitHub repository whose pull request listed two
    // commits and was squashed into a third.
    let pulled = root.join("pulled");
    repo(&pulled);
    let base = commit(&pulled, &[("base.txt", "base\n")], "base");
    git(&pulled, &["switch", "-q", "-c", "feat"]);
    let a = commit(&pulled, &[("a.txt", "a\n")], "a");
    let b = commit(&pulled, &[("b.txt", "b\n")], "b");
    git(&pulled, &["switch", "-q", "main"]);
    git(&pulled, &["merge", "-q", "--squash", "feat"]);
    let squash = commit(&pulled, &[], "squash");
    git(&pulled, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    hosted(&root, "github.com", "acme/widget", &pulled);
    let (github, bitbucket) = hosts_answering().await;
    let pull = github_pull(5, &b, &base, Some(&squash), "2026-10-02T10:00:00Z");
    github.json("/repos/acme/widget/pulls?state=all&sort=created&direction=asc&per_page=100", &serde_json::json!([pull]));
    github.json("/repos/acme/widget/pulls/5", &pull);
    github.json("/repos/acme/widget/pulls/5/commits?per_page=100", &serde_json::json!([{ "sha": a }, { "sha": b }]));
    let mut creating = Capture::new(&kernel, "creating").await;
    creating.bash(&pulled, "gh pr create --title t --body b", "https://github.com/acme/widget/pull/5\n").await;
    let mut merging = Capture::new(&kernel, "merging").await;
    merging.bash(&pulled, "gh pr merge 5 --squash", "").await;

    let own = places(&root, Some(credentials(&root, Some(TOKEN), None)), &github, &bitbucket);
    let done = pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&printed, &pulled]))]), &mut Crawler::new(own)).await;
    let ties = rows(
        &db,
        "SELECT commit.hash AS c, <string> session.kernel_session AS s, via, tool_call.call_id AS call, pull_request.number AS pull, at FROM commit_session",
    )
    .await;
    let mut got: Vec<(String, String, String, Option<i64>)> = ties
        .iter()
        .map(|t| (text(t, "c").expect("c"), text(t, "via").expect("via"), text(t, "call").unwrap_or_default(), int(t, "pull")))
        .collect();
    got.sort();
    let mut want = vec![(unique.clone(), "printed".to_string(), "call_1".to_string(), None)];
    for c in [&a, &b, &squash] {
        want.push(((*c).clone(), "pull_request".to_string(), "call_1".to_string(), Some(5)));
        want.push(((*c).clone(), "pull_request".to_string(), "call_1".to_string(), Some(5)));
    }
    want.sort();
    assert_eq!(got, want, "the ambiguous prefix ties nothing: {done:?}");
    let sessions: std::collections::BTreeSet<Option<String>> = ties.iter().map(|t| text(t, "s")).collect();
    assert_eq!(sessions.len(), 3, "each tie its own session's: {ties:?}");
    assert!(sessions.iter().all(Option::is_some));
    assert!(ties.iter().all(|t| time_of(t, "at").is_some()));
    // Written once.
    pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&printed, &pulled]))]), &mut Crawler::new(places(&root, Some(credentials(&root, Some(TOKEN), None)), &github, &bitbucket))).await;
    assert_eq!(count(&db, "commit_session").await, 7);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The counts a crawl row holds, by name.
const COUNTS: [&str; 30] = [
    "commits_read", "files_blamed", "files_left", "git_calls", "git_failed", "checkouts_read", "refs_read", "files_read",
    "reflog_read", "unreachable_read", "blame_lines_read", "bytes_read", "commits_new", "commit_files_new", "lines_added_new",
    "lines_removed_new", "refs_new", "refs_moved", "refs_gone", "refs_rewritten", "head_moves_new", "placements_changed",
    "landed_new", "survival_changed", "survival_delta", "git_timeouts", "git_ms", "rows_written", "unreachable_read", "commits_read",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crawl_row_holds_every_count_of_its_crawl_a_repository_whose_commits_read_differ_from_those_held_makes_its_crawl_and_the_repos_source_partial_naming_both_pass_source_counts_holds_commits_held_and_commits_read()
{
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "1\n2\n")], "first");
    let tip = commit(&dir, &[("a.txt", "1\n2\n3\n"), ("b.txt", "b\n")], "second");
    // A commit whose author git wrote no email for: git holds it, and it is
    // not read.
    let tree = git(&dir, &["rev-parse", &format!("{tip}^{{tree}}")]);
    let bad = object_as(&dir, "commit", &format!("tree {tree}\nparent {tip}\nauthor Nobody 1790000000 +0000\ncommitter Nobody <n@x> 1790000000 +0000\n\nbad\n"), true);
    git(&dir, &["update-ref", "refs/heads/bad", &bad]);
    let done = pass(&kernel, &db, &params(&[(INCLUDE_PARAM, strings(&[&dir]))]), &mut Crawler::default()).await;
    let crawl = newest_crawl(&db, "work").await;
    for name in COUNTS {
        assert!(int(&crawl, name).is_some(), "{name}: {crawl:?}");
    }
    assert_eq!(
        ["commits_read", "commits_new", "commit_files_new", "lines_added_new", "lines_removed_new", "files_read", "checkouts_read", "placements_changed"]
            .map(|k| int(&crawl, k)),
        [Some(2), Some(2), Some(3), Some(4), Some(0), Some(3), Some(1), Some(2)],
        "{crawl:?}"
    );
    assert_eq!(int(&crawl, "refs_read"), Some(2), "main and bad");
    assert_eq!(int(&crawl, "git_calls"), Some(count(&db, "git_call").await));
    assert_eq!(int(&crawl, "rows_written").map(|n| n > 0), Some(true));
    assert_eq!(text(&crawl, "outcome").as_deref(), Some("partial"));
    assert!(text(&crawl, "note").is_some_and(|n| n.contains("2 commits read of 3 held")), "{crawl:?}");
    let repos = done.source("repos").expect("the repos source");
    assert_eq!(repos.outcome, superx_mod_recon::crawler::Outcome::Partial);
    assert!(repos.note.as_deref().is_some_and(|n| n.contains("work partial, 2 commits read of 3 held")), "{repos:?}");
    let recorded = rows(&db, "SELECT counts FROM pass_source WHERE source = 'repos'").await;
    let counts = field(&recorded[0], "counts").expect("counts");
    assert_eq!((int(counts, "commits_held"), int(counts, "commits_read")), (Some(3), Some(2)));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repository_whose_git_directory_git_cannot_read_is_crawled_unreadable_every_count_0_mainline_head_and_head() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch();
    let dir = root.join("work");
    repo(&dir);
    commit(&dir, &[("a.txt", "a\n")], "first");
    let p = params(&[(INCLUDE_PARAM, strings(&[&dir]))]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    std::fs::write(dir.join(".git").join("HEAD"), "not a ref\n").expect("HEAD broken");
    let done = pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let crawl = newest_crawl(&db, "work").await;
    assert_eq!(
        (text(&crawl, "outcome").as_deref(), text(&crawl, "mainline").as_deref(), text(&crawl, "mainline_from").as_deref(), text(&crawl, "tip")),
        (Some("unreadable"), Some("HEAD"), Some("head"), None),
        "{crawl:?}"
    );
    for name in COUNTS.iter().filter(|n| !matches!(**n, "git_calls" | "git_failed" | "git_timeouts" | "git_ms" | "bytes_read" | "rows_written")) {
        assert_eq!(int(&crawl, name), Some(0), "{name}: {crawl:?}");
    }
    assert!(text(&crawl, "note").is_some_and(|n| n.contains("not a git repository")), "in git's words: {crawl:?}");
    let repos = done.source("repos").expect("the repos source");
    assert!(repos.note.as_deref().is_some_and(|n| n.contains("work unreadable")), "{repos:?}");
    std::fs::remove_dir_all(&root).expect("clean up");
}
