//! superx-recon as git's askpass (#538, step 23): the binary run as git runs
//! it for a mirror's fetch, the prompt its one argument.

mod common;

use std::path::Path;
use std::process::Command;

use common::*;
use superx_mod_recon::crawler::hosts::{Endpoints, ASKPASS_ENV, HOME_ENV};

/// What the askpass printed, what it said and how it ended.
struct Answered {
    out: String,
    said: String,
    ok: bool,
}

/// superx-recon run as git runs its askpass: the prompt its one argument,
/// the host and the home in its environment and nothing else.
fn ask(home: &Path, host: &str, prompt: &str) -> Answered {
    let home = home.to_string_lossy().into_owned();
    let env = [(ASKPASS_ENV, host), (HOME_ENV, home.as_str())];
    assert!(!prompt.contains(TOKEN) && !env.iter().any(|(k, v)| k.contains(TOKEN) || v.contains(TOKEN)), "the token is not in its arguments or environment");
    let out = Command::new(env!("CARGO_BIN_EXE_superx-recon")).env_clear().envs(env).arg(prompt).output().expect("superx-recon runs");
    Answered { out: String::from_utf8_lossy(&out.stdout).into_owned(), said: String::from_utf8_lossy(&out.stderr).into_owned(), ok: out.status.success() }
}

#[test]
fn superx_recon_as_askpass_prints_the_token_for_the_password_prompt_of_its_host_over_https_and_nothing_exit_1_for_another_host_a_username_prompt_a_plain_http_prompt_or_no_token_the_token_never_in_its_arguments_environment_or_error_output() {
    let root = scratch("step23");
    let (home, without) = (root.join("home"), root.join("without"));
    places(&home, Endpoints::default(), &["github.com", "bitbucket.org"]);
    places(&without, Endpoints::default(), &["bitbucket.org"]);
    let before: Vec<_> = std::fs::read_dir(home.join("modules/recon")).expect("its folder").map(|e| e.expect("an entry").file_name()).collect();

    for (host, prompt) in [
        ("github.com", "Password for 'https://x-access-token@github.com': "),
        ("github.com", "Password for 'https://github.com': "),
        ("bitbucket.org", "Password for 'https://x-bitbucket-api-token-auth@bitbucket.org': "),
    ] {
        let a = ask(&home, host, prompt);
        assert!(a.ok, "{prompt}: {}", a.said);
        assert_eq!(a.out, format!("{TOKEN}\n"), "the token and a newline");
        assert_eq!(a.said, "", "{prompt}");
    }
    for (home, host, prompt, why) in [
        (&home, "github.com", "Password for 'https://bitbucket.org': ", "another host"),
        (&home, "gitlab.com", "Password for 'https://gitlab.com': ", "a host the crawler does not read"),
        (&home, "github.com", "Password for 'https://github.com.evil.test': ", "another host by its name"),
        (&home, "github.com", "Username for 'https://github.com': ", "a username prompt"),
        (&home, "github.com", "Password for 'http://github.com': ", "plain http"),
        (&without, "github.com", "Password for 'https://github.com': ", "no token"),
    ] {
        let a = ask(home, host, prompt);
        assert!(!a.ok, "{why}: exit 1");
        assert_eq!(a.out, "", "{why}: nothing printed");
        assert!(a.said.starts_with("superx-recon: as git's askpass: "), "{why}: says why: {}", a.said);
        assert!(!a.said.contains(TOKEN), "{why}");
    }
    let after: Vec<_> = std::fs::read_dir(home.join("modules/recon")).expect("its folder").map(|e| e.expect("an entry").file_name()).collect();
    assert_eq!(after, before, "it started nothing and logged nothing");
    std::fs::remove_dir_all(&root).expect("clean up");
}
