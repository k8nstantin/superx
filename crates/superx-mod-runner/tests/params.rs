//! The runner's own settings (issue #284).
//!
//! A module that owns its database, its schema and its directory should
//! own its knobs too. These pin the properties that make that safe: an
//! unreadable file does not stop the runner, an interrupted write does
//! not destroy what was there, and "unset" stays distinguishable from
//! "set to the default".

use superx_mod_runner::params::{read_at, write_at, Settings};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-runner-params-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn a_missing_file_is_defaults_not_an_error() {
    let path = temp_dir("missing").join("params.json");
    assert_eq!(read_at(&path), Settings::default());
    assert_eq!(read_at(&path).agent_cmd, None, "nothing spawns an unconfigured agent");
}

/// Every knob is optional so the file can say "the operator has not
/// chosen" — which is different from "the operator chose the default",
/// and the difference decides whether a kernel value gets adopted.
#[test]
fn unset_is_distinguishable_from_set_to_the_default() {
    let path = temp_dir("unset").join("params.json");
    let settings = Settings { max_parallel: Some(2), ..Default::default() };
    write_at(&path, &settings).expect("write");

    let read = read_at(&path);
    assert_eq!(read.max_parallel, Some(2), "chosen, and it happens to equal the default");
    assert_eq!(read.tick_secs, None, "not chosen");

    // An unset knob is absent from the file rather than written as null,
    // so the file reads as what the operator actually decided.
    let raw = std::fs::read_to_string(&path).expect("read");
    assert!(raw.contains("max_parallel"));
    assert!(!raw.contains("tick_secs"), "an unchosen knob is not in the file: {raw}");
}

#[test]
fn a_setting_survives_a_round_trip() {
    let path = temp_dir("round").join("params.json");
    let settings = Settings {
        agent_cmd: Some("claude -p".to_string()),
        max_parallel: Some(4),
        tick_secs: Some(30),
        plan_depth: Some(50),
    };
    write_at(&path, &settings).expect("write");
    assert_eq!(read_at(&path), settings);
}

/// A corrupt settings file must not stop the runner from starting, and
/// must not be silently overwritten either — the operator's file is
/// still theirs, and a warning is the honest response.
#[test]
fn an_unreadable_file_yields_defaults_and_is_left_alone() {
    let path = temp_dir("corrupt").join("params.json");
    std::fs::write(&path, "{ this is not json").expect("write");

    assert_eq!(read_at(&path), Settings::default());
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "{ this is not json",
        "the file the operator has is not destroyed by being unreadable"
    );
}

/// Written to a neighbour and renamed, so an interrupted write leaves
/// the previous settings intact rather than a half-file the next read
/// would reject.
#[test]
fn a_write_does_not_leave_a_temporary_behind() {
    let dir = temp_dir("atomic");
    let path = dir.join("params.json");
    write_at(&path, &Settings { tick_secs: Some(7), ..Default::default() }).expect("write");

    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .filter_map(std::result::Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "params.json")
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    assert_eq!(read_at(&path).tick_secs, Some(7));
}

/// Changing one knob must not clear the others — the settings file is
/// amended, not replaced with whatever the last command mentioned.
#[test]
fn changing_one_setting_leaves_the_rest_alone() {
    let path = temp_dir("amend").join("params.json");
    write_at(
        &path,
        &Settings {
            agent_cmd: Some("claude -p".to_string()),
            plan_depth: Some(40),
            ..Default::default()
        },
    )
    .expect("write");

    let mut settings = read_at(&path);
    settings.tick_secs = Some(15);
    write_at(&path, &settings).expect("write");

    let read = read_at(&path);
    assert_eq!(read.agent_cmd.as_deref(), Some("claude -p"), "untouched");
    assert_eq!(read.plan_depth, Some(40), "untouched");
    assert_eq!(read.tick_secs, Some(15));
}
