//! The #3147 pins: the return-family verdict machinery —
//! `return_of_invalid_type`, `return_without_value`,
//! `return_in_generator`, and `return_in_generative_constructor` —
//! against the oracle's `ReturnTypeVerifier` matrix.
//!
//! Upstream premises (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/return_type_verifier.dart`):
//!
//! * `verifyReturnStatement`:54-76 — a generative constructor's
//!   `return e;` reports `return_in_generative_constructor` at the
//!   expression and skips the assignability matrix; a generator's
//!   `return e;`/`=> e` early-returns (the parser's own
//!   `generatorReturnsValue` row owns it, parser_impl.dart:6628/:6855);
//!   a bare `return;` routes to `_checkReturnWithoutValue`.
//! * `_checkReturnWithoutValue`:292-308 — `return_without_value` at the
//!   `return` keyword unless `T` (sync) or `futureValueType(T)` (async)
//!   is `void`/`dynamic`/`Null`/`InvalidType`
//!   (`_isVoidDynamicOrNull`:341-346).
//! * `futureValueType` (`type_system.dart:471-498`): the async target
//!   reads through `Future<S>`/`FutureOr<S>` to `S`, `dynamic`/`void`
//!   stay themselves, everything else reads `Object?`.
//! * A record type's only interface supertypes are `Object` and
//!   `Record` (subtype.dart:283-290) — `(int, int)` reports against
//!   `int`/`num`/`String` and stays clean against `Object`.
//! * A creation send's static type is the instantiated head — `return
//!   D();` asks `D` against the declared return
//!   (`visitInstanceCreationExpression`), never the constructor
//!   declaration's absent `return_ty`.
//! * A generic function-typed expression in a context position reads
//!   its post-inference instantiation — `return g;` with `g: T
//!   Function<T>(T)` inside `int Function(int)` is clean (`S = int
//!   Function(int)`), while the arity `_isFunctionSubtypeOf` cannot
//!   bridge (`U Function<U>(U, int)`) reports
//!   (`relateTypeParameters`' count gate, type_system.dart:1585-1590).

use super::super::types::FileReport;
use super::async_return_tests::{code_names, report};

/// The codes this ticket owns; incidental warnings
/// (`unused_local_variable`, `dead_code`) never blur the verdict.
fn return_codes(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
        .into_iter()
        .filter(|code| {
            matches!(
                *code,
                "return_of_invalid_type"
                    | "return_of_invalid_type_from_closure"
                    | "return_without_value"
                    | "return_in_generator"
                    | "return_in_generative_constructor"
            )
        })
        .collect()
}

// —— bare `return;` —— `_checkReturnWithoutValue`:292-308 ————————————

/// A bare `return;` in a non-void host reports `return_without_value`
/// at the keyword — never `return_of_invalid_type` (oracle: `int f()
/// { return; }` is `RETURN_WITHOUT_VALUE` at the `return`).
#[test]
fn bare_return_in_int_host_reports_without_value() {
    let report = report("int f() { return; }");
    assert_eq!(return_codes(&report), ["return_without_value"]);
}

/// `void`/`dynamic`/`Null`-able targets accept the bare `return;`
/// silently (the `_isVoidDynamicOrNull` ok-set).
#[test]
fn bare_return_in_void_dynamic_nullable_hosts_stays_silent() {
    for source in [
        "void f() { return; }",
        "dynamic f() { return; }",
        "Future<void> f() async { return; }",
    ] {
        let report = report(source);
        assert!(
            return_codes(&report).is_empty(),
            "{source:?}: {:?}",
            code_names(&report)
        );
    }
}

/// A nullable declared return does NOT join the ok-set — `int?` is
/// neither `Null` nor a top type, so the bare `return;` reports
/// (oracle-verified: `return_without_value`). The `Object`/`Never`
/// fixtures spell the head through a declared class — the rig carries
/// no SDK, and an unresolved head is the oracle's `InvalidType` (which
/// joins the ok-set and stays silent, matching upstream's
/// `_isVoidDynamicOrNull`).
#[test]
fn bare_return_in_nullable_host_reports() {
    for source in [
        "int? f() { return; }",
        "class Object {}\nObject? f() { return; }",
        "Future<int?> f() async { return; }",
        "class Never {}\nNever f() { return; }",
    ] {
        let report = report(source);
        assert_eq!(
            return_codes(&report),
            ["return_without_value"],
            "{source:?}"
        );
    }
}

/// The async gate reads `futureValueType(T)`, not `T`: `Future<int>`
/// unwraps to `int`, a non-ok-set face — the bare `return;` reports.
#[test]
fn bare_return_in_async_future_int_reports() {
    let report = report("Future<int> f() async { return; }");
    assert_eq!(return_codes(&report), ["return_without_value"]);
}

// —— generators —— `verifyReturnStatement`:66-68 + parser_impl.dart
// :6628/:6855 ————————————————————————————————————————————————————————

/// A `sync*` arrow body reports `return_in_generator`, never the
/// assignability code (oracle: `Iterable<int> f() sync* => 3;`).
#[test]
fn sync_generator_arrow_reports_return_in_generator() {
    let report = report("Iterable<int> f() sync* => 3;");
    assert_eq!(return_codes(&report), ["return_in_generator"]);
}

/// The same row for an `async*` arrow body.
#[test]
fn async_generator_arrow_reports_return_in_generator() {
    let report = report("Stream<int> f() async* => 3;");
    assert_eq!(return_codes(&report), ["return_in_generator"]);
}

/// A `return e;` statement inside a generator reports
/// `return_in_generator` at the keyword — the verifier's
/// `isGenerator` early-return stands the assignability matrix down.
#[test]
fn sync_generator_statement_reports_return_in_generator() {
    for source in [
        "Iterable<int> f() sync* { return 3; }",
        "Iterable<String> f() sync* { return \"x\"; }",
        "Stream<int> f() async* { return \"x\"; }",
    ] {
        let report = report(source);
        assert_eq!(return_codes(&report), ["return_in_generator"], "{source:?}");
    }
}

/// A bare `return;` inside a generator reports nothing — the parser
/// emits only for a valued return, and the verifier early-returns.
#[test]
fn generator_bare_return_stays_silent() {
    for source in [
        "Iterable<int> f() sync* { return; }",
        "Stream<int> f() async* { return; }",
    ] {
        let report = report(source);
        assert!(
            return_codes(&report).is_empty(),
            "{source:?}: {:?}",
            code_names(&report)
        );
    }
}

/// A `yield` inside a generator is unaffected — the element check is
/// the `yield` family's own, and a matching element stays silent.
#[test]
fn sync_generator_yield_stays_silent() {
    let report = report("Iterable<int> f() sync* { yield 3; }");
    assert!(
        return_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

// —— generative constructors —— `verifyReturnStatement`:57-63 ————

/// `return e;` inside a generative constructor reports
/// `return_in_generative_constructor` at the expression — never the
/// assignability matrix (the `isGenerativeConstructor` early arm).
#[test]
fn generative_ctor_value_return_reports() {
    let report = report("class C { C() { return 3; } }");
    assert_eq!(return_codes(&report), ["return_in_generative_constructor"]);
}

/// A bare `return;` inside a generative constructor reports nothing
/// (the arm only fires on a present expression).
#[test]
fn generative_ctor_bare_return_stays_silent() {
    let report = report("class C { C() { return; } }");
    assert!(
        return_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A FACTORY constructor is not generative — its `return e;` runs the
/// ordinary return check (`factory C() { return 3; }` is the oracle's
/// `return_of_invalid_type` against the container face).
#[test]
fn factory_ctor_value_return_reports_invalid_type() {
    let report = report("class C { factory C() { return 3; } }");
    assert_eq!(return_codes(&report), ["return_of_invalid_type"]);
}

/// The factory's own-container twin stays clean — `return C._();`
/// asks `C` against `C`.
#[test]
fn factory_ctor_matching_return_stays_silent() {
    let report = report("class C { C._(); factory C() { return C._(); } }");
    assert!(
        return_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// The factory's foreign-container pair reports — `return D();` asks
/// `D` against the `C` host (the declared-subtype negative).
#[test]
fn factory_ctor_foreign_return_reports() {
    let report = report("class C { C._(); factory C() { return D(); } }\nclass D {}");
    assert_eq!(return_codes(&report), ["return_of_invalid_type"]);
}

// —— the creation send's static type ———————————————————————————————

/// `return C();` inside an `int` host asks `C` against `int` — the
/// expression's static type is the instantiated head, not the
/// constructor declaration's absent `return_ty` (oracle-verified:
/// `return_of_invalid_type`).
#[test]
fn creation_return_reports_invalid_type() {
    let report = report("class C {}\nint f() { return C(); }");
    assert_eq!(return_codes(&report), ["return_of_invalid_type"]);
}

/// The same read through a factory redirect — `return D();` asks `D`
/// against `int`.
#[test]
fn factory_creation_return_reports_invalid_type() {
    let report = report("class D { D._(); factory D() => D._(); }\nint f() { return D(); }");
    assert_eq!(return_codes(&report), ["return_of_invalid_type"]);
}

// —— record sources —— subtype.dart:283-290 —————————————————————————

/// `return (1, 2);` reports against every non-`Object`/non-`Record`
/// named target — a record type's only interface supertypes are
/// `Object` and `Record`.
#[test]
fn record_return_against_named_host_reports() {
    for source in [
        "int f() { return (1, 2); }",
        "num f() { return (1, 2); }",
        "int f() { return (a: 1, b: \"s\"); }",
    ] {
        let report = report(source);
        assert_eq!(
            return_codes(&report),
            ["return_of_invalid_type"],
            "{source:?}"
        );
    }
}

/// `Object`/`Record` targets accept the record source silently.
#[test]
fn record_return_against_top_targets_stays_silent() {
    for source in [
        "Object f() { return (1, 2); }",
        "(int, int) f() { return (1, 2); }",
    ] {
        let report = report(source);
        assert!(
            return_codes(&report).is_empty(),
            "{source:?}: {:?}",
            code_names(&report)
        );
    }
}

/// The record-record shape mismatch keeps the pairwise rule —
/// `(int, int)` against `(int, String)` reports.
#[test]
fn record_return_against_mismatched_record_reports() {
    let report = report("(int, String) f() { return (1, 2); }");
    assert_eq!(return_codes(&report), ["return_of_invalid_type"]);
}

// —— the value-return matrix —— `verifyReturnExpression`:209-256 ———

/// The ordinary mismatches stay: `String` into `int`, `Null` into a
/// non-nullable `int`, a `Future<int>` source into `int`, and a
/// `void`-typed expression into a non-void/dynamic host.
#[test]
fn value_return_mismatches_report() {
    for source in [
        "int f() { return \"s\"; }",
        "int f() { return null; }",
        "Future<int> g() async => 3;\nint f() { return g(); }",
        "void g() {}\nint f() { return g(); }",
        "void g() {}\nFuture<int> f() async { return g(); }",
    ] {
        let report = report(source);
        assert_eq!(
            return_codes(&report),
            ["return_of_invalid_type"],
            "{source:?}"
        );
    }
}

/// The matching pairs stay silent — `int` into `int`, `Null` into
/// `int?`, and the async body's `Future<int>`-declared `3`.
#[test]
fn value_return_matches_stay_silent() {
    for source in [
        "int f() { return 0; }",
        "int? f() { return null; }",
        "Future<int> f() async => 3;",
        "Future<int> f() async { return 3; }",
    ] {
        let report = report(source);
        assert!(
            return_codes(&report).is_empty(),
            "{source:?}: {:?}",
            code_names(&report)
        );
    }
}

// —— the generic-instantiation arm —— type_system.dart:1585-1590 ————

/// `return g;` with `g: T Function<T>(T)` inside an `int
/// Function(int)` host is clean — the context instantiates the
/// generic tear-off to `int Function(int)` before the assignability
/// ask (oracle: no `return_of_invalid_type`).
#[test]
fn generic_fn_return_instantiates_to_context() {
    let report =
        report("T id<T>(T x) => x;\nint Function(int) f() { T Function<T>(T) g = id; return g; }");
    assert!(
        return_codes(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// The arity the instantiation cannot bridge reports — `U
/// Function<U>(U, int)` never instantiates into `int Function(int)`
/// (two required positionals against one).
#[test]
fn generic_fn_arity_mismatch_reports() {
    let report = report(
        "U two<U>(U x, int y) => x;\nint Function(int) f() { U Function<U>(U, int) g = two; return g; }",
    );
    assert_eq!(return_codes(&report), ["return_of_invalid_type"]);
}
