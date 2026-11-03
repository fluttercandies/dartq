//! The #3839 `type_argument_not_matching_bounds` pins for the invocation
//! type-argument bounds face (`check_call_type_arg_bounds`,
//! `_checkInvocationTypeArguments`, upstream Dart SDK source
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart:542-614).
//!
//! The verdict compares the WRITTEN argument against the substituted
//! bound, and upstream's `isSubtypeOf` reads a `TypeAliasElement` use
//! through its expansion — the alias survives only as the
//! `getDisplayString` decoration the message prints (`element.dart`'s
//! `TypeAliasElementImpl.instantiate`). Judging the bare written head
//! instead dropped every alias whose right-hand side is a FUNCTION type:
//! the co19 extension-type semantics suite's
//! `typedef num Foo(Object o)` against `T extends Object Function(num)`
//! (`V1<Foo>(foo)`) reported
//! `'Foo' doesn't conform to the bound 'Object Function(num)'` — three
//! rows oracle 3.13.7 never reports.
//!
//! Every expectation below is oracle-pinned (3.13.7, `dart analyze
//! --format=machine` on the same fixture text), and the positive rows
//! pin the MESSAGE number too: the argument keeps its written alias
//! spelling (`'Bar'`, `'String2'`, `'Exactly<int>'`), never the
//! expansion the judgment used.

use crate::diagnose::test_rig::{report_ops, rows};

/// The `type_argument_not_matching_bounds` message texts, source order.
fn bound_rows(source: &str) -> Vec<String> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| d.message.to_string())
        .collect()
}

fn assert_no_bound_rows(source: &str) {
    let got = bound_rows(source);
    assert!(
        got.is_empty(),
        "expected no bounds rows, got {got:?} (all rows: {:?})",
        rows(&report_ops(source))
    );
}

/// `typedef num Foo(Object o)` — a function typedef whose expansion
/// (`num Function(Object)`) IS a subtype of the bound
/// `Object Function(num)` (the parameter slot reads contravariantly).
/// Oracle 3.13.7: silent.
#[test]
fn ticket_3839_function_typedef_argument_satisfying_bound_silent() {
    let source = "\
typedef num Foo(Object o);
void g<T extends Object Function(num)>() {}
void f() {
  g<Foo>();
}";
    assert_no_bound_rows(source);
}

/// The un-aliased twin of the same shape — written inline, the argument
/// is a function type up front, so the fold never applies and the
/// silence must come from the subtype test alone (it did before the fix;
/// this keeps the two paths distinguishable).
#[test]
fn ticket_3839_inline_function_type_argument_satisfying_bound_silent() {
    let source = "\
void g<T extends Object Function(num)>() {}
void f() {
  g<num Function(Object)>();
}";
    assert_no_bound_rows(source);
}

/// The alias form of a function type that does NOT satisfy the bound
/// (`num Function(Object)` is not a `num`): oracle 3.13.7 reports
/// `'Bar' doesn't conform to the bound 'num' of the type parameter 'T'.`
/// — the WRITTEN spelling, not the expansion. This is the pin that keeps
/// the judgement-side fold from reaching the message.
#[test]
fn ticket_3839_function_typedef_argument_violating_bound_reports_written_spelling() {
    let source = "\
typedef num Bar(Object o);
void h<T extends num>() {}
void f() {
  h<Bar>();
}";
    assert_eq!(
        bound_rows(source),
        vec!["'Bar' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()]
    );
}

/// A class-headed alias still reports with its own spelling (oracle
/// 3.13.7: `'String2' doesn't conform to the bound 'num' …`) — the
/// judgment folds it, the message does not.
#[test]
fn ticket_3839_class_typedef_argument_violating_bound_reports_written_spelling() {
    let source = "\
typedef String2 = String;
void h<T extends num>() {}
void f() {
  h<String2>();
}";
    assert_eq!(
        bound_rows(source),
        vec!["'String2' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()]
    );
}

/// The plain non-conforming argument (no alias involved) keeps its row —
/// the fold must not silence the ordinary verdict.
#[test]
fn ticket_3839_plain_argument_violating_bound_reports() {
    let source = "\
void h<T extends num>() {}
void f() {
  h<String>();
}";
    assert_eq!(
        bound_rows(source),
        vec!["'String' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()]
    );
}

/// A non-conforming argument against a FUNCTION bound (`String` is not
/// `Object Function(num)`): oracle 3.13.7 reports, and the bound renders
/// as written.
#[test]
fn ticket_3839_non_conforming_argument_against_function_bound_reports() {
    let source = "\
void g<T extends Object Function(num)>() {}
void f() {
  g<String>();
}";
    assert_eq!(
        bound_rows(source),
        vec![
            "'String' doesn't conform to the bound 'Object Function(num)' of the type parameter 'T'."
                .to_string()
        ]
    );
}

/// The satisfying argument (`int` against `T extends num`) stays silent.
#[test]
fn ticket_3839_satisfying_argument_silent() {
    let source = "\
void h<T extends num>() {}
void f() {
  h<int>();
}";
    assert_no_bound_rows(source);
}

/// The extension-member face (`R extends Exactly<T>`, the receiver's own
/// type instantiated into the bound): `int` over `Exactly<int>` conforms,
/// so nothing reports (oracle 3.13.7: silent). This pins the
/// `extension_member_bounds` table the corpus's `expectStaticType` sites
/// ride — the receiver-type drift those rows carry is a separate face.
#[test]
fn ticket_3839_extension_bound_satisfied_silent() {
    let source = "\
typedef Exactly<T> = T Function(T);
extension StaticType<T> on T {
  T expectStaticType<R extends Exactly<T>>() => this;
}
void f() {
  int i = 1;
  i.expectStaticType<Exactly<int>>();
}";
    assert_no_bound_rows(source);
}

/// The same extension face over a `num` receiver with an `Exactly<int>`
/// argument: `int Function(int)` is not a subtype of
/// `num Function(num)` (the parameter slot is contravariant), so oracle
/// 3.13.7 reports against the substituted bound.
#[test]
fn ticket_3839_extension_bound_violated_reports_substituted_bound() {
    let source = "\
typedef Exactly<T> = T Function(T);
extension StaticType<T> on T {
  T expectStaticType<R extends Exactly<T>>() => this;
}
void f() {
  num n = 1;
  n.expectStaticType<Exactly<int>>();
}";
    assert_eq!(
        bound_rows(source),
        vec![
            "'Exactly<int>' doesn't conform to the bound 'Exactly<num>' of the type parameter 'R'."
                .to_string()
        ]
    );
}
