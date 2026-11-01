//! #3472 pins — the switch covering face (witness text, witness selection,
//! and the exhaustiveness gate).
//!
//! # The defect
//!
//! The covering row drifted from the oracle in four ways:
//!
//! - the pre-patterns gate was missing: a `// @dart=2.x` library took the
//!   exhaustiveness pass, which the official runs only under
//!   `Feature.patterns` (constant_verifier.dart:487-507);
//! - the flow's matched type re-decided the verdict even when it was an
//!   unresolved row (`Ty::Unknown`), folding to the nullable top — the
//!   message printed an empty type and the verdict answered over a space the
//!   program never had;
//! - a foreign private enum suppressed the row entirely instead of reporting
//!   it under the private template (`messages.yaml`'s `...Switch*Private`
//!   rows, one shared code name);
//! - the space model lost the two facts the witness text is built from: the
//!   sealed split's instantiations (`Face<num>` splits into `Jack<num>`,
//!   `Queen<num>`, …, types/sealed.dart:77-119) and the cast pattern's
//!   coverage (`B() as C` covers the sub-pattern's `B`, not the cast type —
//!   createCastSpace, shared.dart:704-780).
//!
//! # The contract
//!
//! Upstream Dart SDK source: pkg/analyzer/lib/src/dart/constant/
//! constant_verifier.dart (the pass and its gate),
//! pkg/_fe_analyzer_shared/lib/src/exhaustiveness/{shared,types}.dart (the
//! spaces and the witness text), and pkg/analyzer/messages.yaml (the
//! templates).

use super::test_rig::{assert_silent, code_names, report_ops, report_ops_bare};

/// The covering rows of `report` as `message` texts, in source order. The
/// statements and expressions share one code name with their private
/// variants, so the filter is that one name.
fn covering_rows(source: &str) -> Vec<String> {
    let report = report_ops(source);
    covering_of(&report)
}

fn covering_of(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "non_exhaustive_switch_expression")
        .map(|d| d.message.to_string())
        .collect()
}

/// A pre-patterns library reports nothing: `visitSwitchStatement` runs the
/// exhaustiveness pass only under `Feature.patterns`
/// (constant_verifier.dart:487-507), and a switch expression cannot exist
/// before 3.0. The `bool` scrutinee would split completely under 3.0 — the
/// oracle's legacy row set is empty all the same.
///
/// Baseline (pre-fix): `must_be_exhaustive` answered from
/// `isAlwaysExhaustive(bool)` alone, so the case set `{true}` reported "The
/// type 'bool' isn't exhaustively matched … 'false'".
#[test]
fn legacy_library_switch_is_silent() {
    let report = report_ops_bare(
        "// @dart=2.19\nclass bool {}\nvoid f(bool b) {\n  switch (b) {\n    case true: break;\n  }\n}\n",
    );
    assert_silent(&report);
}

/// A statement switch over `dynamic` is never must-be-exhaustive
/// (`isAlwaysExhaustive` has no `dynamic` arm, type_system.dart:815-857).
/// Baseline: silent already — the pin keeps the gate honest while the
/// expression face below is what the oracle reports.
#[test]
fn dynamic_statement_switch_is_silent() {
    let report = report_ops(
        "void f(dynamic d) {\n  switch (d) {\n    case int(): break;\n  }\n}\n",
    );
    assert_silent(&report);
}

/// A switch expression over `dynamic` IS must-be-exhaustive (the expression
/// face is unconditional, constant_verifier.dart:470-484) and its gap
/// witnesses the non-nullable object (`Object()`, the co19
/// `Object()` witness text).
#[test]
fn dynamic_switch_expression_witnesses_the_object() {
    let rows = covering_rows("int f(dynamic d) => switch (d) { int() => 1 };\n");
    assert_eq!(
        rows,
        ["The type 'dynamic' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Object()'."]
    );
}

/// A `./`-free foreign enum whose gap names a private constant still uses the
/// PLAIN template when the enum lives in the current library
/// (constant_verifier.dart:1082-1109 keys the private variant on
/// `libraryUri != _currentLibrary.uri`): the witness text is the public one.
///
/// This is the same-library half of the private gate — the foreign half
/// needs the library-graph projection the pin rig does not light up, so it
/// stays unpinned here.
#[test]
fn same_library_private_constant_keeps_the_plain_template() {
    let rows = covering_rows(
        "enum E { _c, a }\nString f(E e) => switch (e) { E.a => 'a' };\n",
    );
    assert_eq!(
        rows,
        ["The type 'E' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'E._c'."]
    );
}

/// The sealed split instantiates each subtype with the scrutinee's own
/// arguments (`Face<num>` → `Jack<num>`, `Queen<num>`), so a case over one
/// instantiation (`Queen<int>`) leaves the other instantiations uncovered
/// and the witness is the subtype at the scrutinee's instantiation.
///
/// Baseline (pre-fix): the split carried bare heads, `Queen<int>` covered
/// the whole `Queen`, and the switch was silently exhaustive.
#[test]
fn sealed_split_carries_the_scrutinee_instantiation() {
    let rows = covering_rows(
        "sealed class Face<T> {}\n\
         class Jack<T> extends Face<T> {}\n\
         class Queen<T> extends Face<T> {}\n\
         String f(Face<num> face) => switch (face) {\n\
           Jack<num> _ => 'j',\n\
           Queen<int> _ => 'q',\n\
         };\n",
    );
    assert_eq!(
        rows,
        ["The type 'Face<num>' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Queen<num>()'."]
    );
}

/// A map pattern cannot prove coverage (a map may carry extra keys), so the
/// gap witnesses the scrutinee's own static type with the property column
/// its pattern constrained at the column's static type
/// (`Map<bool, bool>(length: int())`, co19 exhaustiveness_map_A01_t02; the
/// arguments ride the display, static_type.dart:214-239).
///
/// Baseline (pre-fix): the relational field degraded the whole pattern to
/// the unknown space and the head rendered without its arguments —
/// `Map()`.
#[test]
fn map_pattern_witness_renders_the_head_with_arguments() {
    let report = report_ops_bare(
        "class int { bool operator <=(int other) => true; }\n\
         class bool {}\n\
         class Map<K, V> {\n  int get length => 0;\n}\n\
         String f(Map<bool, bool> m) => switch (m) { Map(length: <= 0) => 'small' };\n",
    );
    assert_eq!(
        covering_of(&report),
        ["The type 'Map<bool, bool>' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Map<bool, bool>(length: int())'."]
    );
}

/// A cast pattern covers its SUB-pattern over the matched type, widened to
/// the whole type when the cast type fits inside it and joined with `null`
/// when the cast type is non-nullable (`case B() as C` over a sealed `A`
/// covers `B|Null` — `C()` is the gap, never `B()`).
///
/// Baseline (pre-fix): the cast's own type was pushed as the case space
/// (`C`), so the gap rendered `B()`.
#[test]
fn cast_pattern_covers_the_subpattern_not_the_cast_type() {
    let rows = covering_rows(
        "sealed class A {}\n\
         class B extends A {}\n\
         class C extends A {}\n\
         int f(A a) => switch (a) { B() as C => 0 };\n",
    );
    assert_eq!(
        rows,
        ["The type 'A' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'C()'."]
    );
}

/// The same shape over an extension type: the scrutinee's own spelling leads
/// the message while the space model erases to the representation type.
#[test]
fn cast_pattern_over_an_extension_type_names_the_extension_type() {
    let rows = covering_rows(
        "sealed class A {}\n\
         class B extends A {}\n\
         class C extends A {}\n\
         extension type AET1(A _) {}\n\
         int f(AET1 a) => switch (a) { B() as C => 0 };\n",
    );
    assert_eq!(
        rows,
        ["The type 'AET1' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'C()'."]
    );
}

/// A fully covered switch stays silent, and a generic enum's constants that
/// cannot inhabit the scrutinee's instantiation (`E.b` of `E<String>` over
/// `E<num>`) never become the witness — the co19
/// `exhaustiveness_enum_A01_t01` shape: `E.a`/`E.c` cover `E<num>` whole.
#[test]
fn exhaustive_switches_stay_silent() {
    let sealed = report_ops(
        "sealed class A {}\n\
         class B extends A {}\n\
         class C extends A {}\n\
         int f(A a) => switch (a) { B() => 0, C() => 1 };\n",
    );
    assert_silent(&sealed);
    let enum_scrutinee = report_ops(
        "enum E<T> { a<int>(), b<String>(), c<double>() }\n\
         String f(E<num> e) => switch (e) { E.a => 'a', E.c => 'c' };\n",
    );
    assert!(code_names(&enum_scrutinee).is_empty(), "{:?}", code_names(&enum_scrutinee));
}

/// The same enum with `E.b` in the case set: `b` cannot inhabit `E<num>`, so
/// the missing constant is `c` — the oracle's witness text (`E.c`), never
/// the unreachable `E.b`.
#[test]
fn generic_enum_witness_skips_the_non_inhabitant_constant() {
    let rows = covering_rows(
        "enum E<T> { a<int>(), b<String>(), c<double>() }\n\
         String f(E<num> e) => switch (e) { E.a => 'a', E.b => 'b' };\n",
    );
    assert_eq!(
        rows,
        ["The type 'E<num>' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'E.c'."]
    );
}
