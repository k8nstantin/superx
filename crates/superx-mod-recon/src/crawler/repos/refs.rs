//! The refs and the main line (#538, step 5): every ref a repository and
//! the mirrors of its remotes hold, with the commit each points at; how
//! they moved since the crawler's previous crawl of the repository; and
//! which of them is the main line.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The format `git for-each-ref` prints each ref in: its name, its object
/// and that object's type, the object a tag peels to and its type, and
/// the ref a symbolic ref names, NUL between them.
pub const FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(symref)";

/// One ref, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    /// The commit it points at, a tag's peeled to it; `None` when it
    /// points at no commit.
    pub commit: Option<String>,
    /// The ref a symbolic ref names.
    pub symref: Option<String>,
}

/// The refs `git for-each-ref` printed in [`FORMAT`].
#[must_use]
pub fn parse(out: &str) -> Vec<Ref> {
    out.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\0').collect();
            let name = (*fields.first()?).to_string();
            let commit = match (fields.get(2).copied(), fields.get(4).copied()) {
                (Some("commit"), _) => fields.get(1).map(|s| (*s).to_string()),
                (Some("tag"), Some("commit")) => fields.get(3).map(|s| (*s).to_string()),
                _ => None,
            };
            let symref = fields.get(5).filter(|s| !s.is_empty()).map(|s| (*s).to_string());
            Some(Ref { name, commit, symref })
        })
        .collect()
}

/// Where a set of refs was read: the repository, or one of its remotes'
/// mirrors by its folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Held {
    Repository,
    Mirror(PathBuf),
}

/// Every ref a crawl read, by where it was read and its name: the commit
/// it points at.
pub type Read = BTreeMap<(Held, String), Option<String>>;

/// How the refs moved since the previous crawl.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Moves {
    pub new: i64,
    /// Every ref that points at another commit, those rewritten included:
    /// where, its name, its old commit and its new.
    pub moved: Vec<(Held, String, Option<String>, Option<String>)>,
    pub gone: i64,
}

/// The refs that are new, moved (with their old and new commits) or gone
/// against `before`.
#[must_use]
pub fn compare(before: &Read, now: &Read) -> Moves {
    let mut moves = Moves::default();
    for (key, commit) in now {
        match before.get(key) {
            None => moves.new += 1,
            Some(old) if old != commit => moves.moved.push((key.0.clone(), key.1.clone(), old.clone(), commit.clone())),
            Some(_) => {}
        }
    }
    moves.gone = i64::try_from(before.keys().filter(|k| !now.contains_key(*k)).count()).unwrap_or(i64::MAX);
    moves
}

/// The ref a name resolves to among `refs`, as git's rules for a ref name
/// resolve it (gitrevisions(7)): the name itself, then under `refs/`,
/// `refs/tags/`, `refs/heads/`, `refs/remotes/`, and a remote's `HEAD`.
#[must_use]
pub fn resolve<'a>(name: &str, refs: &'a [Ref]) -> Option<&'a Ref> {
    let candidates = [
        name.to_string(),
        format!("refs/{name}"),
        format!("refs/tags/{name}"),
        format!("refs/heads/{name}"),
        format!("refs/remotes/{name}"),
        format!("refs/remotes/{name}/HEAD"),
    ];
    candidates.iter().find_map(|c| refs.iter().find(|r| r.name == *c && r.commit.is_some()))
}

/// How the main line was chosen, as `crawl.mainline_from` words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    Override,
    HostDefault,
    OriginHead,
    LocalBranch,
    Head,
}

impl From {
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::HostDefault => "host_default",
            Self::OriginHead => "origin_head",
            Self::LocalBranch => "local_branch",
            Self::Head => "head",
        }
    }
}

/// The main line: its ref's full name, how it was chosen, its tip, and
/// the git directory that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mainline {
    pub name: String,
    pub from: From,
    pub tip: Option<String>,
    /// The mirror that holds it; `None` for the repository.
    pub mirror: Option<PathBuf>,
}

/// The main line, the first of these that resolves: the operator's
/// override, as git resolves it in the repository; the host's default
/// branch, in the mirror of origin when it holds it, else as origin's
/// remote-tracking branch; what `refs/remotes/origin/HEAD` points at;
/// `refs/heads/main`, else `refs/heads/master`. `None` when none does:
/// HEAD is then the main line, its tip read apart.
#[must_use]
pub fn mainline(
    repository: &[Ref],
    override_ref: Option<&str>,
    host_default: Option<&str>,
    origin_mirror: Option<(&PathBuf, &[Ref])>,
) -> Option<Mainline> {
    if let Some(r) = override_ref.and_then(|name| resolve(name, repository)) {
        return Some(Mainline { name: r.name.clone(), from: From::Override, tip: r.commit.clone(), mirror: None });
    }
    if let Some(branch) = host_default {
        let head = format!("refs/heads/{branch}");
        if let Some((folder, refs)) = origin_mirror {
            if let Some(r) = refs.iter().find(|r| r.name == head && r.commit.is_some()) {
                return Some(Mainline { name: r.name.clone(), from: From::HostDefault, tip: r.commit.clone(), mirror: Some(folder.clone()) });
            }
        }
        let tracking = format!("refs/remotes/origin/{branch}");
        if let Some(r) = repository.iter().find(|r| r.name == tracking && r.commit.is_some()) {
            return Some(Mainline { name: r.name.clone(), from: From::HostDefault, tip: r.commit.clone(), mirror: None });
        }
    }
    let origin_head = repository.iter().find(|r| r.name == "refs/remotes/origin/HEAD").and_then(|r| r.symref.as_deref());
    if let Some(r) = origin_head.and_then(|target| repository.iter().find(|r| r.name == target && r.commit.is_some())) {
        return Some(Mainline { name: r.name.clone(), from: From::OriginHead, tip: r.commit.clone(), mirror: None });
    }
    for local in ["refs/heads/main", "refs/heads/master"] {
        if let Some(r) = repository.iter().find(|r| r.name == local && r.commit.is_some()) {
            return Some(Mainline { name: r.name.clone(), from: From::LocalBranch, tip: r.commit.clone(), mirror: None });
        }
    }
    None
}
