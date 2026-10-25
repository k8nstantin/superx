//! What the module can be tuned by (#466). Every tunable is a parameter on
//! this module's registry entity, set with
//! `superx param set performance <name> <json>`, and runs on the fallback
//! below until it is set (§9). A value of the wrong shape is reported by
//! name and the fallback stands in for it: the module never runs on a
//! guess, and never hides one.

use std::collections::BTreeMap;

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::MODULE_NAME;

/// The port the module's own URL listens on, at 127.0.0.1.
pub const PORT_PARAM: &str = "attr_performance_port";
const DEFAULT_PORT: u16 = 5153; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_port

/// How often the crawler reads the lines captured since its last pass, in
/// seconds; 0 pauses it.
pub const READ_SECS_PARAM: &str = "attr_performance_read_secs";
const DEFAULT_READ_SECS: u64 = 15; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_read_secs

/// Directories to crawl besides the ones the sessions worked in.
pub const INCLUDE_PARAM: &str = "attr_performance_include";

/// Repositories never crawled, by name or by a prefix of their path.
pub const EXCLUDE_PARAM: &str = "attr_performance_exclude";

/// Seconds between full crawls of the repositories; 0 pauses them.
pub const CRAWL_SECS_PARAM: &str = "attr_performance_crawl_secs";
const DEFAULT_CRAWL_SECS: u64 = 600; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_crawl_secs

/// How far back a crawl reads, in days; 0 reads all of history.
pub const SINCE_DAYS_PARAM: &str = "attr_performance_since_days";
const DEFAULT_SINCE_DAYS: u64 = 0; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_since_days

/// No limit: a limit's fallback, so nothing is left out unless the
/// operator sets one. Less data leads to wrong conclusions (operator,
/// 2026-10-24).
pub const ALL: u64 = u64::MAX;

/// Files blamed per repository to see what survives; 0 blames none.
pub const BLAME_FILES_PARAM: &str = "attr_performance_blame_files";
const DEFAULT_BLAME_FILES: u64 = ALL;

/// Blames run at once; each is a git process of its own.
pub const BLAMES_AT_ONCE_PARAM: &str = "attr_performance_blames_at_once";
const DEFAULT_BLAMES_AT_ONCE: u64 = 8; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_blames_at_once

/// Milliseconds a git call may take before it is abandoned and counted
/// as unreadable. The crawler runs in its own process, in the background:
/// a large repository's whole history takes seconds to read, and a read
/// cut off at the UI's 4 s dropped gryphon-data-lake from every chart
/// (#498).
pub const GIT_TIMEOUT_MS_PARAM: &str = "attr_performance_git_timeout_ms";
const DEFAULT_GIT_TIMEOUT_MS: u64 = 120_000; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_git_timeout_ms

/// Unreachable commits read per repository, at most.
pub const UNREACHABLE_COMMITS_PARAM: &str = "attr_performance_unreachable_commits";
const DEFAULT_UNREACHABLE_COMMITS: u64 = ALL;

/// Paths never blamed, because their lines say nothing about whether work
/// survived. An entry that starts and ends with `/` matches that directory
/// anywhere in a path; any other entry matches the end of a path.
pub const SURVIVAL_SKIP_PARAM: &str = "attr_performance_survival_skip";
const DEFAULT_SURVIVAL_SKIP: &[&str] =
    &["/dist/", ".lock", ".tsbuildinfo", ".png", ".jpg", ".jpeg", ".ico", ".woff", ".woff2", ".svg", ".pdf"];

/// The ref a repository's work lands on, where it is not what `origin/HEAD`
/// says: an object `{repository name: ref}`. The operator's decision; no
/// rule in code can know it.
pub const MAINLINE_REFS_PARAM: &str = "attr_performance_mainline_refs";

/// The parameters in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub port: u16,
    pub read_secs: u64,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub crawl_secs: u64,
    pub since_days: u64,
    pub blame_files: u64,
    pub blames_at_once: u64,
    pub git_timeout_ms: u64,
    pub unreachable_commits: u64,
    pub survival_skip: Vec<String>,
    pub mainline_refs: BTreeMap<String, String>,
}

impl Params {
    /// Whether a repository is one the operator excluded: by its name, or
    /// by a prefix of one of its paths.
    #[must_use]
    pub fn excludes(&self, name: &str, paths: &[&str]) -> bool {
        self.exclude
            .iter()
            .filter(|e| !e.is_empty())
            .any(|e| e == name || paths.iter().any(|p| p.starts_with(e.as_str())))
    }

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
pub const NAMES: [&str; 12] = [
    PORT_PARAM,
    READ_SECS_PARAM,
    INCLUDE_PARAM,
    EXCLUDE_PARAM,
    CRAWL_SECS_PARAM,
    SINCE_DAYS_PARAM,
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
        port: r.port(PORT_PARAM, DEFAULT_PORT),
        read_secs: r.count(READ_SECS_PARAM, 0, DEFAULT_READ_SECS),
        include: r.strings(INCLUDE_PARAM, &[]),
        exclude: r.strings(EXCLUDE_PARAM, &[]),
        crawl_secs: r.count(CRAWL_SECS_PARAM, 0, DEFAULT_CRAWL_SECS),
        since_days: r.count(SINCE_DAYS_PARAM, 0, DEFAULT_SINCE_DAYS),
        blame_files: r.limit(BLAME_FILES_PARAM, DEFAULT_BLAME_FILES),
        blames_at_once: r.count(BLAMES_AT_ONCE_PARAM, 1, DEFAULT_BLAMES_AT_ONCE),
        git_timeout_ms: r.count(GIT_TIMEOUT_MS_PARAM, 1, DEFAULT_GIT_TIMEOUT_MS),
        unreachable_commits: r.limit(UNREACHABLE_COMMITS_PARAM, DEFAULT_UNREACHABLE_COMMITS),
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

    /// A whole number, `min` or more.
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

    /// A limit: a whole number, 0 or more, shown as `all` when there is none.
    fn limit(&mut self, name: &'static str, fallback: u64) -> u64 {
        self.read(
            name,
            "a whole number, 0 or more",
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u64::try_from(i).ok()),
                _ => None,
            },
            |&v| if v == ALL { "all".to_string() } else { v.to_string() },
        )
    }

    fn port(&mut self, name: &'static str, fallback: u16) -> u16 {
        self.read(
            name,
            "a port, 1 to 65535",
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u16::try_from(i).ok()).filter(|&p| p > 0),
                _ => None,
            },
            u16::to_string,
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
        assert_eq!((r.params.port, r.params.read_secs, r.params.crawl_secs), (5153, 15, 600));
        assert_eq!((r.params.blame_files, r.params.unreachable_commits), (ALL, ALL), "nothing left out unless asked");
        let blame = r.settings.iter().find(|s| s.name == BLAME_FILES_PARAM).expect("listed");
        assert_eq!(blame.value, "all");
        assert_eq!(r.params.git_timeout_ms, 120_000, "a whole history's read is not cut off (#498)");
        assert_eq!(r.params.since_days, 0, "all of history");
        assert!(r.params.include.is_empty() && r.params.mainline_refs.is_empty());
        let names: Vec<&str> = r.settings.iter().map(|s| s.name).collect();
        assert_eq!(names, NAMES, "listed in order");
    }

    #[test]
    fn a_set_value_is_the_one_in_force() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, number(6_000));
        values.insert(READ_SECS_PARAM, number(0));
        values.insert(INCLUDE_PARAM, strings(&["/work/a", "/work/b"]));
        let mut refs = superx_kernel::types::Object::new();
        refs.insert("gryphon-data-lake".to_string(), Value::String("origin/sandbox".to_string()));
        values.insert(MAINLINE_REFS_PARAM, Value::Object(refs));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.port, 6_000);
        assert_eq!(r.params.read_secs, 0, "0 pauses the crawler");
        assert_eq!(r.params.include, ["/work/a", "/work/b"]);
        assert_eq!(r.params.mainline_refs.get("gryphon-data-lake").map(String::as_str), Some("origin/sandbox"));
        let port = r.settings.iter().find(|s| s.name == PORT_PARAM).expect("listed");
        assert!(port.set && port.value == "6000");
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_reported_and_the_fallback_stands_in() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, number(70_000));
        values.insert(READ_SECS_PARAM, Value::String("15".to_string()));
        values.insert(BLAMES_AT_ONCE_PARAM, number(0));
        values.insert(SINCE_DAYS_PARAM, number(-3));
        values.insert(EXCLUDE_PARAM, strings(&["ok", ""]));
        let r = from_values(&values);
        assert_eq!(r.params.port, 5153, "no port above 65535");
        assert_eq!(r.params.read_secs, 15, "text is not a number of seconds");
        assert_eq!(r.params.blames_at_once, 8, "0 blames at once would never blame");
        assert_eq!(r.params.since_days, 0);
        assert!(r.params.exclude.is_empty(), "a list with an empty entry is refused whole");
        let named: Vec<&str> = r.problems.iter().map(|p| p.name).collect();
        assert_eq!(named, [PORT_PARAM, READ_SECS_PARAM, EXCLUDE_PARAM, SINCE_DAYS_PARAM, BLAMES_AT_ONCE_PARAM]);
        let port = r.problems.iter().find(|p| p.name == PORT_PARAM).expect("reported");
        assert_eq!(port.found, "70000");
        assert!(r.settings.iter().all(|s| !s.set), "a refused value is not in force");
    }

    #[test]
    fn a_repository_is_excluded_by_its_name_or_a_prefix_of_its_path() {
        let mut values = BTreeMap::new();
        values.insert(EXCLUDE_PARAM, strings(&["skeema", "/tmp/builds/"]));
        let p = from_values(&values).params;
        assert!(p.excludes("skeema", &["/w/skeema"]), "by name");
        assert!(p.excludes("x", &["/w/x", "/tmp/builds/x/.git"]), "by a prefix of one of its paths");
        assert!(!p.excludes("superx", &["/w/SuperX"]));

        // An empty entry would be a prefix of every path: the list is the
        // wrong shape, said so, and nothing is excluded.
        values.insert(EXCLUDE_PARAM, strings(&["skeema", ""]));
        let r = from_values(&values);
        assert!(r.problems.iter().any(|p| p.name == EXCLUDE_PARAM), "{:?}", r.problems);
        assert!(!r.params.excludes("superx", &["/w/SuperX"]) && !r.params.excludes("skeema", &["/w/skeema"]));
    }

    #[test]
    fn zero_is_allowed_where_it_means_something() {
        let mut values = BTreeMap::new();
        values.insert(CRAWL_SECS_PARAM, number(0));
        values.insert(BLAME_FILES_PARAM, number(0));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.crawl_secs, 0, "the repositories paused");
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
