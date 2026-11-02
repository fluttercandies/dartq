//! #3610 pins — `dead_code` structural families from census-20261102-095230
//! (478 missing / 386 extra). Each pin carries the oracle's own answer
//! (dart analyze 3.13.7, `--format=machine`):
//!
//! * the bottom-tested `is` fold decides before the operand kind —
//!   `e is Never` is `booleanLiteral(isNot)` for a reference, a property,
//!   a call result, or `dynamic` (upstream Dart SDK source,
//!   pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:5944-5949),
//!   so the matched arm deads no matter what the operand's static type is;
//! * an if-branch that ENTERED unreachable reports even its empty block —
//!   `visitBlock` polls `checkUnreachableNode` unconditionally, with no
//!   empty-block exemption (`if (i is! int) {}` carries the braces row);
//! * a plain `=` write re-feeds the target's proven constant —
//!   `b = false; if (b) {}` deads the then-block exactly like the
//!   declaration's constant feed;
//! * `&&`/`||` close a dead interval anchored INSIDE the right operand at
//!   the operand's own `flowEnd` (binary_expression_resolver.dart:285/:331
//!   — unconditional at the construct's close), reporting
//!   `firstDeadNode.offset .. rightOperand.end` — never a runaway tail.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// Every `dead_code` row's source text, in report order (the
/// [`ticket_3288_tests`] face): reported offsets index into
/// `CORE_PRELUDE + source`.
fn dead_text(report: &FileReport, source: &str) -> Vec<String> {
    let text = format!("{CORE_PRELUDE}{source}");
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            let start = d.range.offset as usize;
            let end = start + d.range.length as usize;
            text.get(start..end)
                .expect("row span is inside the reported text")
                .to_owned()
        })
        .collect()
}

/// The bottom-tested fold covers a DYNAMIC operand: `d is Never` folds
/// `false`, the empty then-block reports its braces. Oracle 3.13.7
/// (`void t(dynamic d) { if (d is Never) {} }`): `2:19|2`.
#[test]
fn is_never_dynamic_operand_deads_empty_block() {
    let source = "void t(dynamic d) {\n  if (d is Never) {}\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["{}".to_owned()]);
}

/// A spelled-nullable operand: `Object?` folds the same. Oracle on
/// `if (o is Never) { o.hashCode; }`: `3:19|21` — the whole block.
#[test]
fn is_never_nullable_operand_deads_block() {
    let source = "void t(Object? o) {\n  if (o is Never) {\n    o.hashCode;\n  }\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{\n    o.hashCode;\n  }".to_owned()]
    );
}

/// A `final` member's property read (`unreachable_via_expr_property_test`'s
/// family): `nullProperty` is `Null`-typed upstream of any promotion slot,
/// and `Null`-vs-`Never` deads the then arm through the same bottom-tested
/// fold. Oracle on `if (c.nullProperty is Never) { x = null; }`: the block.
#[test]
fn is_never_property_operand_deads_block() {
    let source = "class C { Null get nullProperty => null; }\n\
                  void t(C c, Object? x) {\n  if (c.nullProperty is Never) {\n    x = null;\n  }\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{\n    x = null;\n  }".to_owned()]
    );
}

/// `is!` polarity: `x is! Never` folds `true`, so the ELSE arm deads —
/// a non-empty else block reports its extent.
#[test]
fn is_not_never_deads_else_block() {
    let source = "void t(Object? o) {\n  if (o is! Never) {\n    o.hashCode;\n  } else {\n    o.hashCode;\n  }\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{\n    o.hashCode;\n  }".to_owned()]
    );
}

/// A same-type `is!` folds `false` through the factored-bottom arm — the
/// empty then-block reports its braces (oracle-probed on 3.13.7:
/// `if (i is! int) {}` carries `7:18|2`). This was the earlier
/// `if_branch_folded` calibration's over-suppression.
#[test]
fn is_not_same_type_deads_empty_block() {
    let source = "void t() {\n  int i = 0;\n  if (i is! int) {}\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["{}".to_owned()]);
}

/// The non-bottom `is!` keeps both arms live: `num`-vs-`int` factor is
/// `num`, not bottom — no dead arm, no row. Oracle: silent on dead_code.
#[test]
fn is_not_disjoint_declared_stays_live() {
    let source = "void t() {\n  num n = 0;\n  if (n is! int) {}\n}\n";
    let report = report_ops(source);
    assert!(
        dead_text(&report, source).is_empty(),
        "{:?}",
        dead_text(&report, source)
    );
}

/// The write-side constant feed: a plain `=` re-feeds the target's
/// `bool_value` — `b = false; if (b) { ... } else { ... }` reports the
/// then-block. Oracle 3.13.7: `4:10|19`.
#[test]
fn plain_assign_refeeds_const_bool() {
    let source = "void t() {\n  var b = true;\n  b = false;\n  if (b) {\n    print(1);\n  } else {\n    print(2);\n  }\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{\n    print(1);\n  }".to_owned()]
    );
}

/// The unassigned-declaration twin: `bool c; c = false; if (c) { ... }`
/// deads the then-block the same way — the write proves the constant
/// before any initializer did. Oracle: `11:10|19` in the full fixture.
#[test]
fn plain_assign_feeds_const_bool_without_initializer() {
    let source = "void t() {\n  bool c;\n  c = false;\n  if (c) {\n    print(3);\n  }\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{\n    print(3);\n  }".to_owned()]
    );
}

/// A constant PROPAGATES through reads: `b2 = b` feeds `b2` the value `b`
/// carried — `if (b2)` deads the same arm.
#[test]
fn const_bool_propagates_through_assign_reads() {
    let source = "void t() {\n  var b = false;\n  var b2 = true;\n  b2 = b;\n  if (b2) {\n    print(1);\n  }\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{\n    print(1);\n  }".to_owned()]
    );
}

/// A non-constant write clears the face: `b = f()` proves nothing — both
/// arms stay live (oracle: silent).
#[test]
fn non_const_assign_leaves_condition_live() {
    let source = "bool f() => true;\nvoid t() {\n  var b = false;\n  b = f();\n  if (b) {\n    print(1);\n  }\n}\n";
    let report = report_ops(source);
    assert!(
        dead_text(&report, source).is_empty(),
        "{:?}",
        dead_text(&report, source)
    );
}

/// The `&&`/`||` interior close: a Never-typed call kills the flow
/// mid-RHS, the receiver-of-Never binary is the first dead node, and the
/// operand's own `flowEnd` reports `firstDeadNode.offset ..
/// rightOperand.end` — the row ends at the operand, never at the
/// statement's later extent. Oracle on `c && (x = g()) > 0`: `> 0`.
#[test]
fn logical_rhs_interior_death_closes_at_operand_end() {
    let source = "Never g() => throw 0;\nvoid t(bool c, int x) {\n  var r = c && (x = g()) > 0;\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["> 0".to_owned()]);
}

/// The `||` twin: `(i > 0) || g() > 0`'s RHS dies at the `>` receiver —
/// the operand's `flowEnd` closes the run at `> 0` exactly like `&&`.
#[test]
fn logical_or_rhs_interior_death_closes_at_operand_end() {
    let source = "Never g() => throw 0;\nvoid t(bool c, int x) {\n  var s = c || g() > 0;\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["> 0".to_owned()]);
}

/// A Never-typed call's `is` test inside `&&`'s RHS anchors at the type
/// spelling — the first unreachable node after the operand's exit — and
/// the operand's `flowEnd` reports it through the operand's end.
/// Oracle on `c is int && g() is int`: `4:30|3` — `int` alone.
#[test]
fn logical_rhs_never_call_is_test_rows_the_type_head() {
    let source = "Never g() => throw 0;\nvoid t(Object? c) {\n  var r = c is int && g() is int;\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["int".to_owned()]);
}
