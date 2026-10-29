//! The #2841 pins: a bare callee spelling a GROUNDED NON-FUNCTION type
//! alias is the aliased class's unnamed creation with implicit arguments —
//! upstream routes the alias constructor invocation through the aliased
//! type (`pkg/analyzer`'s instance-creation resolve over the alias
//! element), so no invocation row reports and the raw-constructor
//! inference face (#2831 P1) decides the span keyed on the ALIAS's own
//! parameters. Message text is the pinned oracle row of
//! `Instantiate-to-bound/nonfunction_typedef/static/nonfunction_l1_t01`
//! (94:3). The misroute this fixes: 29 extra
//! `invocation_of_non_function_expression` rows across the
//! nonfunction_typedef suites (A5 recon P4).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// Exactly one row of `name` at `offset:len` carrying `message`.
#[track_caller]
fn assert_row(report: &FileReport, name: &str, offset: usize, len: usize, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name
                && d.range.offset as usize == offset
                && d.range.length as usize == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {name} @ {offset}:{len}; got {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

/// No row of `name` anywhere.
#[track_caller]
fn assert_none(report: &FileReport, name: &str) {
    assert!(
        report.diagnostics.iter().all(|d| d.code.name != name),
        "expected no {name}; got {:#?}",
        rows(report)
    );
}

/// The alias-creation inference message, byte-exact against the oracle's
/// `nonfunction_l1_t01` 94:3 row (`typedef A<X extends C<X>> = C<X>;` —
/// the alias's own parameter is the inferred one).
const ALIAS_INFER_MESSAGE: &str = "Couldn't infer type parameter 'X'.\n\nTried to infer \
    'C<Object?>' for 'X' which doesn't work:\n  Type parameter 'X' is declared to \
    extend 'C<X>' producing 'C<C<Object?>>'.\n\nConsider passing explicit type \
    argument(s) to the generic.\n\n";

/// The corpus shape (`nonfunction_l1_t01`): the unsatisfiable self-bound
/// alias creation reports `could_not_infer` at the spelling over the
/// ALIAS's parameter, and no invocation row misroutes. The oracle's
/// creation-site `type_argument_not_matching_bounds` twin is the expanded
/// class-side check — no twin here (the oracle has none for this shape
/// either).
#[test]
fn grounded_alias_creation_infers_the_alias_parameters() {
    let src = r#"
class C<X extends C<X>> {}
typedef A<X extends C<X>> = C<X>;
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    assert_row(&report, "could_not_infer", offset, 1, ALIAS_INFER_MESSAGE);
    assert_none(&report, "invocation_of_non_function_expression");
    assert_none(&report, "invocation_of_non_function");
    assert_none(&report, "type_argument_not_matching_bounds");
}

/// The materialized registry shape (`_misc`
/// InstanceCreationTestCases/test_typeAlias_generic_class_generic_unnamed_infer_all#0,
/// oracle silent): a satisfiable alias creation with an argument keeps
/// every face silent — the misroute's extra row dies.
#[test]
fn satisfiable_alias_creation_stays_silent() {
    let src = r#"
class A<T> {
  A(T t);
}
typedef B<U> = A<U>;
void f() {
  B(0);
}
"#;
    let report = report_ops(src);
    assert_none(&report, "could_not_infer");
    assert_none(&report, "invocation_of_non_function_expression");
    assert_none(&report, "invocation_of_non_function");
    assert_none(&report, "type_argument_not_matching_bounds");
}

/// A genuine non-function callee (a local holding an int) keeps its
/// invocation report — the route fix must not widen the silence.
#[test]
fn genuine_non_function_callee_still_reports() {
    let src = r#"
main() {
  int x = 0;
  x();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "x();");
    assert_row(
        &report,
        "invocation_of_non_function_expression",
        offset,
        1,
        "The expression doesn't evaluate to a function, so it can't be invoked.",
    );
}

/// A local shadowing the alias spells the VARIABLE, not the type: the
/// route gate skips and the ordinary invocation face reports.
#[test]
fn local_shadowing_the_alias_keeps_the_invocation_report() {
    let src = r#"
class C<X> {}
typedef A<X> = C<X>;
main() {
  int A = 0;
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    assert_row(
        &report,
        "invocation_of_non_function_expression",
        offset,
        1,
        "The expression doesn't evaluate to a function, so it can't be invoked.",
    );
}

/// A function-type alias callee keeps its registry verdict
/// (`invocation_of_non_function/doc_0` — the messages.yaml example, a
/// passing corpus case): the route's grounded gate never catches the
/// function-shaped RHS.
#[test]
fn function_alias_callee_keeps_the_registry_verdict() {
    let src = r#"
typedef Binary = int Function(int, int);
int f() {
  return Binary(1, 2);
}
"#;
    let report = report_ops(src);
    let offset = at(src, "Binary(1, 2)");
    assert_row(
        &report,
        "invocation_of_non_function",
        offset,
        6,
        "'Binary' isn't a function.",
    );
}
