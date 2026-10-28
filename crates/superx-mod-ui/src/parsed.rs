//! A captured line, parsed for the page (#543). Capture keeps each line as
//! its text alone since #541: a parsed copy nested as deep as an agent's
//! JSON stopped capture for a day. The session view, the live stream and
//! the export parse a row's text here, in Rust, as they read it. A row
//! written before #541 carries its parsed copy, which is read as it is.

use std::collections::HashMap;

use serde_json::Value as Json;
use superx_kernel::types::{RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Kernel, MessageRecord, Result};

/// `rows`, each message with its parsed form: the copy the row carries, or
/// its text, `raw_text`, parsed. A text that does not parse leaves the row
/// as it is, its readable `content` still there.
///
/// # Errors
///
/// The engine's, or a read that got no answer, twice.
pub(crate) async fn messages(kernel: &Kernel, mut rows: Vec<MessageRecord>) -> Result<Vec<MessageRecord>> {
    let ids: Vec<RecordId> = rows.iter().filter(|m| m.raw.is_none()).map(|m| m.id.clone()).collect();
    if ids.is_empty() {
        return Ok(rows);
    }
    let texts: Vec<Value> = crate::answered("message texts", || {
        let ids = ids.clone();
        async move { Ok(kernel.db().query("SELECT id, raw_text FROM $ids").bind(("ids", ids)).await?.take(0)?) }
    })
    .await?;
    // By the row's UUID: a map keyed by a record id is one clippy refuses
    // (`mutable_key_type`).
    let text: HashMap<Uuid, String> = texts
        .into_iter()
        .filter_map(|row| match row {
            Value::Object(o) => match (o.get("id"), o.get("raw_text")) {
                (Some(Value::RecordId(id)), Some(Value::String(t))) => uuid_of(id).map(|id| (id, t.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    for m in rows.iter_mut().filter(|m| m.raw.is_none()) {
        let json = uuid_of(&m.id).and_then(|id| text.get(&id)).and_then(|t| serde_json::from_str::<Json>(t).ok());
        if let Some(json) = json {
            m.raw = Some(superx_kernel::message::line_to_object(&json));
        }
    }
    Ok(rows)
}

/// A row id's UUID: the kernel sets every id as one (§11).
fn uuid_of(id: &RecordId) -> Option<Uuid> {
    match &id.key {
        RecordIdKey::Uuid(u) => Some(*u),
        _ => None,
    }
}

/// A transcript line's parsed form, from its payload: the copy capture kept
/// from #471 to #541, or its text parsed. `None` for a line kept by its
/// kind alone, before #471, and for a text that does not parse.
pub(crate) fn line(payload: &Json) -> Option<Json> {
    payload
        .get("line")
        .filter(|l| !l.is_null())
        .cloned()
        .or_else(|| payload.get("raw_text").and_then(Json::as_str).and_then(|t| serde_json::from_str(t).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_line_is_its_kept_copy_or_its_text_parsed() {
        let kept = json!({ "kind": "mode", "line": { "type": "mode", "mode": "plan" } });
        assert_eq!(line(&kept), Some(json!({ "type": "mode", "mode": "plan" })));
        let text = json!({ "kind": "mode", "raw_text": r#"{"type":"mode","mode":"plan"}"# });
        assert_eq!(line(&text), Some(json!({ "type": "mode", "mode": "plan" })));
        assert_eq!(line(&json!({ "kind": "mode" })), None, "kept by its kind alone");
        assert_eq!(line(&json!({ "kind": "mode", "raw_text": "not json {" })), None, "a text that does not parse");
    }
}
