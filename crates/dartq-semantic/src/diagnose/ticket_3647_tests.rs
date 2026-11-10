//! Ticket #3647 pins: `invalid_field_name` — the record field-name faces
//! (private `_x`, `$n` positional collision, `Object`-member collision)
//! had zero emission sites: 188 census misses, no extras.
//!
//! # The wrong fact
//!
//! Upstream runs `reportInvalidFieldNames` on every `RecordTypeAnnotation`
//! (record_type_annotation_resolver.dart:65-98) and
//! `_reportInvalidFieldNames` on every `RecordLiteral`
//! (record_literal_resolver.dart:101-136); the engine kept the field
//! names (`record_ty_stack`'s `names`, `record_literal_names`) but never
//! walked them for the three forbidden shapes.
//!
//! # The contract
//!
//! - Type annotation: `_name` reports private — except a positional `_`
//!   under wildcard-variables; `$n` reports positional iff `n-1` indexes
//!   a positional sibling that is not the field itself; `hashCode` /
//!   `runtimeType` / `noSuchMethod` / `toString` report fromObject.
//! - Record literal: every named field's label reports on its own span —
//!   `_` always (no wildcard exemption), `$n` iff `n-1` < positional
//!   count, the four `Object` names always.
//! - Valid names stay silent (negative control).
//!
//! Oracle provenance: `dart analyze` — pkg/analyzer
//! `record_type_annotation_resolver.dart` / `record_literal_resolver.dart`,
//! `diagnostic.g.dart` `invalid_field_name*` uniqueName rows.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_wildcards, rows};

/// The absolute offset of `needle`'s `n`th occurrence (0-based) in
/// `fixture` as the rig analyzes it: `report_ops` prefixes
/// [`CORE_PRELUDE`], so every fixture-local offset shifts by the
/// prelude's length.
fn at(fixture: &str, needle: &str, n: usize) -> u32 {
    let hit = fixture
        .match_indices(needle)
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{needle:?} occurrence {n} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `invalid_field_name` rows as `(offset, length, message)`, ascending.
fn ifn_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut at: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_field_name")
        .map(|d| {
            (
                d.range.offset,
                d.range.end() - d.range.offset,
                d.message.to_string(),
            )
        })
        .collect();
    at.sort_unstable();
    at
}

const PRIVATE: &str = "Record field names can't be private.";
const POSITIONAL: &str = "Record field names can't be a dollar sign followed by an integer when the integer is the index of a positional field.";
const FROM_OBJECT: &str = "Record field names can't be the same as a member from 'Object'.";

// —— record literal: the four `Object` members each report at their own
// —— label span (`record_expressions_A03`'s co19 shape).
#[test]
fn literal_object_member_names_report() {
    let source =
        "void f() { var r = (hashCode: 1, noSuchMethod: 2, runtimeType: 3, toString: 4); }\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![
            (at(source, "hashCode", 0), 8, FROM_OBJECT.to_string()),
            (at(source, "noSuchMethod", 0), 12, FROM_OBJECT.to_string()),
            (at(source, "runtimeType", 0), 11, FROM_OBJECT.to_string()),
            (at(source, "toString", 0), 8, FROM_OBJECT.to_string()),
        ],
        "every Object-member label reports at its own span: {:?}",
        rows(&r)
    );
}

// —— record literal: a `_`-leading label reports private (no wildcard
// —— exemption — upstream's literal check lacks the positional-`_` arm).
#[test]
fn literal_private_name_reports() {
    let source = "void f() { var r = (_hidden: 1, x: 2); }\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![(at(source, "_hidden", 0), 7, PRIVATE.to_string())],
        "the `_` label reports at its own span: {:?}",
        rows(&r)
    );
}

// —— record literal: `$1` names positional index 0 — one positional
// —— field present, so `0 < 1` reports positional
// —— (`test_positional_named_conflict`'s oracle shape).
#[test]
fn literal_positional_index_collision_reports() {
    let source = "void f() { var r = (0, $1: 2); }\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![(at(source, "$1", 0), 2, POSITIONAL.to_string())],
        "`$1` collides with the one positional field: {:?}",
        rows(&r)
    );
}

// —— record literal: `$2` past the one positional field stays silent
// —— (`index < positionalCount` is the upstream gate).
#[test]
fn literal_positional_index_past_count_silent() {
    let source = "void f() { var r = (0, $2: 2); }\n";
    let r = report_ops(source);
    assert!(
        ifn_rows(&r).is_empty(),
        "`$2` is not a positional index here: {:?}",
        rows(&r)
    );
}

// —— record literal: a named field before the positional still counts
// —— the positional run correctly (`(n: 1, 2)` — source order is not the
// —— partition; the window's own `ident :` prefix marks the named slots).
#[test]
fn literal_named_before_positional() {
    let source = "void f() { var r = ($1: 1, 2); }\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![(at(source, "$1", 0), 2, POSITIONAL.to_string())],
        "the named-first literal's positional count still gates `$1`: {:?}",
        rows(&r)
    );
}

// —— record type annotation: `_`-leading named-region field reports
// —— private at its name token; the sibling literal's own label reports
// —— independently (the oracle emits both — annotation walk and literal
// —— walk are separate upstream checks).
#[test]
fn type_named_private_and_literal_both_report() {
    let source = "(int, {String _hidden}) r = (1, _hidden: \"\");\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![
            (at(source, "_hidden", 0), 7, PRIVATE.to_string()),
            (at(source, "_hidden", 1), 7, PRIVATE.to_string()),
        ],
        "the named-region `_` and the literal's label each report: {:?}",
        rows(&r)
    );
}

// —— record type annotation: a positional field's `_` is legal under
// —— wildcard-variables (upstream `isPositionalWildCard`), but a named
// —— `{…}` `_` still reports.
#[test]
fn type_positional_wildcard_exempt_named_not() {
    let source = "(int _, {int _y}) r = (1, _y: 2);\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![
            (at(source, "_y", 0), 2, PRIVATE.to_string()),
            (at(source, "_y", 1), 2, PRIVATE.to_string()),
        ],
        "positional `_` exempts, named `_` reports: {:?}",
        rows(&r)
    );
}

// —— record type annotation: pre-feature (`wildcard-variables` off) the
// —— positional `_` reports too — `report_ops_wildcards(false)` models
// —— the `// @dart = 3.4` library posture.
#[test]
fn type_positional_private_pre_wildcards() {
    let source = "(int _, String s) r = (1, \"\");\n";
    let r = report_ops_wildcards(source, false);
    assert_eq!(
        ifn_rows(&r),
        vec![(at(source, "_", 0), 1, PRIVATE.to_string())],
        "pre-feature the positional `_` reports: {:?}",
        rows(&r)
    );
}

// —— record type annotation: `$1` on the FIRST positional field is its
// —— own name — upstream exempts `positionalFields.indexOf(field) ==
// —— index`.
#[test]
fn type_positional_self_index_silent() {
    let source = "(int $1, String s) r = (1, \"\");\n";
    let r = report_ops(source);
    assert!(
        ifn_rows(&r).is_empty(),
        "`$1` is field 0's own positional name: {:?}",
        rows(&r)
    );
}

// —— record type annotation: `$2` on the first positional collides with
// —— the second — `index < positionalCount && indexOf != index` fires.
#[test]
fn type_positional_other_index_reports() {
    let source = "(int $2, String s) r = (1, \"\");\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![(at(source, "$2", 0), 2, POSITIONAL.to_string())],
        "`$2` names positional index 1, not itself: {:?}",
        rows(&r)
    );
}

// —— record type annotation: a named-region `$1` collides with
// —— positional field 0 (`indexOf` on a named field is `-1`), while `$3`
// —— is out of range. The literal's `$1` reports likewise (one
// —— positional present).
#[test]
fn type_named_region_positional_index() {
    let source = "(int a, {int $1, int $3}) r = (1, $1: 2, $3: 3);\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![
            (at(source, "$1", 0), 2, POSITIONAL.to_string()),
            (at(source, "$1", 1), 2, POSITIONAL.to_string()),
        ],
        "`$1` collides in annotation and literal, `$3` is out of range: {:?}",
        rows(&r)
    );
}

// —— record type annotation: the four `Object` names report on the
// —— name token (`record_type_annotations_A05`'s co19 shape).
#[test]
fn type_object_member_names_report() {
    let source = "typedef R = ({int hashCode, int noSuchMethod, int runtimeType, int toString});\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![
            (at(source, "hashCode", 0), 8, FROM_OBJECT.to_string()),
            (at(source, "noSuchMethod", 0), 12, FROM_OBJECT.to_string()),
            (at(source, "runtimeType", 0), 11, FROM_OBJECT.to_string()),
            (at(source, "toString", 0), 8, FROM_OBJECT.to_string()),
        ],
        "the named region's Object names all report: {:?}",
        rows(&r)
    );
}

// —— nested record types: the inner record's names and the outer's each
// —— check against their own level (the `(`-walk verifies by arity and
// —— by name-token lexemes).
#[test]
fn nested_record_types_each_check() {
    let source =
        "((int _a, {int hashCode}) x, {int toString}) r = ((1, hashCode: 2), toString: 3);\n";
    let r = report_ops(source);
    assert_eq!(
        ifn_rows(&r),
        vec![
            (at(source, "_a", 0), 2, PRIVATE.to_string()),
            (at(source, "hashCode", 0), 8, FROM_OBJECT.to_string()),
            (at(source, "toString", 0), 8, FROM_OBJECT.to_string()),
            (at(source, "hashCode", 1), 8, FROM_OBJECT.to_string()),
            (at(source, "toString", 1), 8, FROM_OBJECT.to_string()),
        ],
        "inner and outer record names each report: {:?}",
        rows(&r)
    );
}

// —— negative control: a valid record — all faces stay silent.
#[test]
fn valid_field_names_silent() {
    let source = "void f((int a, {String b}) r) { var v = (1, b: \"s\"); }\n";
    let r = report_ops(source);
    assert!(
        ifn_rows(&r).is_empty(),
        "valid field names never report: {:?}",
        rows(&r)
    );
}
