//! The repositories crawled in depth (#538, steps 1 to 14), end to end: a
//! kernel holding sessions' lines as capture keeps them, repositories made
//! with git in a scratch folder, and the module's own database, on the
//! in-memory engine. What a pass writes is read back from the database.

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use common::*;
use superx_kernel::types::{RecordId, Value};
use superx_mod_recon::crawler::hosts::Host;
use superx_mod_recon::crawler::repos::crawl::mirror_with;
use superx_mod_recon::crawler::Crawler;
use superx_mod_recon::params::{EXCLUDE_PARAM, REPOS_AT_ONCE_PARAM};

/// The repositories crawled in the newest pass, by name.
async fn crawled_in(db: &superx_kernel::Db, pass: &superx_mod_recon::crawler::Pass) -> BTreeSet<String> {
    let crawls = pass.source("repos").map(|s| s.crawls.clone()).unwrap_or_default();
    let mut names = BTreeSet::new();
    for c in crawls {
        let found = rows_with(db, "SELECT repository.name AS name FROM $c", "c", Value::RecordId(c)).await;
        names.extend(found.iter().filter_map(|r| text(r, "name")));
    }
    names
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_crawls_in_depth_every_repository_at_a_full_pass_and_at_another_pass_the_new_ones_and_those_a_git_event_of_the_passs_lines_names_by_its_dir_and_no_other_an_excluded_repository_never() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step1");
    let (a, b, c, d) = (root.join("a"), root.join("b"), root.join("c"), root.join("d"));
    for r in [&a, &b, &c] {
        repo(r);
    }
    let mut lines = Lines::new(&kernel, "s1").await;
    for r in [&a, &b, &c] {
        lines.worked_in(r).await;
    }
    let p = params(&[(EXCLUDE_PARAM, Value::Array(vec![Value::String("c".to_string())].into()))]);
    let mut crawler = Crawler::default();

    let first = pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(first.outcome, "complete", "{first:?}");
    assert_eq!(crawled_in(&db, &first).await, ["a", "b"].map(str::to_string).into(), "a full pass crawls every repository but the excluded");

    let second = pass(&kernel, &db, &p, &mut crawler).await;
    assert!(crawled_in(&db, &second).await.is_empty(), "nothing new, no git action: no crawl: {second:?}");

    // The session commits in b and works on in a: only b's git event sends
    // the crawler there.
    let hash = commit(&b, "more");
    lines.committed(&b, &hash, "more").await;
    lines.worked_in(&a).await;
    let third = pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(crawled_in(&db, &third).await, ["b"].map(str::to_string).into(), "{third:?}");

    // A repository new to the list is crawled the pass it joins; one the
    // session commits in but the operator excludes never is.
    repo(&d);
    lines.worked_in(&d).await;
    let hash = commit(&c, "excluded work");
    lines.committed(&c, &hash, "excluded work").await;
    let fourth = pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(crawled_in(&db, &fourth).await, ["d"].map(str::to_string).into(), "{fourth:?}");
    let excluded = rows(&db, "SELECT VALUE id FROM crawl WHERE repository.name = 'c'").await;
    assert!(excluded.is_empty(), "an excluded repository is never crawled");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_repositories_crawled_with_attr_recon_repos_at_once_1_and_at_its_fallback_write_the_same_facts_each_crawls_rows_written_after_its_crawl_row() {
    let root = scratch("step1b");
    let names = ["r1", "r2", "r3"];
    for name in names {
        let dir = root.join(name);
        repo(&dir);
        write(&dir, "src/lib.rs", "pub fn a() {}\n");
        commit(&dir, "lib");
        git(&dir, &["switch", "-q", "-c", "topic"]);
        write(&dir, "src/topic.rs", "x\n");
        commit(&dir, "topic");
        git(&dir, &["switch", "-q", "main"]);
    }
    let mut facts = Vec::new();
    for at_once in [Some(1), None] {
        let kernel = kernel().await;
        let db = own_db().await;
        let mut lines = Lines::new(&kernel, "s").await;
        for name in names {
            lines.worked_in(&root.join(name)).await;
        }
        let set: Vec<(&'static str, Value)> = at_once.map(|n| vec![(REPOS_AT_ONCE_PARAM, number(n))]).unwrap_or_default();
        let p = params(&set);
        let done = pass(&kernel, &db, &p, &mut Crawler::default()).await;
        assert_eq!(done.outcome, "complete", "{done:?}");
        let commits: BTreeSet<(String, String)> = rows(&db, "SELECT repository.name AS r, hash FROM commit")
            .await
            .iter()
            .filter_map(|c| Some((text(c, "r")?, text(c, "hash")?)))
            .collect();
        let places: BTreeSet<(String, String)> = rows(&db, "SELECT commit.hash AS h, place FROM placement")
            .await
            .iter()
            .filter_map(|c| Some((text(c, "h")?, text(c, "place")?)))
            .collect();
        let files = count(&db, "commit_file").await;
        facts.push((commits, places, files));
        // Every row a crawl wrote is written after its crawl's row.
        for crawl in rows(&db, "SELECT id, valid_from FROM crawl").await {
            let id = link(&crawl, "id").expect("an id");
            let later = rows_with(
                &db,
                "SELECT VALUE count() FROM git_call WHERE crawl = $c AND valid_from < $c.valid_from GROUP ALL",
                "c",
                Value::RecordId(id.clone()),
            )
            .await;
            assert!(later.is_empty() || matches!(&later[0], Value::Number(n) if n.to_int() == Some(0)), "a git call before its crawl: {later:?}");
        }
    }
    assert_eq!(facts[0], facts[1], "one at a time or all at once, the same facts");
    assert_eq!(facts[0].0.len(), 9, "three commits in each of three repositories");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A whole repository folder, file by file: what a crawl must leave as it
/// found it.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.strip_prefix(dir).expect("under").to_string_lossy().into_owned(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    out
}

/// The subcommand of a git call's arguments: the first after the overrides.
fn subcommand(args: &[String]) -> String {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-c" {
            i += 2;
            continue;
        }
        return args[i].clone();
    }
    String::new()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_git_call_in_an_operators_repository_carries_the_overrides_no_diff_driver_and_no_optional_lock_never_a_g_placeholder_a_fetch_or_a_write_and_is_a_git_call_row_with_its_step_dir_args_as_run_exit_timed_out_bytes_out_lines_out_and_stderr_whole() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step2");
    let (a, b) = (root.join("a"), root.join("b"));
    for r in [&a, &b] {
        repo(r);
        write(r, "src/lib.rs", "pub fn a() {}\npub fn b() {}\n");
        commit(r, "lib");
        // A tracked file touched since the index was written: a refresh
        // would write the index back.
        write(r, "README.md", "first\n");
    }
    // A ref git cannot read, in b: one call warns on its error output and
    // ends well, another fails on it.
    std::fs::write(b.join(".git/refs/heads/broken"), "not a hash\n").expect("broken ref");
    let before = (snapshot(&a), snapshot(&b));
    let mut lines = Lines::new(&kernel, "s2").await;
    lines.worked_in(&a).await;
    lines.worked_in(&b).await;
    let done = pass(&kernel, &db, &params(&[]), &mut Crawler::default()).await;
    assert_eq!((snapshot(&a), snapshot(&b)), before, "each repository is exactly as the crawl found it");

    let calls = rows(&db, "SELECT step, dir, args, exit, timed_out, bytes_out, lines_out, stderr, crawl, crawl.repository.name AS name FROM git_call").await;
    assert!(!calls.is_empty(), "{done:?}");
    let read_only = ["config", "rev-parse", "for-each-ref", "rev-list", "fsck", "cat-file", "log", "patch-id", "ls-tree", "blame"];
    for call in &calls {
        let args = texts(call, "args").expect("args");
        assert_eq!(args[..4], ["-c", "log.showSignature=false", "-c", "core.fsmonitor=false"], "the overrides first: {args:?}");
        let sub = subcommand(&args);
        assert!(read_only.contains(&sub.as_str()), "{sub} reads nothing but it may not: {args:?}");
        assert!(!args.iter().any(|a| a.contains("%G")), "no signature placeholder: {args:?}");
        if sub == "log" {
            assert!(args.contains(&"--no-textconv".to_string()) && args.contains(&"--no-ext-diff".to_string()), "{args:?}");
        }
        if sub == "blame" {
            assert!(args.contains(&"--no-textconv".to_string()), "{args:?}");
        }
        assert!(text(call, "step").is_some() && link(call, "crawl").is_some(), "{call:?}");
        let own = root.join(text(call, "name").expect("its repository")).join(".git");
        assert_eq!(text(call, "dir").as_deref(), Some(own.to_string_lossy().as_ref()), "in the repository's git directory");
        assert_eq!(flag(call, "timed_out"), Some(false));
    }
    let steps: BTreeSet<String> = calls.iter().filter(|c| text(c, "name").as_deref() == Some("a")).filter_map(|c| text(c, "step")).collect();
    for step in ["setup", "refs", "unreachable", "log", "patches", "reflog", "placement", "tip", "blame"] {
        assert!(steps.contains(step), "a call of the step {step}: {steps:?}");
    }
    let in_b: Vec<&Value> = calls.iter().filter(|c| text(c, "name").as_deref() == Some("b")).collect();
    let refs = in_b.iter().find(|c| texts(c, "args").is_some_and(|a| subcommand(&a) == "for-each-ref")).expect("b's refs");
    assert_eq!((int(refs, "exit"), text(refs, "stderr").as_deref()), (Some(0), Some("warning: ignoring broken ref refs/heads/broken\n")), "what git wrote, whole");
    let shallow = in_b
        .iter()
        .find(|c| texts(c, "args").is_some_and(|a| a.contains(&"--is-shallow-repository".to_string())))
        .expect("the shallow question");
    assert_eq!((int(shallow, "bytes_out"), int(shallow, "lines_out"), text(shallow, "stderr")), (Some(6), Some(1), None), "false and a newline");
    let all = in_b
        .iter()
        .find(|c| texts(c, "args").is_some_and(|a| a.contains(&"--reflog".to_string())))
        .expect("the commits' walk");
    assert_eq!(int(all, "exit"), Some(128), "the broken ref stops the walk");
    assert_eq!(text(all, "stderr").as_deref(), Some("fatal: bad object refs/heads/broken\n"));
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A remote_state's url, push_url, host and path.
type Remote = (Option<String>, Option<String>, Option<String>, Option<String>);

/// Each remote's newest remote_state, by name.
async fn remotes_now(db: &superx_kernel::Db) -> BTreeMap<String, Remote> {
    let mut newest: BTreeMap<String, (chrono::DateTime<chrono::Utc>, Remote)> = BTreeMap::new();
    for r in rows(db, "SELECT name, url, push_url, host, path, valid_from FROM remote_state").await {
        let (Some(name), Some(t)) = (text(&r, "name"), time(&r, "valid_from")) else { continue };
        let fields = (text(&r, "url"), text(&r, "push_url"), text(&r, "host"), text(&r, "path"));
        if newest.get(&name).is_none_or(|(seen, _)| *seen < t) {
            newest.insert(name, (t, fields));
        }
    }
    newest.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_remotes_url_push_url_host_and_path_is_a_remote_state_row_when_it_changes_and_one_with_url_none_once_it_is_gone_the_https_ssh_and_scp_like_forms_give_host_and_path_lowercased_without_git_a_local_path_neither_identity_is_the_configured_email() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step3");
    let a = root.join("a");
    repo(&a);
    git(&a, &["remote", "add", "origin", "https://GitHub.com/Acme/Widgets.git"]);
    git(&a, &["remote", "set-url", "--push", "origin", "ssh://git@github.com:22/acme/widgets.git"]);
    git(&a, &["remote", "add", "up", "git@bitbucket.org:Team/Repo.git"]);
    git(&a, &["remote", "add", "local", "/srv/git/x.git"]);
    git(&a, &["remote", "add", "secure", "ssh://git@GitHub.com:22/Acme/Tools.git"]);
    git(&a, &["remote", "add", "slash", "https://me@Bitbucket.org:443/WS/Repo/"]);
    git(&a, &["remote", "add", "served", "file:///srv/git/y.git"]);
    git(&a, &["config", "user.email", "me@work.test"]);
    let mut lines = Lines::new(&kernel, "s3").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        remotes_now(&db).await,
        [
            ("local".to_string(), (some("/srv/git/x.git"), None, None, None)),
            ("origin".to_string(), (some("https://GitHub.com/Acme/Widgets.git"), some("ssh://git@github.com:22/acme/widgets.git"), some("github.com"), some("acme/widgets"))),
            ("secure".to_string(), (some("ssh://git@GitHub.com:22/Acme/Tools.git"), None, some("github.com"), some("acme/tools"))),
            ("served".to_string(), (some("file:///srv/git/y.git"), None, None, None)),
            ("slash".to_string(), (some("https://me@Bitbucket.org:443/WS/Repo/"), None, some("bitbucket.org"), some("ws/repo"))),
            ("up".to_string(), (some("git@bitbucket.org:Team/Repo.git"), None, some("bitbucket.org"), some("team/repo"))),
        ]
        .into()
    );
    let crawl = rows(&db, "SELECT identity FROM crawl").await;
    assert_eq!(text(&crawl[0], "identity").as_deref(), Some("me@work.test"), "the email it commits as there");
    assert_eq!(count(&db, "remote_state").await, 6);

    // Read again unchanged: no row.
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(count(&db, "remote_state").await, 6, "a row only when a remote changes");

    // One moves, one goes.
    git(&a, &["remote", "set-url", "origin", "https://github.com/acme/other.git"]);
    git(&a, &["remote", "remove", "up"]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let now = remotes_now(&db).await;
    assert_eq!(now["origin"].0.as_deref(), Some("https://github.com/acme/other.git"));
    assert_eq!(now["origin"].3.as_deref(), Some("acme/other"));
    assert_eq!(now["up"], (None, None, None, None), "a remote gone is a row with no url");
    assert_eq!(count(&db, "remote_state").await, 8);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The newest crawl of the repository named `name`.
async fn newest_crawl(db: &superx_kernel::Db, name: &str) -> Value {
    let all = rows_with(db, "SELECT * FROM crawl WHERE repository.name = $n", "n", Value::String(name.to_string())).await;
    all.into_iter().max_by_key(|c| time(c, "valid_from")).expect("a crawl")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refs_are_counted_new_moved_rewritten_and_gone_against_the_previous_crawl_and_not_compared_at_a_repositorys_first_crawl_since_start() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step5a");
    let a = root.join("a");
    repo(&a);
    let base = head(&a);
    for branch in ["moves", "rewritten"] {
        git(&a, &["branch", branch]);
    }
    git(&a, &["tag", "v1"]);
    let mut lines = Lines::new(&kernel, "s5").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    let mut crawler = Crawler::default();
    pass(&kernel, &db, &p, &mut crawler).await;
    let first = newest_crawl(&db, "a").await;
    let counts = |c: &Value| ["refs_read", "refs_new", "refs_moved", "refs_rewritten", "refs_gone"].map(|n| int(c, n).unwrap_or(-1));
    assert_eq!(counts(&first), [4, 0, 0, 0, 0], "main, moves, rewritten and v1, compared with none");
    assert!(text(&first, "note").is_some_and(|n| n.contains("compared with none")), "{first:?}");

    // A new branch; one moved forward; one rewritten onto a commit its old
    // one is not under; the tag gone.
    git(&a, &["branch", "new"]);
    git(&a, &["switch", "-q", "moves"]);
    write(&a, "m.txt", "m\n");
    commit(&a, "forward");
    git(&a, &["switch", "-q", "--orphan", "elsewhere"]);
    write(&a, "e.txt", "e\n");
    let unrelated = commit(&a, "unrelated");
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["update-ref", "refs/heads/rewritten", &unrelated]);
    git(&a, &["tag", "-d", "v1"]);
    // The session's git action sends the crawler there in the next pass.
    lines.committed(&a, &base, "first").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let second = newest_crawl(&db, "a").await;
    assert_eq!(counts(&second), [5, 2, 2, 1, 1], "new and elsewhere new, two moved, one of them rewritten, v1 gone");
    assert!(!text(&second, "note").is_some_and(|n| n.contains("compared with none")));

    // After a restart: not compared.
    lines.committed(&a, &base, "first").await;
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(counts(&newest_crawl(&db, "a").await), [5, 0, 0, 0, 0]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_no_ref_and_no_reflog_reaches_is_read_and_placed_recovered_at_most_attr_recon_unreachable_commits_of_them() {
    let root = scratch("step6");
    let a = root.join("a");
    repo(&a);
    write(&a, "x.txt", "x\n");
    let x = commit(&a, "x");
    write(&a, "y.txt", "y\n");
    let y = commit(&a, "y");
    git(&a, &["reset", "-q", "--hard", "HEAD~2"]);
    git(&a, &["reflog", "expire", "--expire=now", "--all"]);
    for (limit, read) in [(None, 2), (Some(1), 1)] {
        let kernel = kernel().await;
        let db = own_db().await;
        let mut lines = Lines::new(&kernel, "s6").await;
        lines.worked_in(&a).await;
        let set: Vec<(&'static str, Value)> =
            limit.map(|n| vec![(superx_mod_recon::params::UNREACHABLE_COMMITS_PARAM, number(n))]).unwrap_or_default();
        pass(&kernel, &db, &params(&set), &mut Crawler::default()).await;
        let crawl = newest_crawl(&db, "a").await;
        assert_eq!(int(&crawl, "unreachable_read"), Some(2), "every one fsck named is counted");
        let recovered: BTreeSet<String> =
            rows(&db, "SELECT VALUE commit.hash FROM placement WHERE place = 'recovered'").await.iter().filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            }).collect();
        assert_eq!(recovered.len(), read, "{limit:?}: {recovered:?}");
        assert!(recovered.iter().all(|h| *h == x || *h == y));
        let held = rows(&db, "SELECT VALUE hash FROM commit").await.len();
        assert_eq!(held, 1 + read, "the reachable one and the recovered");
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Each file of the commit whose subject is `subject`: (path, change, added,
/// removed, added ignoring space, removed ignoring space, binary).
async fn files_of(db: &superx_kernel::Db, subject: &str) -> Vec<(String, String, i64, i64, i64, i64, bool)> {
    let found = rows_with(
        db,
        "SELECT path, change, added, removed, added_ignoring_space, removed_ignoring_space, binary FROM commit_file WHERE commit.subject = $s ORDER BY path",
        "s",
        Value::String(subject.to_string()),
    )
    .await;
    found
        .iter()
        .map(|f| {
            (
                text(f, "path").unwrap_or_default(),
                text(f, "change").unwrap_or_default(),
                int(f, "added").unwrap_or(-1),
                int(f, "removed").unwrap_or(-1),
                int(f, "added_ignoring_space").unwrap_or(-1),
                int(f, "removed_ignoring_space").unwrap_or(-1),
                flag(f, "binary").unwrap_or(false),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commits_files_are_added_modified_deleted_and_type_changed_a_rename_a_delete_and_an_add_a_binarys_counts_0_and_the_counts_ignoring_space_beside_the_others_a_merges_against_its_first_parent_its_added_and_removed_their_sums() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step7b");
    let a = root.join("a");
    repo(&a);
    write(&a, "a.txt", "one\ntwo\n");
    std::fs::write(a.join("b.bin"), b"\x00\x01\x02bin").expect("binary");
    for f in ["c.txt", "d.txt", "f.txt"] {
        write(&a, f, &format!("{f}\n"));
    }
    commit(&a, "files");
    write(&a, "a.txt", "one\ntwo  \nthree\n");
    std::fs::write(a.join("b.bin"), b"\x00\x01\x03bin").expect("binary");
    git(&a, &["rm", "-q", "c.txt"]);
    git(&a, &["mv", "d.txt", "e.txt"]);
    std::fs::remove_file(a.join("f.txt")).expect("rm");
    std::os::unix::fs::symlink("a.txt", a.join("f.txt")).expect("symlink");
    commit(&a, "changes");
    git(&a, &["switch", "-q", "-c", "side"]);
    write(&a, "g.txt", "g\ng\n");
    commit(&a, "side work");
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["merge", "-q", "--no-ff", "-m", "merged side", "side"]);
    let mut lines = Lines::new(&kernel, "s7").await;
    lines.worked_in(&a).await;
    pass(&kernel, &db, &params(&[]), &mut Crawler::default()).await;
    let row = |p: &str, c: &str, a: i64, r: i64, ba: i64, br: i64, bin: bool| (p.to_string(), c.to_string(), a, r, ba, br, bin);
    assert_eq!(
        files_of(&db, "changes").await,
        vec![
            row("a.txt", "modified", 2, 1, 1, 0, false),
            row("b.bin", "modified", 0, 0, 0, 0, true),
            row("c.txt", "deleted", 0, 1, 0, 1, false),
            row("d.txt", "deleted", 0, 1, 0, 1, false),
            row("e.txt", "added", 1, 0, 1, 0, false),
            row("f.txt", "type_changed", 1, 1, 1, 1, false),
        ],
        "a rename is a delete and an add; spacing alone counts only in added and removed"
    );
    assert_eq!(files_of(&db, "merged side").await, vec![row("g.txt", "added", 2, 0, 2, 0, false)], "a merge against its first parent");
    for c in rows(&db, "SELECT added, removed, math::sum((SELECT VALUE added FROM commit_file WHERE commit = $parent.id)) AS fa, \
                       math::sum((SELECT VALUE removed FROM commit_file WHERE commit = $parent.id)) AS fr FROM commit").await
    {
        assert_eq!((int(&c, "added"), int(&c, "removed")), (int(&c, "fa"), int(&c, "fr")), "a commit's counts sum its files: {c:?}");
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commits_are_placed_mainline_in_flight_branch_and_recovered_with_first_parent_and_a_placement_row_is_written_only_when_it_changes() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step9");
    let (a, wt) = (root.join("a"), root.join("a-wt"));
    repo(&a);
    let m2 = { write(&a, "m.txt", "m\n"); commit(&a, "m2") };
    git(&a, &["switch", "-q", "-c", "side"]);
    let s1 = { write(&a, "s.txt", "s\n"); commit(&a, "s1") };
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let merge = head(&a);
    git(&a, &["branch", "-D", "side"]);
    git(&a, &["switch", "-q", "-c", "parked"]);
    let p1 = { write(&a, "p.txt", "p\n"); commit(&a, "p1") };
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["worktree", "add", "-q", "-b", "feature", &wt.to_string_lossy()]);
    let f1 = { write(&wt, "f.txt", "f\n"); commit(&wt, "f1") };
    let r1 = { write(&a, "r.txt", "r\n"); commit(&a, "r1") };
    git(&a, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&a, &["reflog", "expire", "--expire=now", "--all"]);
    let mut lines = Lines::new(&kernel, "s9").await;
    lines.worked_in(&a).await;
    lines.worked_in(&wt).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let newest = || async {
        let mut by: BTreeMap<String, (chrono::DateTime<chrono::Utc>, String, bool)> = BTreeMap::new();
        for r in rows(&db, "SELECT commit.hash AS h, place, first_parent, valid_from FROM placement").await {
            let h = text(&r, "h").expect("hash");
            let t = time(&r, "valid_from").expect("time");
            if by.get(&h).is_none_or(|(seen, _, _)| *seen < t) {
                by.insert(h, (t, text(&r, "place").expect("place"), flag(&r, "first_parent").expect("first_parent")));
            }
        }
        by.into_iter().map(|(h, (_, p, f))| (h, (p, f))).collect::<BTreeMap<_, _>>()
    };
    let placed = newest().await;
    let at = |h: &str| placed.get(h).cloned().expect("placed");
    assert_eq!(at(&m2), ("mainline".to_string(), true));
    assert_eq!(at(&merge), ("mainline".to_string(), true));
    assert_eq!(at(&s1), ("mainline".to_string(), false), "merged in, off the first-parent history");
    assert_eq!(at(&f1), ("in_flight".to_string(), false), "its branch is checked out in a checkout that is there");
    assert_eq!(at(&p1), ("branch".to_string(), false));
    assert_eq!(at(&r1), ("recovered".to_string(), false));
    let rows_before = count(&db, "placement").await;

    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    assert_eq!(count(&db, "placement").await, rows_before, "nothing moved: no row");

    git(&a, &["merge", "-q", "--no-ff", "-m", "merge parked", "parked"]);
    lines.committed(&a, &head(&a), "merge parked").await;
    pass(&kernel, &db, &p, &mut Crawler::default()).await;
    let crawl = newest_crawl(&db, "a").await;
    assert_eq!(int(&crawl, "placements_changed"), Some(2), "parked's commit lands, and the merge is new");
    assert_eq!(newest().await.get(&p1).cloned(), Some(("mainline".to_string(), false)));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_commits_of_a_squash_merged_branch_land_via_the_squash_commit_for_each_file_whose_version_it_carries_and_not_for_a_file_the_squash_wrote_otherwise() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step10");
    let a = root.join("a");
    repo(&a);
    write(&a, "f1.txt", "1\n");
    write(&a, "f2.txt", "2\n");
    commit(&a, "base");
    git(&a, &["switch", "-q", "-c", "topic"]);
    write(&a, "f1.txt", "1\nmore\n");
    let t1 = commit(&a, "t1");
    write(&a, "f2.txt", "2\nbranch\n");
    let t2 = commit(&a, "t2");
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["merge", "-q", "--squash", "topic"]);
    write(&a, "f2.txt", "2\nsquash\n");
    let squash = commit(&a, "squash topic");
    let mut lines = Lines::new(&kernel, "s10").await;
    lines.worked_in(&a).await;
    pass(&kernel, &db, &params(&[]), &mut Crawler::default()).await;
    let landed: Vec<(String, String, String)> = rows(&db, "SELECT commit.hash AS c, path, via.hash AS v FROM landed_via")
        .await
        .iter()
        .filter_map(|r| Some((text(r, "c")?, text(r, "path")?, text(r, "v")?)))
        .collect();
    assert_eq!(landed, vec![(t1.clone(), "f1.txt".to_string(), squash.clone())], "f1's version landed; f2's did not");
    assert!(!landed.iter().any(|(c, _, _)| *c == t2));
    let crawl = newest_crawl(&db, "a").await;
    assert_eq!(int(&crawl, "landed_new"), Some(1));
    pass(&kernel, &db, &params(&[]), &mut Crawler::default()).await;
    assert_eq!(count(&db, "landed_via").await, 1, "a landing written once");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// Each commit's survival rows, oldest first, by subject.
async fn survivals(db: &superx_kernel::Db) -> BTreeMap<String, Vec<i64>> {
    let mut all: Vec<(chrono::DateTime<chrono::Utc>, String, i64)> = rows(db, "SELECT commit.subject AS s, alive, valid_from FROM survival")
        .await
        .iter()
        .map(|r| (time(r, "valid_from").expect("time"), text(r, "s").expect("subject"), int(r, "alive").expect("alive")))
        .collect();
    all.sort();
    let mut by: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (_, s, alive) in all {
        by.entry(s).or_default().push(alive);
    }
    by
}

/// The paths blamed, in the order of their calls' rows.
async fn blamed(db: &superx_kernel::Db) -> Vec<String> {
    let mut paths: Vec<String> = rows(db, "SELECT args FROM git_call WHERE step = 'blame'")
        .await
        .iter()
        .filter_map(|r| texts(r, "args")?.last().cloned())
        .collect();
    paths.sort();
    paths
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn survival_credits_each_commit_the_lines_blame_gives_it_at_the_tip_never_above_its_added_binaries_skipped_paths_and_the_files_past_attr_recon_blame_files_are_not_blamed_the_last_counted_in_files_left_nothing_is_blamed_while_the_tip_stays_and_a_commit_no_longer_credited_takes_alive_0() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step11");
    let a = root.join("a");
    repo(&a);
    write(&a, "a.txt", "1\n2\n3\n");
    std::fs::write(a.join("b.bin"), b"\x00bin\n").expect("binary");
    write(&a, "logo.png", "not an image\n");
    std::fs::write(a.join("t.dat"), b"\x00dat\n").expect("binary");
    write(&a, "z.txt", "z1\nz2\n");
    commit(&a, "c1");
    // t.dat turns to text: git counts no lines in that change, so c2's
    // added is u.txt's one line while blame credits it t.dat's three too.
    write(&a, "t.dat", "a\nb\nc\n");
    write(&a, "u.txt", "u\n");
    commit(&a, "c2");
    write(&a, "t.dat", "a\nb\nc\nd\n");
    write(&a, "a.txt", "1\n2\nthree\n");
    commit(&a, "c3");
    let mut lines = Lines::new(&kernel, "s11").await;
    lines.worked_in(&a).await;
    let p = params(&[(superx_mod_recon::params::BLAME_FILES_PARAM, number(4))]);
    let mut crawler = Crawler::default();
    pass(&kernel, &db, &p, &mut crawler).await;
    let crawl = newest_crawl(&db, "a").await;
    assert_eq!(blamed(&db).await, ["README.md", "a.txt", "t.dat", "u.txt"], "b.bin binary, logo.png skipped, z.txt past the first 4");
    let counts = |c: &Value| ["files_blamed", "files_left", "blame_lines_read", "survival_changed", "survival_delta"].map(|n| int(c, n).unwrap_or(-1));
    assert_eq!(counts(&crawl), [4, 1, 9, 4, 6]);
    let alive = |v: &[i64]| v.to_vec();
    let s = survivals(&db).await;
    assert_eq!(s.get("c1").map(|v| alive(v)), Some(vec![2]), "1 and 2 of a.txt; z.txt was not blamed");
    assert_eq!(s.get("c2").map(|v| alive(v)), Some(vec![1]), "credited 4, never above its added 1");
    assert_eq!(s.get("c3").map(|v| alive(v)), Some(vec![2]));

    // The tip stays: nothing is blamed.
    lines.committed(&a, &head(&a), "c3").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(counts(&newest_crawl(&db, "a").await), [0, 0, 0, 0, 0]);
    assert_eq!(blamed(&db).await.len(), 4);

    // a.txt rewritten whole: c1 is credited nothing and takes 0.
    write(&a, "a.txt", "x\ny\nz\n");
    commit(&a, "c4");
    lines.committed(&a, &head(&a), "c4").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    let s = survivals(&db).await;
    assert_eq!(s.get("c1").map(|v| alive(v)), Some(vec![2, 0]));
    assert_eq!(s.get("c2").map(|v| alive(v)), Some(vec![1]), "unchanged: no row");
    assert_eq!(s.get("c3").map(|v| alive(v)), Some(vec![2, 1]), "only t.dat's d is left");
    assert_eq!(s.get("c4").map(|v| alive(v)), Some(vec![3]));
    assert_eq!(s.get("first").map(|v| alive(v)), Some(vec![1]));
    assert_eq!(counts(&newest_crawl(&db, "a").await), [4, 1, 9, 3, -2 - 1 + 3]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A checkout_state's operations, operation_commits, rebase_branch,
/// rebase_onto, rebase_done, rebase_left and bisect_start.
type Operations = (Option<Vec<String>>, Option<Vec<String>>, Option<String>, Option<String>, Option<i64>, Option<i64>, Option<String>);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkout_mid_merge_mid_rebase_mid_interactive_rebase_with_its_commands_done_and_left_mid_cherry_pick_mid_revert_mid_bisect_and_mid_am_says_so_in_its_checkout_state_and_one_in_the_middle_of_nothing_has_empty_operations() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step12");
    let a = root.join("a");
    repo(&a);
    write(&a, "c.txt", "base\n");
    let base = commit(&a, "base");
    git(&a, &["switch", "-q", "-c", "ours"]);
    write(&a, "c.txt", "ours\n");
    commit(&a, "ours");
    git(&a, &["switch", "-q", "-c", "theirs", "main"]);
    write(&a, "c.txt", "theirs\n");
    let theirs = commit(&a, "theirs");
    git(&a, &["switch", "-q", "-c", "three", "main"]);
    for (file, text) in [("p1.txt", "p1\n"), ("c.txt", "p2\n"), ("p3.txt", "p3\n")] {
        write(&a, file, text);
        commit(&a, file);
    }
    git(&a, &["switch", "-q", "main"]);
    let wt = |name: &str| root.join(name);
    for (dir, branch, from) in [("merge", "merge", "ours"), ("rebase", "rebase", "ours"), ("inter", "inter", "three"), ("pick", "pick", "ours"), ("revert", "revert", "main"), ("am", "am", "ours"), ("bisect", "hunt", "ours")] {
        git(&a, &["worktree", "add", "-q", "-b", branch, &wt(dir).to_string_lossy(), from]);
    }
    let fails = |dir: &str, args: &[&str], env: &[(&str, &str)]| assert!(!git_may_fail(&wt(dir), args, env), "{dir} stops in the middle");
    fails("merge", &["merge", "theirs"], &[]);
    fails("rebase", &["rebase", "--apply", "theirs"], &[]);
    fails("inter", &["rebase", "-i", "theirs"], &[("GIT_SEQUENCE_EDITOR", "true")]);
    fails("pick", &["cherry-pick", "theirs"], &[]);
    write(&wt("revert"), "c.txt", "v1\n");
    let v1 = commit(&wt("revert"), "v1");
    write(&wt("revert"), "c.txt", "v2\n");
    commit(&wt("revert"), "v2");
    fails("revert", &["revert", &v1], &[]);
    let patch = git(&a, &["format-patch", "-1", "theirs", "-o", &root.join("patches").to_string_lossy()]);
    fails("am", &["am", patch.trim()], &[]);
    for args in [&["bisect", "start"][..], &["bisect", "bad"], &["bisect", "good", &base]] {
        assert!(git_may_fail(&wt("bisect"), args, &[]), "bisect {args:?}");
    }
    let mut lines = Lines::new(&kernel, "s12").await;
    lines.worked_in(&a).await;
    for dir in ["merge", "rebase", "inter", "pick", "revert", "am", "bisect"] {
        lines.worked_in(&wt(dir)).await;
    }
    pass(&kernel, &db, &params(&[]), &mut Crawler::default()).await;
    let mut states: BTreeMap<String, Operations> = BTreeMap::new();
    for r in rows(&db, "SELECT checkout.toplevel AS top, operations, operation_commits, rebase_branch, rebase_onto, rebase_done, rebase_left, bisect_start FROM checkout_state").await {
        let top = text(&r, "top").expect("toplevel");
        let name = Path::new(&top).file_name().expect("name").to_string_lossy().to_string();
        let some = |k: &str| texts(&r, k);
        states.insert(name, (some("operations"), some("operation_commits"), text(&r, "rebase_branch"), text(&r, "rebase_onto"), int(&r, "rebase_done"), int(&r, "rebase_left"), text(&r, "bisect_start")));
    }
    let ops = |o: &[&str]| Some(o.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let s = |v: &str| Some(v.to_string());
    assert_eq!(states["a"], (ops(&[]), None, None, None, None, None, None), "in the middle of nothing");
    assert_eq!(states["merge"], (ops(&["merge"]), ops(&[&theirs]), None, None, None, None, None));
    assert_eq!(states["rebase"], (ops(&["rebase"]), None, s("rebase"), s(&theirs), None, None, None), "rebase-apply without applying");
    assert_eq!(states["inter"], (ops(&["rebase_interactive"]), None, s("inter"), s(&theirs), Some(2), Some(1), None), "p1 and p2 done, p3 left");
    assert_eq!(states["pick"], (ops(&["cherry_pick"]), ops(&[&theirs]), None, None, None, None, None));
    assert_eq!(states["revert"], (ops(&["revert"]), ops(&[&v1]), None, None, None, None, None));
    assert_eq!(states["am"], (ops(&["am"]), None, None, None, None, None, None));
    assert_eq!(states["bisect"], (ops(&["bisect"]), None, None, None, None, None, s("hunt")));
    assert_eq!(states.len(), 8);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_repositories_whose_remotes_name_one_repository_on_a_host_make_one_host_repository_crawled_once_a_pass() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step4a");
    let (a, b, git_base) = (root.join("a"), root.join("b"), root.join("hosts"));
    repo(&a);
    repo(&b);
    hosted(&a, &git_base, "github.com", "acme/widgets");
    git(&a, &["remote", "add", "origin", "https://github.com/Acme/Widgets.git"]);
    git(&b, &["remote", "add", "origin", "git@github.com:acme/widgets.git"]);
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", serde_json::json!({ "id": 7, "full_name": "acme/widgets", "default_branch": "main" }));
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]));
    let mut lines = Lines::new(&kernel, "s4a").await;
    lines.worked_in(&a).await;
    lines.worked_in(&b).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut crawler).await;
    let asked_itself = || server.paths().iter().filter(|p| *p == "/repos/acme/widgets").count();
    assert_eq!(count(&db, "host_repository").await, 1);
    assert_eq!(count(&db, "host_crawl").await, 1, "crawled once in the pass");
    assert_eq!(asked_itself(), 1);
    let hr = rows(&db, "SELECT host, path, host_id FROM host_repository").await;
    assert_eq!((text(&hr[0], "host").as_deref(), text(&hr[0], "path").as_deref(), int(&hr[0], "host_id")), (Some("github.com"), Some("acme/widgets"), Some(7)));

    lines.committed(&a, &head(&a), "first").await;
    lines.committed(&b, &head(&b), "first").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "crawl").await, 4, "both crawled again");
    assert_eq!(count(&db, "host_repository").await, 1, "never a second row");
    assert_eq!(count(&db, "host_crawl").await, 2, "once more, in the second pass");
    assert_eq!(asked_itself(), 2);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mirror_is_made_bare_in_the_repos_folder_and_fetched_with_the_refspec_the_settings_and_the_environment_step_4_names_no_token_in_any_argument_or_variable_a_ref_the_host_deletes_or_rewrites_keeps_in_the_mirror_what_was_fetched_without_a_token_or_with_the_repository_unanswered_no_mirror_is_made() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step4b");
    let (a, git_base, home) = (root.join("a"), root.join("hosts"), root.join("home"));
    repo(&a);
    git(&a, &["branch", "feature"]);
    write(&a, "x.txt", "x\n");
    let fetched = commit(&a, "x");
    let feature = git(&a, &["rev-parse", "feature"]).trim().to_string();
    let host = hosted(&a, &git_base, "github.com", "acme/widgets");
    git(&a, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", serde_json::json!({ "id": 7, "default_branch": "main" }));
    let at = places(&home, endpoints(&server, &git_base), &["github.com"]);
    let mut crawler = Crawler::new(at.clone());
    let mut lines = Lines::new(&kernel, "s4b").await;
    lines.worked_in(&a).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut crawler).await;

    let mirror = home.join("modules/recon/repos/github.com/acme/widgets.git");
    assert_eq!(git(&mirror, &["rev-parse", "--is-bare-repository"]).trim(), "true");
    let m = rows(&db, "SELECT url, folder FROM mirror").await;
    assert_eq!(m.len(), 1);
    assert_eq!((text(&m[0], "url").as_deref(), text(&m[0], "folder").as_deref()), (Some("https://github.com/acme/widgets.git"), Some("github.com/acme/widgets.git")));
    let calls: Vec<Vec<String>> = rows(&db, "SELECT VALUE args FROM git_call WHERE step = 'fetch'")
        .await
        .into_iter()
        .map(|v| match v {
            Value::Array(items) => items.into_iter().filter_map(|i| if let Value::String(s) = i { Some(s) } else { None }).collect(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(calls.len(), 2, "init and fetch");
    let fetch = calls.iter().find(|a| a.contains(&"+refs/*:refs/*".to_string())).expect("the fetch");
    let url = host.to_string_lossy().into_owned();
    assert_eq!(fetch[fetch.len() - 5..], ["fetch", "--no-auto-maintenance", "--no-recurse-submodules", url.as_str(), "+refs/*:refs/*"]);
    assert_eq!(fetch.iter().filter(|a| a.starts_with("+refs")).count(), 1, "the fetch's only refspec");
    let init = calls.iter().find(|a| a.contains(&"init".to_string())).expect("the init");
    assert!(init.ends_with(&["init".to_string(), "--bare".to_string(), "--template=".to_string(), mirror.to_string_lossy().into_owned()]));
    let settings = [
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
        "http.lowSpeedLimit=1",
        "http.lowSpeedTime=60",
        "credential.username=x-access-token",
    ];
    for call in rows(&db, "SELECT VALUE args FROM git_call WHERE dir CONTAINS 'modules/recon/repos'").await {
        let Value::Array(args) = call else { unreachable!("args") };
        let args: Vec<String> = args.into_iter().filter_map(|i| if let Value::String(s) = i { Some(s) } else { None }).collect();
        for setting in settings {
            assert!(args.windows(2).any(|w| w[0] == "-c" && w[1] == setting), "{setting} on every call on a mirror: {args:?}");
        }
    }
    let with = mirror_with(&at, Host::GitHub);
    let env: Vec<(&str, &str)> = with.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let askpass = env!("CARGO_BIN_EXE_superx-recon");
    let home_text = home.to_string_lossy().into_owned();
    assert_eq!(
        env,
        [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ASKPASS", askpass),
            ("SUPERX_RECON_ASKPASS", "github.com"),
            ("SUPERX_HOME", home_text.as_str()),
        ]
    );
    assert_eq!(with.config, settings);
    let everything = format!("{:?}{:?}{}", with, rows(&db, "SELECT args FROM git_call").await, std::fs::read_to_string(mirror.join("config")).expect("config"));
    assert!(!everything.contains(TOKEN), "no token in any argument, variable or file");

    // The host deletes one ref and rewrites another: the mirror keeps both.
    git(&host, &["update-ref", "-d", "refs/heads/feature"]);
    git(&a, &["switch", "-q", "--orphan", "other"]);
    write(&a, "o.txt", "o\n");
    let other = commit(&a, "other");
    git(&a, &["push", "-q", "-f", &url, "other:refs/heads/main"]);
    git(&a, &["switch", "-q", "main"]);
    lines.committed(&a, &fetched, "x").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(git(&mirror, &["rev-parse", "refs/heads/feature"]).trim(), feature, "deleted on the host, kept in the mirror");
    assert_eq!(git(&mirror, &["rev-parse", "refs/heads/main"]).trim(), other);
    assert!(git_may_fail(&mirror, &["cat-file", "-e", &fetched], &[]), "what was fetched stays");
    let moves = git(&mirror, &["reflog", "show", "--format=%H", "refs/heads/main"]);
    assert_eq!(moves.lines().collect::<Vec<_>>(), [other.as_str(), fetched.as_str()]);

    // No token: no mirror. The repository unanswered: no mirror.
    for (name, tokens, answer) in [("none", &[][..], None), ("unanswered", &["github.com"][..], Some(Answer::status(500, "{\"message\":\"Server Error\"}")))] {
        let db = own_db().await;
        let server = Server::start().await;
        github_quiet(&server, "acme/widgets", serde_json::json!({ "id": 7, "default_branch": "main" }));
        if let Some(answer) = answer {
            server.on("/repos/acme/widgets", vec![answer]);
        }
        let home = root.join(name);
        pass(&kernel, &db, &p, &mut Crawler::new(places(&home, endpoints(&server, &git_base), tokens))).await;
        assert_eq!(count(&db, "mirror").await, 0, "{name}");
        assert!(!home.join("modules/recon/repos/github.com").exists(), "{name}");
        assert_eq!(count(&db, "git_call WHERE step = 'fetch'").await, 0, "{name}");
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_main_line_is_chosen_override_then_host_default_from_the_mirrors_branch_or_else_origins_then_origin_head_local_branch_and_head_each_recorded_with_its_word_and_with_no_tip_nothing_is_placed_on_it() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step5b");
    let git_base = root.join("hosts");
    let dir = |name: &str| root.join(name);
    let mut tips: BTreeMap<&str, String> = BTreeMap::new();
    for name in ["over", "missing", "hosted", "fallback", "origins", "local", "headed", "unborn"] {
        repo(&dir(name));
        tips.insert(name, head(&dir(name)));
    }
    // override: the operator names release, ahead of main.
    git(&dir("over"), &["switch", "-q", "-c", "release"]);
    write(&dir("over"), "r.txt", "r\n");
    tips.insert("over", commit(&dir("over"), "release"));
    git(&dir("over"), &["switch", "-q", "main"]);
    // host_default from the mirror: the host's default branch is trunk,
    // which only the host holds.
    let host = hosted(&dir("hosted"), &git_base, "github.com", "acme/hosted");
    git(&host, &["update-ref", "refs/heads/trunk", "refs/heads/main"]);
    git(&dir("hosted"), &["remote", "add", "origin", "https://github.com/acme/hosted.git"]);
    // host_default from origin's: the host's default branch dev is not in
    // its mirror, and origin's dev is here.
    hosted(&dir("fallback"), &git_base, "github.com", "acme/fallback");
    git(&dir("fallback"), &["remote", "add", "origin", "https://github.com/acme/fallback.git"]);
    git(&dir("fallback"), &["update-ref", "refs/remotes/origin/dev", "HEAD"]);
    // origin_head.
    git(&dir("origins"), &["remote", "add", "origin", "/srv/git/origins.git"]);
    git(&dir("origins"), &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&dir("origins"), &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
    write(&dir("origins"), "ahead.txt", "ahead\n");
    commit(&dir("origins"), "ahead");
    // local_branch: master, as there is no main.
    git(&dir("local"), &["branch", "-q", "-m", "main", "master"]);
    // head: neither main nor master.
    git(&dir("headed"), &["branch", "-q", "-m", "main", "work"]);
    // No tip: HEAD unborn, the only commit on feature.
    git(&dir("unborn"), &["branch", "-q", "-m", "main", "feature"]);
    git(&dir("unborn"), &["symbolic-ref", "HEAD", "refs/heads/nothing"]);

    let server = Server::start().await;
    github_quiet(&server, "acme/hosted", serde_json::json!({ "id": 1, "default_branch": "trunk" }));
    github_quiet(&server, "acme/fallback", serde_json::json!({ "id": 2, "default_branch": "dev" }));
    let mut refs = superx_kernel::types::Object::new();
    refs.insert("over".to_string(), Value::String("release".to_string()));
    refs.insert("missing".to_string(), Value::String("nothere".to_string()));
    let p = params(&[(superx_mod_recon::params::MAINLINE_REFS_PARAM, Value::Object(refs))]);
    let mut lines = Lines::new(&kernel, "s5b").await;
    for name in tips.keys() {
        lines.worked_in(&dir(name)).await;
    }
    pass(&kernel, &db, &p, &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]))).await;

    let chosen = |name: &'static str| {
        let db = &db;
        async move {
            let c = newest_crawl(db, name).await;
            (text(&c, "mainline"), text(&c, "mainline_from"), text(&c, "tip"), text(&c, "outcome"))
        }
    };
    let some = |s: &str| Some(s.to_string());
    let tip = |name: &str| some(&tips[name]);
    assert_eq!(chosen("over").await, (some("refs/heads/release"), some("override"), tip("over"), some("complete")));
    assert_eq!(chosen("missing").await, (some("refs/heads/main"), some("local_branch"), tip("missing"), some("complete")), "an override that does not resolve");
    assert_eq!(chosen("hosted").await, (some("refs/heads/trunk"), some("host_default"), tip("hosted"), some("complete")), "the mirror's");
    assert_eq!(chosen("fallback").await, (some("refs/remotes/origin/dev"), some("host_default"), tip("fallback"), some("complete")), "origin's");
    assert_eq!(chosen("origins").await, (some("refs/remotes/origin/main"), some("origin_head"), tip("origins"), some("complete")));
    assert_eq!(chosen("local").await, (some("refs/heads/master"), some("local_branch"), tip("local"), some("complete")));
    assert_eq!(chosen("headed").await, (some("HEAD"), some("head"), tip("headed"), some("complete")));
    let unborn = newest_crawl(&db, "unborn").await;
    assert_eq!(chosen("unborn").await, (some("HEAD"), some("head"), None, some("complete")), "{unborn:?}");
    assert!(text(&unborn, "note").is_some_and(|n| n.contains("no main line resolves")), "{unborn:?}");
    assert_eq!(int(&unborn, "files_blamed"), Some(0));
    let placed = rows(&db, "SELECT VALUE place FROM placement WHERE commit.repository.name = 'unborn'").await;
    assert_eq!(placed, vec![Value::String("branch".to_string())], "nothing on the main line");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// What `git patch-id --stable` says of a commit's change.
fn patch_id_of(dir: &Path, commit: &str) -> String {
    let shown = std::process::Command::new("git").arg("-C").arg(dir).args(["show", "--no-ext-diff", "--no-textconv", commit]).output().expect("show");
    let mut id = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["patch-id", "--stable"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("patch-id");
    std::io::Write::write_all(id.stdin.as_mut().expect("its input"), &shown.stdout).expect("fed");
    let out = id.wait_with_output().expect("its answer");
    String::from_utf8_lossy(&out.stdout).split(' ').next().expect("an id").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_takes_its_hash_tree_parents_emails_names_times_offsets_in_minutes_subject_body_with_its_trailers_signature_whole_and_patch_id_a_cherry_picked_copy_sharing_its_originals_patch_id_and_a_merge_and_an_empty_commit_having_none_a_commit_only_a_mirror_holds_is_read_from_the_mirror_attr_recon_since_days_leaves_older_commits_unread() {
    let kernel = kernel().await;
    let root = scratch("step7a");
    let (a, git_base) = (root.join("a"), root.join("hosts"));
    repo(&a);
    let first = head(&a);
    let now = chrono::Utc::now().timestamp();
    let (authored, committed) = (now - 2 * 86_400, now - 86_400);
    write(&a, "p.txt", "p\n");
    git(&a, &["add", "p.txt"]);
    let (author_date, committer_date) = (format!("{authored} -0400"), format!("{committed} +0530"));
    git_env(
        &a,
        &["commit", "-q", "--cleanup=verbatim", "-m", "subject line\n\n\nbody para\n\nSigned-off-by: Ann Author <ann@x.test>\n"],
        &[
            ("GIT_AUTHOR_NAME", "Ann Author"),
            ("GIT_AUTHOR_EMAIL", "ann@x.test"),
            ("GIT_AUTHOR_DATE", &author_date),
            ("GIT_COMMITTER_NAME", "Cid Committer"),
            ("GIT_COMMITTER_EMAIL", "cid@x.test"),
            ("GIT_COMMITTER_DATE", &committer_date),
        ],
    );
    let people = head(&a);
    // A signed commit, its signature never verified.
    write(&a, "s.txt", "s\n");
    git(&a, &["add", "s.txt"]);
    let tree = git(&a, &["write-tree"]).trim().to_string();
    let object = format!(
        "tree {tree}\nparent {people}\nauthor Sig Ner <sig@x.test> {now} +0000\ncommitter Sig Ner <sig@x.test> {now} +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n wsBcBAAB\n -----END PGP SIGNATURE-----\n\nsigned\n"
    );
    std::fs::write(root.join("signed.txt"), object).expect("object");
    let signed = git(&a, &["hash-object", "-t", "commit", "-w", &root.join("signed.txt").to_string_lossy()]).trim().to_string();
    git(&a, &["update-ref", "refs/heads/main", &signed]);
    // An original and its cherry-picked copy; a merge; an empty commit.
    git(&a, &["switch", "-q", "-c", "side", &first]);
    write(&a, "o.txt", "o\n");
    let original = commit(&a, "original");
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["cherry-pick", "-x", &original]);
    let copy = head(&a);
    git(&a, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let merge = head(&a);
    let empty = commit(&a, "nothing");
    // A commit older than a month, on a branch of its own.
    git(&a, &["switch", "-q", "-c", "old", &first]);
    write(&a, "old.txt", "old\n");
    git(&a, &["add", "old.txt"]);
    git_env(&a, &["commit", "-q", "-m", "old"], &[("GIT_AUTHOR_DATE", "1577836800 +0000"), ("GIT_COMMITTER_DATE", "1577836800 +0000")]);
    let old = head(&a);
    git(&a, &["switch", "-q", "main"]);
    // A commit only the host holds.
    let host = hosted(&a, &git_base, "github.com", "acme/widgets");
    let elsewhere = root.join("elsewhere");
    git(&root, &["clone", "-q", &host.to_string_lossy(), &elsewhere.to_string_lossy()]);
    write(&elsewhere, "m.txt", "m\n");
    let only_mirror = commit(&elsewhere, "mirror only");
    git(&elsewhere, &["push", "-q", "origin", "HEAD:refs/heads/elsewhere"]);
    git(&a, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);
    assert!(!git_may_fail(&a, &["cat-file", "-e", &only_mirror], &[]), "the repository does not hold it");

    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", serde_json::json!({ "id": 7, "default_branch": "main" }));
    let db = own_db().await;
    let mut lines = Lines::new(&kernel, "s7a").await;
    lines.worked_in(&a).await;
    pass(&kernel, &db, &params(&[]), &mut Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]))).await;
    let read = |hash: String| {
        let db = &db;
        async move {
            let found = rows_with(db, "SELECT * FROM commit WHERE hash = $h", "h", Value::String(hash.clone())).await;
            assert_eq!(found.len(), 1, "{hash} read once");
            found.into_iter().next().expect("a row")
        }
    };
    let c = read(people.clone()).await;
    assert_eq!(text(&c, "tree"), Some(git(&a, &["rev-parse", &format!("{people}^{{tree}}")]).trim().to_string()));
    assert_eq!((texts(&c, "parent_hashes"), int(&c, "parents")), (Some(vec![first.clone()]), Some(1)));
    assert_eq!((text(&c, "author").as_deref(), text(&c, "author_name").as_deref()), (Some("ann@x.test"), Some("Ann Author")));
    assert_eq!((text(&c, "committer").as_deref(), text(&c, "committer_name").as_deref()), (Some("cid@x.test"), Some("Cid Committer")));
    assert_eq!((time(&c, "authored_at").map(|t| t.timestamp()), time(&c, "committed_at").map(|t| t.timestamp())), (Some(authored), Some(committed)));
    assert_eq!((int(&c, "author_offset"), int(&c, "committer_offset")), (Some(-240), Some(330)), "minutes east of UTC");
    assert_eq!(text(&c, "subject").as_deref(), Some("subject line"));
    assert_eq!(text(&c, "body").as_deref(), Some("body para\n\nSigned-off-by: Ann Author <ann@x.test>\n"), "the empty lines that open it removed, the trailer kept");
    assert_eq!(text(&c, "signature"), None);
    assert_eq!(text(&c, "patch_id"), Some(patch_id_of(&a, &people)));

    let s = read(signed).await;
    assert_eq!(text(&s, "signature").as_deref(), Some("-----BEGIN PGP SIGNATURE-----\n\nwsBcBAAB\n-----END PGP SIGNATURE-----"));
    assert_eq!(text(&s, "body"), None, "nothing follows its subject");
    let (o, p) = (read(original.clone()).await, read(copy).await);
    assert!(text(&o, "patch_id").is_some());
    assert_eq!(text(&p, "patch_id"), text(&o, "patch_id"), "a copy shares its original's");
    let m = read(merge).await;
    assert_eq!((int(&m, "parents"), text(&m, "patch_id")), (Some(2), None));
    assert_eq!(text(&read(empty).await, "patch_id"), None);
    let mirrored = read(only_mirror).await;
    assert_eq!(text(&mirrored, "subject").as_deref(), Some("mirror only"), "read from the mirror");
    read(old.clone()).await;

    // attr_recon_since_days 30: the old commit unread.
    let db = own_db().await;
    pass(&kernel, &db, &params(&[(superx_mod_recon::params::SINCE_DAYS_PARAM, number(30))]), &mut Crawler::default()).await;
    let since = rows(&db, "SELECT since, started_at FROM crawl").await;
    let (since, started) = (time(&since[0], "since").expect("since"), time(&since[0], "started_at").expect("started"));
    assert!((started - since - chrono::TimeDelta::days(30)).num_seconds().abs() <= 5, "a month before the crawl began");
    assert_eq!(rows_with(&db, "SELECT VALUE hash FROM commit WHERE hash = $h", "h", Value::String(old)).await.len(), 0);
    assert!(count(&db, "commit").await >= 6);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// The entries of every reflog file under `logs`, as git keeps them.
fn reflog_lines(logs: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![logs.to_path_buf()];
    while let Some(here) = stack.pop() {
        if here.is_dir() {
            stack.extend(std::fs::read_dir(&here).expect("a folder").map(|e| e.expect("an entry").path()));
        } else {
            n += std::fs::read_to_string(&here).expect("a reflog").lines().count();
        }
    }
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_reflog_entry_of_a_branch_a_remote_tracking_ref_each_checkouts_head_the_stash_and_a_mirrors_ref_is_one_ref_move_row_with_its_old_new_identity_action_and_at_the_zero_id_none_never_written_twice_across_passes_and_still_there_after_git_expires_it() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step8");
    let (a, wt, git_base, home) = (root.join("a"), root.join("a-wt"), root.join("hosts"), root.join("home"));
    repo(&a);
    let first = head(&a);
    write(&a, "c.txt", "c\n");
    commit(&a, "second");
    // A remote-tracking ref, fetched from a repository of the test's own.
    let upstream = root.join("upstream.git");
    git(&root, &["clone", "-q", "--bare", &a.to_string_lossy(), &upstream.to_string_lossy()]);
    git(&a, &["remote", "add", "up", &upstream.to_string_lossy()]);
    git(&a, &["fetch", "-q", "up"]);
    // A worktree, and the stash.
    git(&a, &["worktree", "add", "-q", "-b", "wt", &wt.to_string_lossy()]);
    write(&wt, "w.txt", "w\n");
    commit(&wt, "in the worktree");
    write(&a, "c.txt", "changed\n");
    git(&a, &["stash", "-q"]);
    // A mirror.
    hosted(&a, &git_base, "github.com", "acme/widgets");
    git(&a, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);
    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", serde_json::json!({ "id": 7, "default_branch": "main" }));
    let mut crawler = Crawler::new(places(&home, endpoints(&server, &git_base), &["github.com"]));
    let mut lines = Lines::new(&kernel, "s8").await;
    lines.worked_in(&a).await;
    lines.worked_in(&wt).await;
    let p = params(&[]);
    pass(&kernel, &db, &p, &mut crawler).await;

    let mirror = home.join("modules/recon/repos/github.com/acme/widgets.git");
    let kept = reflog_lines(&a.join(".git/logs")) + reflog_lines(&a.join(".git/worktrees/a-wt/logs"));
    let in_mirror = reflog_lines(&mirror.join("logs"));
    assert!(in_mirror > 0, "the mirror logs every ref's move");
    assert_eq!(count(&db, "ref_move").await, i64::try_from(kept + in_mirror).expect("a count"), "one row an entry");
    let refs: BTreeSet<String> = rows(&db, "SELECT VALUE ref FROM ref_move WHERE mirror = NONE")
        .await
        .into_iter()
        .filter_map(|v| if let Value::String(s) = v { Some(s) } else { None })
        .collect();
    for wanted in ["HEAD", "refs/heads/main", "refs/heads/wt", "refs/remotes/up/main", "refs/stash"] {
        assert!(refs.contains(wanted), "{wanted} in {refs:?}");
    }
    let born = rows(&db, "SELECT * FROM ref_move WHERE ref = 'refs/heads/main' AND old = NONE AND mirror = NONE").await;
    assert_eq!(born.len(), 1, "the zero id is none");
    assert_eq!(text(&born[0], "new"), Some(first.clone()));
    assert_eq!(text(&born[0], "identity").as_deref(), Some("test@example.test"));
    assert_eq!(text(&born[0], "action").as_deref(), Some("commit (initial): first"));
    assert!(time(&born[0], "at").is_some_and(|t| (chrono::Utc::now() - t).num_minutes() < 10));
    let heads: Vec<Value> = rows(&db, "SELECT VALUE checkout.toplevel FROM ref_move WHERE ref = 'HEAD' AND mirror = NONE").await;
    let tops: BTreeSet<String> = heads.into_iter().filter_map(|v| if let Value::String(s) = v { Some(s) } else { None }).collect();
    assert_eq!(tops.len(), 2, "each checkout's own HEAD: {tops:?}");
    let stash = rows(&db, "SELECT action, checkout FROM ref_move WHERE ref = 'refs/stash'").await;
    assert!(text(&stash[0], "action").is_some_and(|m| m.starts_with("WIP on main")), "{stash:?}");
    // `a = NONE AND b != NONE` drops `b != NONE` (SurrealDB 3.1.4): the
    // rows are sorted here.
    let mirrored = rows(&db, "SELECT mirror, checkout FROM ref_move").await;
    assert_eq!(mirrored.iter().filter(|v| link(v, "mirror").is_some() && link(v, "checkout").is_none()).count(), in_mirror);
    let crawl = newest_crawl(&db, "a").await;
    assert_eq!(int(&crawl, "reflog_read"), Some(i64::try_from(kept + in_mirror).expect("a count")));
    let head_entries = std::fs::read_to_string(a.join(".git/logs/HEAD")).expect("HEAD").lines().count()
        + std::fs::read_to_string(a.join(".git/worktrees/a-wt/logs/HEAD")).expect("its HEAD").lines().count();
    assert_eq!(int(&crawl, "head_moves_new"), Some(i64::try_from(head_entries).expect("a count")));

    // Again: nothing written twice. Expired by git: still there.
    let written = count(&db, "ref_move").await;
    lines.committed(&a, &first, "first").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "ref_move").await, written);
    git(&a, &["reflog", "expire", "--expire=now", "--all"]);
    assert!(reflog_lines(&a.join(".git/logs")) < kept);
    lines.committed(&a, &first, "first").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "ref_move").await, written, "an entry git expired stays as written");
    assert_eq!(count(&db, "crawl").await, 3);
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A commit_session's commit hash, session, via, tool_call and
/// pull_request.
type Tied = (String, RecordId, String, Option<RecordId>, Option<RecordId>);

/// Two commits of `dir` whose hashes share their first seven characters,
/// each on a branch of its own: found among a hundred thousand children
/// of HEAD hashed by one git fast-import, the rest left unreachable.
fn twins(dir: &Path) -> (String, String) {
    let base = head(dir);
    let mut stream = Vec::new();
    for i in 1..=100_000 {
        let message = format!("twin {i}\n");
        stream.extend_from_slice(
            format!("commit refs/heads/twins\nmark :{i}\ncommitter Test Person <test@example.test> 1700000000 +0000\ndata {}\n{message}from {base}\n\n", message.len())
                .as_bytes(),
        );
    }
    let marks = dir.join(".git/twin-marks");
    let mut import = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["fast-import", "--quiet", &format!("--export-marks={}", marks.display())])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("fast-import");
    std::io::Write::write_all(import.stdin.as_mut().expect("its input"), &stream).expect("fed");
    assert!(import.wait().expect("it ends").success());
    let mut by: HashMap<String, Vec<String>> = HashMap::new();
    for line in std::fs::read_to_string(&marks).expect("marks").lines() {
        let hash = line.split(' ').nth(1).expect("a hash").to_string();
        by.entry(hash[..7].to_string()).or_default().push(hash);
    }
    std::fs::remove_file(&marks).expect("marks gone");
    let pair = by.into_values().find(|v| v.len() == 2).expect("a shared prefix among a hundred thousand");
    git(dir, &["update-ref", "refs/heads/twin-a", &pair[0]]);
    git(dir, &["update-ref", "refs/heads/twin-b", &pair[1]]);
    git(dir, &["update-ref", "-d", "refs/heads/twins"]);
    (pair[0].clone(), pair[1].clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_a_session_printed_by_a_hash_prefix_one_commit_holds_is_tied_printed_to_that_session_and_its_tool_call_an_ambiguous_prefix_ties_nothing_and_a_sessions_pull_request_ties_its_listed_commits_and_its_merge_commit_via_pull_request() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step13");
    let (a, git_base) = (root.join("a"), root.join("hosts"));
    repo(&a);
    let first = head(&a);
    write(&a, "c1.txt", "c1\n");
    let c1 = commit(&a, "c1");
    git(&a, &["switch", "-q", "-c", "topic"]);
    write(&a, "c2.txt", "c2\n");
    let c2 = commit(&a, "c2");
    git(&a, &["switch", "-q", "main"]);
    git(&a, &["merge", "-q", "--no-ff", "-m", "merge topic", "topic"]);
    let merge = head(&a);
    hosted(&a, &git_base, "github.com", "acme/widgets");
    git(&a, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);
    let (twin, _) = twins(&a);

    let server = Server::start().await;
    github_quiet(&server, "acme/widgets", serde_json::json!({ "id": 7, "default_branch": "main" }));
    let base = "/repos/acme/widgets";
    let at = "2026-11-01T11:00:00Z";
    server.on(&format!("{base}/pulls?state=all&sort=created&direction=asc&per_page=100"), vec![Answer::json(serde_json::json!([{ "number": 1, "id": 101, "updated_at": at }]))]);
    let octo = serde_json::json!({ "login": "octo", "id": 5, "type": "User" });
    server.on(
        &format!("{base}/pulls/1"),
        vec![Answer::json(serde_json::json!({
            "number": 1, "id": 101, "html_url": "https://github.com/acme/widgets/pull/1", "state": "closed", "merged": true,
            "merge_commit_sha": merge, "title": "topic", "user": octo, "merged_by": octo, "draft": false,
            "created_at": "2026-11-01T10:00:00Z", "updated_at": at, "merged_at": at, "closed_at": at,
            "head": { "ref": "topic", "sha": c2, "repo": { "full_name": "acme/widgets" } },
            "base": { "ref": "main", "sha": c1, "repo": { "full_name": "acme/widgets" } },
            "commits": 1, "additions": 1, "deletions": 0, "changed_files": 1, "comments": 0, "review_comments": 0
        }))],
    );
    server.on(&format!("{base}/pulls/1/commits?per_page=100"), vec![Answer::json(serde_json::json!([{ "sha": c2 }]))]);
    for list in ["pulls/1/files", "pulls/1/reviews", "issues/1/timeline"] {
        server.on(&format!("{base}/{list}?per_page=100"), vec![Answer::json(serde_json::json!([]))]);
    }

    let mut lines = Lines::new(&kernel, "s13").await;
    lines.worked_in(&a).await;
    let mut sessions = Vec::new();
    for key in ["printed", "ambiguous", "by-url", "by-number"] {
        sessions.push(session(&db, key).await);
    }
    let printed = git_event(&db, &sessions[0], "commit", Some(&a), Some(&c1[..7]), None, None).await;
    git_event(&db, &sessions[1], "commit", Some(&a), Some(&twin[..7]), None, None).await;
    let by_url = git_event(&db, &sessions[2], "pr_created", None, None, Some("https://github.com/acme/widgets/pull/1"), None).await;
    let by_number = git_event(&db, &sessions[3], "pr_merged", Some(&a), None, None, Some(1)).await;
    let p = params(&[(superx_mod_recon::params::UNREACHABLE_COMMITS_PARAM, number(0))]);
    let mut crawler = Crawler::new(places(&root.join("home"), endpoints(&server, &git_base), &["github.com"]));
    pass(&kernel, &db, &p, &mut crawler).await;

    let pr = rows(&db, "SELECT VALUE id FROM pull_request").await;
    let pr = match pr.as_slice() {
        [Value::RecordId(id)] => id.clone(),
        other => unreachable!("one pull request: {other:?}"),
    };
    let mut tied: Vec<Tied> = rows(&db, "SELECT commit.hash AS c, session, via, tool_call, pull_request FROM commit_session")
        .await
        .iter()
        .map(|r| (text(r, "c").expect("hash"), link(r, "session").expect("session"), text(r, "via").expect("via"), link(r, "tool_call"), link(r, "pull_request")))
        .collect();
    tied.sort_by(|x, y| (x.0.clone(), x.2.clone(), format!("{:?}", x.1)).cmp(&(y.0.clone(), y.2.clone(), format!("{:?}", y.1))));
    let mut wanted = vec![
        (c1.clone(), sessions[0].clone(), "printed".to_string(), Some(printed.clone()), None),
        (c2.clone(), sessions[2].clone(), "pull_request".to_string(), Some(by_url.clone()), Some(pr.clone())),
        (merge.clone(), sessions[2].clone(), "pull_request".to_string(), Some(by_url.clone()), Some(pr.clone())),
        (c2.clone(), sessions[3].clone(), "pull_request".to_string(), Some(by_number.clone()), Some(pr.clone())),
        (merge.clone(), sessions[3].clone(), "pull_request".to_string(), Some(by_number.clone()), Some(pr.clone())),
    ];
    wanted.sort_by(|x, y| (x.0.clone(), x.2.clone(), format!("{:?}", x.1)).cmp(&(y.0.clone(), y.2.clone(), format!("{:?}", y.1))));
    assert_eq!(tied, wanted, "the ambiguous prefix ties nothing");
    let event_at = rows_with(&db, "SELECT VALUE at FROM git_event WHERE tool_call = $c", "c", Value::RecordId(printed.clone())).await;
    let tie_at = rows_with(&db, "SELECT VALUE at FROM commit_session WHERE tool_call = $c", "c", Value::RecordId(printed)).await;
    assert_eq!(tie_at, event_at, "at the event's at");
    assert_ne!(first, c1);

    // Written once. A push sends the crawler back and prints no commit.
    lines.ran(&a, "git push origin topic", "To github.com:acme/widgets.git\n * [new branch]      topic -> topic\n").await;
    pass(&kernel, &db, &p, &mut crawler).await;
    assert_eq!(count(&db, "crawl").await, 2, "crawled again");
    assert_eq!(count(&db, "commit_session").await, 5);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crawl_row_holds_every_count_of_its_crawl_a_repository_whose_commits_read_differ_from_those_held_makes_its_crawl_and_the_repos_source_partial_naming_both_pass_source_counts_holds_commits_held_and_commits_read() {
    let kernel = kernel().await;
    let db = own_db().await;
    let root = scratch("step14");
    let (a, b) = (root.join("a"), root.join("b"));
    repo(&a);
    write(&a, "b.txt", "one\ntwo\n");
    let tip = commit(&a, "second");
    // b holds a commit git lists and the crawler cannot read: it names no
    // author.
    repo(&b);
    let object = format!("tree {}\nparent {}\ncommitter Test Person <test@example.test> 1700000000 +0000\n\nno author\n", git(&b, &["rev-parse", "HEAD^{tree}"]).trim(), head(&b));
    std::fs::write(root.join("odd.txt"), object).expect("object");
    let odd = git(&b, &["hash-object", "-t", "commit", "-w", "--literally", &root.join("odd.txt").to_string_lossy()]).trim().to_string();
    git(&b, &["update-ref", "refs/heads/odd", &odd]);
    let mut lines = Lines::new(&kernel, "s14").await;
    lines.worked_in(&a).await;
    lines.worked_in(&b).await;
    let ran = pass(&kernel, &db, &params(&[]), &mut Crawler::default()).await;

    let c = newest_crawl(&db, "a").await;
    let n = |name: &str| int(&c, name).unwrap_or(-1);
    let counts: BTreeMap<&str, i64> = [
        ("checkouts_read", 1),
        ("refs_read", 1),
        ("files_read", 2),
        ("reflog_read", 4),
        ("unreachable_read", 0),
        ("blame_lines_read", 3),
        ("commits_read", 2),
        ("commits_new", 2),
        ("commit_files_new", 2),
        ("lines_added_new", 3),
        ("lines_removed_new", 0),
        ("refs_new", 0),
        ("refs_moved", 0),
        ("refs_gone", 0),
        ("refs_rewritten", 0),
        ("head_moves_new", 2),
        ("placements_changed", 2),
        ("landed_new", 0),
        ("files_blamed", 2),
        ("files_left", 0),
        ("survival_changed", 2),
        ("survival_delta", 3),
        ("git_failed", 0),
        ("git_timeouts", 0),
    ]
    .into();
    for (name, wanted) in &counts {
        assert_eq!(n(name), *wanted, "{name}");
    }
    let calls = rows_with(&db, "SELECT bytes_out, ms FROM git_call WHERE crawl.repository.name = $n", "n", Value::String("a".to_string())).await;
    assert_eq!(n("git_calls"), i64::try_from(calls.len()).expect("a count"));
    assert_eq!(n("bytes_read"), calls.iter().map(|g| int(g, "bytes_out").expect("bytes")).sum::<i64>());
    assert!(n("git_ms") >= 0);
    let mut written = 0;
    for (table, path) in [("commit", "repository.name"), ("commit_file", "commit.repository.name"), ("git_call", "crawl.repository.name"), ("ref_move", "repository.name"), ("placement", "commit.repository.name"), ("survival", "commit.repository.name")] {
        written += i64::try_from(rows(&db, &format!("SELECT VALUE id FROM {table} WHERE {path} = 'a'")).await.len()).expect("a count");
    }
    assert_eq!(n("rows_written"), written + 1, "its rows and itself");
    assert_eq!((text(&c, "outcome").as_deref(), text(&c, "tip"), text(&c, "mainline").as_deref(), text(&c, "mainline_from").as_deref()), (Some("complete"), Some(tip), Some("refs/heads/main"), Some("local_branch")));
    assert!(time(&c, "started_at") <= time(&c, "finished_at"));
    assert_eq!(text(&c, "since"), None, "all of history");

    let odd_crawl = newest_crawl(&db, "b").await;
    assert_eq!(text(&odd_crawl, "outcome").as_deref(), Some("partial"));
    assert!(text(&odd_crawl, "note").is_some_and(|n| n.contains("1 commits read of 2 held")), "{odd_crawl:?}");
    let source = ran.source("repos").expect("the repos source");
    assert_eq!(source.outcome, superx_mod_recon::crawler::Outcome::Partial);
    assert!(source.note.as_deref().is_some_and(|n| n.contains("b: 1 commits read of 2 held")), "{:?}", source.note);
    assert_eq!((source.counts.get("commits_held"), source.counts.get("commits_read")), (Some(&4), Some(&3)));
    let stored = rows(&db, "SELECT counts FROM pass_source WHERE source = 'repos'").await;
    let Some(Value::Object(stored)) = stored.first().and_then(|s| if let Value::Object(o) = s { o.get("counts").cloned() } else { None }) else {
        unreachable!("its counts")
    };
    assert_eq!((stored.get("commits_held"), stored.get("commits_read")), (Some(&number(4)), Some(&number(3))));
    std::fs::remove_dir_all(&root).expect("clean up");
}
