//! Parser for the shared `--format=json` envelope of `dart analyze` and
//! `dartq analyze` (the two writers are byte-compatible). One envelope per
//! run: `{"version":1,"diagnostics":[...]}`. Any non-envelope stdout lines
//! (progress noise) are ignored.

use serde_json::Value;

/// One diagnostic, canonicalized from the envelope shape:
/// `diagnostics[].{code, severity, problemMessage,
/// location.{file, range.start.{offset,line,column}, range.end.offset}}`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Diag {
    pub severity: String,
    /// Lowercase snake_case code as both tools emit it; empty when the
    /// writer emits a codeless row.
    pub code: String,
    /// Absolute file path exactly as reported; callers relativize it.
    pub file: String,
    /// 1-based line and column of the range start.
    pub line: u32,
    pub column: u32,
    /// `range.end.offset - range.start.offset`.
    pub length: u64,
    pub message: String,
}

/// Extracts every diagnostic from every envelope line of a run's stdout.
pub fn parse_output(stdout: &str) -> Vec<Diag> {
    let mut diags = Vec::new();
    for line in stdout.lines() {
        if let Some(envelope) = parse_envelope(line) {
            diags.extend(envelope);
        }
    }
    diags
}

/// Parses one envelope line; `None` for anything else on the stream.
pub fn parse_envelope(line: &str) -> Option<Vec<Diag>> {
    let value: Value = serde_json::from_str(line).ok()?;
    let items = value.get("diagnostics")?.as_array()?;
    let mut diags = Vec::with_capacity(items.len());
    for item in items {
        let Some(diag) = parse_diag(item) else {
            continue; // a malformed row cannot be canonicalized
        };
        diags.push(diag);
    }
    Some(diags)
}

fn parse_diag(item: &Value) -> Option<Diag> {
    let severity = item.get("severity")?.as_str()?.to_string();
    let location = item.get("location")?;
    let file = location.get("file")?.as_str()?.to_string();
    let range = location.get("range")?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()?;
    let column = start.get("column")?.as_u64()?;
    let start_offset = start.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let end_offset = range
        .get("end")
        .and_then(|end| end.get("offset"))
        .and_then(Value::as_u64)
        .unwrap_or(start_offset);
    Some(Diag {
        severity,
        code: item
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        file,
        line: u32::try_from(line).ok()?,
        column: u32::try_from(column).ok()?,
        length: end_offset.saturating_sub(start_offset),
        message: item
            .get("problemMessage")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORACLE_LINE: &str = r#"{"version":1,"diagnostics":[{"code":"undefined_identifier","severity":"ERROR","type":"COMPILE_TIME_ERROR","location":{"file":"/tmp/pkg/lib/test.dart","range":{"start":{"offset":30,"line":3,"column":9},"end":{"offset":40,"line":3,"column":19}}},"problemMessage":"Undefined name 'undeclared'.","correctionMessage":"Try correcting the name.","documentation":"https://dart.dev/diagnostics/undefined_identifier"}]}"#;

    const DARTQ_LINE: &str = r#"{"version":1,"diagnostics":[{"code":"undefined_identifier","severity":"ERROR","type":"COMPILE_TIME_ERROR","location":{"file":"/tmp/pkg/lib/test.dart","range":{"start":{"offset":30,"line":3,"column":9},"end":{"offset":40,"line":3,"column":19}}},"problemMessage":"Undefined name 'undeclared'.","documentation":"https://dart.dev/diagnostics/undefined_identifier"}]}"#;

    #[test]
    fn oracle_envelope_shape_parses() {
        let diags = parse_envelope(ORACLE_LINE).expect("parses");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "undefined_identifier");
        assert_eq!(diags[0].severity, "ERROR");
        assert_eq!(diags[0].file, "/tmp/pkg/lib/test.dart");
        assert_eq!(diags[0].line, 3);
        assert_eq!(diags[0].column, 9);
        assert_eq!(diags[0].length, 10);
        assert_eq!(diags[0].message, "Undefined name 'undeclared'.");
    }

    #[test]
    fn dartq_envelope_shape_parses() {
        // Same writer contract, no correctionMessage field.
        assert_eq!(parse_envelope(DARTQ_LINE), parse_envelope(ORACLE_LINE));
    }

    #[test]
    fn noise_lines_are_ignored() {
        let stdout = "Analyzing /tmp/pkg...\nAnalyzing done (1.2s)\n";
        assert!(parse_output(stdout).is_empty());
        assert!(parse_envelope("not json").is_none());
    }

    #[test]
    fn zero_length_range_and_missing_end_offset() {
        let line = r#"{"version":1,"diagnostics":[{"code":"x","severity":"INFO","location":{"file":"/a.dart","range":{"start":{"offset":4,"line":1,"column":5},"end":{"offset":4,"line":1,"column":5}}},"problemMessage":"m"}]}"#;
        let diags = parse_envelope(line).expect("parses");
        assert_eq!(diags[0].length, 0);
        let line = r#"{"version":1,"diagnostics":[{"code":"x","severity":"INFO","location":{"file":"/a.dart","range":{"start":{"offset":4,"line":1,"column":5}}},"problemMessage":"m"}]}"#;
        assert_eq!(parse_envelope(line).expect("parses")[0].length, 0);
    }

    #[test]
    fn rows_without_location_or_severity_are_skipped() {
        let line = r#"{"version":1,"diagnostics":[{"problemMessage":"no location"},{"code":"x","severity":"INFO","location":{"file":"/a.dart","range":{"start":{"offset":0,"line":1,"column":1},"end":{"offset":1,"line":1,"column":2}}},"problemMessage":"ok"}]}"#;
        let diags = parse_envelope(line).expect("parses");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "ok");
    }

    #[test]
    fn multiple_envelopes_merge() {
        let stdout = format!("{ORACLE_LINE}\n{DARTQ_LINE}\n");
        assert_eq!(parse_output(&stdout).len(), 2);
    }
}
