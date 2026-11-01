//! #3582 probes — `argument_type_not_assignable` divergence families.
//!
//! The census (20261102-045457) reports the code at 494 missing / 655
//! extra. Each pin below reproduces one clustered shape with the oracle's
//! own answer written beside it (dart 3.13.7, `--format=machine`).

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, report_ops_imports, rows, RigImport};

/// Probe A — foreign old-style function typedef used nullable: the co19
/// `_CheckExceptionFn` family (70 extra rows). `typedef bool F(x)` is the
/// OLD-style function-alias spelling whose untyped formal reads `dynamic`;
/// a `bool Function(dynamic)` closure source assigns into `F?` and the
/// oracle is silent.
#[test]
fn t3582_probe_foreign_old_style_alias_nullable() {
    // oracle: silent.
    let report = report_ops_imports(
        "void main() { take2(() {}, (e) => e is int); }",
        &[RigImport {
            uri: "package:t/alias.dart",
            prefix: None,
            source: "typedef bool CheckFn(exception); void take2(dynamic body, [CheckFn? check, String reason = '']) {}",
        }],
    );
    assert_eq!(
        code_names(&report).is_empty(),
        true,
        "probe A rows: {:?}",
        rows(&report)
    );
}

/// Probe A′ — same file, no import seam: the local old-style typedef.
#[test]
fn t3582_probe_local_old_style_alias_nullable() {
    // oracle: silent.
    let report = report_ops(
        "typedef bool CheckFn(exception); void take2(dynamic body, [CheckFn? check]) {} void main() { take2(() {}, (e) => e is int); }",
    );
    assert_eq!(
        code_names(&report).is_empty(),
        true,
        "probe A' rows: {:?}",
        rows(&report)
    );
}

/// Probe B — modern function typedef nullable, foreign.
#[test]
fn t3582_probe_foreign_modern_alias_nullable() {
    // oracle: silent.
    let report = report_ops_imports(
        "void main() { take2(() {}, (e) => e is int); }",
        &[RigImport {
            uri: "package:t/alias.dart",
            prefix: None,
            source: "typedef CheckFn = bool Function(dynamic); void take2(dynamic body, [CheckFn? check]) {}",
        }],
    );
    assert_eq!(
        code_names(&report).is_empty(),
        true,
        "probe B rows: {:?}",
        rows(&report)
    );
}

/// Probe C — the `E<T>` enum-entry family (64 extra `int => T` rows) is
/// DEFERRED to the invocation layer: upstream solves the callee's type
/// inference before the ask (`E.named`'s formal arrives already
/// substituted to `int`), so the `argument_type_not_assignable` question
/// never spells a bare `T`. `assignable`'s `Some(false)` on `int <: T` is
/// subtype-correct — the divergence is the caller asking the
/// unsubstituted pair (`invocation.rs` / enum-entry formal resolution),
/// not this verdict. No pin: the face cannot reach the oracle's answer
/// from inside the verdict.
///
/// Probe D — the `T => Object?` extra family (24+ rows: `T`/`X`/`S => Object?`,
/// `T => Map<T, A>`, `T => void Function()`): an in-scope, non-promoted
/// type parameter source answers through its bound, and the ask runs on
/// the WHOLE nullable target — `Object? <: Object?` accepts where
/// decomposing to `Object` + `Null` rejects. Oracle 3.13.7 silent on every
/// row below.
#[test]
fn t3582_param_source_against_nullable_target_reads_bound() {
    // dart analyze on `void eq(Object? a) {} class C<T> { void m(T t) {
    // eq(t); } }`: silent — `T`'s implicit `Object?` bound fits `Object?`.
    let report = report_ops(
        "void eq(Object? a) {} class C<T> { void m(T t) { eq(t); } void n<S>(S s) { eq(s); } }",
    );
    assert_silent(&report);
}

/// A bounded param source against a nullable target that its bound
/// cannot meet stays a report — `T extends num` against `String?`
/// (`num <: String?` is false): the bound-read arm answers, not silence.
#[test]
fn t3582_bounded_param_still_rejects() {
    // dart analyze on `void s(String? a) {} void f<T extends num>(T t) {
    // s(t); }` reports argument_type_not_assignable `num => String?`.
    let report = report_ops(
        "void s(String? a) {} void f<T extends num>(T t) { s(t); }",
    );
    assert_eq!(
        code_names(&report),
        vec!["argument_type_not_assignable"],
        "rows: {:?}",
        rows(&report)
    );
}

/// Probe E — the `Null => T` extra family: `Null` against an in-scope
/// parameter target is `Null <: T`'s potential-nullability question —
/// `Null <: B`. An unbounded `T` reads `Object?` and accepts (oracle
/// silent); a nullable-bound `T extends int?` accepts too.
#[test]
fn t3582_null_into_potentially_nullable_param_silent() {
    // dart analyze on `void f<T>(T t) { f(null); }` — `Null <: T` where
    // `T`'s bound is `Object?`: silent. Same for `T extends int?`.
    let report = report_ops(
        "void f<T>(T t) { f(null); } void g<S extends int?>(S s) { g(null); }",
    );
    assert_silent(&report);
}

/// The non-nullable-bound twin: `S extends num` is never potentially
/// nullable, so `s(null)` keeps a rejection. The oracle's surface row is
/// `could_not_infer` (inference fails `R` before the argument check runs —
/// invocation layer); the verdict's own answer stays `Null <: num` = false,
/// pinned as the arg-check row the current face emits.
#[test]
fn t3582_null_into_nonnullable_param_reports() {
    let report = report_ops(
        "void g<S extends num>(S s) { g(null); }",
    );
    assert_eq!(
        code_names(&report),
        vec!["argument_type_not_assignable"],
        "rows: {:?}",
        rows(&report)
    );
}
