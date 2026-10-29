//! What the module folder holds beside the pid file and the logs (#536):
//! the repos folder, where the crawler keeps its own mirror clones and
//! nothing else writes, and the credentials file, which the operator fills
//! with the hosts' read-only tokens. `run` makes each once the module is
//! registered as starting, before it says it is active, and only when it is
//! absent: a repos folder that is there keeps its mode, and a credentials
//! file that is there is never written again, whatever it holds.

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};

use serde::Serialize;

use crate::instance::Instance;

/// The repos folder's mode when `run` makes it: its owner's alone.
pub const REPOS_MODE: u32 = 0o700; // skill-allow: §9-const — the mode the spec gives the repos folder, not a policy tunable

/// The credentials file's mode when `run` writes it: its owner's alone.
pub const CREDENTIALS_MODE: u32 = 0o600; // skill-allow: §9-const — the mode the spec gives the credentials file, not a policy tunable

/// What the template says, for the operator who opens the file.
pub const NOTE: &str = "Read-only tokens, one per host. github.com: a fine-grained personal access token with read access \
                        to metadata, contents, pull requests, issues, actions, commit statuses, deployments and \
                        administration. bitbucket.org: an API token with the read scopes. Keep this file mode 600. \
                        superx-recon reads it, never writes it once it exists, and never shows a token.";

/// The credentials file `run` writes when there is none: the note, and an
/// empty token for each host.
#[derive(Serialize)]
struct Template {
    #[serde(rename = "_doc")]
    doc: &'static str,
    #[serde(rename = "github.com")]
    github: Entry,
    #[serde(rename = "bitbucket.org")]
    bitbucket: Entry,
}

#[derive(Serialize)]
struct Entry {
    token: &'static str,
}

/// The template, as `run` writes it: pretty JSON and a newline after it.
///
/// # Errors
///
/// serde_json's reason, should it refuse the template.
pub fn template() -> Result<String, String> {
    let template = Template { doc: NOTE, github: Entry { token: "" }, bitbucket: Entry { token: "" } };
    let mut text = serde_json::to_string_pretty(&template).map_err(|e| format!("the credentials template: {e}"))?;
    text.push('\n');
    Ok(text)
}

/// Make the repos folder and the credentials file where they are absent,
/// and leave each that is present as it is.
///
/// # Errors
///
/// The path that could not be made or written, and why.
pub fn prepare(instance: &Instance) -> Result<(), String> {
    let folder = instance.module_dir();
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;

    let repos = instance.repos_dir();
    match DirBuilder::new().mode(REPOS_MODE).create(&repos) {
        // Made now: its mode as given, whatever the umask took from it.
        Ok(()) => std::fs::set_permissions(&repos, Permissions::from_mode(REPOS_MODE))
            .map_err(|e| format!("{}: {e}", repos.display()))?,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", repos.display())),
    }

    let path = instance.credentials_path();
    match OpenOptions::new().write(true).create_new(true).mode(CREDENTIALS_MODE).open(&path) {
        Ok(mut file) => {
            file.set_permissions(Permissions::from_mode(CREDENTIALS_MODE))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            file.write_all(template()?.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn scratch() -> Instance {
        let home = std::env::temp_dir().join(format!("superx-recon-folder-{}", superx_kernel::types::Uuid::new_v7()));
        std::fs::create_dir_all(&home).expect("home");
        Instance::read(&home).expect("instance")
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn run_makes_the_repos_folder_700_when_absent_and_leaves_one_that_is_present_as_it_is() {
        let instance = scratch();
        prepare(&instance).expect("prepare");
        let repos = instance.repos_dir();
        assert!(repos.is_dir());
        assert_eq!(mode(&repos), 0o700);

        // Present, with a mode of its own and a mirror in it: left as it is.
        std::fs::set_permissions(&repos, Permissions::from_mode(0o750)).expect("chmod");
        std::fs::create_dir_all(repos.join("github.com")).expect("a mirror's folder");
        prepare(&instance).expect("prepare again");
        assert_eq!(mode(&repos), 0o750, "its mode is left as it is");
        assert!(repos.join("github.com").is_dir(), "what it holds is left as it is");
        std::fs::remove_dir_all(&instance.home).expect("clean up");
    }

    #[test]
    fn run_writes_credentials_json_600_with_the_template_when_absent_and_never_when_present() {
        let instance = scratch();
        prepare(&instance).expect("prepare");
        let path = instance.credentials_path();
        assert_eq!(mode(&path), 0o600);
        let text = std::fs::read_to_string(&path).expect("the file");
        assert_eq!(text, template().expect("template"));
        assert!(text.ends_with("}\n"), "a newline after the object");
        let written: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(
            written,
            serde_json::json!({ "_doc": NOTE, "github.com": { "token": "" }, "bitbucket.org": { "token": "" } })
        );
        let keys: Vec<&str> = text.lines().filter_map(|l| l.trim_start().strip_prefix('"')?.split('"').next()).collect();
        assert_eq!(keys, ["_doc", "github.com", "token", "bitbucket.org", "token"], "in the template's order");

        // Present, however it differs from the template: never written.
        for (held, held_mode) in [("{}", 0o600), ("not even JSON", 0o644)] {
            std::fs::write(&path, held).expect("the operator's file");
            std::fs::set_permissions(&path, Permissions::from_mode(held_mode)).expect("chmod");
            prepare(&instance).expect("prepare again");
            assert_eq!(std::fs::read_to_string(&path).expect("the file"), held, "never written once it exists");
            assert_eq!(mode(&path), held_mode, "its mode is left as it is");
        }
        std::fs::remove_dir_all(&instance.home).expect("clean up");
    }
}
