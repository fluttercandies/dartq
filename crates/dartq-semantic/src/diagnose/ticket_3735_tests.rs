//! Ticket #3735 pins: the four salvaged shapes from the rejected #3732
//! wave — the await-flatten through a type-variable bound, the `A<void>`
//! written type argument, the bare-container `Type` literal in
//! expression position, and the named-constructor implicit creation's
//! argument faces.
//!
//! Each pin reproduces the upstream behavior the shape implements; the
//! shapes were verified net-neutral against the full lib suite.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

// -- Shape 1: await flatten through a type-variable bound ---------------
//
// `X extends Future<int>` — awaiting `X` must flatten through the bound
// to `int`. The oracle's `TypeSystem::flatten` reads the bound's future
// type; the name-level `flatten_ty` helper could not reach the parameter
// arm (it has no bound table). `flatten_param` walks the bound chain.

/// `X extends Future<int>` — `await x` where `x: X` types to `int`,
/// not `X`. The assignment `int i = await x` must not report a type
/// mismatch (the flattened bound IS `int`).
#[test]
fn await_flattens_through_future_bound() {
    let fixture = "Future<void> f<X extends Future<int>>(X x) async {\n\
                   \x20 int i = await x;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "argument_type_not_assignable").is_empty(),
        "await X flattens through the Future<int> bound"
    );
}

/// `X extends FutureOr<String>` — `await x` types to `String`.
#[test]
fn await_flattens_through_future_or_bound() {
    let fixture = "Future<void> f<X extends FutureOr<String>>(X x) async {\n\
                   \x20 String s = await x;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "argument_type_not_assignable").is_empty(),
        "await X flattens through the FutureOr<String> bound"
    );
}

/// `X extends Future<int>?` — `await x` types to `int?` (the `?`
/// survives the flatten, mirroring the `S?` arm).
#[test]
fn await_flattens_nullable_future_bound() {
    let fixture = "Future<void> f<X extends Future<int>?>(X x) async {\n\
                   \x20 int? i = await x;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "argument_type_not_assignable").is_empty(),
        "await X? flattens through the nullable Future<int> bound"
    );
}

// -- Shape 2: `A<void>` written type argument resolves to `Ty::Void` ----
//
// `literal_group_args` reads the registered type-uses only, and `void`
// carries none — the group's whole parse declined and the raw `T`
// parameter survived into the argument check (`new A<void>(null)`
// reported `Null` against `T` instead of checking against `void`).

/// `A<void>` with a `Null` argument — the written `void` type argument
/// must survive to the formal check, so `null` against `void` does
/// not report `Null`-assignable-to-`T` noise.
#[test]
fn void_type_arg_not_null_against_t() {
    let fixture = "class A<T> {\n\
                   \x20 A(T t);\n\
                   }\n\
                   void f() {\n\
                   \x20 var a = A<void>(null);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    // The `void` argument must not produce a type mismatch against `T`
    // as if `T` were the formal — `Null` against `T` was the bug.
    let null_vs_t = spans_of(&report, &full, "argument_type_not_assignable")
        .iter()
        .any(|span| span.contains("null"));
    assert!(!null_vs_t, "A<void>(null) must not report Null-against-T");
}

// -- Shape 3: bare alias/container heads resolve to `Type` in expression position
//
// `C` in `f(C)` types to `Type`, not `Unknown`. The `_resolve2` rewrite
// fires on the element kind — a class name in expression position IS a
// `TypeLiteral` upstream, regardless of what its var_ty carries.

/// A bare class name in argument position types to `Type`, so passing
/// `C` to a `Type` parameter must not report.
#[test]
fn bare_class_name_is_type_literal() {
    let fixture = "class C {}\n\
                   void f(Type t) {}\n\
                   void g() {\n\
                   \x20 f(C);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "argument_type_not_assignable").is_empty(),
        "bare class name in expression position types to Type"
    );
}

/// A bare typedef name in argument position also types to `Type`.
#[test]
fn bare_typedef_name_is_type_literal() {
    let fixture = "typedef F = void Function();\n\
                   void f(Type t) {}\n\
                   void g() {\n\
                   \x20 f(F);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "argument_type_not_assignable").is_empty(),
        "bare typedef name in expression position types to Type"
    );
}

// -- Shape 4: named-constructor implicit creation argument faces --------
//
// `C.named(...)` — the constructor-reference posture pushes no callee
// row, so `check_arguments` returns early and the count/assignability
// faces never run. The dedicated `check_named_creation_arguments`
// resolves the constructor's formals and walks each face.

/// `C.named(null)` — the named constructor's positional formal `int`
/// must check `Null` against `int` and report the mismatch.
#[test]
fn named_ctor_reports_argument_type_mismatch() {
    let fixture = "class C {\n\
                   \x20 C.named(int x);\n\
                   }\n\
                   void f() {\n\
                   \x20 var c = C.named(null);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let rows = spans_of(&report, &full, "argument_type_not_assignable");
    assert!(
        !rows.is_empty(),
        "C.named(null) must report argument_type_not_assignable"
    );
}

/// `C.named()` with a missing required positional — the count face
/// reports `not_enough_positional_arguments`.
#[test]
fn named_ctor_reports_missing_required() {
    let fixture = "class C {\n\
                   \x20 C.named(int x, int y);\n\
                   }\n\
                   void f() {\n\
                   \x20 var c = C.named(1);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let rows = spans_of(&report, &full, "not_enough_positional_arguments");
    assert!(
        !rows.is_empty(),
        "C.named(1) against (int, int) must report not_enough_positional_arguments"
    );
}
