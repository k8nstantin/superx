//! End-to-end test for the kernel self-log.
//!
//! One test only: `logging::init` installs the GLOBAL tracing
//! subscriber, which a process can do exactly once — so directory
//! creation, write-through, and flush-on-drop are all asserted in a
//! single flow.

use std::error::Error;
use std::fs;

use superx_kernel::logging;

#[test]
fn self_log_creates_dir_writes_and_flushes() -> Result<(), Box<dyn Error>> {
    let tmp = tempfile::tempdir()?;
    // A nested, not-yet-existing path proves create_dir_all behavior.
    let log_dir = tmp.path().join("nested").join("logs");

    let guard = logging::init(&log_dir)?;
    tracing::info!("selflog_marker_line");

    // Dropping the guard flushes buffered lines and stops the writer.
    drop(guard);

    let mut entries = fs::read_dir(&log_dir)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 1, "exactly one rolling log file");
    let file = entries.pop().expect("one entry").path();
    let name = file.file_name().expect("file name").to_string_lossy().to_string();
    assert!(
        name.starts_with("superx.log"),
        "rolling file carries the superx.log prefix, got: {name}",
    );

    let content = fs::read_to_string(&file)?;
    assert!(
        content.contains("selflog_marker_line"),
        "flushed content contains the emitted line",
    );

    // A second init in the same process must refuse loudly, never
    // silently stack a second subscriber.
    let err = logging::init(&log_dir).expect_err("second init must fail");
    assert!(err.to_string().contains("configuration error"));
    Ok(())
}
