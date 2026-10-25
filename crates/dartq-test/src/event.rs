//! Reporter events of `package:test`'s JSON protocol, kept as the runner
//! wrote them.
//!
//! dartq merges the event streams of several runner processes (and of
//! replayed cache entries) into one stream shaped exactly like a single
//! `dart test --reporter json` run. Events are therefore held as an
//! order-preserving JSON value ([`Json`]): forwarding one re-serializes it
//! field for field in the runner's own order, with only the ids (which
//! must be unique across the merged stream) and the `time` stamps (which
//! are relative to the merged run) rewritten.

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A JSON value whose objects keep their keys in source order.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value under `key` of an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(fields) => fields.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    /// The value under `key` of an object, mutably.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Self::Object(fields) => fields.iter_mut().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    /// Set `key` of an object, in place when present, else appended.
    pub fn set(&mut self, key: &str, value: Json) {
        if let Some(slot) = self.get_mut(key) {
            *slot = value;
        } else if let Self::Object(fields) = self {
            fields.push((key.to_owned(), value));
        }
    }

    /// Remove `key` from an object.
    pub fn remove(&mut self, key: &str) {
        if let Self::Object(fields) = self {
            fields.retain(|(name, _)| name != key);
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(number) => number.as_u64(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// `path` (keys from the root) as a string.
    #[must_use]
    pub fn str_at(&self, path: &[&str]) -> Option<&str> {
        path.iter().try_fold(self, |value, key| value.get(key))?.as_str()
    }

    /// `path` as an unsigned integer.
    #[must_use]
    pub fn u64_at(&self, path: &[&str]) -> Option<u64> {
        path.iter().try_fold(self, |value, key| value.get(key))?.as_u64()
    }

    /// The event's `type`.
    #[must_use]
    pub fn kind(&self) -> &str {
        self.str_at(&["type"]).unwrap_or("")
    }

    /// One compact JSON line.
    #[must_use]
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("a JSON value serializes")
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        Self::Number(value.into())
    }
}

/// Parse one runner stdout line as a reporter event: a JSON object with a
/// string `type`. Anything else is a test's raw output.
#[must_use]
pub fn parse(line: &str) -> Option<Json> {
    if !line.starts_with('{') {
        return None;
    }
    let value: Json = serde_json::from_str(line).ok()?;
    value.get("type")?.as_str()?;
    Some(value)
}

/// Rewrite every id an event carries through `map` (suite, group and test
/// ids share one id space, as in the runner).
pub fn remap(event: &mut Json, map: &mut impl FnMut(u64) -> u64) {
    let mut id = |value: &mut Json| {
        if let Some(old) = value.as_u64() {
            *value = map(old).into();
        }
    };
    for key in ["testID", "suiteID"] {
        if let Some(value) = event.get_mut(key) {
            id(value);
        }
    }
    for (object, keys) in [
        ("suite", &["id"][..]),
        ("test", &["id", "suiteID"][..]),
        ("group", &["id", "suiteID", "parentID"][..]),
    ] {
        let Some(inner) = event.get_mut(object) else {
            continue;
        };
        for key in keys {
            if let Some(value) = inner.get_mut(key) {
                id(value);
            }
        }
        if let Some(Json::Array(ids)) = inner.get_mut("groupIDs") {
            ids.iter_mut().for_each(&mut id);
        }
    }
}

/// Replace every occurrence of `from` with `to` in the event's strings
/// (keys stay). Several pairs apply in order, each to the result of the
/// previous one.
pub fn replace_all(event: &mut Json, pairs: &[(&str, &str)]) {
    match event {
        Json::String(text) => {
            for (from, to) in pairs {
                if text.contains(from) {
                    *text = text.replace(from, to);
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(|item| replace_all(item, pairs)),
        Json::Object(fields) => fields.iter_mut().for_each(|(_, value)| replace_all(value, pairs)),
        Json::Null | Json::Bool(_) | Json::Number(_) => {}
    }
}

/// The suite id an event names directly (`suite`, `testStart`, `group`).
#[must_use]
pub fn suite_of(event: &Json) -> Option<u64> {
    event
        .u64_at(&["suite", "id"])
        .or_else(|| event.u64_at(&["test", "suiteID"]))
        .or_else(|| event.u64_at(&["group", "suiteID"]))
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Number(number) => number.serialize(serializer),
            Self::String(text) => serializer.serialize_str(text),
            Self::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Json, E> {
        Ok(serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut fields = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            fields.push((key, value));
        }
        Ok(Json::Object(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Field order, nesting, nulls and escapes survive a round trip.
    #[test]
    fn events_round_trip_in_runner_order() {
        let line = r#"{"test":{"id":3,"name":"a \"b\"\n","suiteID":2,"groupIDs":[1,2],"metadata":{"skip":false,"skipReason":null},"line":null},"type":"testStart","time":7}"#;
        let event = parse(line).expect("event");
        assert_eq!(event.to_line(), line);
        assert_eq!(event.kind(), "testStart");
        assert_eq!(suite_of(&event), Some(2));
        assert!(parse("hello from a test").is_none());
        assert!(parse("{\"no\":\"type\"}").is_none());
        assert!(parse("[1]").is_none());
    }

    /// Every id field is rewritten; nothing else moves.
    #[test]
    fn remap_rewrites_every_id() {
        let mut group = parse(r#"{"group":{"id":5,"suiteID":2,"parentID":4,"name":"g"},"type":"group"}"#).expect("event");
        let mut test = parse(r#"{"test":{"id":6,"suiteID":2,"groupIDs":[4,5]},"type":"testStart"}"#).expect("event");
        let mut done = parse(r#"{"testID":6,"result":"success","type":"testDone"}"#).expect("event");
        let mut root = parse(r#"{"group":{"id":4,"suiteID":2,"parentID":null},"type":"group"}"#).expect("event");
        let mut shift = |id: u64| id + 100;
        for event in [&mut group, &mut test, &mut done, &mut root] {
            remap(event, &mut shift);
        }
        assert_eq!(group.to_line(), r#"{"group":{"id":105,"suiteID":102,"parentID":104,"name":"g"},"type":"group"}"#);
        assert_eq!(test.to_line(), r#"{"test":{"id":106,"suiteID":102,"groupIDs":[104,105]},"type":"testStart"}"#);
        assert_eq!(done.to_line(), r#"{"testID":106,"result":"success","type":"testDone"}"#);
        assert_eq!(root.to_line(), r#"{"group":{"id":104,"suiteID":102,"parentID":null},"type":"group"}"#);
    }

    #[test]
    fn set_and_remove_keep_order() {
        let mut event = parse(r#"{"a":1,"type":"x","time":3}"#).expect("event");
        event.set("time", 9u64.into());
        event.set("extra", Json::Null);
        event.remove("a");
        assert_eq!(event.to_line(), r#"{"type":"x","time":9,"extra":null}"#);
    }
}
