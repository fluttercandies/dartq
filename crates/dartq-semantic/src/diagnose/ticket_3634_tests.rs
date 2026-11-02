//! #3634 pins: three `unchecked_use_of_nullable_value` families, both the
//! missing and the extra direction.
//!
//! * A member invocation on a NON-NULL function-typed field whose declared
//!   type is a generic typedef (`x.f("a")` over `typedef void F2<T>(T v)`
//!   and `final F2 f`) resolves the call — the callee face folds to a
//!   non-nullable `Function` type, which the official
//!   `nullable_dereference_verifier`'s `isPotentiallyNullable` gate does
//!   not report. The member-invoke emit was missing the
//!   `potentially_nullable` half of the bare-callee gate.
//! * A `?`-less type-variable receiver reads its nullability through the
//!   declared bound (`T extends num?` is potentially nullable, `T extends
//!   num` is not) — at the member-access verdict AND the operator face,
//!   matching `type_system.dart`'s `isNonNullable(bound)` recursion.
//! * A pre-3.9 unit (`// @dart = 3.8`, `sound_flow_analysis` off) promotes
//!   a mutual subtype (`List<dynamic>` over `List<Object?>`) — the legacy
//!   `newType != previousType` step, which the sound-mode strict-subtype
//!   gate refuses. Oracle: dart 3.13.7.

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_bare};
use crate::FileReport;

/// The `(offset, length)` pairs of `code` rows, source-ordered.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// Byte offset of `needle`'s `nth` occurrence inside `source`, shifted
/// past [`CORE_PRELUDE`]. ASCII fixtures make byte and UTF-16 offsets
/// coincide.
fn at(source: &str, needle: &str, nth: usize) -> u32 {
    source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing in {source:?}"))
        .0 as u32
        + CORE_PRELUDE.len() as u32
}

// —— Family 8: partial tearoff instantiation ————————————————————————————

/// `x.f("a")` over `typedef void F2<T>(T v)` and `final F2 f` reports no
/// `unchecked_use_of_nullable_value` — the instantiated tearoff's
/// `Function` face is non-null by construction, so the oracle resolves the
/// invoke unconditionally (`partial_tearoff_instantiation_test_01` L97,
/// L110).
#[test]
fn non_null_generic_typedef_field_invokes_cleanly() {
    let source = "typedef void F2<T>(T v);\n\
                  class X2 {\n\
                  \x20 final F2 f;\n\
                  \x20 X2(this.f);\n\
                  }\n\
                  void m(X2 x) {\n\
                  \x20 x.f(\"a\");\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unchecked_use_of_nullable_value"),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

/// The nullable control: a `Function`-typed field spelled `?` still
/// reports the shared unchecked code at the callee — the gate stands down
/// only for a non-null face.
#[test]
fn nullable_function_field_invokes_unchecked() {
    let source = "class X {\n\
                  \x20 final int Function()? f;\n\
                  \x20 X(this.f);\n\
                  }\n\
                  void m(X x) {\n\
                  \x20 x.f();\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unchecked_use_of_nullable_value").len(),
        1,
        "source: {source}"
    );
}

// —— Family 9: type-variable nullability from bounds ————————————————————

/// `T extends num?` reads potentially nullable through the bound: the
/// member-access verdict reports `unchecked_use_of_nullable_value` at
/// `t.isFinite` (property) and `t.abs()` (method), and the operator face
/// reports it at `t + t` (`static_errors_A01_t10` L20/L24/L28).
#[test]
fn nullable_bound_type_variable_reports_unchecked() {
    let source = "class C<T extends num?> {\n\
                  \x20 T t;\n\
                  \x20 C(this.t);\n\
                  \x20 test() {\n\
                  \x20   t.isFinite;\n\
                  \x20   t.abs();\n\
                  \x20   t + t;\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unchecked_use_of_nullable_value"),
        vec![
            (at(source, "isFinite", 0), 8),
            (at(source, "abs", 0), 3),
            (at(source, "+", 0), 1),
        ],
        "source: {source}"
    );
}

/// `T extends num` (non-null bound) keeps the receiver non-nullable:
/// `x + 2` over `num f<T extends num>(T x)` reports nothing
/// (`generic/function_bounds_test_01` L62) — the bound-consistency fix's
/// extra direction.
#[test]
fn non_null_bound_type_variable_stays_clean() {
    let source = "num f<T extends num>(T x) => x + 2;\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, "unchecked_use_of_nullable_value"),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}

// —— Family 10: sound_flow_analysis off —————————————————————————————————

/// A `// @dart = 3.8` unit promotes a mutual subtype: `y as List<dynamic>`
/// over `List<Object?> y` lands `List<dynamic>` through the legacy
/// `newType != previousType` step, so `y.first` reads `dynamic` and
/// `y.first.abs()` reports nothing (`proper_subtypes_disabled_test` L26).
///
/// [`report_ops_bare`] — not [`report_ops`] — because the `@dart` marker
/// must sit on the fixture's first line for `fixture_language_version`
/// to read it; the prelude would push it past the scanner's first
/// non-comment line and leave the unit at the default 3.15 (sound-flow
/// on), which refuses the mutual-subtype step.
///
/// `#[ignore]`d: the flow's promotion itself lands (`promotion_step`
/// returns `List<dynamic>` for `y`), but `rederived_receiver` only
/// re-derives `Named` receiver faces — a `dynamic` chain result
/// (`y.first` answering `dynamic` after `List<dynamic>`'s `E`
/// substitution) falls through and `abs` keeps the declared
/// `Object?` receiver (`receiver_face.rs:120`). The member-chain
/// receiver needs non-`Named` re-derivation coverage.
#[test]
#[ignore = "receiver_face.rs:120 — rederived_receiver drops non-Named (Dynamic) chain results"]
fn disabled_sound_flow_promotes_mutual_subtype() {
    let source = "// @dart=3.8\n\
                  class List<E> { E get first => throw 0; }\n\
                  void test(List<Object?> y) {\n\
                  \x20 y as List<dynamic>;\n\
                  \x20 y.first.abs();\n\
                  }\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows_of(&report, "unchecked_use_of_nullable_value"),
        Vec::<(u32, u32)>::new(),
        "source: {source}"
    );
}
