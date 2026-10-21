//! Is capture keeping up (#452)? The rule, over polls, and the
//! transcripts' times.

use chrono::{DateTime, Duration, TimeZone, Utc};
use superx_mod_ui::health::{judge, newest_change, Facts};

fn t(h: u32, m: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 18, h, m, s).single().expect("time")
}

fn facts(written: Option<DateTime<Utc>>, changed: Option<DateTime<Utc>>) -> Facts {
    Facts { last_written: written, newest: changed.map(|c| (c, "claude_code/44658088".to_string())) }
}

#[test]
fn a_quiet_spell_then_a_late_line_is_no_stall() {
    // 2026-10-18: the last record 11:44:55, quiet, the operator's line at
    // 11:49:58, taken in at 11:51:19 behind a hung connection. Polled at
    // 11:50:11 (where the first rule cried stall) and after.
    let (v, since) = judge(&facts(Some(t(11, 44, 55)), Some(t(11, 49, 58))), None, t(11, 50, 11), 300);
    assert!(!v.stalled, "a line waiting 13 s is not a stall: {v:?}");
    assert_eq!(since, Some(t(11, 49, 58)));
    let (v, since) = judge(&facts(Some(t(11, 49, 58)), Some(t(11, 49, 58))), since, t(11, 51, 20), 300);
    assert!(!v.stalled);
    assert_eq!(since, None, "taken in: nothing is waiting");
    assert_eq!(v.waiting_since, None);
}

#[test]
fn a_dead_capture_under_busy_work_is_a_stall_once_the_wait_passes_the_limit() {
    // 2026-10-13: the last record 10:58; the sessions kept writing.
    let written = Some(Utc.with_ymd_and_hms(2026, 10, 13, 10, 58, 0).single().expect("t"));
    let at = |m: u32| Utc.with_ymd_and_hms(2026, 10, 13, 11, m, 0).single().expect("t");
    let mut since = None;
    // The first poll sees a change just made: it starts the clock there.
    let (v, s) = judge(&facts(written, Some(at(0))), since, at(0) + Duration::seconds(5), 300);
    assert!(!v.stalled);
    since = s;
    // Polls go on; the newest change keeps moving; the wait does not reset.
    let (v, s) = judge(&facts(written, Some(at(3))), since, at(3), 300);
    assert!(!v.stalled, "3 minutes waiting");
    since = s;
    let (v, s) = judge(&facts(written, Some(at(6))), since, at(6), 300);
    assert!(v.stalled, "6 minutes waiting: {v:?}");
    assert_eq!(s, Some(at(0)), "waiting since the first change seen untaken");
    assert_eq!(v.waiting_since.as_deref(), Some(at(0).to_rfc3339().as_str()));
}

#[test]
fn a_remembered_wait_ends_once_anything_is_recorded_after_it() {
    // 2026-10-20 18:32:03: a poll saw a change a second before capture took
    // it, and nothing polled again until 2026-10-21 10:00:30, with capture
    // ten seconds behind the operator's session. Kept, 18:32:03 read as
    // fifteen hours stalled (#464).
    let yesterday = Utc.with_ymd_and_hms(2026, 10, 20, 18, 32, 3).single().expect("t");
    let at = |m: u32, s: u32| Utc.with_ymd_and_hms(2026, 10, 21, 10, m, s).single().expect("t");
    let (v, since) = judge(&facts(Some(at(0, 10)), Some(at(0, 20))), Some(yesterday), at(0, 30), 300);
    assert!(!v.stalled, "ten seconds behind is not fifteen hours stalled: {v:?}");
    assert_eq!(since, Some(at(0, 20)), "it waits since the change waiting now");
}

#[test]
fn busy_work_with_capture_a_few_seconds_behind_is_never_a_stall() {
    // Ten minutes of polls, each landing while capture is six seconds
    // behind a session that keeps writing.
    let mut since = None;
    for k in 0..60 {
        let now = t(12, 0, 0) + Duration::seconds(10 * k);
        let (v, s) = judge(&facts(Some(now - Duration::seconds(8)), Some(now - Duration::seconds(2))), since, now, 300);
        assert!(!v.stalled, "poll {k}: {v:?}");
        since = s;
    }
}

#[test]
fn nothing_ever_recorded_or_nothing_to_compare() {
    let (v, _) = judge(&facts(None, Some(t(10, 0, 0))), None, t(10, 6, 0), 300);
    assert!(v.stalled, "transcripts, and nothing ever recorded");
    let (v, since) = judge(&facts(Some(t(10, 0, 0)), None), Some(t(9, 0, 0)), t(10, 6, 0), 300);
    assert!(!v.stalled && since.is_none(), "no transcript: nothing waits");
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
