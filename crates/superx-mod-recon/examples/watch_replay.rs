//! The watcher replayed over transcripts on disk (#573): every line of every
//! file given, in the order they were written, fed to the watcher as a look
//! feeds what it reads; then the watcher's own routes and the tab's page
//! served over what it found, on 127.0.0.1 at the port given. The same code
//! as the watcher's, with the files in place of the kernel's database.
//!
//! cargo run -p superx-mod-recon --example watch_replay -- <port> <transcript.jsonl>...

use std::collections::BTreeMap;
use std::io::{Read as _, Seek as _, SeekFrom};

use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use superx_mod_recon::params::from_values;
use superx_mod_recon::watcher::{routes, Watched, Watcher};

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().ok_or("a port")?.parse().map_err(|e| format!("the port: {e}"))?;
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
    let s = w.snapshot();
    let found = w.found();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for a in &s.alerts {
        *kinds.entry(format!("alert {:?}", a.kind)).or_default() += 1;
    }
    for f in &found {
        *kinds.entry(format!("found {:?}", f.kind)).or_default() += 1;
    }
    println!("{} lines of {} files, {first} to {last}: {} tasks", index.len(), files.len(), s.tasks.len());
    for (k, n) in &kinds {
        println!("  {n:6} {k}");
    }
    let app = Router::new()
        .merge(routes::router())
        .route("/api/ping", get(|| async { Json(serde_json::json!({ "module": "recon", "version": "replay", "core_url": null })) }))
        .fallback(get(superx_mod_recon::ui::asset))
        .with_state(Watched::new(w));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| e.to_string())?;
    println!("serving on http://127.0.0.1:{port}/#watcher");
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}
