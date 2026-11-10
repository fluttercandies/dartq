//! Ticket #3675 pins: alias-preserving type rendering in diagnostic
//! messages (census-12 lane A — the display-only shapes).
//!
//! The oracle's `getDisplayString(preferTypeAlias)` prints the typedef
//! alias the resolved type carries — `TypeImpl.alias` rides the
//! instantiated type object upstream
//! (`display_string_builder.dart:379-382`), so a type instantiated to
//! bounds still spells its alias head (`'A<Never>'`, never the
//! `Never Function(Never)` expansion). The snapshot's interned types are
//! pure expansions: the alias identity never outlives `MapCtx::map_tree`,
//! so the message faces that render a `TypeId` directly lost the
//! spelling. The bounds face's bare-head arm now renders the
//! instantiate-to-bounds arguments through the WRITTEN bound trees —
//! `Snapshot::grounded_bound_trees` replays the grounding substitution
//! on trees and `display_written_argument` keeps the alias head through
//! the per-position top/bottom flip — against the oracle's exact texts.

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

/// Shape 3.6 — the bare generic alias argument: `foo(G g)` over
/// `typedef G<X extends A<X>> = void Function<Y extends X>()` reports the
/// inverted-pass row, and both its slots keep the alias spelling the
/// written bound `A<X>` carries. Oracle row
/// (`TypeArgumentNotMatchingBoundsTest/
/// test_notRegularBounded_notSuperBounded_parameter_invariant`), exact
/// text.
#[test]
fn bare_alias_head_argument_keeps_the_alias_spelling() {
    let report = report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef G<X extends A<X>> = void Function<Y extends X>();\n\
         foo(G g) {}\n",
    );
    assert_eq!(
        bounds_messages(&report),
        ["'A<Never>' doesn't conform to the bound 'A<A<Never>>' of the type parameter 'X'."],
        "{:?}",
        bounds_messages(&report)
    );
}

/// Two parameters each carrying a cyclic alias bound ground
/// independently — `X extends A<X>` and `Y extends A<Y>` report one row
/// apiece, each the parameter's own bound under its own grounding leaf
/// (the `A<Never>`/`A<A<Never>>` pair the oracle's inverted pass
/// spells).
#[test]
fn each_cyclic_parameter_grounds_to_its_own_bound() {
    let report = report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef H<X extends A<X>, Y extends A<Y>> = void Function();\n\
         foo(H h) {}\n",
    );
    assert_eq!(
        bounds_messages(&report),
        [
            "'A<Never>' doesn't conform to the bound 'A<A<Never>>' of the type parameter 'X'.",
            "'A<Never>' doesn't conform to the bound 'A<A<Never>>' of the type parameter 'Y'.",
        ],
        "{:?}",
        bounds_messages(&report)
    );
}

/// A bound naming a fellow parameter grounds through that parameter's
/// own answer: `Y extends X` beside `X extends A<X>` instantiates `Y` to
/// `A<dynamic>` (the dependent of a cycle completes with the member's
/// substituted default, `default_types_builder.dart:164-180`), which
/// satisfies `Y`'s substituted bound — only `X` reports.
#[test]
fn dependent_parameter_grounds_through_the_cycle_member() {
    let report = report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef H<X extends A<X>, Y extends X> = void Function();\n\
         foo(H h) {}\n",
    );
    assert_eq!(
        bounds_messages(&report),
        ["'A<Never>' doesn't conform to the bound 'A<A<Never>>' of the type parameter 'X'."],
        "{:?}",
        bounds_messages(&report)
    );
}

/// Clause sites forbid the super-bounded pass upstream
/// (`_shouldAllowSuperBoundedTypes`, type_arguments_verifier.dart:
/// 737-748), so `class C extends G` reports the regular pass's grounded
/// `dynamic` leaf — `'A<dynamic>'` against `'A<A<dynamic>>'` — the same
/// written spelling the inverted pass flips to `Never`.
#[test]
fn clause_site_regular_pass_keeps_the_dynamic_grounding() {
    let report = report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef G<X extends A<X>> = void Function();\n\
         class C extends G {}\n",
    );
    assert_eq!(
        bounds_messages(&report),
        ["'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'X'."],
        "{:?}",
        bounds_messages(&report)
    );
}

/// Negative control: an unbounded parameter contributes no row (its
/// `dynamic` grounding satisfies the implicit `Object?` bound), and a
/// class-headed bound renders its ordinary written head — the grounding
/// never invents an alias the source did not write.
#[test]
fn unbounded_and_class_headed_bounds_keep_their_faces() {
    let report = report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef H<X, Y extends A<Y>> = void Function();\n\
         foo(H h) {}\n",
    );
    assert_eq!(
        bounds_messages(&report),
        ["'A<Never>' doesn't conform to the bound 'A<A<Never>>' of the type parameter 'Y'."],
        "{:?}",
        bounds_messages(&report)
    );
}
