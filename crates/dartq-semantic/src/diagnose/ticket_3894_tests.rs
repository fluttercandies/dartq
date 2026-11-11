//! #3894 pins — the merged-member NORM collapse over `FutureOr` columns.
//!
//! The merge's fold reads the candidates' canonical faces: every valid
//! candidate's type columns pass `norm_ty` over their alias-folded
//! spellings before the pairwise reduce (the upstream
//! `normalizeFunctionType` map inside `_topMergeSignatureTypes`), so a
//! `FutureOr<top>` super-face argument folds like the top it normalizes to
//! instead of leaving `x` at the un-normalized first candidate. Census
//! family `features/non-nullable/language/nnbd/top_merge/
//! top_merge_members_direct_error_test` — the ND*/DND* shapes whose pins
//! this file extends (`ticket_3877_tests.rs` carries ND0/ND1/ND7):
//!
//! - ND6 `A<FutureOr<void>> ⊔ B<void>`: NORM collapses the `FutureOr<void>`
//!   column to `void`, `void ⊔ void` keeps `void` — every use of `x` reads
//!   `use_of_void_result`, none `unchecked_use_of_nullable_value` (the
//!   census's 18 missing-void rows: ND6/DND6/WND6 trios per family).
//! - ND3 `A<FutureOr<Object?>> ⊔ B<void>`: `Object? ⊔ void` lands the
//!   crossed arm `Object?` — a signature no raw candidate spells, so the
//!   argument row must name `Object?` (the census's 2 wrong-message rows'
//!   mechanism: the first candidate's `FutureOr<Object?>` verbatim).
//! - DND1 `A<FutureOr<Object?> Function()> ⊔ B<dynamic Function()>`: the
//!   deep shape's function columns normalize field-wise, the merge lands
//!   `Object? Function()`, and the trailing call reads `Object?` — the
//!   census's silent DND argument rows.
//!
//! `takesNum` stands in for the census's `takesObject(Object)` — the rig's
//! `dart:core` stub registers `Object` as a name only, so the `Object`
//! formal suppresses (the same latch the #3628 members pins document);
//! `num` is a real declared non-top head.

use super::FileReport;
use super::test_rig::{code_names, report_ops, rows};

/// The `argument_type_not_assignable` row's message, if the code fired.
fn argument_message(report: &FileReport) -> Option<String> {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
}

/// Members face, the `void` collapse (the ND6 shape): the merged column is
/// `void`, so `x.foo`, `x.toString` and `takesNum(x)` each report
/// `use_of_void_result` — never `unchecked_use_of_nullable_value` (that is
/// the un-normalized `FutureOr<void>` face's answer: nullable-by-union,
/// never `void`).
#[test]
fn members_direct_futureor_void_merged_result_reports_void() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class ND6 extends A<FutureOr<void>> implements B<void> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    let void_rows = names
        .iter()
        .filter(|name| **name == "use_of_void_result")
        .count();
    assert!(
        void_rows == 3,
        "expected use_of_void_result on x.foo/x.toString/takesNum(x), got {names:?} ({:?})",
        rows(&report)
    );
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "did not expect unchecked_use_of_nullable_value (the un-normalized FutureOr<void> face), got {names:?}"
    );
}

/// Members face, the crossed arm over a `FutureOr` column (the ND3 shape):
/// `NORM(FutureOr<Object?>) = Object?` and `Object? ⊔ void` lands `Object?`
/// — no raw candidate spells it, so the argument row names the merged
/// `Object?`, and `x.foo` reports the nullable receiver.
#[test]
fn members_direct_futureor_object_question_void_crosses_to_object_question() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class ND3 extends A<FutureOr<Object?>> implements B<void> {
  void test() {
    var self = this;
    var x = self.member();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo (x: Object?), got {names:?}"
    );
    let msg = argument_message(&report);
    assert!(
        msg.as_deref().is_some_and(|m| m.contains("'Object?'")),
        "expected argument_type_not_assignable naming 'Object?', got {msg:?} ({names:?})"
    );
    assert!(
        !msg.as_deref().is_some_and(|m| m.contains("FutureOr")),
        "the argument row still names the un-normalized FutureOr candidate, got {msg:?}"
    );
}

/// Members face, the deep shape (the DND1 shape): the candidates' function
/// columns normalize field-wise (`NORM(FutureOr<Object?> Function()) =
/// Object? Function()`), the merge lands `Object? Function()`, and the
/// trailing call's result is `Object?` — the argument row names `Object?`,
/// not the first candidate's `FutureOr<Object?>` after one application.
///
/// ROUTED RESIDUAL (#3927, invocation.rs send path — the recon's
/// one-application-short face): the merge answer lands — the surviving
/// `unchecked_use_of_nullable_value` on `x.foo` proves `x` reads the
/// merged function column's nullable return — but the trailing call
/// application (`self.member()()`) types `x` one application short of the
/// oracle, so the `takesNum` argument check never sees the merged
/// `Object?` spelling. Application depth of a merged function-typed
/// member result is #3927's arm (a).
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn members_direct_futureor_function_deep_merged_result_reports() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class DND1 extends A<FutureOr<Object?> Function()> implements B<dynamic Function()> {
  void test() {
    var self = this;
    var x = self.member()();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo (x: Object?), got {names:?}"
    );
    let msg = argument_message(&report);
    assert!(
        msg.as_deref().is_some_and(|m| m.contains("'Object?'")),
        "expected argument_type_not_assignable naming 'Object?', got {msg:?} ({names:?})"
    );
    assert!(
        !msg.as_deref().is_some_and(|m| m.contains("FutureOr")),
        "the argument row still names the un-normalized FutureOr candidate, got {msg:?}"
    );
}
