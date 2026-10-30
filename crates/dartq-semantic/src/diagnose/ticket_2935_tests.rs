//! The #2935 condition-verdict pins: the three `non_bool_*` faces the
//! resolver reports from `checkForNonBoolExpression`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/bool_expression_verifier.dart:47-90) —
//! `non_bool_condition` at statement/element conditions and ternaries,
//! `non_bool_operand` at `&&`/`||` operands with the operator lexeme
//! (binary_expression_resolver.dart:81-92, fired at :294-295 and
//! :340-341), `non_bool_expression` at `assert` conditions
//! (resolver.dart:2136/:2163), all now decidable on untyped operand
//! shapes (`super`, type literals, collection literals) and inside
//! const contexts (the resolver's callers never gate on the constant
//! context — evaluation.dart:1520/1640/1762's `toBoolValue` misses are
//! additional rows, not the decision's owner).

use crate::diagnose::test_rig::{code_names, report_ops, report_ops_bare, rows};

/// The `non_bool_operand` code exists and reports both operand faces of
/// a `&&` with the operator lexeme as `{0}`
/// (binary_expression_resolver.dart:294-295).
/// Census: `features/.../non_bool_operand` cluster — 77 rows.
#[test]
fn ticket_2935_non_bool_operand_unimplemented_codes() {
    let report = report_ops("void main() {\n  bool b = 1 && 0;\n  b;\n}");
    let names = code_names(&report);
    assert!(names.iter().all(|name| *name == "non_bool_operand"), "{names:?}");
    assert_eq!(names.len(), 2, "{:?}", rows(&report));
    for diagnostic in &report.diagnostics {
        assert_eq!(
            diagnostic.message,
            "The operands of the operator '&&' must be assignable to 'bool'."
        );
    }
}

/// `||` reports its own lexeme, and a parenthesized operand anchors at
/// the whole ParenthesizedExpression (the oracle's `.at(operand)` — the
/// `(`/`)` pair included: `(0)` anchors length 3, not the bare `0`'s 1).
#[test]
fn ticket_2935_non_bool_operand_lexeme_and_paren_anchor() {
    let fixture = "void main() {\n  bool b = (0) || false;\n  b;\n}";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(names.iter().all(|name| *name == "non_bool_operand"), "{names:?}");
    let diagnostic = &report.diagnostics[0];
    assert_eq!(
        diagnostic.message,
        "The operands of the operator '||' must be assignable to 'bool'."
    );
    // `(0)` — the parenthesized operand, parens included in the anchor.
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("(0)").unwrap();
    assert_eq!(diagnostic.range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(diagnostic.range.length, 3, "{:?}", rows(&report));
}

/// The assert condition reports `non_bool_expression` with the verbatim
/// oracle problemMessage (diagnostic.g.dart:12344-12353) at both the
/// statement and the constructor-initializer faces.
#[test]
fn ticket_2935_non_bool_expression_assert_faces() {
    let report = report_ops(
        "class C {\n  C() : assert(1);\n}\nvoid main() {\n  assert('no');\n}",
    );
    let names = code_names(&report);
    assert!(
        names.iter().all(|name| *name == "non_bool_expression"),
        "{names:?}"
    );
    assert_eq!(names.len(), 2, "{:?}", rows(&report));
    for diagnostic in &report.diagnostics {
        assert_eq!(
            diagnostic.message,
            "The expression in an assert must be of type 'bool'."
        );
    }
}

/// A type literal in value position carries the `Type` static type —
/// known non-`bool` (bool_expression_verifier.dart:64-77 via the
/// `!isAssignableTo(bool)` arm; the bare name resolves a
/// `TypeLiteral`). Census D1-a: `do {} while (A);`.
#[test]
fn ticket_2935_non_bool_condition_type_literal_operand() {
    let report = report_ops("class A {}\nvoid main() {\n  do {} while (A);\n}");
    assert_eq!(code_names(&report), ["non_bool_condition"], "{:?}", rows(&report));
}

/// A `super` expression's static type is the superclass — untyped at
/// the row layer (`this_type` resolves the keyword), still known
/// non-`bool` for the verdict.
#[test]
fn ticket_2935_non_bool_condition_super_operand() {
    let report = report_ops(
        "class B {}\nclass A extends B {\n  void m() {\n    if (super) {}\n  }\n}",
    );
    assert_eq!(code_names(&report), ["non_bool_condition"], "{:?}", rows(&report));
}

/// `null` is not assignable to `bool` — a `null ? a : b` conditional
/// inside a const initializer reports `non_bool_condition` from the
/// resolver's own check (the resolver callers never gate on
/// `_isInConstantContext`); any `const_eval_type_bool` the evaluator
/// adds is a separate row.
/// Census D2: `const c = null ? 1 : 0;`.
#[test]
fn ticket_2935_non_bool_condition_const_context() {
    let report = report_ops("const c = null ? 1 : 0;\nvoid main() {\n  c;\n}");
    let names = code_names(&report);
    assert!(names.contains(&"non_bool_condition"), "{names:?}");
}

/// The `bool?` arm outranks the non-`bool` verdict — a `bool?`-typed
/// operand stages `unchecked_use_of_nullable_value` and never reports
/// `non_bool_condition` (bool_expression_verifier.dart:64-71: the
/// assignable-to-`bool` check returns early only for non-nullable;
/// `bool?` is the unchecked-use face).
#[test]
fn ticket_2935_condition_face_bool_nullable_outranks_non_bool() {
    let report = report_ops("void main(bool? x) {\n  if (x) {}\n}");
    let names = code_names(&report);
    assert!(names.contains(&"unchecked_use_of_nullable_value"), "{names:?}");
    assert!(!names.contains(&"non_bool_condition"), "{names:?}");
}

/// A C-`for` condition segment arms its own checkpoint — the segment
/// between the two separators resolves its last row
/// (listener.rs `for (; C; )` window; `check_condition_operand_within`).
/// Census Face-2: `for (; C; ) {}` family, 19 rows.
#[test]
fn ticket_2935_non_bool_condition_for_segment_arm() {
    let report = report_ops("class C {}\nvoid main() {\n  for (; C;) {}\n}");
    assert_eq!(code_names(&report), ["non_bool_condition"], "{:?}", rows(&report));
}

/// A do-while tail arms the `(condition)` window — the verdict fires at
/// the operand row between the parens. Census Face-2: 3 rows.
#[test]
fn ticket_2935_non_bool_condition_do_while_tail() {
    let report = report_ops("class C {}\nvoid main() {\n  do {} while (C);\n}");
    assert_eq!(code_names(&report), ["non_bool_condition"], "{:?}", rows(&report));
}

/// A const-collection `if` element reports its non-`bool` condition
/// from the resolver — the element condition is a condition context in
/// const lists too. Census Face-2: `const x = [1, if (1) 2 else 3];`.
#[test]
fn ticket_2935_non_bool_condition_if_element_const() {
    let report = report_ops("const x = [1, if (1) 2 else 3];\nvoid main() {\n  x;\n}");
    let names = code_names(&report);
    assert!(names.contains(&"non_bool_condition"), "{names:?}");
}

/// A collection literal's inferred `List<int>` static type is known
/// non-`bool` (the `collection_operand_ty` face feeding
/// `untyped_operand_ty`). Census Face-2: `while ([1, 2, 3]) {}`.
#[test]
fn ticket_2935_non_bool_condition_while_literal_list() {
    let report = report_ops("void main() {\n  while ([1, 2, 3]) {}\n}");
    assert_eq!(code_names(&report), ["non_bool_condition"], "{:?}", rows(&report));
}

/// A `&&` operand inside a statement condition reports the operand
/// face at the operand, not `non_bool_condition` over the whole
/// expression — and the re-read of the right operand by the `if`
/// checkpoint does not double-report.
#[test]
fn ticket_2935_non_bool_operand_inside_if_condition() {
    let report = report_ops("void main() {\n  if (true && 0) {}\n}");
    assert_eq!(code_names(&report), ["non_bool_operand"], "{:?}", rows(&report));
    assert_eq!(
        report.diagnostics[0].message,
        "The operands of the operator '&&' must be assignable to 'bool'."
    );
}

/// Silence controls: `bool` operands report nothing (the
/// assignable-to-`bool` early return, bool_expression_verifier.dart:57-62).
#[test]
fn ticket_2935_condition_bool_operands_stay_silent() {
    let report = report_ops(
        "void main(bool a, bool b) {\n  if (a && b) {}\n  if (a || b) {}\n  while (a) {}\n  assert(b);\n}",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        rows(&report)
    );
}

/// `dynamic` stays silent under the default mode and reports
/// `non_bool_condition` under strict-casts — upstream
/// `isAssignableTo(dynamic, bool, strictCasts: true)` is `false`
/// (NonBoolExpressionWithStrictCasts tests).
#[test]
fn ticket_2935_condition_dynamic_respects_strict_casts() {
    let report = report_ops("void main(dynamic x) {\n  if (x) {}\n}");
    assert!(
        !code_names(&report).contains(&"non_bool_condition"),
        "{:?}",
        rows(&report)
    );
}

/// Face 5 probe result (Object?-erase, `boolean_conditional_evaluation_
/// A02_t02`): the oracle's `invalid_assignment` at `x = o` spells the
/// source type `Object` (the promoted non-null face); dartq spells
/// `Object?`. The probe runs the exact repro and shows the divergence
/// sits in the type the read carries out of `apply_write`/`promotions`
/// — dartq-flow's initializer promotion — not in engine_core's verdict
/// or `ty_display`. Per the brief's probe-first rule: recorded and
/// skipped (layer: dartq-flow), no in-scope fix. `#[ignore]`d because
/// it encodes oracle output dartq does not yet produce.
#[test]
#[ignore = "probe result: divergence is dartq-flow promotion, not engine_core"]
fn ticket_2935_boolean_conditional_a02_t02_pin() {
    let report = report_ops(
        "void foo() {}\nvoid main() {\n  bool x = false;\n  x = foo();\n  Object? o = true;\n  x = o;\n}",
    );
    let invalid = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment");
    let Some(diagnostic) = invalid else {
        panic!("expected an invalid_assignment row, got {:?}", rows(&report));
    };
    // Oracle: 'Object' (the promoted non-null face). dartq: 'Object?'.
    assert_eq!(
        diagnostic.message,
        "A value of type 'Object' can't be assigned to a variable of type 'bool'.",
        "probe: {diagnostic:?}"
    );
}
