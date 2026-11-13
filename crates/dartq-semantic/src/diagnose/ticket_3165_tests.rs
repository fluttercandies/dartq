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

/// `[...< 0]`'s rest-element sub-pattern: the rest's inner pattern matches
/// the LIST value itself — over a `dynamic` scrutinee the list pattern's
/// own type is `List<dynamic>`, and `<` resolves no member on it (oracle
/// row at the `<` operator token, resolver.dart's `.at(node.operator)`).
#[test]
fn rest_element_relational_reports_list_element() {
    let source = "f(x) {\n  switch (x) {\n    case [...< 0]:\n      break;\n  }\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            35,
            1,
            "The operator '<' isn't defined for the type 'List<dynamic>'.".to_string()
        )]
    );
}

/// The same law over a typed scrutinee: the rest sub-pattern matches
/// `List<num>` — `<` still resolves no member on the list (the live 3.13.7
/// oracle reports; the element type never reaches the operator face).
#[test]
fn rest_element_relational_typed_list_quiet() {
    let source = "f(List<num> x) {\n  switch (x) {\n    case [...< 0]:\n      break;\n  }\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            45,
            1,
            "The operator '<' isn't defined for the type 'List<num>'.".to_string()
        )]
    );
}

/// `foo >>> 42` under `@dart=2.12` — the scanner splits `>>>` into
/// `>>`+`>`, the parser re-glues into a zero-width synthetic `>>>` and the
/// oracle reports `undefined_operator '>>>'` at the `>>` position len 0
/// (live 3.13.7 machine format `UNDEFINED_ERROR|7|7|0` — offset 97 in this
/// fixture). The declared `operator >>>` stays `<unnamed>` pre-feature, so
/// the use-site consult misses even with the declaration present.
#[test]
fn triple_shift_undefined_reports_reglued() {
    let source = "// @dart=2.12\nclass Foo {\n  Foo operator >>>(_) => this;\n}\nmain() {\n  Foo foo = new Foo();\n  foo >>> 42;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            97,
            0,
            "The operator '>>>' isn't defined for the type 'Foo'.".to_string()
        )]
    );
}

/// `foo >>>= 42` under `@dart=2.12` — `>>`+`>=` re-glues the same way
/// (oracle `UNDEFINED_OPERATOR|7|7|0`, offset 97 len 0 here).
#[test]
fn triple_shift_compound_reports_reglued() {
    let source = "// @dart=2.12\nclass Foo {\n  Foo operator >>>(_) => this;\n}\nmain() {\n  Foo foo = new Foo();\n  foo >>>= 42;\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            97,
            0,
            "The operator '>>>' isn't defined for the type 'Foo'.".to_string()
        )]
    );
}

/// `Square(area: v6)` over `Square<T extends MetricUnits>` — the binder's
/// getter returns `Unit<T>` instantiated through `Square<MetricUnits>` —
/// `++` consults `+` on `Unit<MetricUnits>` → `undefined_operator '+'`
/// (live 3.13.7 machine format `UNDEFINED_OPERATOR|11|7|2` — the `++`
/// operator token at offset 246).
#[test]
fn pattern_bound_generic_getter_reports() {
    let source = "abstract class MetricUnits {}\nclass Unit<T extends MetricUnits> {\n  const Unit();\n}\nclass Square<T extends MetricUnits> {\n  const Square();\n  Unit<T> get area => const Unit();\n}\nmain() {\n  for (final Square(area: v6) in [const Square()]) {\n    v6++;\n  }\n}\n";
    assert_eq!(
        rows(source),
        vec![(
            246,
            2,
            "The operator '+' isn't defined for the type 'Unit<MetricUnits>'.".to_string()
        )]
    );
}
