//! Reading the values the kernel keeps whole: a path into an object, a
//! value's text, number or flag, a list, a timestamp an agent wrote, and the
//! size of a value written out as JSON. The kernel keeps each line as its
//! own value types, so the crawler reads them as they are.

use chrono::{DateTime, Utc};
use superx_kernel::types::{Number, Object, Value};

/// The value at `path` inside `v`, through objects.
#[must_use]
pub fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut here = v;
    for key in path {
        match here {
            Value::Object(o) => here = o.get(*key)?,
            _ => return None,
        }
    }
    Some(here)
}

/// The text at `path`.
#[must_use]
pub fn text<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    match at(v, path)? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// The text at `path`, owned, when there is some.
#[must_use]
pub fn some_text(v: &Value, path: &[&str]) -> Option<String> {
    text(v, path).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The whole number at `path`.
#[must_use]
pub fn int(v: &Value, path: &[&str]) -> Option<i64> {
    match at(v, path)? {
        Value::Number(n) => whole(n),
        _ => None,
    }
}

/// A number as a whole number: an integer, or a float with no fraction.
#[must_use]
pub fn whole(n: &Number) -> Option<i64> {
    match n {
        Number::Int(i) => Some(*i),
        Number::Float(f) if f.is_finite() && f.fract() == 0.0 => n.to_int(),
        Number::Float(_) => None,
        Number::Decimal(_) => n.to_int(),
    }
}

/// The number at `path`, as a float.
#[must_use]
pub fn float(v: &Value, path: &[&str]) -> Option<f64> {
    match at(v, path)? {
        Value::Number(n) => n.to_f64(),
        _ => None,
    }
}

/// The flag at `path`.
#[must_use]
pub fn flag(v: &Value, path: &[&str]) -> Option<bool> {
    match at(v, path)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

/// The list at `path`; empty when there is none.
#[must_use]
pub fn list<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    match at(v, path) {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    }
}

/// The object at `path`.
#[must_use]
pub fn object<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Object> {
    match at(v, path)? {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

/// A timestamp an agent wrote as RFC 3339 text, at `path`.
#[must_use]
pub fn time(v: &Value, path: &[&str]) -> Option<DateTime<Utc>> {
    let t = text(v, path)?;
    DateTime::parse_from_rfc3339(t).ok().map(|d| d.with_timezone(&Utc))
}

/// A count of characters, as the database keeps counts.
#[must_use]
pub fn chars(s: &str) -> i64 {
    i64::try_from(s.chars().count()).unwrap_or(i64::MAX)
}

/// A count of lines: the lines of a text, the last one counted whether or
/// not it ends with a newline; none in an empty text.
#[must_use]
pub fn lines(s: &str) -> i64 {
    i64::try_from(s.lines().count()).unwrap_or(i64::MAX)
}

/// The number of bytes `v` takes written out as compact JSON, the way the
/// agent wrote it: strings escaped as JSON escapes them. A float is counted
/// as Rust writes it, which may differ from JSON by a character.
#[must_use]
pub fn json_len(v: &Value) -> i64 {
    i64::try_from(json_bytes(v)).unwrap_or(i64::MAX)
}

fn json_bytes(v: &Value) -> usize {
    match v {
        Value::None | Value::Null => "null".len(),
        Value::Bool(true) => "true".len(),
        Value::Bool(false) => "false".len(),
        Value::Number(Number::Int(i)) => i.to_string().len(),
        Value::Number(Number::Float(f)) => {
            if f.is_finite() {
                f.to_string().len()
            } else {
                "null".len()
            }
        }
        Value::Number(Number::Decimal(d)) => string_bytes(&d.to_string()),
        Value::String(s) => string_bytes(s),
        Value::Array(a) => 2 + a.iter().map(json_bytes).sum::<usize>() + a.len().saturating_sub(1),
        Value::Object(o) => {
            2 + o.iter().map(|(k, v)| string_bytes(k) + 1 + json_bytes(v)).sum::<usize>() + o.len().saturating_sub(1)
        }
        // A line the kernel kept holds only what JSON holds; anything else
        // is counted as the text it would be written as.
        other => string_bytes(&format!("{other:?}")),
    }
}

/// A string written as JSON: quoted, with `"` and `\` escaped, the control
/// characters JSON names short and the rest as `\u00XX`.
fn string_bytes(s: &str) -> usize {
    2 + s
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
            c if (c as u32) < 0x20 => 6,
            c => c.len_utf8(),
        })
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use superx_kernel::message::json_to_object;

    fn v(json: serde_json::Value) -> Value {
        Value::Object(json_to_object(&json))
    }

    #[test]
    fn a_path_reads_through_objects_and_stops_at_anything_else() {
        let line = v(serde_json::json!({ "message": { "usage": { "output_tokens": 12, "tier": "standard" } }, "list": [1] }));
        assert_eq!(int(&line, &["message", "usage", "output_tokens"]), Some(12));
        assert_eq!(text(&line, &["message", "usage", "tier"]), Some("standard"));
        assert_eq!(int(&line, &["message", "usage", "tier"]), None, "text is not a number");
        assert_eq!(at(&line, &["list", "0"]), None, "a list is not walked by key");
        assert_eq!(list(&line, &["list"]).len(), 1);
        assert!(list(&line, &["missing"]).is_empty());
    }

    #[test]
    fn a_whole_number_may_be_written_as_a_float_but_a_fraction_is_not_one() {
        assert_eq!(whole(&Number::Float(3.0)), Some(3));
        assert_eq!(whole(&Number::Float(3.5)), None);
        assert_eq!(whole(&Number::Int(-2)), Some(-2));
    }

    #[test]
    fn a_timestamp_is_read_as_the_agent_wrote_it() {
        let line = v(serde_json::json!({ "timestamp": "2026-10-14T10:52:04.771Z", "bad": "yesterday" }));
        assert_eq!(time(&line, &["timestamp"]).map(|t| t.timestamp_millis()), Some(1_791_975_124_771));
        assert_eq!(time(&line, &["bad"]), None);
    }

    #[test]
    fn json_length_is_the_length_of_the_compact_encoding() {
        for json in [
            serde_json::json!({ "command": "echo \"hi\"\n", "n": 12, "ok": true, "none": null, "list": [1, "two", { "x": [] }] }),
            serde_json::json!({ "unicode": "naïve — ✓", "control": "\u{1}\t" }),
        ] {
            let expected = serde_json::to_string(&json).expect("json").len();
            assert_eq!(json_len(&Value::Object(json_to_object(&json))), i64::try_from(expected).expect("fits"), "{json}");
        }
    }

    #[test]
    fn lines_and_chars_count_as_a_person_counts_them() {
        assert_eq!(lines(""), 0);
        assert_eq!(lines("a"), 1);
        assert_eq!(lines("a\nb\n"), 2);
        assert_eq!(lines("a\n\nb"), 3);
        assert_eq!(chars("naïve"), 5);
    }
}
