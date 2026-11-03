//! What survives (#538, step 11): when the main line's tip is not the one
//! last blamed whole, every file of the tip's tree blamed at the tip, but
//! binaries, skipped paths and the files past the budget; each commit's
//! alive is the lines blame credits it, never more than it added. A blame
//! that fails writes no survival, and the next crawl blames the tip again.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::crawler::hosts::held::row_key;
use crate::crawler::repos::crawl::Crawl;
use crate::crawler::repos::git::{self, Call, With};
use crate::crawler::repos::record::OK;
use crate::crawler::store::Row;
use crate::crawler::Stop;

/// The paths of the blobs `git ls-tree -r -z` printed, in the tree's order.
#[must_use]
pub fn blobs(out: &[u8]) -> Vec<String> {
    out.split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (info, path) = entry.split_once('\t')?;
            (info.split(' ').nth(1) == Some("blob")).then(|| path.to_string())
        })
        .collect()
}

/// The lines `git blame --incremental` credits each commit: the sum of
/// each group's line count.
#[must_use]
pub fn credited(out: &str) -> HashMap<String, i64> {
    let mut lines: HashMap<String, i64> = HashMap::new();
    for line in out.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.len() != 4 || !crate::crawler::repos::commits::is_hash(fields[0]) {
            continue;
        }
        if fields[1..].iter().all(|f| f.bytes().all(|b| b.is_ascii_digit())) {
            if let Ok(n) = fields[3].parse::<i64>() {
                *lines.entry(fields[0].to_string()).or_default() += n;
            }
        }
    }
    lines
}

impl Crawl<'_> {
    /// Step 11. `None` once stopped.
    pub(super) async fn survival(&mut self, stop: &mut Stop) -> Option<()> {
        let Some(tip) = self.mainline.tip.clone() else { return Some(()) };
        if self.job.blamed.as_deref() == Some(tip.as_str()) {
            // Nothing is blamed while the tip stays.
            self.blamed = Some(tip);
            return Some(());
        }
        let (dir, with) = self.tip_dir();
        let raw = With { raw: true, ..with.clone() };
        let tree = self.rec.run("tip", &dir, &["ls-tree", "-r", "-z", "--full-tree", &tip], &raw, OK, stop).await?;
        let (Some(0), Some(out)) = (tree.exit, &tree.raw) else { return Some(()) };
        let files = blobs(out);

        // The newest change of each path on the main line: a binary one is
        // not blamed.
        let when: HashMap<String, DateTime<Utc>> = self
            .commits
            .iter()
            .filter(|(hash, _)| self.places.get(*hash) == Some(&"mainline"))
            .map(|(_, c)| (row_key(&c.row), c.committed_at))
            .collect();
        let mut newest: HashMap<&str, (DateTime<Utc>, bool)> = HashMap::new();
        for (commit, path, binary) in &self.known.files {
            let Some(at) = when.get(commit) else { continue };
            if newest.get(path.as_str()).is_none_or(|(seen, _)| seen < at) {
                newest.insert(path.as_str(), (*at, *binary));
            }
        }
        let budget = usize::try_from(self.ctx.params.blame_files).unwrap_or(usize::MAX);
        let mut chosen: Vec<String> = Vec::new();
        for path in files {
            if newest.get(path.as_str()).is_some_and(|(_, binary)| *binary) || self.ctx.params.skips_survival(&path) {
                continue;
            }
            if chosen.len() < budget {
                chosen.push(path);
            } else {
                self.counts.files_left += 1;
            }
        }

        // Each blamed at the tip, attr_recon_blames_at_once at once.
        let at_once = usize::try_from(self.ctx.params.blames_at_once).unwrap_or(usize::MAX).clamp(1, Semaphore::MAX_PERMITS);
        let gate = Arc::new(Semaphore::new(at_once));
        let timeout = self.rec.timeout();
        let mut running: JoinSet<(String, Call)> = JoinSet::new();
        for path in chosen {
            let (gate, dir, with, tip) = (Arc::clone(&gate), dir.clone(), with.clone(), tip.clone());
            running.spawn(async move {
                let _turn = gate.acquire_owned().await;
                let call = git::call(&dir, &["blame", "--incremental", &tip, "--", &path], &with, timeout).await;
                (path, call)
            });
        }
        let mut credits: HashMap<String, i64> = HashMap::new();
        let mut failed: Vec<String> = Vec::new();
        while let Some(joined) = stop.unless(running.join_next()).await? {
            let Ok((path, call)) = joined else { continue };
            self.counts.files_blamed += 1;
            if self.rec.record("blame", &call, OK) {
                for (commit, n) in credited(&call.stdout) {
                    self.counts.blame_lines_read += n;
                    *credits.entry(commit).or_default() += n;
                }
            } else {
                failed.push(path);
            }
        }
        if !failed.is_empty() {
            // No survival from this crawl: the next one blames the tip again.
            return Some(());
        }
        let mut rows: Vec<(String, i64, i64)> = Vec::new();
        for (hash, c) in &self.commits {
            let key = row_key(&c.row);
            let before = self.known.survival.get(&key).copied();
            let alive = credits.get(hash).map(|n| (*n).min(c.added).max(0));
            match (alive, before) {
                (Some(alive), before) if before != Some(alive) => rows.push((hash.clone(), alive, before.unwrap_or(0))),
                (None, Some(before)) if before > 0 => rows.push((hash.clone(), 0, before)),
                _ => {}
            }
        }
        rows.sort();
        for (hash, alive, before) in rows {
            let Some(c) = self.commits.get(&hash) else { continue };
            self.rows.create("survival", Row::new().link("commit", &c.row).link("crawl", &self.id).int("alive", alive));
            self.known.survival.insert(row_key(&c.row), alive);
            self.counts.survival_changed += 1;
            self.counts.survival_delta += alive - before;
        }
        self.blamed = Some(tip);
        Some(())
    }
}
