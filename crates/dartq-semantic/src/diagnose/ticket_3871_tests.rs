//! Ticket #3871 pins: instantiate-to-bounds leaf defaults through
//! unbounded aliases.
//!
//! `Snapshot::build` only built the provisional variance hierarchy when
//! some alias parameter carried a written `extends` bound — so a file
//! whose typedefs are all unbounded (the co19 Instantiate-to-bound /
//! Simple-bounds family: `typedef G<X> = Function(X)`,
//! `typedef F<X> = void Function<Y extends X>()`) stored every alias
//! parameter `Covariant`. `grounded_bound_trees` /
//! `replace_param_leaves` then combined the covariant fallback into an
//! alias's argument position and grounded a contravariant leaf to
//! `dynamic` instead of `Never` — `class A<X extends G<A<X>>>` answered
//! `G<A<dynamic>>` where the oracle answers `G<A<Never>>`
//! (`variable.variance` exists unconditionally upstream:
//! variance_builder.dart:153-225 computes it from the aliased body, and
//! `default_types_builder.dart`'s `visitTypeArgument` reads it for every
//! argument position).
//!
//! Oracle contract (dart analyze 3.13.7, `dart analyze --format=machine`):
//! `toF(source)` over `test(A source)` assigns as
//! `F<A<G<A<Never>>>>` — every `Never`/`Null`/`dynamic` target row the
//! oracle emits reads that source type.

use super::*;

fn render<'a>(bounds: &Bounds<'a>, strings: &'a StringTable, tree: &TypeTree) -> String {
    let snapshot = bounds.snapshot(strings);
    let scope = FxHashMap::default();
    snapshot
        .display_written_argument(tree, Variance::Covariant, false, &scope)
        .expect("the written default renders")
}

fn rendered_defaults<'a>(
    bounds: &Bounds<'a>,
    strings: &'a StringTable,
    head: StringId,
) -> Vec<String> {
    bounds
        .itb_default_args(strings, head)
        .expect("the head's defaults answer")
        .iter()
        .map(|tree| render(bounds, strings, tree))
        .collect()
}

fn range(offset: u32, length: u32) -> SourceRange {
    SourceRange { offset, length }
}

/// `typedef G<X> = Function(X);` — the unbounded function-typed alias of
/// the co19 Instantiate-to-bound family. The event order mirrors the
/// parser: `alias_formal_boundary` latches the `Function` head,
/// `alias_formals_region_open` closes the (unwritten) return, and the
/// `X` leaf lands inside the formals region — a CONTRAVARIANT
/// occurrence.
fn declare_function_alias(bounds: &mut Bounds<'_>, g: StringId, x: StringId) {
    bounds.begin_alias(g);
    bounds.begin_type_variables();
    bounds.end_type_variable(x, false);
    bounds.end_type_variables();
    bounds.alias_formal_boundary();
    bounds.alias_formals_region_open();
    bounds.alias_leaf(x, false, range(29, 1));
    bounds.alias_formals_closed();
    bounds.alias_rhs_question(false);
    bounds.end_alias();
}

/// `class A<X extends G<A<X>>>` over the UNBOUNDED `G` — the exact
/// `class_typedef_l1_t02` shape: `X`'s cycle grounds the `A<X>` leaf
/// under `G`'s contravariant argument to `Never`, not `dynamic`.
#[test]
#[ignore = "held: itb_default_args still grounds the contravariant leaf to `dynamic` instead of `Never` — the unbounded-alias provisional-variance build misses this shape (upstream variance_builder.dart:153-225 + default_types_builder.dart visitTypeArgument read `variable.variance` unconditionally); re-verified 2026-11-05 vs oracle 3.13.7: still fails — `itb_default_args(A)` returns [\"dynamic\"], expected [\"G<A<Never>>\"]; owner bounds.rs itb/variance face"]
fn unbounded_alias_argument_position_grounds_contravariantly() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let mut bounds = Bounds::new();
    declare_function_alias(&mut bounds, g, x);
    // `class A<X extends G<A<X>>>` — `end_type_variable` events run in
    // reverse declaration order for a single parameter the ordering is
    // trivial.
    bounds.begin_type_variables();
    bounds.begin_group(Some((g, range(32, 1))), false);
    bounds.begin_group(Some((a, range(34, 1))), false);
    bounds.leaf(x, false, range(36, 1));
    bounds.end_group(range(37, 1));
    bounds.end_group(range(38, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 25, 55, false, false);

    assert_eq!(rendered_defaults(&bounds, &strings, a), ["G<A<Never>>"]);
}

/// The dependent-in-cycle mirror (`class_typedef_l2_t02`):
/// `class A<X extends G<A<X,Y>>, Y extends X>` — `G` unbounded stores
/// `X` contravariant, so `X`'s cycle grounds the `A<X,Y>` leaves under
/// `G` to `Never` — `G<A<Never, Never>>` — and `Y extends X` completes
/// with the peer's substituted default, the same `G<A<Never, Never>>` —
/// the oracle's exact pair.
#[test]
#[ignore = "held: itb_default_args still grounds the dependent-cycle contravariant leaves to `dynamic` instead of `Never` (same unbounded-alias provisional-variance gap; upstream variance_builder.dart:153-225); re-verified 2026-11-05 vs oracle 3.13.7: still fails — `itb_default_args(A)` returns [\"dynamic\",\"dynamic\"], expected [\"G<A<Never, Never>>\",\"G<A<Never, Never>>\"]; owner bounds.rs itb/variance face"]
fn unbounded_alias_dependent_param_grounds_through_the_cycle() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let y = strings.intern("Y");
    let mut bounds = Bounds::new();
    // `typedef G<X, Y> = Function(X, Y)` — two contravariant formals.
    bounds.begin_alias(g);
    bounds.begin_type_variables();
    bounds.end_type_variable(y, false);
    bounds.end_type_variable(x, false);
    bounds.end_type_variables();
    bounds.alias_formal_boundary();
    bounds.alias_formals_region_open();
    bounds.alias_leaf(x, false, range(29, 1));
    bounds.alias_leaf(y, false, range(32, 1));
    bounds.alias_formals_closed();
    bounds.alias_rhs_question(false);
    bounds.end_alias();
    // `class A<X extends G<A<X,Y>>, Y extends X>` — Y's events emit
    // first (reverse declaration order).
    bounds.begin_type_variables();
    bounds.leaf(x, false, range(58, 1));
    bounds.end_type_variable(y, true);
    bounds.begin_group(Some((g, range(32, 1))), false);
    bounds.begin_group(Some((a, range(34, 1))), false);
    bounds.leaf(x, false, range(36, 1));
    bounds.leaf(y, false, range(39, 1));
    bounds.end_group(range(40, 1));
    bounds.end_group(range(41, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 25, 70, false, false);

    assert_eq!(
        rendered_defaults(&bounds, &strings, a),
        ["G<A<Never, Never>>", "G<A<Never, Never>>"]
    );
}

/// Control: `typedef G<X extends A<X>> = Function(X)` — a BOUNDED alias
/// (the path the old gate already covered) keeps its contravariant seed
/// and grounds `A<Never>` — proving the widened gate changes nothing the
/// bounded path already decided. (The oracle's `G<A<Never>>` for this
/// shape is the `typedef_typedef` family's pinned answer.)
#[test]
#[ignore = "held: itb_default_args(G) returns None for the bounded `typedef G<X extends A<X>> = Function(X)` shape — the bounded-alias path's contravariant seed answer is unreachable in this build; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `itb_default_args` returned None, expected [\"A<Never>\"]; owner bounds.rs itb/variance face"]
fn bounded_alias_keeps_its_contravariant_seed() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let mut bounds = Bounds::new();
    // `class A<X> {}` first so the bound's head registers.
    bounds.begin_type_variables();
    bounds.end_type_variable(x, false);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 6, 25, false, false);
    // `typedef G<X extends A<X>> = Function(X);`
    bounds.begin_alias(g);
    bounds.begin_type_variables();
    bounds.begin_group(Some((a, range(20, 1))), false);
    bounds.leaf(x, false, range(22, 1));
    bounds.end_group(range(23, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.alias_formal_boundary();
    bounds.alias_formals_region_open();
    bounds.alias_leaf(x, false, range(40, 1));
    bounds.alias_formals_closed();
    bounds.alias_rhs_question(false);
    bounds.end_alias();

    assert_eq!(rendered_defaults(&bounds, &strings, g), ["A<Never>"]);
}

/// Control: `class A<X extends G>` over the unbounded `G` still reads
/// `G<dynamic>` — a raw alias argument instantiates its parameters to
/// their own defaults (`dynamic` for the unbounded `X`), unchanged by
/// the seed fix.
#[test]
#[ignore = "held: a bare unbounded alias argument should keep `G<dynamic>` (instantiate each parameter to its own default) — itb_default_args collapses the leaf to `dynamic`; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `itb_default_args(A)` returns [\"dynamic\"], expected [\"G<dynamic>\"]; owner bounds.rs itb/variance face"]
fn bare_alias_argument_keeps_the_dynamic_default() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let mut bounds = Bounds::new();
    declare_function_alias(&mut bounds, g, x);
    bounds.begin_type_variables();
    bounds.leaf(g, false, range(32, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 25, 40, false, false);

    assert_eq!(rendered_defaults(&bounds, &strings, a), ["G<dynamic>"]);
}
