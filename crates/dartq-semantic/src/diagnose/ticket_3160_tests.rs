//! #3160 `invalid_null_aware_operator` pins: the dead `?.`/`?[`/`?..`/
//! `...?`/`?`-marker family — a strictly non-nullable receiver (or the
//! unconditional static-qualifier face) makes the operator dead, and the
//! short-circuit climb swaps the message when an earlier operator in the
//! same chain already shorted the receiver (upstream `error_verifier.dart`
//! `_checkForUnnecessaryNullAware` :7668-7752 and the `_NullAwareKind`
//! table :8804-8850). Every expectation is a verbatim oracle 3.13.7 row.

use super::super::test_rig::report_ops_bare;

/// The `invalid_null_aware_operator` rows of `report_ops_bare`, as
/// (offset, length, message) triples — the message discriminates the
/// shared-name variants (`_after_short_circuit`, `_element`, the map-entry
/// key/value faces) the oracle routes through one code name. The bare rig
/// keeps the fixture's own offsets (no prelude shift); every fixture
/// declares the members it names, so the only rows under test are the
/// null-aware family.
fn rows(source: &str) -> Vec<(u32, u32, String)> {
    report_ops_bare(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// A nullable receiver is exactly what `?.` exists for — silent.
#[test]
fn nullable_receiver_quiet() {
    let source = "f(String? s) {\n  s?.length;\n}\n";
    assert_eq!(rows(source), Vec::<(u32, u32, String)>::new());
}

/// A non-nullable receiver's `?.` reports at the operator (oracle len 2).
#[test]
fn non_nullable_access_reports() {
    let source = "f(String s) {\n  s?.length;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            15,
            2,
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
                .to_string()
        )]
    );
}

/// A class name in the receiver slot is the static-qualifier face: `C?.x`
/// reports unconditionally — no type check runs (upstream
/// `visitReceiverPropertyExtraction`'s `StaticQualifier` arm :2083-2090).
#[test]
fn static_qualifier_reports_unconditionally() {
    let source = "class C {\n  static int x = 0;\n}\nf() {\n  C?.x;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            41,
            2,
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
                .to_string()
        )]
    );
}

/// The second `?.` of a shorted chain takes the `after_short_circuit`
/// message — its receiver is non-null only because the first `?.` shorted
/// it (`previousShortCircuitingOperator` :7679-7697).
#[test]
fn short_circuit_second_operator_reports_variant() {
    let source = "f(String? s) {\n  s?.length?.isEven;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            26,
            2,
            "The receiver can't be 'null' because of short-circuiting, so the null-aware operator '?.' can't be used."
                .to_string()
        )]
    );
}

/// `x?[0]` anchors the `?[` pair — the oracle's `atOffset` runs the `?`
/// token's start through the `[`'s end (len 2).
#[test]
fn index_operator_reports_bracket_span() {
    let source = "f(List<int> x) {\n  x?[0];\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            20,
            2,
            "The receiver can't be null, so the null-aware operator '?[' is unnecessary."
                .to_string()
        )]
    );
}

/// `x?..[0]`'s cascade section reports the `?..` operator (len 3) — the
/// `?` rode the `?..` token, which already spans `?` through `..`.
#[test]
fn cascade_index_reports_cascade_operator() {
    let source = "f(List<int> x) {\n  x?..[0];\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            20,
            3,
            "The receiver can't be null, so the null-aware operator '?..' is unnecessary."
                .to_string()
        )]
    );
}

/// `[...?x]`'s null-aware spread reports the whole `...?` operator token
/// (len 4) against the operand's static type.
#[test]
fn null_aware_spread_reports() {
    let source = "f(List<int> x) {\n  [...?x];\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            20,
            4,
            "The receiver can't be null, so the null-aware operator '?...' is unnecessary."
                .to_string()
        )]
    );
}

/// A nullable spread operand keeps `...?` legal — silent.
#[test]
fn nullable_spread_quiet() {
    let source = "f(List<int>? x) {\n  [...?x];\n}\n";
    assert_eq!(rows(source), Vec::<(u32, u32, String)>::new());
}

/// `[0, ?c]`'s null-aware element reports at the `?` marker (len 1) with
/// the element-specific message — the `_element` kind is outside the
/// short-circuit family.
#[test]
fn null_aware_element_reports() {
    let source = "const c = \"\";\nconst l = [0, ?c];\n";
    assert_eq!(
        rows(source),
        vec![(
            28,
            1,
            "The element can't be null, so the null-aware operator '?' is unnecessary.".to_string()
        )]
    );
}

/// `{?k: v}` reports at the key's `?` marker with the map-entry-key
/// message.
#[test]
fn map_entry_key_reports() {
    let source = "const c = 0;\nconst m = {?0: c};\n";
    assert_eq!(
        rows(source),
        vec![(
            24,
            1,
            "The map entry key can't be null, so the null-aware operator '?' is unnecessary."
                .to_string()
        )]
    );
}

/// `{k: ?v}` reports at the value's `?` marker with the map-entry-value
/// message.
#[test]
fn map_entry_value_reports() {
    let source = "const c = 0;\nconst m = {0: ?c};\n";
    assert_eq!(
        rows(source),
        vec![(
            27,
            1,
            "The map entry value can't be null, so the null-aware operator '?' is unnecessary."
                .to_string()
        )]
    );
}

/// A nullable element operand keeps `?e` legal — silent.
#[test]
fn nullable_element_quiet() {
    let source = "f(int? c) {\n  [0, ?c];\n}\n";
    assert_eq!(rows(source), Vec::<(u32, u32, String)>::new());
}
