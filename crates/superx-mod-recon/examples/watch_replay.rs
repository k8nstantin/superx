//! The watcher replayed over transcripts on disk (#573): every line of every
//! file given, in the order they were written, fed to the watcher as a look
//! feeds what it reads; then its alerts, runs and measures as JSON. The same
//! code as the watcher's, with the files in place of the kernel's database.
//!
//! cargo run -p superx-mod-recon --example watch_replay -- <out.json> <transcript.jsonl>...

use std::collections::BTreeMap;
use std::io::{Read as _, Seek as _, SeekFrom};

use chrono::{DateTime, Utc};
use superx_mod_recon::params::from_values;
use superx_mod_recon::watcher::{measure, routes, Watcher};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("an output path")?;
    let files: Vec<String> = args.collect();
    let p = from_values(&BTreeMap::new()).params;
    // Every line's time, file and place, then all of them in time order.
    let mut index: Vec<(DateTime<Utc>, usize, u64, usize)> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let bytes = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
        let mut at = 0usize;
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) {
                if let Some(t) = v.get("timestamp").and_then(|t| t.as_str()).and_then(|t| DateTime::parse_from_rfc3339(t).ok()) {
                    index.push((t.with_timezone(&Utc), i, u64::try_from(at).unwrap_or(0), line.len()));
                }
            }
            at += line.len();
        }
    }
    index.sort();
    let (Some(first), Some(last)) = (index.first().map(|x| x.0), index.last().map(|x| x.0)) else { return Err("no line".to_string()) };
    let mut handles: Vec<std::fs::File> = files.iter().map(std::fs::File::open).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let mut w = Watcher::new();
    let mut buf = Vec::new();
    for (t, i, at, n) in &index {
        let h = &mut handles[*i];
        h.seek(SeekFrom::Start(*at)).map_err(|e| e.to_string())?;
        buf.resize(*n, 0);
        h.read_exact(&mut buf).map_err(|e| e.to_string())?;
        let Ok(line) = serde_json::from_slice::<serde_json::Value>(&buf) else { continue };
        let key = std::path::Path::new(&files[*i]).file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        w.feed(&key, &format!("claude_code/{key}"), &key, *t, &line, &p);
    }
    w.settle(last, first, &p);
    let s = w.snapshot(&p, false);
    let zone = chrono::FixedOffset::west_opt(4 * 3600).ok_or("zone")?;
    let (alerts, runs) = (s.alerts.len(), s.runs.len());
    // What GET /api/watch answers, in the operator's zone (AST).
    let view = routes::View { models: measure::models(&s), days: measure::days(&s, zone), snapshot: s };
    std::fs::write(&out, serde_json::to_string_pretty(&view).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    println!("{} lines of {} files: {alerts} alerts, {runs} runs listed", index.len(), files.len());
    Ok(())
}
