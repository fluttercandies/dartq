//! The #2516 pins: closure-param UPWARD inference through
//! element-bearing member-read arguments (the informative_data.dart
//! `forCorrespondingPairs` family — `f(Iterable<T1>, Iterable<T2>, void
//! Function(T1,T2))` where arg 1 is an extension-getter result typed
//! `Unknown` at capture). `solve_arg_member_ty` re-derives the read's
//! static type from `member_reads` and feeds ONLY the constraint
//! gather/`unify_upward` — `level.args` keeps `Unknown` so the
//! assignability verdict never double-counts (the reverted
//! `typed_argument_row` regression). With `T1`/`T2` solved, the
//! downward re-bind (#2500) types the closure's untyped formals and the
//! member reads inside no longer emit the unchecked-nullable FP.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn unchecked_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `forCorrespondingPairs` face: `xs.withDecl` is a generic
/// extension getter `on List<T> -> Iterable<T>` — `member_reads` types
/// it `Iterable<int>`, `T1 := int` and `T2 := String` solve upward,
/// and the closure formals re-bind so `a + b.length` reads real members
/// — zero unchecked rows.
#[test]
fn upward_member_read_arg_solves_closure_formals() {
    let source = r#"
void forPairs<T1, T2>(Iterable<T1> a, Iterable<T2> b, void cb(T1 x, T2 y)) {}
extension<T> on List<T> {
  Iterable<T> get withDecl => this;
}
void main() {
  List<int> xs = List<int>();
  List<String> ys = List<String>();
  forPairs(xs.withDecl, ys.withDecl, (a, b) => a + b.length);
}
"#;
    let report = report_ops(source);
    let rows = unchecked_rows(&report);
    assert!(
        rows.is_empty(),
        "{:?}",
        report.diagnostics.iter().map(|d| (d.code.name, &d.message)).collect::<Vec<_>>()
    );
}

/// The verdict-face guard: `level.args` still carries `Unknown` for
/// the member-read argument — the upward feed must not retroactively
/// type the assignability verdict. A member-read arg whose element
/// type does NOT match the formal's constraint keeps its own visible
/// row (or stays silent) exactly as before — the pin asserts no
/// `could_not_infer` fabricates from the read face the model cannot
/// pin verbatim.
#[test]
fn upward_feed_does_not_report_could_not_infer() {
    let source = r#"
void forPairs<T1, T2>(Iterable<T1> a, Iterable<T2> b, void cb(T1 x, T2 y)) {}
extension<T> on List<T> {
  Iterable<T> get withDecl => this;
}
void main() {
  List<int> xs = List<int>();
  forPairs(xs.withDecl, xs.withDecl, (a, b) => a + b);
}
"#;
    let report = report_ops(source);
    assert!(
        !report.diagnostics.iter().any(|d| d.code.name == "could_not_infer"),
        "{:?}",
        report.diagnostics.iter().map(|d| (d.code.name, &d.message)).collect::<Vec<_>>()
    );
}

/// The reentrancy guard (the `f_bounded_mutually_recursive` panic): a
/// mutually-recursive bound `T extends Comparable<T>` routes the bound
/// gate's `infer_closure`/`infer_schema_closure` through a nested `Named`
/// argument list — the recursive fold must materialize the pool's argument
/// list BEFORE recursing (the inner `intern_list`/`function_generic` takes
/// `borrow_mut` on the same RefCell, and a read borrow held across it
/// panics). The pin only asserts the pass completes: the panic was the
/// whole defect.
#[test]
fn mutually_recursive_bound_does_not_reenter_args_pool() {
    let source = r#"
class Comparable<T> { int compareTo(T other) => 0; }
void f<T extends Comparable<T>>(T t) {}
void main() {
  f(0);
}
"#;
    let report = report_ops(source);
    // Reaching here is the assertion — the closure fold over
    // `Comparable<T>` recursed into the same `args` RefCell.
    let _ = report.diagnostics.len();
}

/// The schema-closure twin: `infer_schema_closure`'s `Named` arm held the
/// same read borrow across its recursive fold. A generic context whose
/// bound nests another `Named` (`S extends List<T>` under `T`) exercises
/// the same re-intern path through the schema arm.
#[test]
fn mutually_recursive_schema_closure_does_not_reenter() {
    let source = r#"
class Comparable<T> { int compareTo(T other) => 0; }
class C<T extends Comparable<T>, S extends List<T>> { C(T t, S s); }
void main() {
  var c = C(0, List<int>());
}
"#;
    let report = report_ops(source);
    let _ = report.diagnostics.len();
}
