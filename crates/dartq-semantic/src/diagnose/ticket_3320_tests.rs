//! #3320 pins — where the `... the type 'Function'` receiver actually comes
//! from, and the regression floor a fix must keep.
//!
//! # Corrected mechanism (probed on the analyzer's own corpus)
//!
//! The family's EXTRA rows (`The method 'x' isn't defined for the type
//! 'Function'.`) do **not** originate in the callee-return materialization.
//! [`Engine::instantiate_call_return`] already honors the ticket's contract:
//! a context-solved instantiation, the declared return, its declared bound
//! for an unsolved bounded parameter, else the `Unknown` under-report — it
//! never yields a bare `Function` metaclass.
//!
//! The `Function` face is *synthesized in pass 2* by the `Receiver::Unknown`
//! fallback of `resolve_member_row` (`row_resolution.rs`):
//!
//! * the merge types any non-nameable call result (`Unknown`, `dynamic`,
//!   `void`) as `Receiver::Unknown`;
//! * `resolve_member_row`'s `Unknown` arm then asks
//!   `receiver_decl_shape(read_span)` (`row_resolution.rs:4025`) with the
//!   span of the *completed send* (`rawDyn(1).m` → the span of `rawDyn(1)`);
//! * that leg looks up the `refs` entry at the span's offset — the bare
//!   callee identifier — and answers its **tear-off** type: `RefTarget::Decl`
//!   over a `TopLevelFn` (`:4037`-`:4063`, `fn_sig_ty`), `RefTarget::Member`
//!   over a method (`:4031`), `RefTarget::Local` over a local function
//!   (`:4065`). A function type is then a *receiver* face, and
//!   `check_shape_receiver` renders its miss display as `Function`
//!   (`row_resolution.rs:7138`), right next to the `call` resolution that
//!   would be legal for a real function receiver.
//!
//! Three inputs reach that fallback, each with its own owner:
//!
//! 1. **Untyped / unsolved callee returns** — `later(v) => new Future...`
//!    (declared return `Unknown`), `T f<T>()` with no constraint, and a
//!    declared `dynamic`/`void` return. Nothing about the call types the
//!    result, the merge keeps `Unknown`, and the tear-off leak turns it into
//!    `Function`. (Probes: `plainDyn(1).m()`, `castObject(true).m()`,
//!    `unbounded().m()` — all `Function`; the same calls parenthesized
//!    (`(plainDyn)(1).m()`), prefixed (`o.plainDyn(1).m()`), written through
//!    `this.m()`, or read through a getter stay silent — the span's offset
//!    is then no bare function reference.)
//! 2. **Creation-expression arguments do not constrain inference** — with a
//!    plain argument the solve reaches the send (`identity(1).m()` names
//!    `int`), with a creation argument it does not (`identity(A()).m()`
//!    names `Function` instead of `A`).
//! 3. **A conditional's condition context does not reach a cascade target** —
//!    `bool q = castObject(true)` solves `T := bool`, and
//!    `(castObject(true)..m()) ? 1 : 2` must too (the oracle reports `bool`),
//!    but the cascade target never receives the condition's context.
//!
//! # What these pins hold
//!
//! The passing pins are the floor the `row_resolution.rs` owner (#3316) must
//! keep while gating the declaration-shape leg on "the receiver span is not a
//! completed send" (a `call_ends` lookup with `read_span.offset`, or an
//! `Intent::Invoke` probe on the row at that offset). The `#[ignore]`d pins
//! are the target behavior; they flip by deleting the attribute once the
//! owning lanes land (nothing here needs a `diagnose.rs` edit — the module is
//! registered through `diagnose/tests.rs`'s `#[path]` tail).

use super::test_rig::report_ops;
use super::*;

/// The messages of every row carrying `code`.
fn messages_named(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| row.message.to_string())
        .collect()
}

/// Every `code @offset: message` row whose message contains `needle`.
fn mentions(report: &FileReport, needle: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|row| format!("{} @{}: {}", row.code.name, row.range.offset, row.message))
        .filter(|row| row.contains(needle))
        .collect()
}

/// No row may name the `Function` shape: the receiver of a non-function
/// expression must never be typed as the function metaclass.
#[track_caller]
fn assert_no_function_receiver(report: &FileReport) {
    let rows = mentions(report, "the type 'Function'.");
    assert!(rows.is_empty(), "no row may name 'Function': {rows:#?}");
}

// —— The passing floor ——————————————————————————————————————————————

/// A REAL function receiver keeps the `Function` method face: the sentinel
/// booleans must not be "suppress everything named Function" — the fix gates
/// on the send's span, never on the receiver type.
#[test]
fn real_function_receiver_keeps_the_function_face() {
    let report = report_ops("void f(void Function() g) { g.bar(); }");
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert!(
        methods[0].contains("the type 'Function'."),
        "a function receiver keeps its shape display: {methods:?}"
    );
}

/// A declared return types the call's result, so the member walk runs on the
/// declared head — never on the callee's tear-off (`Future` carries no members
/// in this rig, so the row names `Future`; silence is the same pass).
#[test]
fn declared_return_types_the_call_result() {
    let report = report_ops(
        "Future<dynamic> later(Object v) => throw 0;\
         void f() { later(1).then((x) {}); }",
    );
    assert_no_function_receiver(&report);
    for row in mentions(&report, "'then'") {
        assert!(row.contains("type 'Future'"), "{row}");
    }
}

/// An argument-solved generic parameter reaches the send's result: the
/// upward solve is the existing face this ticket must not disturb.
#[test]
fn argument_solved_generic_types_the_receiver() {
    let report = report_ops(
        "T identity<T>(T v) => v;\
         void f() { identity(1).idMiss(); }",
    );
    assert_no_function_receiver(&report);
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert!(methods[0].contains("the type 'int'."), "{methods:?}");
}

/// An explicit type argument fixes the free parameter, so the receiver names
/// the instantiation (`bool`), not the callee's spelling.
#[test]
fn explicit_type_argument_types_the_receiver() {
    let report = report_ops(
        "T castObject<T>(Object value) => value as T;\
         void f() { castObject<bool>(true).castMiss(); }",
    );
    assert_no_function_receiver(&report);
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert!(methods[0].contains("the type 'bool'."), "{methods:?}");
}

/// The enclosing declaration's written type is the send's context, so the
/// free parameter solves downward (`T := bool`) — the `pending_decl_ty` face.
#[test]
fn declared_context_types_the_receiver() {
    let report = report_ops(
        "T castObject<T>(Object value) => value as T;\
         void f() { bool q = castObject(true); q.ctxMiss(); }",
    );
    assert_no_function_receiver(&report);
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert!(methods[0].contains("the type 'bool'."), "{methods:?}");
}

/// With no context and no argument constraint the `var`-inferred slot keeps
/// the under-report: the member access stays silent (never a `Function`
/// receiver) — the oracle's `dynamic` face.
#[test]
fn context_free_generic_keeps_the_under_report() {
    let report = report_ops(
        "T castObject<T>(Object value) => value as T;\
         void f() { var x = castObject(true); x.varMiss(); }",
    );
    assert_no_function_receiver(&report);
    assert!(
        mentions(&report, "varMiss").is_empty(),
        "{:#?}",
        mentions(&report, "varMiss")
    );
}

/// An explicit `.call` on a callable class resolves through the member walk:
/// the explicit-`call` face stays silent, as it must after the gate.
#[test]
fn explicit_call_member_resolves() {
    let report = report_ops(
        "class Callable { void call() {} }\
         void f(Callable c) { c.call(); }",
    );
    assert_no_function_receiver(&report);
    assert!(
        mentions(&report, "'call'").is_empty(),
        "{:#?}",
        mentions(&report, "'call'")
    );
}

/// A chained send (`g()()`) invoked twice produces no member row: the inner
/// send's result is a function *value*, and invoking it is legal — the
/// tear-off leak must not turn the chain into a `Function` receiver.
#[test]
fn chained_send_callee_is_not_a_receiver_face() {
    let report = report_ops("void f(int Function() Function() g) { g()(); }");
    assert_no_function_receiver(&report);
}

// —— The target pins (the `receiver_decl_shape` send gate is #3316's) ——

/// `later(v) => new Future.value(v)` has no declared return: the call's
/// result must be the body-inferred `Future<...>` (the oracle resolves
/// `.then` against it), never the callee's tear-off.
#[test]
fn untyped_top_level_callee_return_is_not_a_function_metaclass() {
    let report = report_ops(
        "later(v) => new Future.value(v);\
         void f() { later('A').then((x) {}); }",
    );
    assert_no_function_receiver(&report);
    for row in mentions(&report, "'then'") {
        assert!(row.contains("type 'Future'"), "{row}");
    }
}

/// A context-free unbounded generic call infers `dynamic` upstream, so the
/// member access is silent; today the tear-off leak names `Function`.
#[test]
fn context_free_unsolved_generic_receiver_is_dynamic_not_function() {
    let report = report_ops(
        "T castObject<T>(Object value) => value as T;\
         void f() { castObject(true).cfMiss(); }",
    );
    assert_no_function_receiver(&report);
    assert!(
        mentions(&report, "cfMiss").is_empty(),
        "{:#?}",
        mentions(&report, "cfMiss")
    );
}

/// A declared `dynamic` return types the result `dynamic`: the receiver is
/// not a function and the member access is silent.
#[test]
fn dynamic_callee_result_is_silent() {
    let report = report_ops(
        "dynamic plainDyn(Object v) => v;\
         void f() { plainDyn(1).dynMiss(); }",
    );
    assert_no_function_receiver(&report);
    assert!(
        mentions(&report, "dynMiss").is_empty(),
        "{:#?}",
        mentions(&report, "dynMiss")
    );
}

/// The conditional's condition is a `bool` context; the cascade target
/// receives it (upstream analyzes the cascade target under the enclosing
/// context), so `T := bool` and the miss names `bool` — today it names
/// `Function`.
#[test]
#[ignore = "target: the condition context does not reach a cascade target \
            (invocation.rs context plumbing) and the tear-off leak follows — #3316"]
fn conditional_condition_context_types_the_cascade_target() {
    let report = report_ops(
        "T castObject<T>(Object value) => value as T;\
         void f() { (castObject(true)..condMiss()) ? 1 : 2; }",
    );
    assert_no_function_receiver(&report);
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert!(methods[0].contains("the type 'bool'."), "{methods:?}");
}

/// A creation-expression argument carries its static type into the solve
/// (`identity(A())` binds `T := A`): the argument row closes untyped (the
/// creation parses as a send of the class name), and the send-close solve
/// reads the created class through `creation_arg_ty` (#3359).
#[test]
fn creation_argument_solves_the_generic_parameter() {
    let report = report_ops(
        "class A {}\
         T identity<T>(T v) => v;\
         void f() { identity(A()).makeMiss(); }",
    );
    assert_no_function_receiver(&report);
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert!(methods[0].contains("the type 'A'."), "{methods:?}");
}
