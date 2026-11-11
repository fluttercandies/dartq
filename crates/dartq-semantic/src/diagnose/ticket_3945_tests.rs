//! #3945 pins — the `listener.rs` successor packet.
//!
//! (1) The `??` join's key follows the folded NODE row. `end_if_null`
//! stages `if_null_results[rhs_row] = (lhs_row, LUB(NonNull(lhs), rhs))`,
//! and `fold_binary` then replaces both operands with one `Value` row
//! spanning the whole node (whose own `ty` is deliberately `UNKNOWN` —
//! the callable-coercion join lives in `assignability.rs`'s
//! `join_operand_ty`). The parenthesized group's `inner_ty`
//! (`end_parenthesized_expression`) reads that node row, so a join keyed
//! only on the operand row was missed whenever the fold moved the top
//! row — an INVOKED right side (`(fa() ?? fb())`) leaves its callee head
//! row, never a value row. The re-key is additive: the operand key stays
//! (the `inner_ty` open-bracket guard reads the stored left offset, and
//! `invocation.rs`'s `receiver_face_ungrounded` matches keys anywhere
//! inside the extent).
//!
//! (2) The `var`-bind WRITE side. The read side is verbatim
//! (`bind_inferred_ty` preserves every written group including `dynamic`
//! segments, `local_declared_ty` returns the stored face), and the
//! explicit-instantiation creation shapes report their
//! `invalid_assignment` rows — the census case's eight rows are
//! `new A<int, dynamic>(…)`-style DIRECT constructions, which the pin
//! below covers.
//!
//! (3) A `<T>`-headed function literal's body reaches the checks under a
//! MATCHED generic context too (`int Function<T>(T) f = <T>(T x) {
//! return "s"; }` — the oracle reports `return_of_invalid_type_from_
//! closure 'String'`, and so does this tree). The literal's own `<T>`
//! group parses before `begin_function_expression`, so the capture's
//! count is seeded from the just-closed window (adjacency-gated) instead
//! of `closure_type_variable`, which never sees a pre-begin group.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine`.

use super::test_rig::{code_names, report_ops, rows};
use crate::diagnose::FileReport;

fn messages_of<'a>(report: &'a FileReport, code: &str) -> Vec<&'a str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.as_str())
        .collect()
}

// —— (1) the `??` group row carries the join ————————————————————

/// An INVOKED right operand's join reaches the parenthesized group's row:
/// `(fa() ?? fb())` over `A?`/`B` (`B extends A`) is `A`, so the group's
/// assignment to `int` reports the `'A'` face. Before the re-key the
/// group row stayed `Unknown` and every face over it suppressed.
#[test]
fn invoked_right_operand_group_types_as_the_if_null_join() {
    let report = report_ops(
        "class A {}\n\
         class B extends A { void onlyB() {} }\n\
         A? fa() => null;\n\
         B fb() => B();\n\
         void main() {\n\
         \x20 int g = (fa() ?? fb());\n\
         \x20 g.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "invalid_assignment");
    assert!(
        hits.iter()
            .any(|m| m.contains("'A'") && m.contains("'int'")),
        "expected invalid_assignment 'A' vs 'int' on the group: {:#?}",
        rows(&report)
    );
}

/// The same join drives the member face: `(fa() ?? fb()).onlyB()` reports
/// `undefined_method 'onlyB'` on `'A'` — the send resolves against the
/// group's joined receiver, never an `Unknown` suppress.
#[test]
fn invoked_right_operand_group_receiver_resolves_the_member() {
    let report = report_ops(
        "class A {}\n\
         class B extends A { void onlyB() {} }\n\
         A? fa() => null;\n\
         B fb() => B();\n\
         void main() {\n\
         \x20 var x = (fa() ?? fb()).onlyB();\n\
         \x20 x.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "undefined_method");
    assert!(
        hits.iter()
            .any(|m| m.contains("'onlyB'") && m.contains("'A'")),
        "expected undefined_method 'onlyB' on 'A': {:#?}",
        rows(&report)
    );
}

/// The keep-side of the re-key: `(fa() ?? fb()).hashCode` answers on
/// `Object`'s face, so the joined receiver resolves the member and no
/// row fires (the re-key must not turn the group into a miss).
#[test]
fn invoked_right_operand_group_keeps_object_members_silent() {
    let report = report_ops(
        "class A {}\n\
         class B extends A { void onlyB() {} }\n\
         A? fa() => null;\n\
         B fb() => B();\n\
         void main() {\n\
         \x20 var x = (fa() ?? fb()).hashCode;\n\
         \x20 x.toString();\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter")
            && !code_names(&report).contains(&"undefined_method"),
        "{:#?}",
        rows(&report)
    );
}

// —— (2) the `var`-bind write side ————————————————————————————————

/// An explicit-instantiation creation's written group survives into the
/// assignability face: `A<int, String> a = new A<int, dynamic>(…)` keeps
/// the `dynamic` segment, so the row spells `'A<int, dynamic>'` against
/// `'A<int, String>'`. This is the census case's eight-row shape (the
/// direct construction, not the `var`-inferred read).
#[test]
fn explicit_creation_args_reach_the_assignment_face() {
    let report = report_ops(
        "class A<S, T> {\n\
         \x20 A(S s, T t) {}\n\
         }\n\
         void main() {\n\
         \x20 A<int, String> a = new A<int, dynamic>(0, null);\n\
         \x20 a.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "invalid_assignment");
    assert!(
        hits.iter()
            .any(|m| m.contains("'A<int, dynamic>'") && m.contains("'A<int, String>'")),
        "expected 'A<int, dynamic>' vs 'A<int, String>': {:#?}",
        rows(&report)
    );
}

// —— (3) the `<T>`-headed literal's body channel ————————————————

/// A GENERIC literal under a MATCHED generic context reaches the return
/// check: `int Function<T>(T) f = <T>(T x) { return "s"; }` reports
/// `return_of_invalid_type_from_closure 'String'` against `'int'` (the
/// 3.13.7 oracle's own row), and the mismatch direction
/// (`int Function(int) f = <T>(T x) { return "s"; }`) reports too — so
/// the literal's own events always arrived and the gap was only the
/// context gate.
///
/// The `<T>` group is parsed before `begin_function_expression` (upstream
/// `parseLiteralListSetMapOrFunction` :8740-8753 runs `parseVariables`
/// before `parseFunctionExpression`), so `closure_type_variable` — which
/// keys on the literal's own frame being open — never counted it and the
/// capture's `type_params` stayed 0: `closure_ctx_matches(ctx, 0)`
/// rejected the matched 1-parameter context and the body's `return`
/// never reached it. `begin_function_expression` now seeds the count from
/// the just-closed type-variables window (adjacency-gated on the `>` that
/// sits right before the literal's `(`), so the matched context applies
/// and the return check spells the binder's own `'T'`/`'String'` face.
/// The two sibling pins in `ticket_3912_tests.rs`
/// (`generic_literal_return_checks_against_the_context_signature`,
/// `written_alias_args_never_substitute_a_shadowed_binder`) cover the
/// binder-spelling half.
#[test]
fn generic_literal_under_a_matched_context_checks_the_body() {
    let report = report_ops(
        "void main() {\n\
         \x20 int Function<T>(T) f = <T>(T x) { return \"s\"; };\n\
         \x20 f.toString();\n\
         }\n",
    );
    let hits = messages_of(&report, "return_of_invalid_type_from_closure");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].contains("'String'") && hits[0].contains("'int'"),
        "expected 'String' vs 'int': {:#?}",
        rows(&report)
    );
}

// —— (4) routed: the #3914 ordering ————————————————————————————————

/// Routed — the live read-latch bridge's remaining half is an ORDERING
/// problem, and the only viable shape is the deferred consult.
///
/// Evidence for the shape choice: `Flow::published_facts`' own doc
/// (dartq-flow record.rs:1036-1039) states an executable's rows do not
/// exist until its own replay at `Flow::exec_end` (record.rs:1294-1337) —
/// the replay walks the recorded event stream, which only closes there.
/// Incremental mid-replay publication is therefore impossible, and every
/// listener callback inside the executable runs BEFORE its facts exist.
/// `drain_published_read_types` (listener.rs) can only ever answer from
/// EARLIER executables.
///
/// The two consults that starve are in engine_core.rs:
/// `fold_binary`'s `promoted_operand_ty` (the `read_type_at` closure in
/// the fold) and `fold_conditional`'s `arm_ty`. Answering them after the
/// fact requires re-deciding the composite's type at teardown — a pure
/// binary/conditional type recompute entry (the fold itself mutates the
/// row stack and cannot be re-run). Owner: engine_core.rs (Lane BE
/// #3564) for the recompute entry, with the listener-side pending-consult
/// queue here.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn routed_promoted_operand_consults_answer_after_the_replay() {
    let report = report_ops(
        "void f(num n) {\n\
         \x20 if (n is int) {\n\
         \x20   int x = n + 1;\n\
         \x20   x = x;\n\
         \x20 }\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "the promoted operand must type the fold: {:#?}",
        rows(&report)
    );
}
