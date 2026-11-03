//! The commits tied to sessions (#538, step 13): a commit a session's
//! command printed, by the one commit its printed hash begins; and the
//! commits a pull request a session created or merged has listed, or its
//! merge made. Each tie written once.

use std::collections::HashSet;
use std::path::Path;

use superx_kernel::types::RecordId;
use superx_kernel::Result;

use crate::crawler::hosts::crawl::Done;
use crate::crawler::hosts::held::row_key;
use crate::crawler::repos::crawl::Crawl;
use crate::crawler::repos::known::{self, GitEvent, HeldPull};
use crate::crawler::store::Row;

/// The checkout on the list whose tree holds `dir`: the deepest root that
/// is `dir` or holds it.
#[must_use]
pub fn holder<'a>(checkouts: &'a [(String, RecordId)], dir: &str) -> Option<&'a (String, RecordId)> {
    checkouts.iter().filter(|(root, _)| Path::new(dir).starts_with(root)).max_by_key(|(root, _)| root.len())
}

/// The commits of `sorted` whose hash begins with `prefix`.
fn beginning<'a>(sorted: &'a [(String, RecordId)], prefix: &str) -> &'a [(String, RecordId)] {
    let prefix = prefix.to_lowercase();
    let start = sorted.partition_point(|(h, _)| h.as_str() < prefix.as_str());
    let end = start + sorted[start..].iter().take_while(|(h, _)| h.starts_with(&prefix)).count();
    &sorted[start..end]
}

impl Crawl<'_> {
    /// Step 13, once the host crawls are done.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer for
    /// the pull requests of a host repository another worker took.
    pub(super) async fn sessions(&mut self, done: &[Done]) -> Result<()> {
        let mut sorted: Vec<(String, RecordId)> = self.commits.iter().map(|(h, c)| (h.clone(), c.row.clone())).collect();
        sorted.sort();
        let ours = |dir: &str| holder(&self.ctx.checkouts, dir).is_some_and(|(_, repository)| *repository == self.job.repository);

        // The pull requests of the host repositories its remotes name: of
        // those this worker crawled, as their crawls hold them; of the
        // others, as the database does.
        let mut pulls: Vec<HeldPull> = Vec::new();
        let mut others: Vec<RecordId> = Vec::new();
        for (_, hp) in &self.named {
            if let Some(d) = done.iter().find(|d| d.hp == *hp) {
                for (number, p) in &d.memory.pulls {
                    pulls.push(HeldPull {
                        row: p.row.clone(),
                        number: *number,
                        url: p.url.clone(),
                        listed: p.commits.keys().cloned().collect(),
                        merge_sha: p.state.as_ref().and_then(|s| s.merge_sha.clone()),
                    });
                }
            } else if let Some(row) = self.ctx.hosts.known.repositories.get(&(hp.host, hp.path.clone())) {
                others.push(row.clone());
            }
        }
        pulls.extend(known::pulls(&self.ctx.db, &others).await?);

        let mut ties: Vec<(RecordId, &GitEvent, &'static str, Option<RecordId>)> = Vec::new();
        for e in &self.ctx.events {
            if e.session.is_none() {
                continue;
            }
            match e.action.as_str() {
                "commit" | "merge" | "revert" | "cherry_pick" => {
                    let (Some(hash), Some(dir)) = (&e.hash, &e.dir) else { continue };
                    if !ours(dir) {
                        continue;
                    }
                    if let [(_, commit)] = beginning(&sorted, hash) {
                        ties.push((commit.clone(), e, "printed", None));
                    }
                }
                "pr_created" | "pr_merged" => {
                    let matched: Vec<&HeldPull> = match (&e.url, &e.dir, e.pr_number) {
                        (Some(url), _, _) => pulls.iter().filter(|p| p.url.as_deref() == Some(url.as_str())).collect(),
                        (None, Some(dir), Some(number)) if ours(dir) => pulls.iter().filter(|p| p.number == number).collect(),
                        _ => Vec::new(),
                    };
                    for p in matched {
                        for hash in p.listed.iter().chain(p.merge_sha.iter()) {
                            for (_, commit) in beginning(&sorted, hash) {
                                ties.push((commit.clone(), e, "pull_request", Some(p.row.clone())));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let mut written: HashSet<(String, String, &'static str)> = HashSet::new();
        for (commit, e, via, pull) in ties {
            let Some(session) = &e.session else { continue };
            let key = (row_key(&commit), row_key(session), via);
            if self.known.sessions.contains(&(key.0.clone(), key.1.clone(), via.to_string())) || !written.insert(key) {
                continue;
            }
            self.tail.create(
                "commit_session",
                Row::new()
                    .link("commit", &commit)
                    .link("session", session)
                    .link("tool_call", &e.tool_call)
                    .text("via", via)
                    .some_link("pull_request", pull.as_ref())
                    .time("at", e.at),
            );
        }
        Ok(())
    }
}
