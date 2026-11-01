//! #3490 pins — the pattern never-match verdict reads the canonical `Null`
//! face.
//!
//! # The defect
//!
//! A pattern whose matched value type is `Null` was judged against the
//! pattern's type through the name-level mutual-exclusion companion fed the
//! RAW scrutinee face: a written `Null` annotation capture spells
//! `Ty::Named{Null, nullable: false}`, `NameTy::from_semantic` maps that to
//! a plain non-nullable interface head (not [`NameTy::Null`]), so
//! `is_potentially_nullable` answered false and every `Null` pair was
//! decided as two unrelated interface heads — the row fired (or the
//! cast-pattern faces failed to fire) where the oracle disagrees.
//! `'null case Null'` produced `pattern_never_matches_value_type` +
//! `cast_from_null_always_fails` where the oracle produces
//! `unnecessary_cast_pattern`, and `Null x; case A? _` fired a false
//! never-match.
//!
//! # The contract
//!
//! The verdict is `!typeSystem.canBeSubtypeOf(matchedValueType,
//! requiredType)` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart:624-653) whose Null arms
//! (pkg/analyzer/lib/src/dart/element/type_system.dart:147-159) decide over
//! `isPotentiallyNullable` — a `Null` face fits exactly the
//! potentially-nullable opposite (every nullable spelling, the top faces,
//! `Null` itself, a `FutureOr` with a nullable element, a `?`-less type
//! parameter through its bound) and is disjoint from every non-nullable
//! one. The cast-pattern triple rides the same verdict
//! (`visitCastPattern`'s `cast_from_null_always_fails`,
//! best_practices_verifier.dart:208-218; the `matchedTypeIsSubtypeOfRequired`
//! `unnecessary_cast_pattern` gate).
//!
//! Every expectation below was oracle-verified against
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze --format machine`
//! (batch files under /tmp/dq3490: a.dart functions f1-f8, b.dart g1-g10)
//! and cross-checked against the extracted corpus rows in
//! `conformance/expected.tsv`
//! (`diagnostics/pattern_never_matches_value_type/…`) and the upstream
//! spec source pkg/analyzer/test/src/diagnostics/
//! pattern_never_matches_value_type_test.dart. The oracle emits a
//! `dead_code` row beside the reporting fixtures; the dead-region face is
//! #3465's domain and is not pinned here.

use super::test_rig::{assert_silent, report_ops};

/// The pattern-family rows of `report` as `(code, message)` in emission
/// order: the never-match / cast-triple codes whose verdicts this ticket
/// owns. Unrelated faces (the dead-region rows, the parse diagnostics) are
/// other tickets' pins.
fn pattern_rows(source: &str) -> Vec<(&'static str, String)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "pattern_never_matches_value_type"
                    | "unnecessary_cast_pattern"
                    | "cast_from_null_always_fails"
            )
        })
        .map(|d| (d.code.name, d.message.to_string()))
        .collect()
}

/// `Null x; case Null _` — `Null` is its own subtype
/// (`canBeSubtypeOf(Null, Null)`: the right face is potentially nullable),
/// so the declared-variable pattern matches. Baseline (pre-fix): the
/// written `Named{Null}` capture read as a non-nullable interface head and
/// the verdict stayed silent only by the same-head accident.
///
/// Oracle a.dart f1: zero rows (spec `test_matchedNull_requiredNull`).
#[test]
fn matched_null_required_null_is_silent() {
    assert_silent(&report_ops("void f(Null x) {\n  if (x case Null _) {}\n}\n"));
}

/// `Null x; case A? _` — `Null` fits every nullable spelling. Baseline
/// (pre-fix): the false never-match this ticket exists for.
///
/// Oracle a.dart f2: zero rows (spec `test_requiredNullable_matchedNull`
/// mirrored; expected.tsv `test_matchedNull_…` the same face).
#[test]
fn matched_null_nullable_required_is_silent() {
    assert_silent(&report_ops(
        "class A {}\nvoid f(Null x) {\n  if (x case A? _) {}\n}\n",
    ));
}

/// `Null x; case A _` — `Null` is disjoint from every non-nullable face.
///
/// Oracle a.dart f3: `The matched value type 'Null' can never match the
/// required type 'A'.` (spec `test_matchedNull_requiredNotNullable`).
#[test]
fn matched_null_non_nullable_required_reports() {
    assert_eq!(
        pattern_rows("class A {}\nvoid f(Null x) {\n  if (x case A _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type 'Null' can never match the required type 'A'.".to_string()
        )]
    );
}

/// `Null x; case Object _` — the nullable-vs-top-non-nullable cross is
/// disjoint (`Null <: Object` is false in NNBD).
///
/// Oracle a.dart f4: `'Null' can never match … 'Object'` (spec
/// `test_matchedNull_requiredObject`).
#[test]
fn matched_null_object_required_reports() {
    assert_eq!(
        pattern_rows("void f(Null x) {\n  if (x case Object _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type 'Null' can never match the required type 'Object'.".to_string()
        )]
    );
}

/// `A x; case Null _` — the required-`Null` arm: a non-nullable matched
/// face can never be `null`.
///
/// Oracle a.dart f5: `'A' can never match … 'Null'` (spec
/// `test_requiredNull_matchedNotNullable`).
#[test]
fn required_null_non_nullable_matched_reports() {
    assert_eq!(
        pattern_rows("class A {}\nvoid f(A x) {\n  if (x case Null _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type 'A' can never match the required type 'Null'.".to_string()
        )]
    );
}

/// `A? x; case Null _` — the nullable matched face can be `null`.
///
/// Spec `test_requiredNull_matchedNullable_interfaceType`: silent (oracle
/// batch a.dart f5 family; expected.tsv `-`).
#[test]
fn required_null_nullable_matched_is_silent() {
    assert_silent(&report_ops(
        "class A {}\nvoid f(A? x) {\n  if (x case Null _) {}\n}\n",
    ));
}

/// `dynamic x; case Null _` — `dynamic` is a top face, potentially
/// nullable.
///
/// Spec `test_requiredNull_matchedNullable_dynamicType`: silent.
#[test]
fn required_null_dynamic_matched_is_silent() {
    assert_silent(&report_ops("void f(dynamic x) {\n  if (x case Null _) {}\n}\n"));
}

/// `T x; case Null _` — an unbounded type parameter's implicit `Object?`
/// bound is nullable (`isPotentiallyNullable(T)` recurses into the bound).
///
/// Oracle b.dart g1: zero rows (spec
/// `test_requiredNull_matchedNullable_typeParameterType_implicitBound`).
#[test]
fn required_null_unbounded_parameter_is_silent() {
    assert_silent(&report_ops("void f<T>(T x) {\n  if (x case Null _) {}\n}\n"));
}

/// `T extends num x; case Null _` — a non-nullable bound reads
/// non-nullable, so the parameter can never be `null`.
///
/// Oracle b.dart g2: `'T' can never match … 'Null'` (spec
/// `test_requiredNull_matchedNotNullable_typeParameterType`).
#[test]
fn required_null_bounded_parameter_reports() {
    assert_eq!(
        pattern_rows("void f<T extends num>(T x) {\n  if (x case Null _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type 'T' can never match the required type 'Null'.".to_string()
        )]
    );
}

/// `void Function() x; case Null _` — the function shape's own
/// nullability decides (the name-level sentinel cannot see it; the
/// `Ty`-shaped arm reads the `?` flag).
///
/// Oracle b.dart g3: `'void Function()' can never match … 'Null'` (spec
/// `test_requiredNull_matchedNotNullable_functionType`).
#[test]
fn required_null_function_matched_reports() {
    assert_eq!(
        pattern_rows("void f(void Function() x) {\n  if (x case Null _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type 'void Function()' can never match the required type 'Null'."
                .to_string()
        )]
    );
}

/// `(int,) x; case Null _` — the record shape reads the same way.
///
/// Oracle b.dart g4: `'(int,)' can never match … 'Null'`.
#[test]
fn required_null_record_matched_reports() {
    assert_eq!(
        pattern_rows("void f((int,) x) {\n  if (x case Null _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type '(int,)' can never match the required type 'Null'.".to_string()
        )]
    );
}

/// `Null x; case FutureOr<int> _` reports (the element is non-nullable,
/// `isPotentiallyNullable(FutureOr<int>)` = false) while `FutureOr<int?>`
/// stays silent (a nullable element keeps the union nullable).
///
/// Oracle b.dart g6/g7: row at g6, zero rows at g7.
#[test]
fn matched_null_future_or_decides_by_element() {
    assert_eq!(
        pattern_rows("void f(Null x) {\n  if (x case FutureOr<int> _) {}\n}\n"),
        [(
            "pattern_never_matches_value_type",
            "The matched value type 'Null' can never match the required type 'FutureOr<int>'."
                .to_string()
        )]
    );
    assert_silent(&report_ops(
        "void f(Null x) {\n  if (x case FutureOr<int?> _) {}\n}\n",
    ));
}

/// `Null x; case _ as Null` — the cast-pattern triple's headline: `Null
/// <: Null` makes the cast unnecessary, and nothing else reports.
///
/// Oracle a.dart f6: exactly `Unnecessary cast pattern.` (the ticket's
/// named defect — pre-fix this fixture produced
/// `pattern_never_matches_value_type` + `cast_from_null_always_fails`).
#[test]
fn cast_pattern_from_null_to_null_is_unnecessary() {
    assert_eq!(
        pattern_rows("void f(Null x) {\n  if (x case _ as Null) {}\n}\n"),
        [("unnecessary_cast_pattern", "Unnecessary cast pattern.".to_string())]
    );
}

/// `Null x; case _ as A?` — `Null <: A?` makes the cast unnecessary and
/// the never-match silent.
///
/// Oracle a.dart f8: exactly `Unnecessary cast pattern.`
#[test]
fn cast_pattern_from_null_to_nullable_is_unnecessary() {
    assert_eq!(
        pattern_rows("class A {}\nvoid f(Null x) {\n  if (x case _ as A?) {}\n}\n"),
        [("unnecessary_cast_pattern", "Unnecessary cast pattern.".to_string())]
    );
}

/// `Null x; case _ as A` — a non-nullable target kills the cast twice:
/// the never-match proves the pattern dead and `visitCastPattern`'s null
/// face reports the always-throwing cast. Emission order is the cast
/// check's own (never-match, then the null face).
///
/// Oracle a.dart f7: `cast_from_null_always_fails` +
/// `pattern_never_matches_value_type` (`'Null'` vs `'A'`) + `dead_code`.
#[test]
fn cast_pattern_from_null_to_non_nullable_dies() {
    assert_eq!(
        pattern_rows("class A {}\nvoid f(Null x) {\n  if (x case _ as A) {}\n}\n"),
        [
            (
                "pattern_never_matches_value_type",
                "The matched value type 'Null' can never match the required type 'A'.".to_string()
            ),
            (
                "cast_from_null_always_fails",
                "This cast always throws an exception because the expression always evaluates to 'null'.".to_string()
            ),
        ]
    );
}

/// The literal-`null` scrutinee (`switch (null)`) carries the same faces —
/// the ticket's named `'null case Null'` family shape.
///
/// Oracle b.dart g9: `cast_from_null_always_fails` +
/// `pattern_never_matches_value_type` (`'Null'` vs `'int'`) + `dead_code`;
/// g10 (`case Null _`) zero rows.
#[test]
fn literal_null_scrutinee_cast_pattern_dies() {
    assert_eq!(
        pattern_rows("void f() {\n  switch (null) {\n    case _ as int:\n      break;\n  }\n}\n"),
        [
            (
                "pattern_never_matches_value_type",
                "The matched value type 'Null' can never match the required type 'int'.".to_string()
            ),
            (
                "cast_from_null_always_fails",
                "This cast always throws an exception because the expression always evaluates to 'null'.".to_string()
            ),
        ]
    );
    assert_silent(&report_ops(
        "void f() {\n  switch (null) {\n    case Null _:\n      break;\n  }\n}\n",
    ));
}
