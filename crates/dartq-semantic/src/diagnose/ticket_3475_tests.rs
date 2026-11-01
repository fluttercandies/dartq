//! #3475 pins — the displayed-type boundary (`ty_display`) and the
//! type-argument bounds face it feeds.
//!
//! # The defect
//!
//! The type-argument bounds row renders both sides of the question through
//! `Engine::ty_display`, the diagnostic-message face of a `Ty`. That face
//! filled instantiate-to-bounds only at the TOP of the type it printed: a
//! written `G` (bare generic head) displayed `G<num>`, but the same head
//! one level down displayed bare — `'List<C<G>>'` where the oracle prints
//! `'List<C<G<num>>>'`, `'Map<G, G>'` where the oracle prints
//! `'Map<G<num>, G<num>>'`. The oracle instantiates every written
//! annotation at resolution (`DartType.getDisplayString` prints the
//! resolved `staticType`), so the fill belongs to every nesting depth, not
//! to the row's top-level head.
//!
//! # The contract
//!
//! Upstream Dart SDK source: pkg/analyzer/lib/src/dart/element/type.dart
//! (`instantiateTypeFormalsToBounds`) and
//! pkg/analyzer/lib/src/generated/display_string_builder.dart (the
//! `FunctionType`/`RecordType`/interface display forms). One function —
//! [`super::assignability`]'s `display_filled_ty` behind `ty_display` —
//! owns the fill for every message slot; the emission sites
//! (`check_call_type_arg_bounds`'s argument and bound, the assignment
//! faces, the covering rows) only hand it a `Ty`.
//!
//! Every expectation below was taken from the pinned oracle:
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze <fixture>` run on the
//! fixture's body (the same declarations, `dart:core` from the SDK).
//!
//! # Unreachable fill positions (reported, not pinned)
//!
//! Three positions of the fill have no row to assert on today, because the
//! call-site type-argument capture — not the display face — drops them
//! before `check_call_type_arg_bounds` runs. Fixtures and oracle behaviour:
//!
//! - a BARE function type as the whole written argument:
//!   `A<void Function()>()` — oracle `'void Function()' doesn't conform …`,
//!   dartq no row (no raw head involved, so this is not a fill question);
//! - a function type whose return spells a generic instantiation:
//!   `A<List<C<int> Function(int)>>()` — oracle
//!   `'List<C<int> Function(int)>' doesn't conform …`, dartq no row, while
//!   the same list with `int Function(int)` does report;
//! - a record type anywhere in a written argument group:
//!   `A<List<(G, G)>>()` — oracle `'List<(G<num>, G<num>)>' doesn't
//!   conform …`, dartq prints `'List<G, G>'` (the record is captured as two
//!   separate arguments, so even the shape is lost).
//!
//! `display_filled_ty`'s `Record` arm and its function-return leg are kept
//! for completeness; they are exercised the moment the capture carries
//! those shapes.

use super::test_rig::report_ops;

/// The `type_argument_not_matching_bounds` messages of `report`, in source
/// order — the only rows the fixtures below assert on (the rig also
/// reports unused-locals on some of them, which this filter drops).
fn bounds_rows(source: &str) -> Vec<String> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| d.message.to_string())
        .collect()
}

/// Oracle-verified fixture preamble: `A<T extends num>` is the bound
/// under test (`A<x>` must report `x` against `num`), `G<T extends num>`
/// is the raw head the fill instantiates, `C<T>` carries one argument
/// without constraining it (`C<G>` keeps the raw `G` nested).
const PREAMBLE: &str = "class A<T extends num> { A(); } \
     class C<T> { C(); } \
     class G<T extends num> { G(); } \
     ";

/// A raw head nested inside a named argument list displays filled:
/// `dart analyze` on
/// `A<List<C<G>>>()` reports
/// `'List<C<G<num>>>' doesn't conform to the bound 'num' of the type
/// parameter 'T'.`
///
/// Baseline (pre-fix): the fill ran on the row's top-level head only, and
/// the argument's own head is a `Named` (`List`) that is not raw — the
/// nested `G` survived bare: `'List<C<G>>'`.
#[test]
fn named_argument_list_fills_nested_raw_heads() {
    let source = format!("{PREAMBLE}void main() {{ A<List<C<G>>>(); }}");
    assert_eq!(
        bounds_rows(&source),
        ["'List<C<G<num>>>' doesn't conform to the bound 'num' of the type parameter 'T'."],
    );
}

/// Every argument position of one row fills independently: `dart analyze`
/// on `A<Map<G, G>>()` reports
/// `'Map<G<num>, G<num>>' doesn't conform to the bound 'num' of the type
/// parameter 'T'.`
///
/// Baseline (pre-fix): `'Map<G, G>'`.
#[test]
fn every_argument_position_fills() {
    let source = format!("{PREAMBLE}void main() {{ A<Map<G, G>>(); }}");
    assert_eq!(
        bounds_rows(&source),
        ["'Map<G<num>, G<num>>' doesn't conform to the bound 'num' of the type parameter 'T'."],
    );
}

/// The fill reaches inside a function type's return and formal columns —
/// the shape the corpus's `expectStaticType<Exactly<... Function(...)>>`
/// rows carry. `dart analyze` on
/// `A<List<int Function(G)>>()` reports
/// `'List<int Function(G<num>)>' doesn't conform to the bound 'num' of the
/// type parameter 'T'.`
///
/// Baseline (pre-fix): `'List<int Function(G)>'`.
///
/// The argument sits inside a named head because a BARE function type as
/// the whole written argument reaches no row at all — `A<int Function(G)>()`
/// and even `A<void Function()>()` (no raw head involved) report nothing
/// while the oracle reports both; the call-site type-argument capture drops
/// the shape before the bounds face sees it. That gap is outside this
/// file's boundary and is recorded in the module docs below.
#[test]
fn function_shape_fills_return_and_formals() {
    let source = format!("{PREAMBLE}void main() {{ A<List<int Function(G)>>(); }}");
    assert_eq!(
        bounds_rows(&source),
        [
            "'List<int Function(G<num>)>' doesn't conform to the bound 'num' of the type \
             parameter 'T'."
        ],
    );
}

/// The fill never touches a head that already spells its arguments:
/// `dart analyze` on `A<G<int>>()` reports
/// `'G<int>' doesn't conform to the bound 'num' of the type parameter
/// 'T'.` (the written argument is not replaced by the bound `num`).
#[test]
fn written_arguments_display_verbatim() {
    let source = format!("{PREAMBLE}void main() {{ A<G<int>>(); }}");
    assert_eq!(
        bounds_rows(&source),
        ["'G<int>' doesn't conform to the bound 'num' of the type parameter 'T'."],
    );
}

/// A function-typed type argument keeps its own display — the fill has no
/// raw head to touch, and the function text is neither flattened to the
/// bare `Function` head nor re-spelled. `dart analyze` on
/// `A<List<int Function(int)>>()` reports
/// `'List<int Function(int)>' doesn't conform to the bound 'num' of the
/// type parameter 'T'.`
///
/// The named wrapper carries the row for the same reason as
/// [`function_shape_fills_return_and_formals`]; the bare spelling
/// `A<int Function(int)>()` reports nothing here (capture gap, module
/// docs).
#[test]
fn function_typed_argument_keeps_its_display() {
    let source = format!("{PREAMBLE}void main() {{ A<List<int Function(int)>>(); }}");
    assert_eq!(
        bounds_rows(&source),
        [
            "'List<int Function(int)>' doesn't conform to the bound 'num' of the type \
             parameter 'T'."
        ],
    );
}

/// The two recorded `Function`-face sites produce `Ty`s, never text: a
/// function-typed local receiver reaches the miss face with the receiver's
/// own display, and its `Object` member read resolves without a row.
/// `dart analyze` on
/// `void f(int Function(int) g) { var x = g.foo; }` reports
/// `The getter 'foo' isn't defined for the type 'int Function(int)'.`
/// and on `var s = g.toString();` nothing (the `Object` face answers).
///
/// The row's text is the shape receiver's display, rendered by the
/// boundary ([`super::assignability`]'s `render_ty`); the carrier sites
/// (`invocation.rs`'s member-receiver side table and its return face) hand
/// over the `Ty` and build no string of their own.
#[test]
fn function_receiver_rows_render_through_the_boundary() {
    let source = "void f(int Function(int) g) { var x = g.foo; }\n\
         void h(int Function(int) g) { var s = g.toString(); }";
    let report = report_ops(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("undefined_"))
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect();
    assert_eq!(
        rows,
        ["undefined_getter: The getter 'foo' isn't defined for the type 'int Function(int)'."],
    );
}
