//! The C7 census wave's nullable-safety pin file: the three in-scope
//! lint faces whose operand coverage the census flagged red —
//! `prefer_null_aware_operators` (3 upstream cases),
//! `prefer_null_aware_method_calls` (2), and
//! `unnecessary_nullable_for_final_variable_declarations` (8). Each pin
//! runs through the production bridge; the semantic-gated rules run
//! with the face mounted (`Fixture::run_semantic`).

use crate::testsupport::{Fixture, one_rule};

// —— prefer_null_aware_operators ——————————————————————————————————————

/// `test_identifierEqualEqualNull_null_elseMethodCall`
/// (pkg/linter/test/rules/prefer_null_aware_operators_test.dart:32-42):
/// `a == null ? null : a.b()!` reports — the result arm peels the
/// postfix `!`, then the invocation send, to reach the operand.
#[test]
fn null_aware_bang_else_method_call_reports() {
    let f = Fixture::parse("void f() {\n  a == null ? null : a.b()!;\n}\n");
    let lints = f.run(&one_rule("prefer_null_aware_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_nullEqualEqualIdentifier_null_elseMethodInvocation` (:110-120):
/// `null == a ? null : a.b()!` — the null literal on the left still
/// picks the right operand.
#[test]
fn null_aware_null_left_else_method_invocation_reports() {
    let f = Fixture::parse("void f() {\n  null == a ? null : a.b()!;\n}\n");
    let lints = f.run(&one_rule("prefer_null_aware_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_prefixedIdentifierEqualEqualNull_null_elseMethodInvocation`
/// (:172-183): `a.b == null ? null : a.b!.c()!` — a member-access
/// operand peels the `!`/send/access chain to `a.b`.
#[test]
fn null_aware_prefixed_operand_else_method_invocation_reports() {
    let f = Fixture::parse("void f() {\n  a.b == null ? null : a.b!.c()!;\n}\n");
    let lints = f.run(&one_rule("prefer_null_aware_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

// —— prefer_null_aware_method_calls ———————————————————————————————————

/// `test_conditional_propertyAccess_sameProperty`
/// (prefer_null_aware_method_calls_test.dart:37-43): `p.m != null ?
/// p.m!() : null` on a `dynamic` receiver reports — the upstream check
/// is purely syntactic (`operand.toSource() ==
/// condition.leftOperand.toSource()`); no member resolution gate.
#[test]
fn null_aware_method_call_dynamic_receiver_reports() {
    let source = "void f(dynamic p) {\n  p.m != null ? p.m!() : null;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("prefer_null_aware_method_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_ifNotNull_propertyAccess_sameProperty` (:64-72):
/// `if (p.m != null) { p.m!(); }` on `dynamic` reports.
#[test]
fn null_aware_method_call_if_dynamic_receiver_reports() {
    let source = "void f(dynamic p) {\n  if (p.m != null) {\n    p.m!();\n  }\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("prefer_null_aware_method_calls"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

// —— unnecessary_nullable_for_final_variable_declarations —————————————

/// `test_list` (unnecessary_nullable_for_final_variable_declarations_test.dart:21-28):
/// `final [int a, num? c] = [0, 1]` — the list-pattern destructuring
/// declaration visits each element; `num? c` matched against the
/// non-nullable matched-value type reports at `c`.
#[test]
fn nullable_final_list_pattern_reports() {
    let source = "f() {\n  final [int a, num? c] = [0, 1];\n  print('$a$c');\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `test_nullableType_field` (:89-98): a private or static field with a
/// nullable written type and a non-nullable initializer reports.
#[test]
fn nullable_final_field_reports() {
    let source = "class A {\n  final int? _i = 1;\n  static final int? j = 1;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 2, "{lints:?}");
}

/// `test_nullableType_field_extension` (:100-108): the static field of
/// an extension reports.
#[test]
fn nullable_final_extension_field_reports() {
    let source =
        "extension E on Object {\n  static final int? _e = 1;\n  static final int? e = 1;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 2, "{lints:?}");
}

/// `test_record` (:128-138): `final (List<int>? a, num? c) = ([], 1)` —
/// the record-pattern destructure visits each field; a nullable written
/// type whose matched-value type is non-nullable reports.
#[test]
fn nullable_final_record_pattern_reports() {
    let source = "f() {\n  final (List<int>? a, num? c) = ([], 1);\n  print('$a$c');\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 2, "{lints:?}");
}

/// `test_nullableType_topLevel` (:110-117) and `test_nullableType_variable`
/// (:119-126): nullable top-level and local `final`s with non-nullable
/// initializers report at the name.
#[test]
fn nullable_final_top_level_and_local_report() {
    let source = "final int? _i = 1;\nfinal int? i = 1;\nconst int? ic = 1;\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 3, "{lints:?}");

    let source = "f() {\n  final int? _i = 1;\n  final int? i = 1;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 2, "{lints:?}");
}
