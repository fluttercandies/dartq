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

/// Same contract as [`rows`], but the array is optional: absent yields no
/// rows instead of a missing-field violation (the reserved additive arrays'
/// absent-tolerant posture).
pub(crate) fn rows_optional<'v>(
    root: &'v Map<String, Value>,
    key: &str,
) -> Result<Vec<(usize, &'v Value)>, SchemaViolation> {
    match root.get(key) {
        Some(value) => Ok(array(value, key)?.iter().enumerate().collect()),
        None => Ok(Vec::new()),
    }
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

/// The §15.8 economics applied to a face's own path-valued rows (the
/// daemon's full-tier `explain` short-id table): `symbols` maps short ids
/// to FILE paths — the values are the very strings substituted out of the
/// rows, a different value domain than the query/impact table's
/// `label|name` entity references (the split the daemon's session-economics
/// contract pins; §15.8-2's entity-reference table stays the pin above).
/// `since` is the same delta cursor. Both absent-tolerant.
pub(crate) fn path_session_fields(obj: &Map<String, Value>) -> Result<(), SchemaViolation> {
    if let Some(table) = obj.get("symbols") {
        let path = "symbols";
        let table = object(table, path)?;
        for (id, entry) in table {
            string(entry, &join(path, id))?;
        }
    }
    if let Some(cursor) = obj.get("since") {
        non_negative(cursor, "since")?;
    }
    Ok(())
}

/// The §15.6 edit-row floor shared by the fix patch and assists documents:
/// the red-line locator trio (`offset`/`length`/`replacement`) plus the
/// provenance tag (`source` — the row never drops it, §15.8), and the
/// optional compact-tier anchor (`line`/`column` plus the context flanks;
/// empty context strings are omitted, §15.8.5). Callers run their own
/// `exact_fields` first: the fix patch rows carry more optional columns
/// (`rule`/`message`/`priority`/`stale`) than the assists rows.
pub(crate) fn edit_floor(obj: &Map<String, Value>, p: &str) -> Result<(), SchemaViolation> {
    let offset = join(p, "offset");
    non_negative(field(obj, "offset", p)?, &offset)?;
    let length = join(p, "length");
    non_negative(field(obj, "length", p)?, &length)?;
    string(field(obj, "replacement", p)?, &join(p, "replacement"))?;
    string(field(obj, "source", p)?, &join(p, "source"))?;
    if let Some(anchor) = obj.get("anchor") {
        let ap = join(p, "anchor");
        let anchor = object(anchor, &ap)?;
        exact_fields(anchor, ANCHOR, ANCHOR_OPTIONAL, &ap)?;
        let line = join(&ap, "line");
        one_based(field(anchor, "line", &ap)?, &line)?;
        let column = join(&ap, "column");
        one_based(field(anchor, "column", &ap)?, &column)?;
        if let Some(before) = anchor.get("context_before") {
            string(before, &join(&ap, "context_before"))?;
        }
        if let Some(after) = anchor.get("context_after") {
            string(after, &join(&ap, "context_after"))?;
        }
    }
    Ok(())
}

pub(crate) const ANCHOR: &[&str] = &["line", "column"];
pub(crate) const ANCHOR_OPTIONAL: &[&str] = &["context_before", "context_after"];

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
