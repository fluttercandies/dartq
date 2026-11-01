//! #3575 pins — the `type_argument_not_matching_bounds` over-fire family:
//! four display/verdict roots the 20261102-014958 census isolated in the
//! super-bounded and instantiate-to-bound corpora, each pinned by the
//! oracle's own row text on that corpus.
//!
//! # The defects
//!
//! * **The unpromoted extension receiver** (the `flow_analysis` family,
//!   1052 of the 2879 extras): the extension-member bounds face bound the
//!   `on T` parameter to the receiver's push-time face, so a flow-promoted
//!   local (`if (i1 != null) i1.expectStaticType<Exactly<int>>()`) checked
//!   `Exactly<int>` against the UNPROMOTED `Exactly<int?>` — a row the
//!   oracle (which reads the instantiated member type over the promoted
//!   receiver) never reports. The promoted face joins only after pass 1
//!   (`Engine::read_types` fills post-walk), so a potentially-nullable
//!   frozen receiver face now ABSTAINS — the under-report direction; a
//!   non-nullable face cannot be null-narrowed and decides as before.
//! * **The facts-less message inversion** (the `typedef_l1` family): the
//!   verdict's inverted pass runs `replace_top_bottom_with_facts`, where an
//!   alias parameter's computed variance decides that nothing flips; the
//!   message's substitution source ran the variance-blind
//!   `replace_top_bottom`, flipping a contravariant-position `dynamic` to
//!   `Never` and printing `A<A<Never>>` where the oracle prints
//!   `A<A<dynamic>>`.
//! * **The expanded argument/bound spelling** (the `Simple-bounds` and
//!   `super_bounded_types_error` families): the oracle's message keeps an
//!   alias application's decoration (`'G<Null>'`, bound `'G<dynamic>'`) —
//!   `getDisplayString` writes the alias, only the subtype test unaliases —
//!   while the fully expanded model printed `Never Function(Null)`. The
//!   message now renders the WRITTEN argument tree (the inverted pass's
//!   top/bottom flip re-applied per position) and substitutes the bound's
//!   leaves through those rendered texts.
//! * **The lazy-parameter id collision**: `param_scope` allocated every
//!   fresh site-side name at one counter value without advancing it, so
//!   two distinct parameters shared an id and the shared id displayed
//!   under whichever name the text scan found first (`Never Function(Y)`
//!   for a written `Function(N)`).
//! * **The stacked nullability**: a nullable bound leaf over a nullable
//!   substituted argument printed `X??` — substitution replaces the
//!   parameter wholesale, so `X?` over `X := X?` prints `X?`.

use super::test_rig::report_ops;

/// The `type_argument_not_matching_bounds` messages of `report`, in source
/// order — the only rows these fixtures assert on.
fn bounds_rows(source: &str) -> Vec<String> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| d.message.to_string())
        .collect()
}

/// The promoted-receiver row the corpus carries 1052 times: the
/// promotion-latched nullable receiver abstains, so the matching argument
/// under `if (i1 != null)` stays silent (the unpromoted `int?` bound fired
/// `Exactly<int?>` before). `Exactly` is the corpus mock's own typedef
/// (`T Function(T)`), whose invariance is what makes the unpromoted pair
/// fail.
#[test]
fn promoted_extension_receiver_binds_the_flow_face() {
    let rows = bounds_rows(
        "typedef Exactly<T> = T Function(T);\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int? i1 = 1;\n\
         \x20 if (i1 != null) {\n\
         \x20   i1.expectStaticType<Exactly<int>>();\n\
         \x20 }\n\
         }\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// The unpromoted NULLABLE receiver abstains the same way — the frozen
/// `int?` face cannot tell a promotion from its absence at pass 1, and the
/// under-report is the sanctioned direction (the oracle would check
/// `Exactly<int?>` there; a corpus case exercising that pair has yet to
/// appear — the family's 1052 rows are all promoted receivers).
#[test]
fn nullable_unpromoted_extension_receiver_abstains() {
    let rows = bounds_rows(
        "typedef Exactly<T> = T Function(T);\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int? i1 = 1;\n\
         \x20 i1.expectStaticType<Exactly<int>>();\n\
         }\n",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// A NON-nullable frozen receiver face cannot be null-narrowed by a
/// pass-2 re-derivation, so it decides here exactly as before — the
/// firing control for the abstain gate above.
#[test]
fn non_nullable_extension_receiver_still_fires() {
    let rows = bounds_rows(
        "typedef Exactly<T> = T Function(T);\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void main() {\n\
         \x20 int i1 = 1;\n\
         \x20 i1.expectStaticType<Exactly<String>>();\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'Exactly<String>' doesn't conform to the bound 'Exactly<int>' of the type parameter 'R'."],
    );
}

/// The contravariant alias parameter keeps its top argument through the
/// inverted pass the verdict ran — the bound prints `A<A<dynamic>>`, never
/// the variance-blind inversion's `A<A<Never>>` (the `typedef_l1_t02`
/// oracle row).
#[test]
fn contravariant_alias_bound_keeps_the_unflipped_top() {
    let rows = bounds_rows(
        "class A<X> {}\n\
         typedef G<X extends A<X>> = void Function(X);\n\
         void main() {\n\
         \x20 G<A<dynamic>> x;\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'X'."],
    );
}

/// An alias-headed argument keeps its decoration in the message — the
/// oracle's `G<Null>` row over the bare-alias bound `G<dynamic>`
/// (`class_typedef_l1_t02`).
#[test]
fn alias_argument_keeps_its_decoration() {
    let rows = bounds_rows(
        "typedef G<X> = void Function(X);\n\
         class A<X extends G> {}\n\
         void main() {\n\
         \x20 A<G<Null>> x;\n\
         }\n",
    );
    assert_eq!(
        rows,
        ["'G<Null>' doesn't conform to the bound 'G<dynamic>' of the type parameter 'X'."],
    );
}

/// The inverted pass's flip renders on the written function shape: the
/// unwritten `dynamic` return flips to `Never` covariantly while the formal
/// keeps its parameter name (`super_bounded_types_error2_test` 58:21 — the
/// shared lazy id printed `Y` for a written `N` before).
#[test]
fn inverted_function_argument_flips_only_the_top() {
    let rows = bounds_rows(
        "typedef F<X extends Function(X)> = X Function();\n\
         void f<N extends Null>() {\n\
         \x20 F<Function(N)> x;\n\
         }\n\
         void g<Y extends Null>() {\n\
         \x20 F<Function(Y)> y;\n\
         }\n",
    );
    assert_eq!(
        rows,
        [
            "'Never Function(N)' doesn't conform to the bound 'dynamic Function(Never Function(N))' of the type parameter 'X'.",
            "'Never Function(Y)' doesn't conform to the bound 'dynamic Function(Never Function(Y))' of the type parameter 'X'.",
        ],
    );
}

/// The substituted nullable leaf does not stack nullability: the bound
/// `FutureOr<A<X?>>` with `X := X?` prints one `?` (the
/// `class_FutureOr_l1_t05` oracle row).
#[test]
fn nullable_substituted_leaf_does_not_stack() {
    let rows = bounds_rows(
        "import 'dart:async';\n\
         class A<X extends FutureOr<A<X?>>> {}\n",
    );
    assert_eq!(
        rows,
        ["'X?' doesn't conform to the bound 'FutureOr<A<X?>>' of the type parameter 'X'."],
    );
}
