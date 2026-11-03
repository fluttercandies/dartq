//! #3713 pins — `unchecked_use_of_nullable_value` identifier-path
//! promotion gap. Sub-clusters from the census sweep:
//! record-field promotion, private-field promotion, local `!=`/`is`
//! promotion, extension `on` nullable, generic `T` bound, `unary-`
//! spelling fallback, and `++` spelling fallback.

use super::test_rig::{code_names, report_ops, rows};

fn unchecked(source: &str) -> Vec<String> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.message.to_string())
        .collect()
}

/// `int? x; if (x != null) x.abs();` — promoted local, member access.
/// Oracle: silent.
#[test]
fn repro_local_promoted_member() {
    let src = "void f() {\n  int? x = 5;\n  if (x != null) {\n    x.abs();\n  }\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}

/// `({int i, String s})? r1; if (r1 != null) r1.i;` — promoted record
/// local, field access. Oracle: silent.
#[test]
fn repro_record_field_promoted() {
    let src = "void f() {\n  ({int i, String s})? r1;\n  r1 = (i: 42, s: 'x');\n  if (r1 != null) {\n    r1.i;\n  }\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}

/// Private final field `int? _x` promoted by `_x != null` inside the
/// declaring class body — `_x.isOdd`. Oracle: silent.
#[test]
fn repro_private_field_promoted() {
    let src = "class A {\n  final int? _x;\n  A(this._x);\n  void test() {\n    if (_x != null) {\n      _x.isOdd;\n    }\n  }\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}

/// `a._x is int` then `a._x.isEven` — property promotion on a foreign
/// receiver. Oracle: silent.
#[test]
fn repro_foreign_field_promoted() {
    let src = "class A {\n  final int? _x;\n  A(this._x);\n}\nvoid f() {\n  A a = A(42);\n  if (a._x is int) {\n    a._x.isEven;\n  }\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}

/// `T extends C?` calling `t.foo()` where `foo` is an `extension on C?`
/// member — the extension covers the nullable `C?`. Oracle: silent.
#[test]
fn repro_extension_on_nullable() {
    let src = "class C {}\nextension on C? {\n  int foo() => 1;\n}\nclass A<T extends C?> {\n  T t;\n  A(this.t);\n  void test() {\n    t.foo();\n  }\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}

/// `-x` on `Object?` — the `unary-` operator spelling resolves through the
/// extension's `-` member (upstream Dart SDK `operator -()` registers under
/// `-`, not `unary-`). Oracle: silent.
#[test]
fn repro_unary_minus_extension() {
    let src = "class C {\n  Object? operator -() => this;\n}\nvoid f() {\n  Object? x = C();\n  -x;\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}

/// `x++` on `Object?` — `++` resolves through the extension's `+` operator
/// member (the `+`/`++` spelling family). Oracle: silent.
#[test]
fn repro_prefix_increment_extension() {
    let src = "class C {\n  Object? operator +(Object? other) => this;\n}\nvoid f() {\n  Object? x = C();\n  ++x;\n}\n";
    assert_eq!(unchecked(src), Vec::<String>::new(), "{:#?}", rows(&report_ops(src)));
}
