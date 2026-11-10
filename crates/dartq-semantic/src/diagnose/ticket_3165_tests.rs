//! #3165 `undefined_operator` pins: the rest-element relational sub-pattern
//! (`case [...< 0]` — matched type `List<E>` of the scrutinee's element, never
//! a property column), the `>>>`/`>>>=` re-glue under `no-triple-shift`
//! (`@dart=2.12` — the scanner splits `>>>` into `>>`+`>` and `>>>=` into
//! `>>=`+`>`, and the oracle reports `undefined_operator '>>>'` at the
//! `>>`/`>>=` position len 0), and the object-pattern shorthand binder's
//! parameterized getter return (`Square(area: v6)` over `Square<T extends
//! MetricUnits>` — the binder reads the receiver-instantiated face
//! `Unit<MetricUnits>`). Every expectation is a verbatim oracle 3.13.7 row.

use super::test_rig::report_ops_bare;

/// The `undefined_operator` rows of `report_ops_bare`, as (offset, length,
/// message) triples — the bare rig keeps the fixture's own offsets (no
/// prelude shift); every fixture declares the members it names, so the only
/// rows under test are the operator family.
fn rows(source: &str) -> Vec<(u32, u32, String)> {
    report_ops_bare(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// `[...< 0]`'s rest-element sub-pattern: the matched type is `List<E>` of
/// the scrutinee's element (`List<dynamic>` over a `dynamic` scrutinee) —
/// `<` resolves no member on `List` → `undefined_operator '<'`.
#[test]
fn rest_element_relational_reports_list_element() {
    let source = "f(x) {\n  switch (x) {\n    case [...< 0]:\n      break;\n  }\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            41,
            1,
            "The operator '<' isn't defined for the type 'List<dynamic>'.".to_string()
        )]
    );
}

/// `f(List<num>)`'s rest-element sub-pattern: `List<num>`'s element is
/// `num` — `<` resolves on `num` → silent.
#[test]
fn rest_element_relational_typed_list_quiet() {
    let source = "f(List<num> x) {\n  switch (x) {\n    case [...< 0]:\n      break;\n  }\n}\n";
    assert_eq!(rows(source), Vec::<(u32, u32, String)>::new());
}

/// `foo >>> 42` under `@dart=2.12` — the scanner splits `>>>` into
/// `>>`+`>`, the oracle re-glues and reports `undefined_operator '>>>'` at
/// the `>>` position len 0.
#[test]
fn triple_shift_undefined_reports_reglued() {
    let source = "// @dart=2.12\nclass Foo {\n  Foo operator >>>(_) => this;\n}\nmain() {\n  Foo foo = new Foo();\n  foo >>> 42;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            79,
            0,
            "The operator '>>>' isn't defined for the type 'Foo'.".to_string()
        )]
    );
}

/// `foo >>>= 42` under `@dart=2.12` — `>>=`+`>` re-glues the same way.
#[test]
fn triple_shift_compound_reports_reglued() {
    let source = "// @dart=2.12\nclass Foo {\n  Foo operator >>>(_) => this;\n}\nmain() {\n  Foo foo = new Foo();\n  foo >>>= 42;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            79,
            0,
            "The operator '>>>' isn't defined for the type 'Foo'.".to_string()
        )]
    );
}

/// `Square(area: v6)` over `Square<T extends MetricUnits>` — the binder's
/// getter returns `Unit<T>` instantiated through `Square<MetricUnits>` —
/// `++` consults `+` on `Unit<MetricUnits>` → `undefined_operator '+'`.
#[test]
fn pattern_bound_generic_getter_reports() {
    let source = "abstract class MetricUnits {}\nclass Unit<T extends MetricUnits> {\n  const Unit();\n}\nclass Square<T extends MetricUnits> {\n  const Square();\n  Unit<T> get area => const Unit();\n}\nmain() {\n  for (final Square(area: v6) in [const Square()]) {\n    v6++;\n  }\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            208,
            2,
            "The operator '+' isn't defined for the type 'Unit<MetricUnits>'.".to_string()
        )]
    );
}
