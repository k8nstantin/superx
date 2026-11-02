//! A repository's refs (#538, step 5): every ref of the repository and of
//! its mirrors with the commit it points at, compared with the refs the
//! crawler read at its previous crawl of the same repository since
//! superx-recon started.

use std::collections::BTreeMap;

/// Where a ref is held: the repository's git directory, or a mirror's
/// folder.
pub type Place = String;

/// The refs one crawl read: by where they are held and their full name,
/// the commit each points at, a tag's peeled to its commit.
pub type Refs = BTreeMap<(Place, String), String>;

/// One ref as `git for-each-ref` printed it: its full name, the commit it
/// points at, and the ref a symbolic ref names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub commit: String,
    pub symref: Option<String>,
}

/// What `git for-each-ref --format='%(refname)%09%(objectname)%09%(*objectname)%09%(symref)'`
/// printed: a tag's peeled object in place of the tag's own.
#[must_use]
pub fn parse(out: &str) -> Vec<Ref> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_string();
            let object = fields.next()?;
            let peeled = fields.next().unwrap_or_default();
            let symref = fields.next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            let commit = if peeled.is_empty() { object } else { peeled };
            (!name.is_empty() && !commit.is_empty()).then(|| Ref { name, commit: commit.to_string(), symref })
        })
        .collect()
}

/// How the refs changed since the previous crawl: new, moved, gone, and
/// the moved ones whose old commit to check against their new one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub new: i64,
    pub moved: Vec<((Place, String), String, String)>,
    pub gone: i64,
}

/// `now` against `before`.
#[must_use]
pub fn compare(before: &Refs, now: &Refs) -> Changes {
    let mut changes = Changes::default();
    for (key, commit) in now {
        match before.get(key) {
            None => changes.new += 1,
            Some(old) if old != commit => changes.moved.push((key.clone(), old.clone(), commit.clone())),
            Some(_) => {}
        }
    }
    changes.gone = i64::try_from(before.keys().filter(|k| !now.contains_key(*k)).count()).unwrap_or(i64::MAX);
    changes
}
