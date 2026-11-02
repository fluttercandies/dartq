//! #3616 pins — the super-bounded inversion family of
//! `type_argument_not_matching_bounds` (census F3).
//!
//! The rule the oracle applies (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/element/replace_top_bottom_visitor.dart`,
//! the arm at the top of `process`): a contravariant `S` becomes `Object?`
//! iff `S <: Never`. Two defects sat on that rule:
//!
//! * the message mirror read the written spelling instead of the relation,
//!   so `Never?` (the `Null` projection — `Null <: Never` is false) flipped
//!   to `Object?` in every reported argument (`'Never Function(Object?)'`
//!   where the oracle prints `'Never Function(Never?)'`);
//! * a use-site type-parameter id carried no bound at the check, so
//!   `N extends Never` never satisfied `N <: Never`: the inverted pass ran
//!   unflipped, failed, and fabricated a row (`super_bounded_types_test`
//!   `FcovCyclicCoBound<Function(N)>`), while the same blindness made the
//!   blanket nested gate swallow `N extends Null`'s genuine rows
//!   (`super_bounded_types_error2_test` `CFcon<Fcon<N>>`).
//!
//! Every expectation below is the oracle row of the corresponding census
//! case (`conformance/expected.tsv`, pins `super_bounded_types_error2_test`
//! / `super_bounded_types_test`), same message text.

use super::FileReport;
use super::test_rig::report_ops;

/// The `type_argument_not_matching_bounds` messages of one report, in
/// report order.
fn bounds_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "type_argument_not_matching_bounds")
        .map(|row| row.message.to_string())
        .collect()
}

/// The shared super-bounded alias/class face the census fixtures declare
/// (`super_bounded_types_error2_test`): `FcovCyclicCoBound<X>` keeps `X`
/// in its return position (covariant), `Fcon<X>` in a formal (contra-
/// variant), `Function(X)` is the cyclic bound.
const SUPER_BOUNDED_SCAFFOLD: &str = "\
typedef Fcon<X> = Function(X);
typedef Fcov<X> = X Function();
typedef FcovCyclicBound<X extends A<X>> = X Function();
typedef FcovCyclicCoBound<X extends Function(X)> = X Function();
class A<X> {}
class CFcon<X extends Fcon<X>> {}
class CFcov<X extends Fcov<X>> {}
";

/// `S <: Never` is a relation, not a spelling: `Never?` is `Null` and never
/// flips, while a genuine `Never` leaf does (the `FcovCyclicCoBound<
/// Function(Never)>` control below stays super-bounded because it flips).
///
/// Oracle rows (`super_bounded_types_error2_test`,
/// `testContravariantSuperboundError<N extends Null>`), exact text.
#[test]
fn never_question_leaf_keeps_the_null_projection() {
    let source = format!(
        "{SUPER_BOUNDED_SCAFFOLD}\
void testContravariantSuperboundError<N extends Null>() {{
  FcovCyclicCoBound<Function(Never?)> x1;
  FcovCyclicCoBound<Function(Never)> x2;
  FcovCyclicCoBound<Function(Null)> x3;
  CFcon<Fcon<Never?>> x4;
  CFcon<Fcon<Never>> x5;
  CFcon<Fcon<Null>> x6;
  CFcon<Fcon<N>> x7;
}}
"
    );
    let report = report_ops(&source);
    let mut messages = bounds_messages(&report);
    messages.sort();
    let mut expected = vec![
        // The `Never?` arguments: written spelling, no flip.
        "'Never Function(Never?)' doesn't conform to the bound 'dynamic \
         Function(Never Function(Never?))' of the type parameter 'X'."
            .to_string(),
        "'Fcon<Never?>' doesn't conform to the bound 'Fcon<Fcon<Never?>>' \
         of the type parameter 'X'."
            .to_string(),
        // `Null` (the same type, spelled) behaves identically.
        "'Never Function(Null)' doesn't conform to the bound 'dynamic \
         Function(Never Function(Null))' of the type parameter 'X'."
            .to_string(),
        "'Fcon<Null>' doesn't conform to the bound 'Fcon<Fcon<Null>>' of \
         the type parameter 'X'."
            .to_string(),
        // The site-scope parameter: `N extends Null`, so no flip either —
        // and the row the blanket nested gate used to swallow.
        "'Fcon<N>' doesn't conform to the bound 'Fcon<Fcon<N>>' of the \
         type parameter 'X'."
            .to_string(),
    ];
    expected.sort();
    assert_eq!(messages, expected, "rows: {:#?}", report.diagnostics);
}

/// Both directions in ONE file: the same parameter name declared with
/// different bounds in two scopes decides per site — `N extends Null`
/// reports the genuine row, `N extends Never` is super-bounded (the
/// inverted pass flips `N`, so it never reports).
///
/// Oracle rows: `super_bounded_types_error2_test` (`N extends Null`,
/// `'Fcon<N>'` reported) and `super_bounded_types_test` (`N extends Never`,
/// no bounds rows at all).
#[test]
fn bottom_bounded_parameter_is_super_bounded_not_a_fabrication() {
    let source = format!(
        "{SUPER_BOUNDED_SCAFFOLD}\
void nullBounded<N extends Null>() {{
  CFcon<Fcon<N>> reports;
}}
void neverBounded<N extends Never>() {{
  CFcon<Fcon<N>> silent;
  CFcon<Fcon<Never>> superBounded;
}}
"
    );
    let report = report_ops(&source);
    assert_eq!(
        bounds_messages(&report),
        vec![
            "'Fcon<N>' doesn't conform to the bound 'Fcon<Fcon<N>>' of the \
             type parameter 'X'."
                .to_string()
        ],
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The covariant family of the same parameter (`super_bounded_types_test`,
/// `testContravariantSuperbound<N extends Never>` — every row there is an
/// engine fabrication): with `N`'s bound resolved the inverted pass flips
/// `N` in the contravariant positions and the whole set stays silent.
#[test]
fn never_bounded_parameter_covariant_family_stays_silent() {
    let source = format!(
        "{SUPER_BOUNDED_SCAFFOLD}\
void testContravariantSuperbound<N extends Never>() {{
  FcovCyclicCoBound<Function(N)> x20;
  CFcon<Fcon<N>> x22;
  FcovCyclicBound<A<N>> x6;
  CFcov<Fcov<N>> x16;
}}
"
    );
    let report = report_ops(&source);
    assert!(
        bounds_messages(&report).is_empty(),
        "fabricated rows: {:#?}",
        report.diagnostics
    );
}

/// Two-way control — the ordinary instantiate-to-bounds / simple-bounds
/// faces do not move. `X?` against the cyclic bound `A<X?>` is a regular,
/// inverted-identical failure (co19 `Instantiate-to-bound/class/static/
/// class_l1_t08`'s only oracle row, exact text).
#[test]
fn instantiate_to_bound_class_static_control_does_not_move() {
    let source = "class A<X extends A<X?>> {}\n";
    let report = report_ops(source);
    assert_eq!(
        bounds_messages(&report),
        vec![
            "'X?' doesn't conform to the bound 'A<X?>' of the type \
             parameter 'X'."
                .to_string()
        ],
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The simple-bounds control (co19 `Simple-bounds/static/typedef_l1_t04`'s
/// row shape): a plain `X extends num` miss still reports once, with the
/// oracle's text.
#[test]
fn simple_bounds_control_does_not_move() {
    let source = "\
class B<X extends num> {}
void f() {
  B<num> ok;
  B<Object> bad;
}
";
    let report = report_ops(source);
    assert_eq!(
        bounds_messages(&report),
        vec![
            "'Object' doesn't conform to the bound 'num' of the type \
             parameter 'X'."
                .to_string()
        ],
        "rows: {:#?}",
        report.diagnostics
    );
}
