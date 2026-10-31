//! #3224 pins — `consume_send`'s void-operand face read the invocation's
//! RESULT type in place of the INVOKED EXPRESSION's own type, and the
//! completed send's static type never reached the callee row at all.
//!
//! Upstream `FunctionExpressionInvocationResolver.resolve` (upstream Dart
//! SDK source, pkg/analyzer/lib/src/dart/resolver/
//! function_expression_invocation_resolver.dart:55-56) reads
//! `receiverType = function.typeOrThrow` — the function expression's own
//! static type, pre-invocation — and hands THAT to
//! `_checkForUseOfVoidResult`, which reports only when it is
//! `VoidTypeImpl` (:163-177). The engine's send consume instead wrote the
//! invocation's result onto the callee row and then tested
//! `row.ty == Void`, so a `void`-returning call over a function-typed
//! callee landed `Void` on the row and reported the legal discard:
//! `typedef F<X> = void Function<Y extends X>(); void f(F<int> a) { a(); }`
//! — the invoked expression `a` is a `FunctionType`, never `void`.
//!
//! The consume now captures the pre-invocation discriminator (was the
//! invoked expression itself a completed send — `v()()`, the one shape
//! whose own static type IS a send result), tests the void face on that,
//! and writes the completed send's static type back onto the row
//! (`node.recordStaticType(returnType)`) once every pre-invocation face
//! has run. The write is what makes an invoked row answer its result
//! through `receiver_static_ty` instead of its callee reference.
//!
//! No live fixture reaches the re-worked face: a send link arms only on a
//! callee row abutting the argument group, and a nested invocation's
//! callee row never does (`v()()` never consumes) — so the spurious
//! report above was the only shape the old check ever produced. The
//! consumed-result faces below pin what must keep reporting, and the
//! discard face pins what must stay silent.
//!
//! The binary-LHS face (`v() + 1`) is NOT pinned here: it stays silent for
//! a reason outside this module — `mark_operator` tags only `Intent::Read`
//! rows and `fold_binary` bails for every non-`&&`/`||` operator without a
//! mark, so an invoked operand never reaches the void check. The row now
//! carries the result type that face needs; the gate itself lives in
//! `assignability.rs`/`engine_core.rs`.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `tests.rs` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves — the same convention `ticket_3215_tests.rs` keeps).

use crate::diagnose::test_rig::{code_names, report_ops, rows, CORE_PRELUDE};

/// The message the oracle spells for every face (messages.yaml
/// `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// The offsets of one code's rows, ascending.
fn offsets(report: &crate::diagnose::types::FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The source offset of the fixture's LAST `needle`, as a report offset
/// (the rig prepends [`CORE_PRELUDE`]).
fn at_last(fixture: &str, needle: &str) -> u32 {
    u32::try_from(CORE_PRELUDE.len() + fixture.rfind(needle).expect("needle")).expect("offset")
}

/// The spurious face: `a`'s declared `F<int>` folds to a function shape
/// whose return is `void`, but the INVOKED EXPRESSION `a` is a
/// `FunctionType` — upstream's `_checkForUseOfVoidResult(function,
/// function.typeOrThrow)` answers false, and the expression statement is
/// the legal discard. Nothing reports.
#[test]
fn ticket_3224_alias_typed_formal_discard_stays_silent() {
    let report = report_ops(
        "typedef F<X> = void Function<Y extends X>();\n\
         \x20 void f(F<int> a) {\n\
         \x20   a();\n\
         \x20 }",
    );
    assert!(code_names(&report).is_empty(), "{:?}", rows(&report));
}

/// A bare `v();` expression statement stays the legal discard: nothing
/// consumes the value, so neither the invoked-expression face (the callee
/// `v` is a function value) nor the operand faces fire.
#[test]
fn ticket_3224_expression_statement_discard_stays_silent() {
    let report = report_ops("void v() {}\nvoid main() {\n  v();\n}");
    assert!(code_names(&report).is_empty(), "{:?}", rows(&report));
}

/// A genuinely-`void` call whose result IS consumed — `int x = v();` —
/// still reports: the declaration's own operand face fires at the
/// callee's name token (the `MethodInvocation` `.at(methodName)` anchor),
/// and the faces above must not have traded it away.
#[test]
fn ticket_3224_void_result_used_by_declaration_reports() {
    let fixture = "void v() {}\nvoid main() {\n  int x = v();\n}";
    let report = report_ops(fixture);
    let at = offsets(&report, "use_of_void_result");
    assert_eq!(at, [at_last(fixture, "v()")], "{:?}", rows(&report));
    assert!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "use_of_void_result")
            .all(|d| d.message == VOID_MSG)
    );
}
