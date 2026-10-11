//! Structural checks shared by the per-domain validators. Private to the
//! crate: every domain module expresses its contract with these, and
//! [`crate::SchemaViolation`] is the only error surface.

use serde_json::{Map, Value};

use crate::SchemaViolation;

pub(crate) fn violation(
    path: impl Into<String>,
    expected: impl Into<String>,
    found: impl Into<String>,
) -> SchemaViolation {
    SchemaViolation {
        path: path.into(),
        expected: expected.into(),
        found: found.into(),
    }
}

/// `"" + "key"` is `"key"`, not `".key"`.
pub(crate) fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// Names containers by kind (an error message never needs their contents)
/// and renders scalars verbatim (`"semanticx"`, `2`, `1.0`, `null`).
pub(crate) fn describe(value: &Value) -> String {
    match value {
        Value::Object(_) => "an object".to_owned(),
        Value::Array(_) => "an array".to_owned(),
        other => other.to_string(),
    }
}

pub(crate) fn object<'v>(
    value: &'v Value,
    path: &str,
) -> Result<&'v Map<String, Value>, SchemaViolation> {
    value
        .as_object()
        .ok_or_else(|| violation(path, "an object", describe(value)))
}

pub(crate) fn array<'v>(value: &'v Value, path: &str) -> Result<&'v [Value], SchemaViolation> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| violation(path, "an array", describe(value)))
}

pub(crate) fn string<'v>(value: &'v Value, path: &str) -> Result<&'v str, SchemaViolation> {
    value
        .as_str()
        .ok_or_else(|| violation(path, "a string", describe(value)))
}

pub(crate) fn boolean(value: &Value, path: &str) -> Result<bool, SchemaViolation> {
    value
        .as_bool()
        .ok_or_else(|| violation(path, "a boolean", describe(value)))
}

pub(crate) fn integer(value: &Value, path: &str) -> Result<u64, SchemaViolation> {
    value
        .as_u64()
        .ok_or_else(|| violation(path, "an integer", describe(value)))
}

/// A non-negative integer: every count, offset, and length in the published
/// shapes. JSON has no negative spelling for `u64`, so this is [`integer`]
/// with the contract spelled out at the call site.
pub(crate) fn non_negative(value: &Value, path: &str) -> Result<u64, SchemaViolation> {
    integer(value, path)
}

/// A 1-based integer: line and column positions.
pub(crate) fn one_based(value: &Value, path: &str) -> Result<u64, SchemaViolation> {
    let n = integer(value, path)?;
    if n >= 1 {
        Ok(n)
    } else {
        Err(violation(path, "an integer >= 1", n.to_string()))
    }
}

/// A required field that may legitimately be `null`: the producers spell
/// "no value" as JSON null on the total columns (`container` of a top-level
/// declaration row, `max_rss_bytes` without getrusage) — see the §15.8.5
/// omission note in the crate docs.
pub(crate) fn string_or_null(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    if value.is_null() {
        return Ok(());
    }
    string(value, path).map(|_| ())
}

/// Same for an integer column that may be `null` (`max_rss_bytes`).
pub(crate) fn non_negative_or_null(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    if value.is_null() {
        return Ok(());
    }
    non_negative(value, path).map(|_| ())
}

/// A §15.8.5 `[startOffset, endOffset)` range pair — the range spelling
/// faces designed under the token-economics rule adopt (older faces keep
/// their pinned `offset`+`length` columns).
pub(crate) fn range_pair(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let pair = array(value, path)?;
    if pair.len() != 2 {
        return Err(violation(
            path,
            "a two-element [start, end) array",
            describe(value),
        ));
    }
    non_negative(&pair[0], &join(path, "start"))?;
    non_negative(&pair[1], &join(path, "end")).map(|_| ())
}

pub(crate) fn field<'v>(
    obj: &'v Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'v Value, SchemaViolation> {
    obj.get(key)
        .ok_or_else(|| violation(join(path, key), "present", "missing"))
}

pub(crate) fn rows<'v>(
    root: &'v Map<String, Value>,
    key: &str,
) -> Result<impl Iterator<Item = (usize, &'v Value)>, SchemaViolation> {
    Ok(array(field(root, key, "")?, key)?.iter().enumerate())
}

pub(crate) fn enum_spelling(
    value: &Value,
    path: &str,
    allowed: &[&str],
) -> Result<(), SchemaViolation> {
    let spelling = string(value, path)?;
    let expected = allowed
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(" or ");
    if allowed.contains(&spelling) {
        Ok(())
    } else {
        Err(violation(path, expected, describe(value)))
    }
}

/// The §15.7 stable path form `label|name` — pinned by containment; the
/// label grammar is the producer's business.
pub(crate) fn symbol_ref(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let r#ref = string(value, path)?;
    if r#ref.contains('|') {
        Ok(())
    } else {
        Err(violation(
            path,
            "a `label|name` path reference",
            describe(value),
        ))
    }
}

/// Rejects undeclared fields: the enforcement behind "a new field forces a
/// documented schema change" (crate docs, versioning discipline).
pub(crate) fn exact_fields(
    obj: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    path: &str,
) -> Result<(), SchemaViolation> {
    for key in obj.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            let declared = if optional.is_empty() {
                format!("one of: {}", required.join(", "))
            } else {
                format!(
                    "one of: {}; optional: {}",
                    required.join(", "),
                    optional.join(", ")
                )
            };
            return Err(violation(join(path, key), declared, "an undeclared field"));
        }
    }
    Ok(())
}

/// Tier exactness (crate docs, "Verbosity tiers"): a field the payload's
/// tier must omit. The `expected` text names the tier so a CI failure reads
/// as the contract, not a type error.
pub(crate) fn absent(
    obj: &Map<String, Value>,
    key: &str,
    path: &str,
    tier: crate::Verbosity,
) -> Result<(), SchemaViolation> {
    if let Some(value) = obj.get(key) {
        Err(violation(
            join(path, key),
            format!("omitted at the {tier} tier"),
            describe(value),
        ))
    } else {
        Ok(())
    }
}

/// Pins the version field of a payload to its constant.
pub(crate) fn pinned_version(
    obj: &Map<String, Value>,
    key: &str,
    version: u32,
) -> Result<(), SchemaViolation> {
    let value = field(obj, key, "")?;
    if value.as_u64() == Some(u64::from(version)) {
        Ok(())
    } else {
        Err(violation(
            key,
            format!("the integer {version}"),
            describe(value),
        ))
    }
}

/// The §15.8 session fields shared by the query and impact faces: a compact
/// symbol table (`short id → label|name`) and the delta cursor echo. Both
/// absent-tolerant — no current emitter sends them; see the crate docs,
/// "Token economics".
pub(crate) fn session_fields(obj: &Map<String, Value>) -> Result<(), SchemaViolation> {
    if let Some(table) = obj.get("symbols") {
        let path = "symbols";
        let table = object(table, path)?;
        for (id, entry) in table {
            symbol_ref(entry, &join(path, id))?;
        }
    }
    if let Some(cursor) = obj.get("since") {
        non_negative(cursor, "since")?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn schema_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value["properties"]
        .as_object()
        .expect("properties object")
        .keys()
        .cloned()
        .collect();
    keys.sort_unstable();
    keys
}

#[cfg(test)]
pub(crate) fn const_names(consts: &[&str]) -> Vec<String> {
    let mut names = consts.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    names.sort_unstable();
    names
}
