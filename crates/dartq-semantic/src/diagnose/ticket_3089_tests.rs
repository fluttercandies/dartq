//! The #3089 pins — a POSITIONAL object-pattern field reads the getter its
//! own sub-pattern's variable names (`Circle(var c as num)` reads `c`), so
//! the `undefined_getter` row the oracle emits is no longer dropped.
//!
//! Upstream resolves every object-pattern field's property get through
//! `resolveObjectPatternPropertyGet` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:1669-1715): the
//! field's written name, else `field.pattern.variablePattern?.name`
//! (`:1675-1676`), else `dynamic` — no getter read at all (`:1677-1679`).
//! The fallback unwraps exactly the transparent pattern wrappers — cast,
//! null assert, null check, parentheses
//! (pkg/analyzer/lib/src/dart/ast/ast.dart:6640, :39354, :39708, :40734),
//! each delegating to the shared variable-pattern interface (`:57337`); a
//! list, map, record, constant, or logical-and sub-pattern answers none. The
//! lookup then runs over the head's required type and reports
//! `undefined_getter` at the name token through the read-failure path
//! (`:1681-1697`). The `_fe_analyzer_shared` field loop calls it for every
//! field, positional ones included
//! (pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
//! 1793-1800).
//!
//! The listener's `handle_pattern_field` routes the field's getter from the
//! sub-pattern's variable latch; `finalize_pattern_getters`
//! (`unused_finalization.rs:106-139`) scores it and emits the row. Oracle
//! provenance: `dart analyze` 3.13.1 machine format, every expectation
//! below probed against the real binary first.

use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops};

/// The `undefined_getter` rows of `source`, rendered `@offset: message`
/// with the offset mapped back into `source` (the rig mounts
/// [`CORE_PRELUDE`] ahead of the fixture, so the report's ranges are
/// absolute into the mounted unit).
fn getter_rows(source: &str) -> Vec<String> {
    let report = report_ops(source);
    let base = CORE_PRELUDE.len();
    report
        .diagnostics
        .iter()
        .filter(|diag| diag.code.name == "undefined_getter")
        .map(|diag| format!("@{}: {}", diag.range.offset as usize - base, diag.message))
        .collect()
}

/// `Circle(var c1 as num)` — the cast wrapper is transparent, so the field
/// reads getter `c1` and misses (oracle: `UNDEFINED_GETTER` at the variable
/// token, `The getter 'c1' isn't defined for the type 'Circle'.`).
#[test]
fn positional_field_declared_variable_reports_undefined_getter() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(var c1 as num)) {\n\
                      sink = c1;\n\
                    }\n\
                  }\n";
    let at = source.find("c1").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'c1' isn't defined for the type 'Circle'."
        )],
        "positional field routes the sub-pattern's variable as its getter"
    );
}

/// The un-cast twin: a bare declared variable is likewise the getter name
/// (oracle: `UNDEFINED_GETTER` at `c2`).
#[test]
fn positional_field_bare_declared_variable_reports() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(var c2)) {\n\
                      sink = c2;\n\
                    }\n\
                  }\n";
    let at = source.find("c2").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'c2' isn't defined for the type 'Circle'."
        )],
        "bare positional variable names the getter too"
    );
}

/// A null-check suffix stays transparent (oracle: the row anchors at the
/// variable token `c3`, not at the `?`).
#[test]
fn positional_field_null_check_variable_reports_at_the_variable() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(var c3?)) {\n\
                      sink = c3;\n\
                    }\n\
                  }\n";
    let at = source.find("c3").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'c3' isn't defined for the type 'Circle'."
        )],
        "the null-check wrapper resolves through to the variable"
    );
}

/// Parentheses stay transparent as well (oracle: `UNDEFINED_GETTER` at
/// `c4`).
#[test]
fn positional_field_parenthesized_variable_reports() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle((var c4))) {\n\
                      sink = c4;\n\
                    }\n\
                  }\n";
    let at = source.find("c4").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'c4' isn't defined for the type 'Circle'."
        )],
        "the parenthesis wrapper resolves through to the variable"
    );
}

/// A positional field whose name IS a member of the head resolves and stays
/// silent — the named shorthand (`:var x`) and the positional `var y` alike
/// resolve on `Rect` (oracle: only `POSITIONAL_FIELD_IN_OBJECT_PATTERN`,
/// which is a separate face this pin does not cover).
#[test]
fn resolved_positional_and_named_getters_stay_silent() {
    let source = "class Rect { final num x; final num y; Rect(this.x, this.y); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Rect(:var x, var y)) {\n\
                      sink = x;\n\
                      sink = y;\n\
                    }\n\
                  }\n";
    assert!(
        getter_rows(source).is_empty(),
        "resolved getters never report: {:?}",
        getter_rows(source)
    );
}

/// Per-field names: one resolved positional name and one miss on the same
/// head report exactly the miss (oracle: `UNDEFINED_GETTER` for `nope`
/// only).
#[test]
fn positional_field_names_are_per_field() {
    let source = "class Rect { final num x; final num y; Rect(this.x, this.y); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Rect(var x, var nope)) {\n\
                      sink = x;\n\
                      sink = nope;\n\
                    }\n\
                  }\n";
    let at = source.find("nope").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'nope' isn't defined for the type 'Rect'."
        )],
        "each field carries its own getter name"
    );
}

/// The named shorthand control (`:var bad`) — untouched by the positional
/// routing and still reporting at the variable token (oracle: `8:23` in the
/// probe file, the `bad` token).
#[test]
fn named_shorthand_still_reports() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(:var bad)) {\n\
                      sink = bad;\n\
                    }\n\
                  }\n";
    let at = source.find("bad").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'bad' isn't defined for the type 'Circle'."
        )],
        "the named arm keeps its own routing"
    );
}

/// The named shorthand control that resolves: `Circle(:var r)` is silent
/// (the getter's return types the binder; no row).
#[test]
fn named_shorthand_resolved_is_silent() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(:var r)) {\n\
                      sink = r;\n\
                    }\n\
                  }\n";
    assert_silent(&report_ops(source));
}

/// A LIST sub-pattern never names a getter (oracle: no `UNDEFINED_GETTER`,
/// only `POSITIONAL_FIELD_IN_OBJECT_PATTERN`) — the latch dies with the
/// bracket instead of leaking into the field boundary.
#[test]
fn list_subpattern_variable_is_not_a_getter() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle([var a])) {\n\
                      sink = a;\n\
                    }\n\
                  }\n";
    assert!(
        getter_rows(source).is_empty(),
        "a list element is no field getter: {:?}",
        getter_rows(source)
    );
}

/// A MAP sub-pattern value is no field getter either (oracle: no
/// `UNDEFINED_GETTER`).
#[test]
fn map_subpattern_variable_is_not_a_getter() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle({1: var m})) {\n\
                      sink = m;\n\
                    }\n\
                  }\n";
    assert!(
        getter_rows(source).is_empty(),
        "a map value is no field getter: {:?}",
        getter_rows(source)
    );
}

/// A logical-and sub-pattern reads no getter (oracle: no
/// `UNDEFINED_GETTER` for `Circle(var q && num)`).
#[test]
fn logical_and_subpattern_variable_is_not_a_getter() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(var q && num)) {\n\
                      sink = q;\n\
                    }\n\
                  }\n";
    assert!(
        getter_rows(source).is_empty(),
        "a logical-and operand is no field getter: {:?}",
        getter_rows(source)
    );
}

/// A latch from an EARLIER root pattern must not survive into a later
/// wildcard field (`var y` then `Circle(_)` — the oracle reports no
/// `UNDEFINED_GETTER` at all).
#[test]
fn earlier_root_variable_does_not_name_a_later_field_getter() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case var y) {\n\
                      sink = y;\n\
                    }\n\
                    if (o case Circle(_)) {\n\
                      sink = 0;\n\
                    }\n\
                  }\n";
    assert!(
        getter_rows(source).is_empty(),
        "no cross-root latch leak: {:?}",
        getter_rows(source)
    );
}

/// Within one pattern the latch is consumed per field: a wildcard second
/// field reuses nothing from the first (`Circle(var u, _)` reports `u`
/// once, oracle: a single `UNDEFINED_GETTER` at `u`).
#[test]
fn wildcard_field_reuses_no_latch() {
    let source = "class Circle { final num r; Circle(this.r); }\n\
                  void f(Object o, Object? sink) {\n\
                    if (o case Circle(var u, _)) {\n\
                      sink = u;\n\
                    }\n\
                  }\n";
    let at = source.find("u, _").unwrap();
    assert_eq!(
        getter_rows(source),
        vec![format!(
            "@{at}: The getter 'u' isn't defined for the type 'Circle'."
        )],
        "one row, anchored at the first field's variable"
    );
}
