//! The #3064 pins — the generic send's downward context: the index
//! formal (`a[g()]++`) and the numeric argument context
//! (`h(a.remainder(f()))`).
//!
//! Upstream resolves every send under a context type: an index
//! expression analyzes its index under `[]`'s first formal
//! (resolver.dart:3378, `indexContextType` — property_element_resolver
//! hands the read element's `firstParameterType`, the write element's
//! for a pure `a[e] = v`), and an argument of `+`/`-`/`*`/`%`/
//! `remainder`/`clamp` reads the numeric context ladder
//! (`refineNumericInvocationContext`, type_system.dart:1881-1990) over
//! the receiver's static type and the enclosing position's context. A
//! generic-identity callee `T f<T>()` binds `T` from exactly that
//! context — `staticType: num` / `typeArgumentTypes: [num]` in the
//! oracle's `test_contextType_readWrite_writeLower`,
//! `test_clamp_double_context_int`, and `test_remainder_int_context_int`.
//!
//! Without the downward face the index argument stayed `Unknown` (the
//! compound write's `[]=` check had nothing to reject — the missing
//! `argument_type_not_assignable` at the index), and the numeric
//! refinement starved (`remainder` fell back to its declared `num`, so
//! an `int` sink over-reported).

use super::test_rig::report_ops;
use super::*;

fn codes(folded: &FileReport) -> Vec<&str> {
    folded
        .diagnostics
        .iter()
        .map(|diag| diag.code.name)
        .collect()
}

/// Drops the `undefined_method` rows the rig's minimal `int`/`double`
/// stubs owe (`class int` in the prelude declares the operator face
/// only, while the real corpus mounts dart:core where `remainder`/
/// `clamp` resolve through the frozen tables) — the pins below judge the
/// argument-type faces, not the stub's member misses.
fn without_stub_member_misses(folded: FileReport) -> FileReport {
    FileReport {
        diagnostics: folded
            .diagnostics
            .into_iter()
            .filter(|diag| diag.code.name != "undefined_method")
            .collect(),
        ..folded
    }
}

const INDEX_CLASS: &str = "class A {\n\
     \x20 int operator [](num index) => 0;\n\
     \x20 operator []=(int index, int value) {}\n\
     }\n\
     T g<T>() => throw 0;\n";

/// The upstream `test_contextType_readWrite_writeLower` shape: the
/// compound `a[g()]++` analyzes the index under `[]`'s `num` formal
/// (`hasRead: true` — `resolveForWrite`'s read element), so `g<T>` binds
/// `num` and the WRITE face's `[]=` index formal (`int`) rejects it —
/// `argument_type_not_assignable` 'num' => 'int' at the index argument.
#[test]
fn compound_index_argument_reports_write_formal() {
    let folded = report_ops(&format!("{INDEX_CLASS}void f(A a) {{\n  a[g()]++;\n}}\n"));
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "`a[g()]++` infers `num` from the `[]` formal and rejects against `[]=`'s `int`: {:?}",
        folded
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The compound-assignment twin: `a[g()] += 1` reads the same pair of
/// faces and reports the same single row.
#[test]
fn compound_index_assign_reports_write_formal() {
    let folded = report_ops(&format!(
        "{INDEX_CLASS}void f(A a) {{\n  a[g()] += 1;\n}}\n"
    ));
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "`a[g()] += 1` rejects the `[]`-inferred `num` against `[]=`'s `int`"
    );
}

/// The pure read checks only `[]`'s own formal — the `num` the context
/// inferred assigns cleanly, and nothing reports (upstream
/// `checkIndexExpressionIndex`'s `writeElement: null` arm).
#[test]
fn pure_read_index_argument_stays_silent() {
    let folded = report_ops(&format!("{INDEX_CLASS}void f(A a) {{\n  a[g()];\n}}\n"));
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`a[g()]` infers `num` from the `[]` formal and checks it against `[]` alone"
    );
}

/// The pure write `a[g()] = 0` resolves under `[]=`'s first formal
/// (`resolveForWrite`'s `hasRead: false` — the write element's
/// `firstParameterType`): `g<T>` binds `int` and the write check passes.
/// Inferring from the read spelling instead would bind `num` and
/// over-report.
#[test]
fn pure_write_index_argument_infers_setter_formal() {
    let folded = report_ops(&format!("{INDEX_CLASS}void f(A a) {{\n  a[g()] = 0;\n}}\n"));
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`a[g()] = 0` infers `int` from `[]=`'s index formal — silent"
    );
}

/// Matching read/write formals keep the compound access silent: the
/// `[]`-inferred `int` assigns into both index formals.
#[test]
fn matching_formals_compound_stays_silent() {
    let folded = report_ops(
        "class B {\n\
         \x20 int operator [](int index) => 0;\n\
         \x20 operator []=(int index, int value) {}\n\
         }\n\
         T g<T>() => throw 0;\n\
         void f(B b) {\n\
         \x20 b[g()]++;\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`b[g()]++` over `int`/`int` formals binds `int` — silent"
    );
}

/// A non-generic argument row keeps its own declared type: `c()`'s
/// `String` rejects against BOTH index formals of the compound access
/// (`checkIndexExpressionIndex`'s read arm against `num`, its write arm
/// against `int`) — the pre-existing #2937 check intact.
#[test]
fn nongeneric_index_argument_keeps_declared_type() {
    let folded = report_ops(&format!(
        "{INDEX_CLASS}String c() => '';\nvoid f(A a) {{\n  a[c()]++;\n}}\n"
    ));
    assert_eq!(
        codes(&folded),
        vec![
            "argument_type_not_assignable",
            "argument_type_not_assignable"
        ],
        "`a[c()]++` checks the declared `String` against both index formals"
    );
}

/// The upstream `test_remainder_int_context_int` shape: `h(int x)`
/// hands the `remainder` send the context `int`, the ladder
/// (`refineNumericInvocationContext`) hands `f<T>` the same `int`, and
/// the refined `int` return assigns into `h` — the row the starving
/// ladder over-reported ('num' => 'int') is gone.
#[test]
fn remainder_argument_infers_int_from_outer_context() {
    let folded = without_stub_member_misses(report_ops(
        "T f<T>() => throw 0;\n\
         void h(int x) {}\n\
         void g(int a) {\n\
         \x20 h(a.remainder(f()));\n\
         }\n",
    ));
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`h(a.remainder(f()))` infers `int` twice (the argument and the refined return) — silent"
    );
}

/// The upstream `test_clamp_double_context_int` shape: a `double`
/// receiver under an `int` context answers the ladder's fallback `num`
/// (`double` is no `int` subtype, `int` admits no `double`), so the
/// clamp send types `num` and `h(int x)` reports — the EXTRA the
/// starving ladder already produced, kept honest by the inferred `num`
/// arguments.
#[test]
fn clamp_args_infer_num_under_int_context() {
    let folded = without_stub_member_misses(report_ops(
        "T f<T>() => throw 0;\n\
         void h(int x) {}\n\
         void g(double a) {\n\
         \x20 h(a.clamp(f(), f()));\n\
         }\n",
    ));
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "`h(a.clamp(f(), f()))` types `num` against `h`'s `int`"
    );
}

/// The ladder's admit cut: a `num`-admitting context (`h2(num x)`)
/// refines nothing — the arguments bind `num`, the `remainder` return
/// stays `num`, and the `num` sink accepts.
#[test]
fn remainder_under_num_context_stays_num() {
    let folded = without_stub_member_misses(report_ops(
        "T f<T>() => throw 0;\n\
         void h2(num x) {}\n\
         void g(int a) {\n\
         \x20 h2(a.remainder(f()));\n\
         }\n",
    ));
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`h2(a.remainder(f()))` binds `num` end to end — silent"
    );
}

/// A second positional argument binds its own formal: the outer `int`
/// context reaches `f()` through `h`'s second parameter, and the first
/// argument's `int` literal stands.
#[test]
fn second_position_argument_binds_its_formal() {
    let folded = without_stub_member_misses(report_ops(
        "T f<T>() => throw 0;\n\
         void h(int a, int b) {}\n\
         void g(int x) {\n\
         \x20 h(x.remainder(f()), 3);\n\
         }\n",
    ));
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "the second formal's `int` context binds `f`'s `T` — silent"
    );
}
