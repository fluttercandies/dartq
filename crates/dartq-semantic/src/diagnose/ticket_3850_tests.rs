//! Ticket #3850 pins: the `itb_default_args` alias decoration face.
//!
//! A parameter whose instantiate-to-bounds default rode an alias-headed
//! written bound keeps the oracle's `getDisplayString(preferTypeAlias)`
//! spelling — `TypeImpl.alias` decorates the instantiated type upstream
//! (`display_string_builder.dart:379-382`), so `class A<X extends G>`
//! over `typedef G<X> = X Function()` answers `G<dynamic>`, never the
//! `dynamic Function()` expansion the interned default carries. The
//! trees `itb_default_args` hands the display arms are the WRITTEN bound
//! trees under the grounding substitution — the same
//! `DefaultTypesBuilder._computeDefaultType` pair of passes
//! (default_types_builder.dart:142-180) the interned answer runs — with
//! every bare generic head inside filled to its own defaults, so the
//! verdict's `fold_alias_verdict` still reads the identical unfolded
//! type.

use super::*;

/// Renders one returned tree through the snapshot's written-argument
/// display face — the same renderer the `type_argument_not_matching_bounds`
/// arm uses, so the pinned strings read exactly like the emitted rows.
fn render<'a>(bounds: &Bounds<'a>, strings: &'a StringTable, tree: &TypeTree) -> String {
    let snapshot = bounds.snapshot(strings);
    let scope = FxHashMap::default();
    snapshot
        .display_written_argument(tree, Variance::Covariant, false, &scope)
        .expect("the written default renders")
}

/// The rendered per-parameter defaults `itb_default_args` answers for
/// `head`, in declaration order.
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

/// `typedef G<X> = X Function();` — the shared alias fixture of the
/// co19 Instantiate-to-bound/Simple-bounds families.
fn declare_function_alias(bounds: &mut Bounds<'_>, g: StringId, x: StringId) {
    bounds.begin_alias(g);
    bounds.begin_type_variables();
    bounds.end_type_variable(x, false);
    bounds.end_type_variables();
    bounds.alias_formal_boundary();
    bounds.alias_leaf(x, false, range(20, 1));
    bounds.alias_formals_region_open();
    bounds.alias_formals_closed();
    bounds.alias_rhs_question(false);
    bounds.end_alias();
}

/// `class A<X extends G>` — the bare alias-headed bound of
/// `Simple-bounds/static/class_typedef_l1_t01.dart` — defaults to
/// `G<dynamic>`: the oracle's resolved type keeps the `G` alias
/// decoration whose `dynamic` argument is `G`'s own instantiate-to-bounds
/// fill, where the interned default carries only the `dynamic Function()`
/// expansion.
#[test]
fn bare_alias_bound_defaults_keep_the_alias_spelling() {
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

/// `typedef A<X extends G<C<X, Y>>, Y extends X>` over `class C<X, Y>`
/// (Instantiate-to-bound `nonfunction_typedef_l2_t01.dart`): the {X, Y}
/// cycle grounds every covariant leaf to `dynamic`, and the alias head
/// still wraps its argument — `G<C<dynamic, dynamic>>` beside `dynamic`,
/// the exact pair the oracle's `F<A<G<C<dynamic, dynamic>>, dynamic>>`
/// source display spells.
#[test]
fn cyclic_alias_bound_keeps_its_head_through_the_grounding() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let c = strings.intern("C");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let y = strings.intern("Y");
    let mut bounds = Bounds::new();
    declare_function_alias(&mut bounds, g, x);
    // `class C<X, Y> {}` — the parser completes the variables in reverse
    // declaration order (`end_type_variables` restores it), so the events
    // run `Y` first.
    bounds.begin_type_variables();
    bounds.end_type_variable(y, false);
    bounds.end_type_variable(x, false);
    bounds.end_type_variables();
    bounds.note_container(Some(c), 25, 40, false, false);
    // `typedef A<X extends G<C<X, Y>>, Y extends X> = C<X, Y>;`
    bounds.begin_alias(a);
    bounds.begin_type_variables();
    bounds.leaf(x, false, range(80, 1));
    bounds.end_type_variable(y, true);
    bounds.begin_group(Some((g, range(60, 1))), false);
    bounds.begin_group(Some((c, range(62, 1))), false);
    bounds.leaf(x, false, range(64, 1));
    bounds.leaf(y, false, range(67, 1));
    bounds.end_group(range(68, 1));
    bounds.end_group(range(69, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.alias_begin_group(Some((c, range(90, 1))));
    bounds.alias_leaf(x, false, range(92, 1));
    bounds.alias_leaf(y, false, range(95, 1));
    bounds.alias_end_group(range(96, 1));
    bounds.alias_rhs_question(false);
    bounds.end_alias();

    assert_eq!(
        rendered_defaults(&bounds, &strings, a),
        ["G<C<dynamic, dynamic>>", "dynamic"]
    );
}

/// `class A<X extends A>` — the bare self-referential head reaches its
/// own declaration, so `_breakRawTypeCycles` poisons the whole bound
/// `dynamic` before the default-type pass runs: `X` defaults `dynamic`
/// (the oracle's `F<A<dynamic>?>` source display), never `A<dynamic>`.
#[test]
fn self_referential_bare_head_poisons_to_dynamic() {
    let strings = StringTable::new();
    let a = strings.intern("A");
    let x = strings.intern("X");
    let mut bounds = Bounds::new();
    bounds.begin_type_variables();
    bounds.leaf(a, false, range(20, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 6, 25, false, false);

    assert_eq!(rendered_defaults(&bounds, &strings, a), ["dynamic"]);
}

/// `class A<X extends G<Y>, Y extends num>` — the acyclic dependent
/// completes with the peer's substituted default, so the alias keeps its
/// written argument under the grounding: `G<num>` beside `num`.
#[test]
fn argument_bearing_alias_bound_substitutes_the_peer_default() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let y = strings.intern("Y");
    let num = strings.intern("num");
    let mut bounds = Bounds::new();
    declare_function_alias(&mut bounds, g, x);
    bounds.begin_type_variables();
    bounds.leaf(num, false, range(50, 3));
    bounds.end_type_variable(y, true);
    bounds.begin_group(Some((g, range(32, 1))), false);
    bounds.leaf(y, false, range(34, 1));
    bounds.end_group(range(35, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 25, 55, false, false);

    assert_eq!(rendered_defaults(&bounds, &strings, a), ["G<num>", "num"]);
}

/// `class A<X extends List<G>>` — an alias leaf nested inside an
/// ordinary argument list still fills its own defaults:
/// `List<G<dynamic>>`, the `preferTypeAlias` spelling the oracle's
/// instantiated bound carries at every depth.
#[test]
fn nested_alias_leaf_fills_its_own_defaults() {
    let strings = StringTable::new();
    let g = strings.intern("G");
    let a = strings.intern("A");
    let x = strings.intern("X");
    let list = strings.intern("List");
    let mut bounds = Bounds::new();
    declare_function_alias(&mut bounds, g, x);
    bounds.begin_type_variables();
    bounds.begin_group(Some((list, range(32, 4))), false);
    bounds.leaf(g, false, range(37, 1));
    bounds.end_group(range(38, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 25, 42, false, false);

    assert_eq!(
        rendered_defaults(&bounds, &strings, a),
        ["List<G<dynamic>>"]
    );
}

/// Control: a class-headed bound keeps its own face — `X extends int`
/// defaults `int`, and an unbounded parameter reads the implicit
/// `dynamic` — the written-tree pass invents nothing the interned face
/// did not already answer.
#[test]
fn ordinary_bounds_keep_their_faces() {
    let strings = StringTable::new();
    let a = strings.intern("A");
    let x = strings.intern("X");
    let y = strings.intern("Y");
    let int = strings.intern("int");
    let mut bounds = Bounds::new();
    bounds.begin_type_variables();
    bounds.end_type_variable(y, false);
    bounds.leaf(int, false, range(20, 3));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(a), 6, 30, false, false);

    assert_eq!(rendered_defaults(&bounds, &strings, a), ["int", "dynamic"]);
}
