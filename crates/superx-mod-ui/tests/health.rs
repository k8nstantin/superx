//! Is capture keeping up (#452)? The rule, and the transcripts' times.

use chrono::{Duration, TimeZone, Utc};
use superx_mod_ui::health::{judge, newest_change};

fn at(minute: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 13, 10, minute, 0).single().expect("time")
}

#[test]
fn transcripts_that_kept_changing_past_the_last_record_mean_capture_stalled() {
    // 2026-10-13: the last record at 10:58, transcripts still written at 11:30.
    let v = judge(Some(at(58) - Duration::minutes(0)), Some((at(58) + Duration::minutes(32), "claude_code/b6338687".into())), 300);
    assert!(v.stalled);
    assert_eq!(v.transcript.as_deref(), Some("claude_code/b6338687"));
    assert_eq!(v.stall_secs, 300);

    // Capture a few seconds behind a live transcript is keeping up.
    let v = judge(Some(at(30)), Some((at(30) + Duration::seconds(4), "claude_code/x".into())), 300);
    assert!(!v.stalled, "{v:?}");

    // Nobody working: the transcripts stopped before the last record.
    let v = judge(Some(at(30)), Some((at(10), "claude_code/x".into())), 300);
    assert!(!v.stalled);

    // Transcripts, and nothing ever recorded.
    assert!(judge(None, Some((at(10), "claude_code/x".into())), 300).stalled);
    // No transcript to compare: nothing to say.
    assert!(!judge(Some(at(10)), None, 300).stalled);
    assert!(!judge(None, None, 300).stalled);
}

#[test]
fn the_newest_change_is_read_from_the_files_themselves() {
    let dir = std::env::temp_dir().join(format!("superx-health-{}-{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap_or(0)));
    std::fs::create_dir_all(&dir).expect("dir");
    let older = dir.join("a.jsonl");
    let newer = dir.join("b.jsonl");
    std::fs::write(&older, "{}\n").expect("a");
    std::fs::write(&newer, "{}\n").expect("b");
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&older).expect("open").set_modified(then).expect("mtime");
    let files = vec![
        ("claude_code/a".to_string(), older.clone()),
        ("claude_code/b".to_string(), newer.clone()),
        ("claude_code/gone".to_string(), dir.join("gone.jsonl")),
        ("claude_code/dir".to_string(), dir.clone()),
    ];
    let (when, who) = newest_change(&files).expect("a change");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(who, "claude_code/b", "a missing file and a directory say nothing");
    assert!(Utc::now() - when < Duration::minutes(1));
}
