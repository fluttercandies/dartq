//! The #3552 pins: `body_might_complete_normally`'s async-context arm
//! answers through the UNWRAPPED future value type — the official
//! `_contextTypeForImposed` builds `FutureOr<futureValueType(S)>` over
//! the imposed return, and the verdict asks that union's nullability the
//! same way a written `FutureOr` answers (`isPotentiallyNonNullable` then
//! the `futureOrBase` silent faces, type_analyzer.dart:687-702).
//!
//! The defect: the name-level `future_or_base` stopped at one hop, so a
//! context whose value type was itself a `FutureOr` kept a synthetic
//! `FutureOr<FutureOr<..>>` face the official's recursive base never
//! produces — `FutureOr<FutureOr<void>>` reported the ERROR twin against
//! a silent oracle, and a nullable inner `FutureOr` (`FutureOr<FutureOr<
//! int>?>`) reported the ERROR twin where the oracle carries the NULLABLE
//! warning (`isNullable` reads the immediate argument's `?`, not the deep
//! base's). The staged declared face now folds each written type
//! argument's unresolved `Named` heads to `Ty::Unknown` (the oracle's
//! `InvalidType`) while the declaring frame's scope still binds —
//! `FutureOr<NonExistent>` joins the silent base faces — and the
//! verdict's nullability question delegates to the argument exactly as
//! upstream's `isNullable` does.
//!
//! Oracle provenance: `dart analyze` 3.13.7
//! (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze --format machine`)
//! over each fixture's text; every expectation below quotes the live
//! verdict row set (the `body_might_complete_normally*` rows only — the
//! filter keeps auxiliary rows like `non_type_as_type_argument` out of
//! the answer).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// Every `body_might_complete_normally*` row as `(code, offset)`,
/// source-ordered — the two completion codes are the only rows these
/// pins decide, so the fixture's other rows (unresolved names, unused
/// declarations) never enter the answer.
fn completion_rows(source: &str) -> Vec<(&'static str, u32)> {
    let report = report_ops(source);
    let mut rows: Vec<(&'static str, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name.starts_with("body_might_complete_normally"))
        .map(|row| (row.code.name, row.range.offset))
        .collect();
    rows.sort_unstable();
    rows
}

/// The offset of the `nth` (0-based) occurrence of `needle` in the
/// FIXTURE text, shifted past the prelude the rig mounts ahead of it.
/// Every fixture here is ASCII, so byte and UTF-16 offsets coincide.
fn at(source: &str, needle: &str, nth: usize) -> u32 {
    let found = source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing"));
    found.0 as u32 + CORE_PRELUDE.len() as u32
}

/// The headline shape: the async body's completion context reads the
/// declared return's `futureValueType` — `FutureOr<FutureOr<void>>`
/// unwraps one hop to `FutureOr<void>`, and the `FutureOr<FutureOr<
/// void>>` union's recursive `futureOrBase` lands on `void`, one of the
/// silent faces.
///
/// Probed: no completion row at all — the oracle emits nothing for the
/// same text.
#[test]
fn async_nested_future_or_void_stays_silent() {
    let source = "FutureOr<FutureOr<void>> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The `Future`-headed spelling of the same context: `futureValueType(
/// Future<FutureOr<void>>)` is `FutureOr<void>`, so the union again bases
/// on `void`.
///
/// Probed: no completion row.
#[test]
fn async_future_of_future_or_void_stays_silent() {
    let source = "Future<FutureOr<void>> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The sync twin: a written `FutureOr<FutureOr<void>>` return asks the
/// same recursive `futureOrBase` — `void` is silent.
///
/// Probed: no completion row.
#[test]
fn sync_nested_future_or_void_stays_silent() {
    let source = "FutureOr<FutureOr<void>> f() {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The `?`-headed declared union: `futureValueType` strips the suffix
/// before the unwrap, so `FutureOr<FutureOr<void>>?` still reads the
/// `FutureOr<FutureOr<void>>` context — silent.
///
/// Probed: no completion row.
#[test]
fn async_nullable_head_over_nested_void_stays_silent() {
    let source = "FutureOr<FutureOr<void>>? f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// A nullable INNER `FutureOr` element: `futureValueType(FutureOr<
/// FutureOr<void>?>)` is `FutureOr<void>?`, and `isNullable` reads the
/// argument's `?` first — the union is nullable, then `futureOrBase`
/// lands on `void` anyway: silent.
///
/// Probed: no completion row.
#[test]
fn async_nullable_inner_future_or_void_stays_silent() {
    let source = "FutureOr<FutureOr<void>?> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The severity-correction shape: `isNullable(FutureOr<FutureOr<int>?>)`
/// answers through the immediate argument's `?` — the union is nullable,
/// and `futureOrBase` lands on `int` (no silent face) — the NULLABLE
/// twin, whose span is the declaring name. The prior deep-base read
/// reported the ERROR twin against the same oracle row.
///
/// Probed: one row, `body_might_complete_normally_nullable` at `f`'s
/// name — "This function has a nullable return type of 'FutureOr<
/// FutureOr<int>?>', but ends without returning a value."
#[test]
fn async_nullable_inner_future_or_int_reports_the_warning_twin() {
    let source = "FutureOr<FutureOr<int>?> f() async {}\n";
    assert_eq!(
        completion_rows(source),
        [(
            "body_might_complete_normally_nullable",
            at(source, "f()", 0)
        )],
        "{:#?}",
        completion_rows(source)
    );
}

/// The `Future`-headed spelling of the same nullable-inner context:
/// `futureValueType(Future<FutureOr<int>?>)` is `FutureOr<int>?`, the
/// same nullable union.
///
/// Probed: one row, `body_might_complete_normally_nullable` at `f`'s
/// name.
#[test]
fn async_future_of_nullable_future_or_int_reports_the_warning_twin() {
    let source = "Future<FutureOr<int>?> f() async {}\n";
    assert_eq!(
        completion_rows(source),
        [(
            "body_might_complete_normally_nullable",
            at(source, "f()", 0)
        )],
        "{:#?}",
        completion_rows(source)
    );
}

/// Control: the non-nullable nested union keeps the ERROR twin —
/// `isNullable(FutureOr<FutureOr<int>>)` delegates to `int` (non-
/// nullable), and `int` is no silent base.
///
/// Probed: one row, `body_might_complete_normally` at `f`'s name —
/// "The body might complete normally, causing 'null' to be returned,
/// but the return type, 'FutureOr<FutureOr<int>>', is a potentially
/// non-nullable type."
#[test]
fn async_nested_future_or_int_keeps_the_error_twin() {
    let source = "FutureOr<FutureOr<int>> f() async {}\n";
    assert_eq!(
        completion_rows(source),
        [("body_might_complete_normally", at(source, "f()", 0))],
        "{:#?}",
        completion_rows(source)
    );
}

/// The ticket's named `FutureOr<R>` family is a FIRING face upstream —
/// a bare type-parameter element is potentially non-nullable
/// (`isNullable`'s promoted-bound gate never consults `bound`), so the
/// ERROR twin stays.
///
/// Probed: one row, `body_might_complete_normally` at `g`'s name —
/// "the return type, 'FutureOr<R>', is a potentially non-nullable
/// type."
#[test]
fn async_future_or_type_parameter_keeps_the_error_twin() {
    let source = "FutureOr<R> g<R>() async {}\n";
    assert_eq!(
        completion_rows(source),
        [("body_might_complete_normally", at(source, "g<", 0))],
        "{:#?}",
        completion_rows(source)
    );
}

/// Positive control: a genuinely non-nullable async fall-through still
/// fires — `Future<int>`'s value type `int` is no silent face.
///
/// Probed: one row, `body_might_complete_normally` at `f`'s name —
/// "the return type, 'FutureOr<int>', is a potentially non-nullable
/// type."
#[test]
fn async_future_int_still_reports_the_error_twin() {
    let source = "Future<int> f() async {}\n";
    assert_eq!(
        completion_rows(source),
        [("body_might_complete_normally", at(source, "f()", 0))],
        "{:#?}",
        completion_rows(source)
    );
}

/// Control: the nullable-element union keeps the WARNING twin —
/// `FutureOr<Object?>`'s `Object?` value type is nullable, and it is no
/// silent base.
///
/// Probed: one row, `body_might_complete_normally_nullable` at `f`'s
/// name — "This function has a nullable return type of 'FutureOr<
/// Object?>', but ends without returning a value."
#[test]
fn async_future_or_object_question_keeps_the_warning_twin() {
    let source = "FutureOr<Object?> f() async {}\n";
    assert_eq!(
        completion_rows(source),
        [(
            "body_might_complete_normally_nullable",
            at(source, "f()", 0)
        )],
        "{:#?}",
        completion_rows(source)
    );
}

/// The alias-element arm: a typedef's captured right-hand side answers
/// the base question — `Nothing` folds to `void`, one of the silent
/// faces.
///
/// Probed: no completion row.
#[test]
fn async_future_or_alias_to_void_stays_silent() {
    let source = "typedef Nothing = void;\nFutureOr<Nothing> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The sync twin of the alias-element arm: the written `FutureOr<
/// Nothing>` return resolves `void` — silent.
///
/// Probed: no completion row.
#[test]
fn sync_future_or_alias_to_void_stays_silent() {
    let source = "typedef Nothing = void;\nFutureOr<Nothing> f() {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The alias-to-`FutureOr` arm: `NF`'s right-hand side is the union
/// itself, so `FutureOr<NF>` bases through two hops — `void`, silent.
///
/// Probed: no completion row.
#[test]
fn async_future_or_alias_to_future_or_stays_silent() {
    let source = "typedef NF = FutureOr<void>;\nFutureOr<NF> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The deeper spelling of the same: `futureValueType(FutureOr<FutureOr<
/// NF>>)` is `FutureOr<NF>`, and the base chain unfolds `NF` →
/// `FutureOr<void>` → `void` — silent.
///
/// Probed: no completion row.
#[test]
fn async_nested_future_or_alias_to_future_or_stays_silent() {
    let source =
        "typedef NF = FutureOr<void>;\nFutureOr<FutureOr<NF>> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// Control: an alias whose right-hand side is non-nullable keeps the
/// ERROR twin — `NInt` reads `int`. The diagnostic's display keeps the
/// WRITTEN spelling (`'FutureOr<NInt>'`, the oracle's own text), never
/// the folded `int` — the staging fold resolves for the verdict only.
///
/// Probed: one row, `body_might_complete_normally` at `f`'s name —
/// "the return type, 'FutureOr<NInt>', is a potentially non-nullable
/// type."
#[test]
fn async_future_or_alias_to_int_keeps_the_error_twin() {
    let source = "typedef NInt = int;\nFutureOr<NInt> f() async {}\n";
    assert_eq!(
        completion_rows(source),
        [("body_might_complete_normally", at(source, "f()", 0))],
        "{:#?}",
        completion_rows(source)
    );
}

/// The `InvalidType` element arm: an argument that resolves to no type
/// in scope is `InvalidType` upstream — the `futureOrBase` silent set
/// admits it, so `FutureOr<NonExistent>` stays silent (the arg's own
/// `non_type` row is the face's business; the completion walk adds
/// nothing).
///
/// Probed: no `body_might_complete_normally*` row — the oracle emits
/// only `non_type_as_type_argument` for the same text.
#[test]
fn async_future_or_unresolved_element_stays_silent() {
    let source = "FutureOr<NonExistent> f() async {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The sync twin of the `InvalidType` element arm: the staging fold
/// writes the same `Ty::Unknown` stand-in the async path reads.
///
/// Probed: no `body_might_complete_normally*` row.
#[test]
fn sync_future_or_unresolved_element_stays_silent() {
    let source = "FutureOr<NonExistent> f() {}\n";
    assert_eq!(completion_rows(source), Vec::new());
}

/// The local-function arm (`stage_local_fn_completion`): the frame pop
/// releases the same staging — a local `async` declaration over the
/// nested `void` union stays silent while its outer sibling keeps the
/// ERROR twin the context still owes.
///
/// Probed: exactly one row, `body_might_complete_normally` at `g`'s
/// name, none at the local `f`.
#[test]
fn local_async_nested_future_or_void_stays_silent() {
    let source = "\
int g() {\n\
\x20 FutureOr<FutureOr<void>> f() async {}\n\
}\n";
    assert_eq!(
        completion_rows(source),
        [("body_might_complete_normally", at(source, "g()", 0))],
        "{:#?}",
        completion_rows(source)
    );
}
