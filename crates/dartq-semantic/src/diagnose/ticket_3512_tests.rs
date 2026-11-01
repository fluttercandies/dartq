//! #3512 pins — a callable class assigns to `Function` whether or not the
//! use writes type arguments.
//!
//! # The contract
//!
//! `isAssignableTo`'s callable arm (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:875-884) accepts a
//! non-nullable interface source whenever `getCallMethodType` is non-null
//! (`:559-566` — `call` looked up through the inheritance chain and
//! instantiated by the receiver's own type arguments). The follow-up
//! `acceptsFunctionType` ask (`:121-126`) is unconditional, `call` being a
//! function-typed member and `Function` the supertype of every function
//! type, so the member's SIGNATURE never decides there — only its
//! existence does. Existence is independent of the written instantiation:
//!
//! ```dart
//! class C0 { void call() {} }
//! class C1<T> { void call(T t) {} }
//! void probe() {
//!   Function a = C0();       // silent twin control — no type arguments
//!   Function b = C1<int>();  // the #3512 row: silent too
//! }
//! ```
//!
//! An args-conditioned second proof is how the instantiated head drifted
//! into a false `invalid_assignment` on legal code (#3512). Both spellings
//! now run the one member-face proof (`call_tear_off_verdict`), alias heads
//! folded first: the proof reads the `call` member of the substituted
//! container by bare name, so a written argument list cannot lose it.
//!
//! Where the written signature DOES decide is a written function-type
//! target (`void Function(int) f = C1<int>()`): there `callMethodType` is
//! related to the target by `isFunctionSubtypeOf`, so the instantiated
//! `void call(int)` accepts an `int`-taking target and rejects a
//! `String`-taking one with the oracle's `invalid_assignment`.
//!
//! # The rejections this fix must not buy silence with
//!
//! - a class with no `call` at all (`callMethodType` null —
//!   `InvalidAssignment_ImplicitCallReferenceTest/test_invalid_noCall_functionContext`);
//! - a `static call` — no instance face (`getMember3` answers instance
//!   members only);
//! - a `call` GETTER — `lookUpMethod` answers a method element or nothing
//!   (`language/function/type_call_getter2_test`);
//! - a nullable receiver — the acceptance is gated on `!isNullable(fromType)`
//!   (co19 `nnbd/implicit_conversions_A03_t01`).
//!
//! # Oracle verification
//!
//! Every expectation below was oracle-verified against
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze --format machine` over
//! the shapes spelled here: bare / instantiated / inherited /
//! mixin-applied / alias spellings silent; the no-`call`, static-`call`,
//! `call`-getter and nullable-receiver rows each reporting
//! `INVALID_ASSIGNMENT` at the whole right-hand expression.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops};

/// The assignability rows of `source` — `(code, offset, written text at
/// the row)` per row, the row messages in the same order, and the prefixed
/// source the offsets index. `invalid_assignment` is the declaration
/// channel, `argument_type_not_assignable` the same verdict in argument
/// position; side rows of the rig (unused locals, prelude resolution noise)
/// are other faces and stay out of these pins.
fn verdicts(source: &str) -> (Vec<(&'static str, u32, String)>, Vec<String>, String) {
    // The rig mounts the prelude ahead of the fixture, so the row offsets
    // index the prefixed text the third element carries.
    let prefixed = format!("{CORE_PRELUDE}{source}");
    let report: FileReport = report_ops(source);
    let (rows, messages) = report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "invalid_assignment" | "argument_type_not_assignable"
            )
        })
        .map(|d| {
            (
                (
                    d.code.name,
                    d.range.offset,
                    prefixed[d.range.offset as usize..d.range.end() as usize].to_string(),
                ),
                d.message.to_string(),
            )
        })
        .unzip();
    (rows, messages, prefixed)
}

/// Offset of `needle` inside the first occurrence of `line` in `source`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source
        .find(line)
        .unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

/// The bare head (the silent twin control) and the instantiated head
/// accept alike — and so do their inherited, mixin-applied and alias
/// spellings: the proof is the `call` member's existence on the substituted
/// container, never the written argument list.
///
/// Oracle: silent on all six rows (the `C0` / `C1<int>` shape of
/// pkg/analyzer `ImplicitCallReferenceTest`, plus the instantiated
/// superclass / `with M<T>` / `typedef` variants).
#[test]
fn bare_instantiated_inherited_and_aliased_callable_heads_accept() {
    let fixture = "class C0 {\n  void call() {}\n}\n\
                   class C1<T> {\n  void call(T t) {}\n}\n\
                   class Base<T> {\n  void call(T t) {}\n}\n\
                   class C2<T> extends Base<T> {}\n\
                   mixin M<T> {\n  void call(T t) {}\n}\n\
                   class C3<T> with M<T> {}\n\
                   typedef CC = C0;\n\
                   typedef GG<T> = C1<T>;\n\
                   void probe() {\n\
                     Function a = C0();\n\
                     Function b = C1<int>();\n\
                     Function c = C2<String>();\n\
                     Function d = C3<int>();\n\
                     Function e = CC();\n\
                     Function f = GG<int>();\n\
                   }\n";
    let (rows, ..) = verdicts(fixture);
    assert_eq!(rows, Vec::new(), "{rows:#?}");
}

/// A written function-type target reads the INSTANTIATED `call` face: the
/// `int`-taking target absorbs `C1<int>()`'s `void call(int)` and stays
/// silent, while the `String`-taking target reports — which is also the
/// proof that the acceptance above is not blanket silence.
///
/// Oracle: one `INVALID_ASSIGNMENT` at `C1<int>()` on the `bad` row,
/// message `A value of type 'C1<int>' can't be assigned to a variable of
/// type 'void Function(String)'.`
#[test]
fn a_written_function_target_reads_the_instantiated_call_face() {
    let fixture = "class C1<T> {\n  void call(T t) {}\n}\n\
                   void probe() {\n\
                     void Function(int) ok = C1<int>();\n\
                     void Function(String) bad = C1<int>();\n\
                   }\n";
    let (rows, messages, source) = verdicts(fixture);
    assert_eq!(
        rows,
        [(
            "invalid_assignment",
            at(
                &source,
                "void Function(String) bad = C1<int>();",
                "C1<int>()"
            ),
            "C1<int>()".to_string()
        )],
        "{rows:#?}"
    );
    let message = messages.first().expect("the `bad` row has a message");
    assert!(
        message.starts_with("A value of type 'C1<int>' can't be assigned to")
            && message.contains("'void Function(String)'")
            && message.ends_with('.'),
        "{message:?}"
    );
}

/// The rejections that keep the acceptance honest: a class with no `call`
/// (`callMethodType` null, type_system.dart:910-915), a `static call` (no
/// instance face on `getMember3`), and a `call` GETTER (`lookUpMethod`
/// answers a method element only, type.dart:1011-1040) each report at the
/// whole right-hand expression.
///
/// Oracle: three `INVALID_ASSIGNMENT` rows, one per assignment.
#[test]
fn no_call_static_call_and_call_getter_still_reject() {
    let fixture = "class NoCall {\n  void run() {}\n}\n\
                   class StaticCall<T> {\n  static void call() {}\n}\n\
                   class GetterCall {\n  void Function() get call => () {};\n}\n\
                   void probe() {\n\
                     Function a = NoCall();\n\
                     Function b = StaticCall<int>();\n\
                     Function c = GetterCall();\n\
                   }\n";
    let (rows, _, source) = verdicts(fixture);
    assert_eq!(
        rows,
        [
            (
                "invalid_assignment",
                at(&source, "Function a = NoCall();", "NoCall()"),
                "NoCall()".to_string()
            ),
            (
                "invalid_assignment",
                at(&source, "Function b = StaticCall<int>();", "StaticCall<int>()"),
                "StaticCall<int>()".to_string()
            ),
            (
                "invalid_assignment",
                at(&source, "Function c = GetterCall();", "GetterCall()"),
                "GetterCall()".to_string()
            ),
        ],
        "{rows:#?}"
    );
}

/// A nullable receiver is outside the acceptance (`!isNullable(fromType)`
/// guards the whole arm), for the instantiated and the bare head alike
/// (co19 `nnbd/implicit_conversions_A03_t01`).
///
/// Oracle: `INVALID_ASSIGNMENT` at each receiver operand.
#[test]
fn a_nullable_callable_receiver_still_rejects() {
    let fixture = "class C0 {\n  void call() {}\n}\n\
                   class C1<T> {\n  void call(T t) {}\n}\n\
                   void probe(C1<int>? generic, C0? bare) {\n\
                     Function a = generic;\n\
                     Function b = bare;\n\
                   }\n";
    let (rows, _, source) = verdicts(fixture);
    assert_eq!(
        rows,
        [
            (
                "invalid_assignment",
                at(&source, "Function a = generic;", "generic"),
                "generic".to_string()
            ),
            (
                "invalid_assignment",
                at(&source, "Function b = bare;", "bare"),
                "bare".to_string()
            ),
        ],
        "{rows:#?}"
    );
}
