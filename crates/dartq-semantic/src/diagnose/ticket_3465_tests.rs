//! The #3465 pins: `dead_code` coverage for branches whose deadness a
//! semantic verdict proves but the flow's `cond_split`/`if_branch_folded`
//! never folds — plus the two span-shape clamps on emitted
//! `FlowFact::DeadCode` rows.
//!
//! Upstream law (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/dead_code_verifier.dart:217-340`):
//! `checkUnreachableNode` walks top-down and latches the FIRST dead node
//! — a constant-condition `if`'s dead arm rows the arm's own extent: a
//! `{`-block arm reports `{`..`}` (`checkBlock` latches the block
//! itself), a statement arm reports `firstToken.offset..end`, and an
//! `else if` chain's dead else arm reports the WHOLE nested `if`
//! statement (the `if` IS the else arm's statement). dartq's flow reaches
//! those rows only when the condition folds to `bool_value`; when a
//! semantic verdict row already decided the condition
//! (`unnecessary_type_check`, `unnecessary_null_comparison`) the dead
//! arm's row is synthesized from the verdict's polarity — `'true'` kills
//! the else arm, `'false'` the then arm.
//!
//! The span-shape clamps pin the emitted-row reshape law:
//! * the `;`-clamp (a marker row trimming its trailing `;`) only applies
//!   when the marker covers an assignment — `a.foo;` on a `Never`
//!   receiver keeps its `;` because `foo` is a whole statement, not an
//!   assignment's tail;
//! * the `]`-closer extension never swallows a `;` after a NULL-AWARE
//!   index (`n?[i]` reports `i]`, not `i];`).

use crate::diagnose::test_rig::{report_ops, report_ops_bare};

/// The `dead_code` rows of `report` as `(offset, length)`, source order.
fn dead_rows(report: &super::FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "dead_code")
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// The offset of `needle`'s LAST occurrence inside `fixture` shifted past
/// the prelude — `report_ops` prepends
/// [`crate::diagnose::test_rig::CORE_PRELUDE`].
fn fixture_offset(fixture: &str, needle: &str) -> usize {
    crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind(needle).unwrap()
}

/// A `typedef`-spelled operand defeats the flow's fold (`topx is! fx`'s
/// `Expr::branches` stays empty) while the semantic verdict still answers
/// `is!` as `always 'false'` — the dead THEN arm reports its `{`-block
/// whole, oracle shape.
#[test]
fn typedef_operand_is_not_dead_then_block() {
    let fixture = "typedef fx(x);\n\
                   void topx(x) {}\n\
                   void g() {\n  if (topx is! fx) { print(0); } else { print(1); }\n}";
    let report = report_ops(fixture);
    let off = fixture_offset(fixture, "{ print(0); }");
    assert_eq!(
        dead_rows(&report),
        [(off as u32, "{ print(0); }".len() as u32)]
    );
}

/// The same `is!`-verdict's dead arm when it is a bare statement: the row
/// runs the statement's first token through its `;`.
#[test]
fn typedef_operand_is_not_dead_then_statement() {
    let fixture = "typedef fx(x);\n\
                   void topx(x) {}\n\
                   void g() {\n  if (topx is! fx) throw \"x\";\n}";
    let report = report_ops(fixture);
    let off = fixture_offset(fixture, "throw \"x\";");
    assert_eq!(
        dead_rows(&report),
        [(off as u32, "throw \"x\";".len() as u32)]
    );
}

/// An `always 'true'` verdict kills the ELSE arm — `x != null` on a
/// non-nullable local (`unnecessary_null_comparison`) reports the else
/// block whole.
#[test]
fn non_nullable_neq_null_dead_else_block() {
    let fixture = "void f(int x) {\n  if (x != null) { print(0); } else { print(1); }\n}";
    let report = report_ops(fixture);
    let off = fixture_offset(fixture, "{ print(1); }");
    assert_eq!(
        dead_rows(&report),
        [(off as u32, "{ print(1); }".len() as u32)]
    );
}

/// `x == null` on the same non-nullable local folds `always 'false'` —
/// the dead THEN block reports.
#[test]
fn non_nullable_eq_null_dead_then_block() {
    let fixture = "void f(int x) {\n  if (x == null) { print(0); } else { print(1); }\n}";
    let report = report_ops(fixture);
    let off = fixture_offset(fixture, "{ print(0); }");
    assert_eq!(
        dead_rows(&report),
        [(off as u32, "{ print(0); }".len() as u32)]
    );
}

/// `else if`'s dead arm rows the WHOLE nested `if` statement — `if (y ==
/// 0) {..} else {..}` included — not just the nested `if`'s head.
#[test]
fn else_if_dead_arm_rows_whole_nested_statement() {
    let fixture = "typedef fx(x);\n\
                   void topx(x) {}\n\
                   void g(int y) {\n  if (topx is fx) { print(9); } else if (y == 0) { print(0); } else { print(1); }\n}";
    let report = report_ops(fixture);
    let needle = "if (y == 0) { print(0); } else { print(1); }";
    let off = fixture_offset(fixture, needle);
    assert_eq!(dead_rows(&report), [(off as u32, needle.len() as u32)]);
}

/// A `Never`-receiver member read is a whole dead STATEMENT — the marker
/// keeps its `;` (`foo;`), because the `;`-clamp is gated on the marker
/// covering an assignment (`n?.p = i`'s `p = i` arm), not on every
/// mid-expression anchor.
#[test]
fn never_member_statement_keeps_semicolon() {
    let fixture = "void f(Never a) {\n  a.foo;\n}";
    let report = report_ops_bare(fixture);
    let off = fixture.find("foo;").unwrap();
    assert_eq!(dead_rows(&report), [(off as u32, "foo;".len() as u32)]);
}

/// A `Never`-receiver PROPERTY read the same — `hashCode;` keeps its `;`.
#[test]
fn never_getter_statement_keeps_semicolon() {
    let fixture = "Never get d => throw 0;\ntest() => d.hashCode;\n";
    let report = report_ops_bare(fixture);
    let off = fixture.find("hashCode;").unwrap();
    assert_eq!(dead_rows(&report), [(off as u32, "hashCode;".len() as u32)]);
}

/// The null-aware WRITE keeps the marker-assignment clamp — `n?.p = i`
/// still reports `p = i` (never `p = i;`), the clamp's true home.
#[test]
fn null_aware_write_tail_still_excludes_semicolon() {
    let fixture = "void f(Null n, int i) {\n  n?.p = i;\n  print(0);\n}";
    let report = report_ops_bare(fixture);
    let off = fixture.find("p = i").unwrap();
    assert_eq!(dead_rows(&report), [(off as u32, "p = i".len() as u32)]);
}

/// A NULL-AWARE index's `]`-closer row never swallows the statement `;` —
/// `n?[i]` reports `i]` (the `]` IS the row's closer), not `i];`.
#[test]
fn null_aware_index_row_excludes_semicolon() {
    let fixture = "void f(Null n, int i) {\n  n?[i];\n}";
    let report = report_ops_bare(fixture);
    let off = fixture.find("i]").unwrap();
    assert_eq!(dead_rows(&report), [(off as u32, "i]".len() as u32)]);
}

/// The reachable-statement control: a live `if` whose condition folds
/// nothing emits NO `dead_code` — the synthesis only fires on a real
/// verdict row, and `i > 0` carries none.
#[test]
fn live_if_stays_silent() {
    let fixture = "void f(int i) {\n  if (i > 0) { print(0); } else { print(1); }\n}";
    assert_eq!(dead_rows(&report_ops(fixture)), []);
}

/// A verdict site inside a COMPOSED condition never synthesizes — `a ||
/// x == null`'s `x == null` half folds `always 'false'` but the `a` side
/// still splits the branch (`false`-on-one-side of `||` decides
/// nothing), so no dead row may appear. (`a && x == null` would row the
/// then-block through the flow's own `false`-fold — the `||` shape is
/// the one where only over-eager synthesis could emit.)
#[test]
fn composed_condition_synthesizes_nothing() {
    let fixture = "void f(bool a, int x) {\n  if (a || x == null) { print(0); } else { print(1); }\n}";
    assert_eq!(dead_rows(&report_ops(fixture)), []);
}
