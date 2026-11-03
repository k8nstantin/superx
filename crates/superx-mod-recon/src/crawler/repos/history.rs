//! Where each commit sits and what of it landed (#538, steps 9 and 10):
//! every commit of the crawl's since placed `mainline`, `in_flight`,
//! `branch` or `recovered`, whether a ref reaches it read in the git
//! directory that holds the ref; and each file a commit off the main line
//! wrote that the main line's first-parent history carries the same
//! version of, landed through the oldest commit that wrote it there.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use superx_kernel::types::RecordId;

use crate::crawler::hosts::held::row_key;
use crate::crawler::repos::commits;
use crate::crawler::repos::crawl::{lines, Crawl, COMMITS_PER_READ};
use crate::crawler::repos::git::With;
use crate::crawler::repos::record::OK;
use crate::crawler::store::Row;
use crate::crawler::Stop;

/// The hashes a `git rev-list` printed.
fn hashes(out: Option<&str>) -> Vec<String> {
    out.unwrap_or("").lines().map(str::trim).filter(|h| commits::is_hash(h)).map(str::to_string).collect()
}

impl Crawl<'_> {
    /// Step 9: one place for every commit the repository holds, committed
    /// in the crawl's since, a row when it has none or its newest differs.
    /// `None` once stopped.
    pub(super) async fn placements(&mut self, stop: &mut Stop) -> Option<()> {
        let mut mainline: HashSet<String> = HashSet::new();
        if let Some(tip) = self.mainline.tip.clone() {
            let (dir, with) = self.tip_dir();
            let reached = self.rec.run("placement", &dir, &["rev-list", &tip], &with, OK, stop).await?;
            mainline = hashes(reached.ok()).into_iter().collect();
            let first = self.rec.run("placement", &dir, &["rev-list", "--first-parent", &tip], &with, OK, stop).await?;
            self.first_parent = hashes(first.ok());
        }
        let first_parent: HashSet<&String> = self.first_parent.iter().collect();

        // The branches the present checkouts have checked out.
        let checked_out: BTreeSet<String> = self
            .job
            .checkouts
            .iter()
            .filter(|c| c.present)
            .filter_map(|c| c.branch.as_ref())
            .map(|b| format!("refs/heads/{b}"))
            .filter(|r| self.repo_refs.iter().any(|x| x.name == *r && x.commit.is_some()))
            .collect();
        let (dir, with) = self.repository();
        let mut in_flight: HashSet<String> = HashSet::new();
        if !checked_out.is_empty() {
            let mut args = vec!["rev-list"];
            args.extend(checked_out.iter().map(String::as_str));
            let reached = self.rec.run("placement", &dir, &args, &with, OK, stop).await?;
            in_flight = hashes(reached.ok()).into_iter().collect();
        }
        // Any other ref of the repository or of its mirrors.
        let reached = self.rec.run("placement", &dir, &["rev-list", "--all"], &with, OK, stop).await?;
        let mut branch: HashSet<String> = hashes(reached.ok()).into_iter().collect();
        for m in self.mirrors.clone() {
            let with = self.mirror_with(m.hp.host);
            let reached = self.rec.run("placement", &m.folder, &["rev-list", "--all"], &with, OK, stop).await?;
            branch.extend(hashes(reached.ok()));
        }

        let since = self.since;
        let mut placed: Vec<(String, RecordId, &'static str, bool)> = self
            .commits
            .iter()
            .filter(|(_, c)| since.is_none_or(|s| c.committed_at >= s))
            .map(|(hash, c)| {
                let place = if mainline.contains(hash) {
                    "mainline"
                } else if in_flight.contains(hash) {
                    "in_flight"
                } else if branch.contains(hash) {
                    "branch"
                } else {
                    "recovered"
                };
                (hash.clone(), c.row.clone(), place, first_parent.contains(hash))
            })
            .collect();
        placed.sort_by(|a, b| a.0.cmp(&b.0));
        for (hash, row, place, first) in placed {
            self.places.insert(hash, place);
            let key = row_key(&row);
            if self.known.placements.get(&key).is_some_and(|(p, f)| p == place && *f == first) {
                continue;
            }
            self.rows.create(
                "placement",
                Row::new().link("commit", &row).link("crawl", &self.id).text("place", place).flag("first_parent", first),
            );
            self.known.placements.insert(key, (place.to_string(), first));
            self.counts.placements_changed += 1;
        }
        Some(())
    }

    /// Step 10: for each commit placed off the main line, each path its
    /// change added or modified, landed through the oldest commit of the
    /// main line's first-parent history that gave the path the same blob.
    /// `None` once stopped.
    pub(super) async fn landed(&mut self, stop: &mut Stop) -> Option<()> {
        let Some(tip) = self.mainline.tip.clone() else { return Some(()) };
        let off: Vec<String> = self
            .places
            .iter()
            .filter(|(_, place)| matches!(**place, "branch" | "in_flight" | "recovered"))
            .map(|(hash, _)| hash.clone())
            .collect();
        // The version each wrote: from this crawl's reading, else read
        // again from the git directory that holds it.
        let mut wrote: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        let mut again: BTreeMap<PathBuf, (With, Vec<String>)> = BTreeMap::new();
        for hash in off {
            if let Some(files) = self.changes.get(&hash) {
                wrote.insert(hash, files.iter().filter_map(|f| Some((f.path.clone(), f.blob.clone()?))).collect());
            } else if self.in_repository.contains(&hash) {
                let (dir, with) = self.repository();
                again.entry(dir).or_insert_with(|| (with, Vec::new())).1.push(hash);
            } else if let Some(folder) = self.mirror_only.get(&hash).cloned() {
                let host = self.mirrors.iter().find(|m| m.folder == folder).map(|m| m.hp.host);
                if let Some(host) = host {
                    let with = self.mirror_with(host);
                    again.entry(folder).or_insert_with(|| (with, Vec::new())).1.push(hash);
                }
            }
        }
        let diff = ["--no-renames", "--diff-merges=first-parent", "--root", "--no-abbrev", "--no-color", "-z", "--format=%x01%H", "--raw"];
        for (dir, (with, hashes)) in again {
            for page in hashes.chunks(COMMITS_PER_READ) {
                let input = With { input: Some(lines(page)), raw: true, ..with.clone() };
                let mut args = vec!["log", "--no-walk=unsorted", "--stdin"];
                args.extend(diff);
                let read = self.rec.run("landed", &dir, &args, &input, OK, stop).await?;
                if let (Some(0), Some(out)) = (read.exit, &read.raw) {
                    for (hash, files) in commits::changes(out) {
                        wrote.insert(hash, files.into_iter().filter_map(|f| Some((f.path, f.blob?))).collect());
                    }
                }
            }
        }
        let wanted: HashSet<(String, String)> = wrote.values().flatten().cloned().collect();
        if wanted.is_empty() {
            return Some(());
        }
        // The main line's first-parent history, newest first: each pair's
        // oldest commit the repository holds a row for.
        let (dir, with) = self.tip_dir();
        let raw = With { raw: true, ..with };
        let mut args = vec!["log", "--first-parent"];
        args.extend(diff);
        args.push(&tip);
        let history = self.rec.run("landed", &dir, &args, &raw, OK, stop).await?;
        let mut via: HashMap<(String, String), String> = HashMap::new();
        if let (Some(0), Some(out)) = (history.exit, &history.raw) {
            let order = commits::changes(out);
            for hash in self.first_parent.iter() {
                if !self.commits.contains_key(hash) {
                    continue;
                }
                for f in order.get(hash).into_iter().flatten() {
                    if let Some(blob) = &f.blob {
                        let pair = (f.path.clone(), blob.clone());
                        if wanted.contains(&pair) {
                            via.insert(pair, hash.clone());
                        }
                    }
                }
            }
        }
        for (hash, files) in wrote {
            let Some(commit) = self.commits.get(&hash).map(|c| c.row.clone()) else { continue };
            for (path, blob) in files {
                let Some(through) = via.get(&(path.clone(), blob)).and_then(|h| self.commits.get(h)).map(|c| c.row.clone()) else {
                    continue;
                };
                let key = (row_key(&commit), path.clone(), row_key(&through));
                if !self.known.landed.insert(key) {
                    continue;
                }
                self.rows.create(
                    "landed_via",
                    Row::new().link("commit", &commit).text("path", path).link("via", &through).link("crawl", &self.id),
                );
                self.counts.landed_new += 1;
            }
        }
        Some(())
    }
}
