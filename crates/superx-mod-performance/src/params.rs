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

/// How often the reader reads the lines captured since its last pass, in
/// seconds; 0 pauses it.
pub const READ_SECS_PARAM: &str = "attr_performance_read_secs";
const DEFAULT_READ_SECS: u64 = 15; // skill-allow: §9-const — bootstrap fallback, overridable by attr_performance_read_secs

/// The parameters in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub port: u16,
    pub read_secs: u64,
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
pub const NAMES: [&str; 2] = [PORT_PARAM, READ_SECS_PARAM];

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
    let params = Params { port: r.port(PORT_PARAM, DEFAULT_PORT), read_secs: r.count(READ_SECS_PARAM, DEFAULT_READ_SECS) };
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

    /// A whole number, 0 or more.
    fn count(&mut self, name: &'static str, fallback: u64) -> u64 {
        self.read(
            name,
            "a whole number, 0 or more",
            fallback,
            |v| match v {
                Value::Number(n) => n.to_int().and_then(|i| u64::try_from(i).ok()),
                _ => None,
            },
            u64::to_string,
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

    #[test]
    fn nothing_set_is_every_fallback() {
        let r = from_values(&BTreeMap::new());
        assert!(r.problems.is_empty());
        assert_eq!(r.settings.len(), NAMES.len(), "every parameter is listed");
        assert!(r.settings.iter().all(|s| !s.set));
        assert_eq!(r.params.port, 5153);
        assert_eq!(r.params.read_secs, 15);
        let names: Vec<&str> = r.settings.iter().map(|s| s.name).collect();
        assert_eq!(names, NAMES, "listed in order");
    }

    #[test]
    fn a_set_value_is_the_one_in_force() {
        let mut values = BTreeMap::new();
        values.insert(PORT_PARAM, number(6_000));
        values.insert(READ_SECS_PARAM, number(0));
        let r = from_values(&values);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.params.port, 6_000);
        assert_eq!(r.params.read_secs, 0, "0 pauses the reader");
        let port = r.settings.iter().find(|s| s.name == PORT_PARAM).expect("listed");
        assert!(port.set && port.value == "6000");
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_reported_and_the_fallback_stands_in() {
        for (wrong, found) in [(number(70_000), "70000"), (number(0), "0"), (Value::String("5153".to_string()), "\"5153\"")] {
            let mut values = BTreeMap::new();
            values.insert(PORT_PARAM, wrong);
            let r = from_values(&values);
            assert_eq!(r.params.port, 5153, "{found} is not a port");
            let problem = r.problems.iter().find(|p| p.name == PORT_PARAM).expect("reported");
            assert_eq!(problem.found, found);
            assert!(r.settings.iter().all(|s| !s.set), "a refused value is not in force");
        }
        for (wrong, found) in [(number(-1), "-1"), (Value::String("15".to_string()), "\"15\"")] {
            let mut values = BTreeMap::new();
            values.insert(READ_SECS_PARAM, wrong);
            let r = from_values(&values);
            assert_eq!(r.params.read_secs, 15, "{found} is not a whole number of seconds");
            let problem = r.problems.iter().find(|p| p.name == READ_SECS_PARAM).expect("reported");
            assert_eq!(problem.found, found);
        }
    }
}
