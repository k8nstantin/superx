//! Pure-helper tests for `superx --initialize` (issue #120). The full
//! provisioning flow needs a real server + TTY and is QA'd live per
//! the README protocol.

use superx::initialize::{bind_from_endpoint, credentials_path, resolve_password, save_credentials};
use superx_kernel::provision::escape_surql;

#[test]
fn credentials_file_sits_beside_the_datastore() {
    let p = credentials_path(std::path::Path::new("./db/superx-v2.db"));
    assert_eq!(p, std::path::PathBuf::from("./db/superx-credentials"));
}

#[test]
fn credentials_roundtrip_via_file() {
    // The env var would win over the file — make sure this test sees
    // only the file.
    std::env::remove_var("SUPERX_KERNEL_PASSWORD");
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("db").join("instance.db");

    assert!(resolve_password(&data_dir).is_none(), "nothing yet");
    let saved = save_credentials(&data_dir, "any password will do").expect("save");
    assert!(saved.ends_with("superx-credentials"));
    assert_eq!(
        resolve_password(&data_dir).as_deref(),
        Some("any password will do")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&saved).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "credentials are owner-only");
    }
}

#[test]
fn bind_derivation_from_endpoint() {
    assert_eq!(bind_from_endpoint("ws://127.0.0.1:8000"), "127.0.0.1:8000");
    assert_eq!(bind_from_endpoint("http://0.0.0.0:9999/"), "0.0.0.0:9999");
    assert_eq!(bind_from_endpoint("wss://host:1"), "host:1");
}

#[test]
fn surql_password_escaping() {
    assert_eq!(escape_surql("plain"), "plain");
    assert_eq!(escape_surql("it's"), "it\\'s");
    assert_eq!(escape_surql(r"back\slash"), r"back\\slash");
}

#[test]
fn pidfile_sits_beside_the_datastore() {
    let p = superx::initialize::pid_path(std::path::Path::new("./db/superx-v2.db"));
    assert_eq!(p, std::path::PathBuf::from("./db/superx.pid"));
}

#[test]
fn live_pid_roundtrip_and_stale_cleanup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("db").join("instance.db");

    // No pidfile yet.
    assert!(superx::initialize::read_live_pid(&data_dir).is_none());

    // Our own (alive) pid roundtrips.
    let me = std::process::id();
    superx::initialize::write_pidfile(&data_dir, me).expect("write");
    assert_eq!(superx::initialize::read_live_pid(&data_dir), Some(me));

    // A dead pid is stale: read yields None AND the file is cleaned up.
    let dead = std::process::Command::new("true")
        .spawn()
        .and_then(|mut c| {
            let pid = c.id();
            c.wait().map(|_| pid)
        })
        .expect("spawn/wait true");
    superx::initialize::write_pidfile(&data_dir, dead).expect("write");
    assert!(superx::initialize::read_live_pid(&data_dir).is_none());
    assert!(
        !superx::initialize::pid_path(&data_dir).exists(),
        "stale pidfile removed"
    );
}
