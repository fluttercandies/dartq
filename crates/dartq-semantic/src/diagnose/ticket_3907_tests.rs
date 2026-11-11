//! #3907 lane AN pins — the promoted-receiver operator face and the
//! member-callee core-table miss arm:
//!
//! * E13-3 census family (`number_operator_typing_test` 470:5/471:4/472:4,
//!   491:5/492:4/493:4, 512:5/513:4/514:4, 517:8/518:8 — 11 rows):
//!   `undefined_operator` extras where a promoted local's inc/dec read
//!   should see the promotion (`i--`, `++i`, `d--`, `n++`, `d += never`
//!   inside a dead window). Probed mechanism: the pass-2 operator face
//!   already consults the promotion latch — `promotions.promoted` in
//!   `resolve_bare_row`'s operand arm and the `read_type_at` consult in
//!   `check_compound_assignment` both fire — so the reported `Object` face
//!   is the flow layer's own answer: the inc/dec interp arm writes
//!   `promote_to_non_null(declared)` (the unpromoted `Object`) instead of
//!   the resolved operator's return type (`int`/`double`/`num`), demoting
//!   the local and un-promoting every later read (upstream
//!   `postfix_expression_resolver.dart:174-186` passes
//!   `operatorReturnType` to `flow.postIncDec`, and
//!   `flow_analysis.dart`'s `_write` keeps a subtype write promoted).
//!   The acceptance pin stays `#[ignore]`d at the flow owner
//!   (`crates/dartq-flow`'s `Ev::UnaryPrefixAssign`/`UnaryPostfixAssign`
//!   arm — the row_resolution.rs face needs no change once the write
//!   types the operator result).
//! * gap2 member-callee miss arm: `x.first()` over `List<T?>` where
//!   `T extends Function` — the walk misses on the rig's stub `List` (no
//!   element model), so `member_defined`'s found arm never ran and the
//!   class-family miss tail emitted `undefined_method`. The core-table
//!   accessor arm now claims the `CORE_MEMBER_RETURNS` member and runs
//!   the same `check_member_callee_function_nullable` verdict the found
//!   arm runs (the existing `ticket_2919`/`ticket_3816` pins are the
//!   oracle contract; the pins here cover the arm's own edges).

use super::test_rig::report_ops;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The promoted operand's binary read resolves through the promotion
/// latch: `d` is `Object` promoted to `double`, so `d - 1`'s operator
/// face reads `double` — on the rig's member-less `double` stub the
/// walk honestly misses, but the reported row names `'double'` (the
/// promoted face), never `'Object'` (the declared face). The pass-2
/// operator face itself reads the promoted type; the inc/dec write's
/// demotion is the flow lane's file.
#[test]
fn promoted_binary_operand_read_declares_the_operator() {
    let report = report_ops(
        "\
void f(Object d) {
  if (d is! double) return;
  var x = d - 1;
}
",
    );
    let rows = texts(&report, "undefined_operator");
    assert_eq!(rows.len(), 1, "one operator row on the stub face: {rows:?}");
    assert!(
        rows[0].contains("'double'") && !rows[0].contains("'Object'"),
        "the read face is the promoted double, not the declared Object: {rows:?}"
    );
}

/// The inc/dec family keeps the promotion: `i++`'s write stores the
/// resolved OPERATOR's result type (`postfix_expression_resolver.dart`'s
/// `postIncDec` hands flow `operatorReturnType`), and `_write` keeps a
/// subtype-typed write promoted (`flow_analysis.dart:6319`, :7881-7905) —
/// so the local stays on its promoted `int` face and every later
/// `+`/`-`/`+=` resolves there. Oracle: silent.
#[test]
fn promoted_receiver_inc_dec_keeps_the_operator_defined() {
    let report = report_ops(
        "\
void f(Object i) {
  if (i is! int) return;
  i++;
  i--;
  ++i;
  --i;
  i += 1;
}
",
    );
    assert!(
        offsets(&report, "undefined_operator").is_empty(),
        "the promoted int face declares + and -, got {:?}",
        texts(&report, "undefined_operator")
    );
}

/// The core-table miss arm claims the accessor the walk cannot see:
/// `x.first()` over `List<T?>` (`T extends Function`) reports the
/// FUNCTION variant on the whole callee `x.first` and never the
/// `undefined_method` miss — upstream's `List.first` is a real member,
/// so the `CORE_MEMBER_RETURNS` answer claims the row (the
/// `ticket_2919`/`ticket_3816` twins pin the same contract on their own
/// fixtures).
#[test]
fn member_callee_core_table_miss_arm_claims_the_accessor() {
    let report = report_ops(
        "\
m<T extends Function>(List<T?> x) {
  x.first();
}
",
    );
    let unchecked = offsets(&report, "unchecked_use_of_nullable_value");
    assert_eq!(
        unchecked.len(),
        1,
        "the function-variant row fires once, got {:?}",
        texts(&report, "unchecked_use_of_nullable_value")
    );
    assert!(
        texts(&report, "unchecked_use_of_nullable_value")[0]
            .contains("The function can't be unconditionally invoked"),
        "the function variant: {:?}",
        texts(&report, "unchecked_use_of_nullable_value")
    );
    assert!(
        offsets(&report, "undefined_method").is_empty(),
        "the table-defined member is no miss, got {:?}",
        texts(&report, "undefined_method")
    );
}

/// A member outside the table keeps the miss: `x.removeMiddle()` on the
/// same `List<T?>` receiver reports `undefined_method` — the arm claims
/// only spellings `CORE_MEMBER_RETURNS` carries.
#[test]
fn member_callee_miss_arm_keeps_the_genuine_miss() {
    let report = report_ops(
        "\
m<T extends Function>(List<T?> x) {
  x.removeMiddle();
}
",
    );
    let methods = texts(&report, "undefined_method");
    assert_eq!(
        methods.len(),
        1,
        "the untabled member still reports, got {methods:?}"
    );
    assert!(
        methods[0].contains("removeMiddle"),
        "the miss names the spelling, got {methods:?}"
    );
}
