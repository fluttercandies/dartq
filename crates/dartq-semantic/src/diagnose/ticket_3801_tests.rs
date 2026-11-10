//! The #3801 pins: which parameters a raw-creation inference reports at all.
//!
//! `chooseTypes` solves the parameters in declaration order, substituting the
//! bound with the types inferred *so far*
//! (`substituteTypeFromIterables`, type_analyzer_operations.dart:1080-1145),
//! and `inferTypeParameterFromAll` leaves a parameter `unknown` when it has no
//! declared bound (a null `extendsConstraint`) or when its substituted bound
//! is still exactly a bare parameter of this inference — a bare `unknown`
//! merges into the empty upper and `chooseTypeFromConstraint` answers the
//! unknown lower back (:1210-1273, :1018-1075). An `unknown` parameter answers
//! no row and prints `_` inside a reported parameter's substituted bound.
//!
//! Both pins below are byte-exact against the pinned oracle (Dart SDK 3.13.7)
//! `dart analyze --format=machine` rows for the same fixtures.
//!
//! Only the row *selection* is pinned here plus the substituted-bound texts
//! the twin (`type_argument_not_matching_bounds`) already spells correctly;
//! the two fixtures whose `producing` text carries a `_` slot pin the count
//! and the parameter name, because the `could_not_infer` detail builder still
//! reads `CreationInferenceRow::bound` rather than its `infer_bound` twin.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The messages of every `name` row at `offset:len`.
fn messages_at<'r>(report: &'r FileReport, name: &str, offset: usize, len: usize) -> Vec<&'r str> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name
                && d.range.offset as usize == offset
                && d.range.length as usize == len
        })
        .map(|d| d.message.as_str())
        .collect()
}

/// The `type parameter 'X'.` opener the row's text carries.
fn names_parameter(message: &str, parameter: &str) -> bool {
    message.starts_with(&format!("Couldn't infer type parameter '{parameter}'."))
}

/// `class A<X1 extends X2, X2 extends A<X1, X2>> {}` with the bare creation
/// `A();` — `X1`'s bound is the bare parameter `X2`, so `X1` stays unknown:
/// one row naming `X2`, never two. Oracle
/// `Instantiate-to-bound/class/static/class_l2_t01` 66:3.
#[test]
fn ticket_3801_bare_parameter_bound_answers_no_row() {
    let src = r#"
class A<X1 extends X2, X2 extends A<X1, X2>> {}
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    let infer = messages_at(&report, "could_not_infer", offset, 1);
    assert_eq!(infer.len(), 1, "one row, not two; got {:#?}", rows(&report));
    assert!(
        names_parameter(infer[0], "X2"),
        "the reported parameter is X2: {:#?}",
        rows(&report)
    );
    let twin = messages_at(&report, "type_argument_not_matching_bounds", offset, 1);
    assert_eq!(twin.len(), 1, "one twin, not two; got {:#?}", rows(&report));
    assert!(
        twin[0].contains("of the type parameter 'X2'."),
        "the twin names X2: {:#?}",
        rows(&report)
    );
}

/// `class B<X extends B<X>> {} class A<X1 extends X2, X2 extends B<X2>> {}`
/// with `A();` — `X1` stays unknown, `X2` is solved to `B<Object?>` and fails
/// its own F-bound. Byte-exact against the oracle's 72:3 pair
/// (`Instantiate-to-bound/class/static/class_l2_t08`), in both faces.
#[test]
fn ticket_3801_unsolved_sibling_is_silent_and_the_solved_one_reports() {
    let src = r#"
class B<X extends B<X>> {}
class A<X1 extends X2, X2 extends B<X2>> {}
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    assert_eq!(
        messages_at(&report, "could_not_infer", offset, 1),
        [
            "Couldn't infer type parameter 'X2'.\n\nTried to infer 'B<Object?>' for 'X2' which \
          doesn't work:\n  Type parameter 'X2' is declared to extend 'B<X2>' producing \
          'B<B<Object?>>'.\n\nConsider passing explicit type argument(s) to the generic.\n\n"
        ],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        messages_at(&report, "type_argument_not_matching_bounds", offset, 1),
        ["'B<Object?>' doesn't conform to the bound 'B<B<Object?>>' of the type parameter 'X2'."],
        "{:#?}",
        rows(&report)
    );
}

/// The alias twin: `typedef A<X1 extends B<X1, X2>, X2> = B<X1, X2>` over
/// `class B<X extends B<X, Y>, Y>` with `A();` — `X1` *is* solved (its bound
/// is structured, not a bare parameter) and reports, while the unbounded `X2`
/// stays unknown. Byte-exact twin against the oracle's 77:3 row
/// (`Instantiate-to-bound/nonfunction_typedef/static/nonfunction_l2_t06`),
/// which spells the unsolved `X2` slot as `dynamic`.
#[test]
fn ticket_3801_alias_leaves_the_unbounded_parameter_unsolved() {
    let src = r#"
class B<X extends B<X, Y>, Y> {}
typedef A<X1 extends B<X1, X2>, X2> = B<X1, X2>;
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    let infer = messages_at(&report, "could_not_infer", offset, 1);
    assert_eq!(infer.len(), 1, "one row, not two; got {:#?}", rows(&report));
    assert!(
        names_parameter(infer[0], "X1"),
        "the reported parameter is X1: {:#?}",
        rows(&report)
    );
    assert_eq!(
        messages_at(&report, "type_argument_not_matching_bounds", offset, 1),
        [
            "'B<Object?, Object?>' doesn't conform to the bound 'B<B<Object?, Object?>, dynamic>' \
          of the type parameter 'X'."
        ],
        "{:#?}",
        rows(&report)
    );
}

/// The under-report direction: `class A<X1 extends X2, X2> {}` with `A();`
/// solves nothing (`X1`'s bound is the bare unsolved `X2`, `X2` has no bound),
/// so the creation is silent in both faces — the oracle reports nothing at
/// all for this fixture.
#[test]
fn ticket_3801_all_parameters_unsolved_stays_silent() {
    let src = r#"
class A<X1 extends X2, X2> {}
main() {
  A();
}
"#;
    let report = report_ops(src);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "could_not_infer"
                && d.code.name != "type_argument_not_matching_bounds"),
        "unsolved parameters report nothing; got {:#?}",
        rows(&report)
    );
}
