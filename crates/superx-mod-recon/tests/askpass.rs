//! superx-recon as git's askpass (#538, step 23): the executable this
//! package builds, run as git runs it, with a credentials file of the
//! test's own and a token made up here.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The executable this package builds.
const EXE: &str = env!("CARGO_BIN_EXE_superx-recon");

/// A token of the test's own, never a real one.
const TOKEN: &str = "recon-askpass-token-4c7e19";

/// A home whose module folder holds a credentials file giving github.com
/// the token, and bitbucket.org none.
fn home() -> PathBuf {
    let home = std::env::temp_dir().join(format!("superx-recon-askpass-{}", superx_kernel::types::Uuid::new_v7()));
    let module = home.join("modules").join("recon");
    std::fs::create_dir_all(&module).expect("module folder");
    let path = module.join("credentials.json");
    std::fs::write(&path, format!(r#"{{"github.com": {{"token": "{TOKEN}"}}, "bitbucket.org": {{"token": ""}}}}"#)).expect("credentials");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    home
}

/// Run it as git's askpass for `host` with `prompt`: its exit, standard
/// output and error output.
fn ask(home: &Path, host: &str, prompt: &str) -> (Option<i32>, String, String) {
    let out = Command::new(EXE)
        .arg(prompt)
        .env_clear()
        .env("SUPERX_RECON_ASKPASS", host)
        .env("SUPERX_HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .output()
        .expect("it runs");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn superx_recon_as_askpass_prints_the_token_for_the_password_prompt_of_its_host_over_https_and_nothing_exit_1_for_another_host_a_username_prompt_a_plain_http_prompt_or_no_token_the_token_never_in_its_arguments_environment_or_error_output()
{
    let home = home();
    for prompt in ["Password for 'https://x-access-token@github.com': ", "Password for 'https://github.com': "] {
        assert_eq!(ask(&home, "github.com", prompt), (Some(0), format!("{TOKEN}\n"), String::new()), "{prompt}");
    }
    for (host, prompt, why) in [
        ("github.com", "Password for 'https://x-access-token@gitlab.example': ", "another host"),
        ("github.com", "Password for 'https://x-bitbucket-api-token-auth@bitbucket.org': ", "a host the askpass was not named for"),
        ("gitlab.example", "Password for 'https://x@gitlab.example': ", "a host the crawler does not read"),
        ("github.com", "Username for 'https://github.com': ", "a username prompt"),
        ("github.com", "Password for 'http://x-access-token@github.com': ", "a plain http prompt"),
        ("bitbucket.org", "Password for 'https://x-bitbucket-api-token-auth@bitbucket.org': ", "no token"),
    ] {
        let (exit, out, err) = ask(&home, host, prompt);
        assert_eq!((exit, out.as_str()), (Some(1), ""), "{why}");
        assert!(err.starts_with("superx-recon askpass: ") && !err.contains(TOKEN), "{why}: {err}");
    }
    // Asked for nothing it writes nothing: no log, no other file.
    let made: Vec<PathBuf> = std::fs::read_dir(home.join("modules").join("recon")).expect("folder").flatten().map(|e| e.path()).collect();
    assert_eq!(made, [home.join("modules").join("recon").join("credentials.json")], "it starts nothing and logs nothing");
    std::fs::remove_dir_all(&home).expect("clean up");
}
