//! The repositories crawled in depth (#538, steps 1 to 14), end to end: a
//! kernel holding sessions' lines as capture keeps them, repositories made
//! with git in a scratch folder, and the module's own database, on the
//! in-memory engine. What a pass writes is read back from the database.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use common::*;
use superx_kernel::types::Value;
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
