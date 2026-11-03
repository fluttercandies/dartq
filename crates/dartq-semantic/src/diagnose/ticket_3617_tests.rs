//! Ticket #3617 pins — alias-headed arguments inside a bounded type
//! argument (`type_argument_not_matching_bounds`): the super-bounded
//! inversion must run on the WRITTEN trees so an alias application's
//! declared parameter variance shields its subtree the way the oracle's
//! `ReplaceTopBottomVisitor._instantiatedTypeAlias` arm does
//! (`typeParameter.variance.combine(variance)`, upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/element/replace_top_bottom_visitor.dart:78`).
//!
//! The expanded `TypeId`-level `replace_top_bottom_with_facts` walk could
//! not reproduce that shielding: an invariant alias parameter spreads its
//! occurrences across a signature's return and formal positions, and the
//! expanded walk flipped each occurrence independently — `A<G<A<Never>>>`
//! over `typedef G<X> = X Function(X)` fabricated the passing inverted
//! type `C<Never> Function(C<Never>)` and silently dropped the row the
//! oracle reports. The written-level walk keeps `G`'s invariant parameter
//! shielding `A<Never>` as a unit, so the inverted type still fails the
//! regular check and the row surfaces — oracle-verbatim
//! (`dart analyze` over co19 `Instantiate-to-bound/nonfunction_typedef`).
//!
//! Message text is the pin: offsets are fixture-layout-local.

use super::FileReport;
use super::test_rig::{assert_silent, report_ops, rows};

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

/// The rep case (`nonfunction_typedef_l1_t03`): an invariant formals
/// alias `G<X> = X Function(X)` used as the argument of a bounded alias
/// `A<X extends G<C<X>>>`. A `Never`/`dynamic` argument inside `G` must
/// keep `A`'s invariant shielding — `A<G<A<Never>>>` reports the
/// alias-spelled `'G<A<Never>>'` (NOT the expanded `C<Never>
/// Function(C<Never>)` the old expanded walk fabricated into a passing
/// inversion). Oracle rows for `A<G<A<dynamic>>>` (dynamic inverts to
/// `Never`), `A<G<A<Never>>>`, and the nested `A<G<A<G<A<Never>>>>>`/`A`
/// inner-arg rows, oracle-verbatim.
#[test]
fn invariant_alias_arg_reports_with_alias_spelling() {
    let report = report_ops(
        "typedef G<X> = X Function(X);\n\
         class C<X> {}\n\
         typedef A<X extends G<C<X>>> = C<X>;\n\
         typedef F<X> = void Function<Y extends X>();\n\
         F<X> toF<X>(X x) => <Y extends X>() {};\n\
         void test(A source) {\n\
           var fsource = toF(source);\n\
           F<A<G<A<dynamic>>>> target = fsource;\n\
           F<A<G<A<Never>>>> target0 = fsource;\n\
           F<A<G<A<G<dynamic>>>>> target4 = fsource;\n\
           F<A<G<A<G<A<dynamic>>>>>> target5 = fsource;\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report),
        [
            // `A source` — the bare head's instantiate-to-bounds arm:
            // the grounded invariant arg `G<C<Never>>` reports.
            "'G<C<Never>>' doesn't conform to the bound 'G<C<G<C<Never>>>>' of the type parameter 'X'.",
            // A<G<A<dynamic>>> — the dynamic leaf flips to Never; the
            // invariant-shielded A<Never> arg still fails the bound.
            "'G<A<Never>>' doesn't conform to the bound 'G<C<G<A<Never>>>>' of the type parameter 'X'.",
            // A<G<A<Never>>> — oracle-verbatim.
            "'G<A<Never>>' doesn't conform to the bound 'G<C<G<A<Never>>>>' of the type parameter 'X'.",
            // A<G<A<G<dynamic>>> — outer arg 'G<A<G<Never>>>' fails, and
            // the nested G<dynamic>→G<Never> inner-arg row surfaces.
            "'G<A<G<Never>>>' doesn't conform to the bound 'G<C<G<A<G<Never>>>>>' of the type parameter 'X'.",
            "'G<Never>' doesn't conform to the bound 'G<C<G<Never>>>' of the type parameter 'X'.",
            // A<G<A<G<A<dynamic>>>> — 'G<A<G<A<Never>>>>' + inner
            // 'G<A<Never>>'.
            "'G<A<G<A<Never>>>>' doesn't conform to the bound 'G<C<G<A<G<A<Never>>>>>>' of the type parameter 'X'.",
            "'G<A<Never>>' doesn't conform to the bound 'G<C<G<A<Never>>>>' of the type parameter 'X'.",
        ],
        "invariant-alias arg rows must keep the alias spelling: {:#?}",
        rows(&report)
    );
}

/// The bare-head (`raw`) arm of the same family: `A source` / `A a = A()`
/// instantiate `A`'s `X` to the grounded bound `G<C<X>>` grounded to
/// `G<C<Never>>` — the invariant `G` shield keeps `C<Never>` as the
/// oracle's `'G<C<Never>>' doesn't conform to 'G<C<G<C<Never>>>>'` (the
/// old walk's expanded `C<Never> Function(C<Never>)` fabricated a pass).
#[test]
fn bare_alias_head_reports_grounded_invariant() {
    let report = report_ops(
        "typedef G<X> = X Function(X);\n\
         class C<X> {}\n\
         typedef A<X extends G<C<X>>> = C<X>;\n\
         void test(A source) {}\n\
         main() {\n\
           A a = A();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report),
        [
            "'G<C<Never>>' doesn't conform to the bound 'G<C<G<C<Never>>>>' of the type parameter 'X'.",
            "'G<C<Never>>' doesn't conform to the bound 'G<C<G<C<Never>>>>' of the type parameter 'X'.",
        ],
        "bare A heads must report the grounded invariant arg: {:#?}",
        rows(&report)
    );
}

/// The self-referential alias-bound family (`typedef_typedef_l1_t10`):
/// `typedef A<X> = void Function(X); typedef G<X extends A<X>> = X
/// Function(X);` — `G<A<Null>>` checks the arg `A<Null>` against `G`'s
/// bound `A<A<Null>>` (`A` covariant: `Null Function(Null)` vs `A<Null>
/// Function(A<Null>)` → `Null <: A<Null>` fails), and `G<A<A<Null>>>`
/// nests one level deeper. The alias-spelled `'A<Null>'`/`'A<A<Null>>'`
/// rows are oracle-verbatim.
#[test]
fn self_alias_bound_reports() {
    let report = report_ops(
        "typedef A<X> = void Function(X);\n\
         typedef G<X extends A<X>> = X Function(X);\n\
         typedef F<X> = void Function<Y extends X>();\n\
         F<X> toF<X>(X x) => <Y extends X>() {};\n\
         void test(G source) {\n\
           var fsource = toF(source);\n\
           F<G<A<Null>>> t1 = fsource;\n\
           F<G<A<A<Null>>>> t2 = fsource;\n\
         }\n",
    );
    let messages = bounds_messages(&report);
    assert_eq!(
        messages.len(),
        2,
        "the self-alias bound family must report both arg levels: {:#?}",
        rows(&report)
    );
    assert_eq!(
        messages[0],
        "'A<Null>' doesn't conform to the bound 'A<A<Null>>' of the type parameter 'X'.",
    );
    assert_eq!(
        messages[1],
        "'A<A<Null>>' doesn't conform to the bound 'A<A<A<Null>>>' of the type parameter 'X'.",
    );
}

/// Negative control — a well-bounded alias-headed argument stays silent:
/// `A<dynamic>` (`dynamic` satisfies `G<C<X>>` bound) and `A<Never>`
/// (`Never` is bottom — satisfies every bound). The written-level
/// inversion must NOT fabricate a report on already-passing arguments.
#[test]
fn well_bounded_alias_args_stay_silent() {
    let report = report_ops(
        "typedef G<X> = X Function(X);\n\
         class C<X> {}\n\
         typedef A<X extends G<C<X>>> = C<X>;\n\
         typedef F<X> = void Function<Y extends X>();\n\
         F<X> toF<X>(X x) => <Y extends X>() {};\n\
         void test(C source) {\n\
           var fsource = toF(source);\n\
           F<A<dynamic>> target2 = fsource;\n\
           F<A<Never>> target6 = fsource;\n\
         }\n",
    );
    assert!(
        bounds_messages(&report).is_empty(),
        "well-bounded alias args must not report: {:#?}",
        rows(&report)
    );
}

/// Negative control — a plain non-alias bounded mismatch still reports
/// through the same verdict path (the inversion change must not
/// suppress the ordinary `C<int>` vs `X extends num` row), and its
/// well-bounded twin stays silent.
#[test]
fn non_alias_bound_mismatch_still_reports() {
    let report = report_ops(
        "class C<X extends num> {}\n\
         C<int> ok() => C<int>();\n\
         C<String> bad() => C<String>();\n",
    );
    // The verifier checks every `NamedType` position: the `C<String>`
    // return-type annotation AND the `C<String>()` constructor argument
    // each report the same bound violation (oracle: one row per site).
    assert_eq!(
        bounds_messages(&report),
        [
            "'String' doesn't conform to the bound 'num' of the type parameter 'X'.",
            "'String' doesn't conform to the bound 'num' of the type parameter 'X'.",
        ],
        "the non-alias bounds row must keep reporting at each site: {:#?}",
        rows(&report)
    );
}
