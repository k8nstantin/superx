//! What the crawler can be tuned by (#466). Every limit is a parameter on
//! this module's registry entity, set with
//! `superx param set repos <name> <json>`, and runs on the fallback below
//! until it is set (§9). A value of the wrong shape is reported by name and
//! the fallback stands in for it: the crawler never runs on a guess, and
//! never hides one.

use std::collections::BTreeMap;

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::MODULE_NAME;

/// Directories to crawl besides the ones the sessions worked in.
pub const INCLUDE_PARAM: &str = "attr_repos_include";

/// Repositories never crawled, by name or by a prefix of their path.
pub const EXCLUDE_PARAM: &str = "attr_repos_exclude";

/// Seconds between crawls of a repository; 0 crawls only when asked.
pub const CRAWL_SECS_PARAM: &str = "attr_repos_crawl_secs";
const DEFAULT_CRAWL_SECS: u64 = 600; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_crawl_secs

/// How far back a crawl reads, in days; 0 reads all of history.
pub const SINCE_DAYS_PARAM: &str = "attr_repos_since_days";
const DEFAULT_SINCE_DAYS: u64 = 0; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_since_days

/// Repositories read in one pass, at most.
pub const MAX_REPOS_PARAM: &str = "attr_repos_max";
const DEFAULT_MAX_REPOS: u64 = 8; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_max

/// Files blamed per repository to see what survives; 0 blames none.
pub const BLAME_FILES_PARAM: &str = "attr_repos_blame_files";
const DEFAULT_BLAME_FILES: u64 = 4_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_blame_files

/// Blames run at once; each is a git process of its own.
pub const BLAMES_AT_ONCE_PARAM: &str = "attr_repos_blames_at_once";
const DEFAULT_BLAMES_AT_ONCE: u64 = 8; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_blames_at_once

/// Milliseconds a git call may take before it is abandoned and counted
/// as unreadable.
pub const GIT_TIMEOUT_MS_PARAM: &str = "attr_repos_git_timeout_ms";
const DEFAULT_GIT_TIMEOUT_MS: u64 = 4_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_git_timeout_ms

/// Unreachable commits read per repository, at most.
pub const UNREACHABLE_COMMITS_PARAM: &str = "attr_repos_unreachable_commits";
const DEFAULT_UNREACHABLE_COMMITS: u64 = 5_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_repos_unreachable_commits

/// Paths never blamed, because their lines say nothing about whether work
/// survived. An entry that starts and ends with `/` matches that directory
/// anywhere in a path; any other entry matches the end of a path.
pub const SURVIVAL_SKIP_PARAM: &str = "attr_repos_survival_skip";
const DEFAULT_SURVIVAL_SKIP: &[&str] =
    &["/dist/", ".lock", ".tsbuildinfo", ".png", ".jpg", ".jpeg", ".ico", ".woff", ".woff2", ".svg", ".pdf"];

/// The ref a repository's work lands on, where it is not what `origin/HEAD`
/// says: an object `{repository name: ref}`. The operator's decision; no
/// rule in code can know it.
pub const MAINLINE_REFS_PARAM: &str = "attr_repos_mainline_refs";

/// The parameters in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub crawl_secs: u64,
    pub since_days: u64,
    pub max_repos: u64,
    pub blame_files: u64,
    pub blames_at_once: u64,
    pub git_timeout_ms: u64,
    pub unreachable_commits: u64,
    pub survival_skip: Vec<String>,
    pub mainline_refs: BTreeMap<String, String>,
}

impl Params {
    /// Whether `path` is one the crawler never blames.
    #[must_use]
    pub fn skips_survival(&self, path: &str) -> bool {
        self.survival_skip.iter().any(|entry| {
            if entry.len() > 1 && entry.starts_with('/') && entry.ends_with('/') {
                path.contains(entry.as_str()) || path.starts_with(&entry[1..])
            } else {
                path.ends_with(entry.as_str())
            }
        })
    }
}

/// One parameter as it stands: the value in force, as `status` prints it,
/// and whether it was set or is the fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: &'static str,
    pub value: String,
    pub set: bool,
}

/// A parameter set to a value of the wrong shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub name: &'static str,
    pub found: String,
    pub wanted: &'static str,
}

/// The parameters in force, how each one stands, and what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub params: Params,
    pub settings: Vec<Setting>,
    pub problems: Vec<Problem>,
}

/// Every parameter's name, in the order `status` lists them.
pub const NAMES: [&str; 11] = [
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    CRAWL_SECS_PARAM,
    SINCE_DAYS_PARAM,
    MAX_REPOS_PARAM,
    BLAME_FILES_PARAM,
    BLAMES_AT_ONCE_PARAM,
    GIT_TIMEOUT_MS_PARAM,
    UNREACHABLE_COMMITS_PARAM,
    SURVIVAL_SKIP_PARAM,
    MAINLINE_REFS_PARAM,
];

/// Read every parameter from this module's registry entity. Before the
/// module is registered, every parameter is its fallback.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the substrate cannot answer, or a
/// parameter's row is corrupt.
pub async fn resolve(kernel: &Kernel) -> Result<Resolved> {
    let entity = match kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await {
        Ok(entity) => entity,
        // An instance defines no module type before its first boot, so no
        // module is registered on it yet.
        Err(KernelError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let mut values = BTreeMap::new();
    if let Some(entity) = entity {
        for name in NAMES {
            if let Some(v) = kernel.get_parameter(entity.clone(), name).await? {
                values.insert(name, v);
            }
        }
    }
    Ok(from_values(&values))
}

/// The parameters from the values set, by name; an unset one is its
/// fallback, and so is one of the wrong shape, which is reported.
#[must_use]
pub fn from_values(values: &BTreeMap<&'static str, Value>) -> Resolved {
    let mut r = Reader { values, settings: Vec::new(), problems: Vec::new() };
    let params = Params {
        include: r.strings(INCLUDE_PARAM, &[]),
        exclude: r.strings(EXCLUDE_PARAM, &[]),
        crawl_secs: r.count(CRAWL_SECS_PARAM, 0, DEFAULT_CRAWL_SECS),
        since_days: r.count(SINCE_DAYS_PARAM, 0, DEFAULT_SINCE_DAYS),
        max_repos: r.count(MAX_REPOS_PARAM, 1, DEFAULT_MAX_REPOS),
        blame_files: r.count(BLAME_FILES_PARAM, 0, DEFAULT_BLAME_FILES),
        blames_at_once: r.count(BLAMES_AT_ONCE_PARAM, 1, DEFAULT_BLAMES_AT_ONCE),
        git_timeout_ms: r.count(GIT_TIMEOUT_MS_PARAM, 1, DEFAULT_GIT_TIMEOUT_MS),
        unreachable_commits: r.count(UNREACHABLE_COMMITS_PARAM, 0, DEFAULT_UNREACHABLE_COMMITS),
        survival_skip: r.strings(SURVIVAL_SKIP_PARAM, DEFAULT_SURVIVAL_SKIP),
        mainline_refs: r.refs(MAINLINE_REFS_PARAM),
    };
    Resolved { params, settings: r.settings, problems: r.problems }
}

/// Reads typed values out of the set parameters, noting how each stands.
struct Reader<'a> {
    values: &'a BTreeMap<&'static str, Value>,
    settings: Vec<Setting>,
    problems: Vec<Problem>,
}

impl Reader<'_> {
    /// The value set under `name`, parsed; the fallback when it is unset or
    /// `parse` refuses it, which is reported as wanting `wanted`.
    fn read<T>(
        &mut self,
        name: &'static str,
        wanted: &'static str,
        fallback: T,
        parse: impl Fn(&Value) -> Option<T>,
        show: impl Fn(&T) -> String,
    ) -> T {
        let parsed = match self.values.get(name) {
            None => None,
            Some(v) => {
                let p = parse(v);
                if p.is_none() {
                    self.problems.push(Problem { name, found: shown(v), wanted });
                }
                p
            }
        };
        let set = parsed.is_some();
        let value = parsed.unwrap_or(fallback);
        self.settings.push(Setting { name, value: show(&value), set });
        value
    }

    fn count(&mut self, name: &'static str, min: u64, fallback: u64) -> u64 {
        let wanted = if min == 0 { "a whole number, 0 or more" } else { "a whole number, 1 or more" };
        self.read(
            name,
            wanted,
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u64::try_from(i).ok()).filter(|&i| i >= min),
                _ => None,
            },
            u64::to_string,
        )
    }

    fn strings(&mut self, name: &'static str, fallback: &[&str]) -> Vec<String> {
        self.read(
            name,
            "a list of non-empty strings",
            fallback.iter().map(|s| (*s).to_string()).collect(),
            |v| match v {
                Value::Array(items) => items
                    .iter()
                    .map(|i| match i {
                        Value::String(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            },
            |list| if list.is_empty() { "none".to_string() } else { list.join(", ") },
        )
    }

    fn refs(&mut self, name: &'static str) -> BTreeMap<String, String> {
        self.read(
            name,
            "an object of repository names to refs",
            BTreeMap::new(),
            |v| match v {
                Value::Object(o) => o
                    .iter()
                    .map(|(k, r)| match r {
                        Value::String(s) if !s.is_empty() => Some((k.clone(), s.clone())),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            },
            |refs| {
                if refs.is_empty() {
                    "none".to_string()
                } else {
                    refs.iter().map(|(repo, r)| format!("{repo} → {r}")).collect::<Vec<_>>().join(", ")
                }
            },
        )
    }
}

/// A value as the operator would write it back.
fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Number(n) => n.to_int().map_or_else(|| format!("{n:?}"), |i| i.to_string()),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(i: i64) -> Value {
        Value::Number(superx_kernel::types::Number::Int(i))
    }

    fn strings(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| (*s).to_string()).collect::<Vec<String>>().into())
    }

    #[test]
    fn nothing_set_is_every_fallback() {
        let r = from_values(&BTreeMap::new());
        assert!(r.problems.is_empty());
        assert_eq!(r.settings.len(), NAMES.len(), "every parameter is listed");
        assert!(r.settings.iter().all(|s| !s.set));
        assert_eq!(r.params.max_repos, 8);
        assert_eq!(r.params.blame_files, 4_000);
        assert_eq!(r.params.git_timeout_ms, 4_000);
        assert_eq!(r.params.since_days, 0, "all of history");
        assert!(r.params.include.is_empty() && r.params.mainline_refs.is_empty());
        let names: Vec<&str> = r.settings.iter().map(|s| s.name).collect();
        assert_eq!(names, NAMES, "listed in order");
    }

    #[test]
    fn a_set_value_is_the_one_in_force() {
        let mut values = BTreeMap::new();
        values.insert(MAX_REPOS_PARAM, number(12));
        values.insert(INCLUDE_PARAM, strings(&["/work/a", "/work/b"]));
        let mut refs = superx_kernel::types::Object::new();
        refs.insert("gryphon-data-lake".to_string(), Value::String("origin/sandbox".to_string()));
        values.insert(MAINLINE_REFS_PARAM, Value::Object(refs));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.max_repos, 12);
        assert_eq!(r.params.include, ["/work/a", "/work/b"]);
        assert_eq!(r.params.mainline_refs.get("gryphon-data-lake").map(String::as_str), Some("origin/sandbox"));
        let max = r.settings.iter().find(|s| s.name == MAX_REPOS_PARAM).expect("listed");
        assert!(max.set && max.value == "12");
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_reported_and_the_fallback_stands_in() {
        let mut values = BTreeMap::new();
        values.insert(MAX_REPOS_PARAM, Value::String("eight".to_string()));
        values.insert(BLAMES_AT_ONCE_PARAM, number(0));
        values.insert(SINCE_DAYS_PARAM, number(-3));
        values.insert(EXCLUDE_PARAM, strings(&["ok", ""]));
        let r = from_values(&values);
        assert_eq!(r.params.max_repos, 8);
        assert_eq!(r.params.blames_at_once, 8, "0 blames at once would never blame");
        assert_eq!(r.params.since_days, 0);
        assert!(r.params.exclude.is_empty(), "a list with an empty entry is refused whole");
        let named: Vec<&str> = r.problems.iter().map(|p| p.name).collect();
        assert_eq!(named, [EXCLUDE_PARAM, SINCE_DAYS_PARAM, MAX_REPOS_PARAM, BLAMES_AT_ONCE_PARAM]);
        let max = r.problems.iter().find(|p| p.name == MAX_REPOS_PARAM).expect("reported");
        assert_eq!(max.found, "\"eight\"");
        assert!(r.settings.iter().all(|s| !s.set), "a refused value is not in force");
    }

    #[test]
    fn zero_is_allowed_where_it_means_something() {
        let mut values = BTreeMap::new();
        values.insert(CRAWL_SECS_PARAM, number(0));
        values.insert(BLAME_FILES_PARAM, number(0));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.crawl_secs, 0, "only when asked");
        assert_eq!(r.params.blame_files, 0, "blame nothing");
    }

    #[test]
    fn survival_skips_built_bundles_lockfiles_and_binaries() {
        let p = from_values(&BTreeMap::new()).params;
        for path in ["ui/dist/assets/index.js", "dist/index.html", "Cargo.lock", "ui/tsconfig.tsbuildinfo", "logo.svg", "a/b.PDF.pdf"] {
            assert!(p.skips_survival(path), "{path} is skipped");
        }
        for path in ["src/lib.rs", "distance.rs", "docs/dist.md", "lockfile.rs"] {
            assert!(!p.skips_survival(path), "{path} is blamed");
        }
    }
}
