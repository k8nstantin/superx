//! Params-file + precedence tests (issue #125).

use std::path::PathBuf;

use superx::config::{load_params, params_path, resolve, save_params, Params};

#[test]
fn params_roundtrip_via_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();

    // Absent file → defaults object.
    let empty = load_params(&home).expect("load absent");
    assert!(empty.endpoint.is_none());

    // Resolve with nothing → fallbacks; save writes the file.
    let config = resolve(home.clone(), &empty, None, None, None, None, None, None);
    assert_eq!(config.endpoint, "ws://127.0.0.1:8000");
    assert_eq!(config.data_dir, home.join("db/superx-v2.db"));
    let path = save_params(&config).expect("save");
    assert_eq!(path, params_path(&home));

    // Reload: values persisted.
    let loaded = load_params(&home).expect("load");
    assert_eq!(loaded.endpoint.as_deref(), Some("ws://127.0.0.1:8000"));
    assert_eq!(loaded.data_dir.as_deref(), Some("db/superx-v2.db"));
    assert_eq!(loaded.log_filter.as_deref(), Some("info"));
}

#[test]
fn precedence_flag_beats_file_beats_fallback() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();
    let file = Params {
        endpoint: Some("ws://filehost:1111".into()),
        namespace: Some("filens".into()),
        database: None, // falls through to fallback
        data_dir: Some("custom/data.db".into()),
        log_dir: None,
        log_filter: Some("debug".into()),
    };

    let config = resolve(
        home.clone(),
        &file,
        Some("ws://flaghost:2222".into()), // flag wins over file
        None,                              // file wins over fallback
        None,                              // fallback
        None,
        None,
        None,
    );
    assert_eq!(config.endpoint, "ws://flaghost:2222");
    assert_eq!(config.namespace, "filens");
    assert_eq!(config.database, "kernel");
    assert_eq!(config.data_dir, home.join("custom/data.db"));
    assert_eq!(config.log_filter, "debug");
}

#[test]
fn absolute_paths_in_params_stay_absolute() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = Params {
        data_dir: Some("/abs/data.db".into()),
        ..Params::default()
    };
    let config = resolve(tmp.path().to_path_buf(), &file, None, None, None, None, None, None);
    assert_eq!(config.data_dir, PathBuf::from("/abs/data.db"));
}

#[test]
fn malformed_params_file_is_a_loud_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();
    std::fs::create_dir_all(home.join("params")).unwrap();
    std::fs::write(params_path(&home), "{ not json").unwrap();
    let err = load_params(&home).expect_err("malformed must error");
    assert!(err.contains("not valid JSON"));
}
