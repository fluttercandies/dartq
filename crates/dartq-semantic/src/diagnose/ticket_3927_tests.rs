//! #3927 pins — the invocation.rs callee-shape packet: the four named
//! fill-ins plus the callee-shape matrix clusters landed here.
//!
//! * the creation-argument capture's own-type outranking (M4, routed from
//!   #3917): a const argument's captured face is the const evaluator's
//!   DOWNWARD answer, so `check_named_creation_arguments` must read the
//!   argument's own static type the way the sibling walks do.
//! * the call bounds face's strict relation (AI-a, routed from #3917):
//!   upstream asks `isSubtypeOf` (`type_arguments_verifier.dart:373`).
//! * the `??` composite send argument (AI-c, routed from #3917): the
//!   argument face consults the join-aware [`Engine::rhs_row_ty`].
//! * the trailing-call application depth (DND1, routed from #3911): a
//!   group chained directly onto a completed send invokes its RESULT.
//! * the bound display's binder-verbatim render (routed from #3912):
//!   `subst_params`'s `Ty::Function` arm keeps the written `<T extends
//!   num>` group, which shadows the substitution map.

use super::FileReport;
use super::test_rig::{code_names, report_ops, rows};

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// Every row's `(code, message)` pair, source-ordered — the probe shape a
/// routed pin asserts over when the expected code is not yet known.
fn all_rows(report: &FileReport) -> Vec<(String, String)> {
    let mut rows = report.diagnostics.iter().collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter()
        .map(|d| (d.code.name.to_string(), d.message.to_string()))
        .collect()
}

/// The `??` composite as a send argument reads the JOIN (`f(a ?? 3)` under
/// `void f(int x)` asks `int`, never the bare `int?` operand). The
/// composite parks as an `Unknown` `Value` row — the name-level model
/// cannot spell the join — so the argument face must consult
/// [`Engine::rhs_row_ty`] after the row's own face answers nothing.
///
/// The second statement keeps this pin honest about the mechanism: the
/// join must actually COMPUTE for the same composite shape (`String s =
/// a ?? 3` reports the oracle's `int` -> `String` row). A report where
/// the fold's composite row is drained before any face reads it would
/// pass the send half vacuously (silence either way) while silently
/// losing this row — so the assignment half fails under the drain.
#[test]
fn if_null_composite_send_argument_joins() {
    let report = report_ops(
        "void f(int x) {}\n\
         void m(int? a) {\n\
         \x20 f(a ?? 3);\n\
         \x20 String s = a ?? 3;\n\
         }\n",
    );
    assert!(
        texts(&report, "argument_type_not_assignable").is_empty(),
        "the joined argument fits the formal, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
    let assignment = texts(&report, "invalid_assignment");
    assert_eq!(
        assignment.len(),
        1,
        "the join must compute for the assignment read too (oracle: one \
         `int` -> `String` row), got {assignment:?} ({:?})",
        all_rows(&report)
    );
    assert!(
        assignment[0].contains("'int'") && assignment[0].contains("'String'"),
        "the row spells the joined argument and the declared type like \
         the oracle, got: {assignment:?}"
    );
}

/// The `??` composite's join is still a real verdict: a nullable `String`
/// joined with `int` is `Object`, which `void f(int x)` refuses.
///
/// The below-mark recovery claims the folded `Value` row and `rhs_row_ty`
/// answers the join `LUB(NonNull(String?), int)` = `Object` (upstream
/// `_resolveIfNull`, binary_expression_resolver.dart:218-252). The verdict
/// itself needed `Object <: int` decidable — the non-nullable root sits
/// under nothing but the nullable tops, so `assignable`'s `Object` source
/// arm answers `Some(false)` against every concrete head the world can
/// name (#3989).
#[test]
fn if_null_composite_send_argument_still_rejects_a_foreign_join() {
    let report = report_ops(
        "void f(int x) {}\n\
         void m(String? s) {\n\
         \x20 f(s ?? 3);\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert_eq!(
        atna.len(),
        1,
        "the `String | int` join refuses `int`, got {atna:?} ({:?})",
        all_rows(&report)
    );
}

/// The regular-bounded check asks the STRICT relation (upstream
/// `type_arguments_verifier.dart:373` `isSubtypeOf`): `void` against
/// `num`/`Object`/a function-typed bound refuses. The assignable face's
/// void posture would grant every one of them.
#[test]
fn void_type_argument_refuses_the_bounds() {
    let report = report_ops(
        "void f<T extends num>(T t) {}\n\
         void g<T extends Object>() {}\n\
         void h<T extends int Function()>() {}\n\
         void m() {\n\
         \x20 f<void>(3);\n\
         \x20 g<void>();\n\
         \x20 h<void>();\n\
         }\n",
    );
    let text = texts(&report, "type_argument_not_matching_bounds");
    assert_eq!(
        text.len(),
        3,
        "void refuses every written bound, got {text:?}"
    );
    assert!(
        text[0].contains("'void'") && text[0].contains("'num'"),
        "the first row spells the argument and the bound, got {text:?}"
    );
}

/// The `dynamic` type argument is NOT a void argument: upstream's
/// `isSubtypeOf(dynamic, num)` is false too, so a written `dynamic`
/// refuses a concrete bound (oracle-confirmed: 3 rows).
#[test]
fn dynamic_type_argument_refuses_the_bounds() {
    let report = report_ops(
        "void f<T extends num>(T t) {}\n\
         void g<T extends Object>() {}\n\
         void m() {\n\
         \x20 f<dynamic>(3);\n\
         \x20 g<dynamic>();\n\
         }\n",
    );
    let text = texts(&report, "type_argument_not_matching_bounds");
    assert_eq!(
        text.len(),
        2,
        "dynamic refuses every written bound, got {text:?}"
    );
}

/// The bound display keeps the signature's own binder verbatim: a written
/// `T Function<T extends num>(T)` bound's `T` leaves are bound by the
/// `<T extends num>` group, never by the outer parameter the substitution
/// map spells. The oracle prints `'T Function<T extends num>(T)'`; a
/// substitution that leaks the outer `T` prints `'exp2 Function(exp2)'`.
#[test]
fn routed_bound_display_keeps_the_generic_signature_binder() {
    let report = report_ops(
        "typedef exp2 = void Function<T extends num>();\n\
         void func1<T extends T Function<T extends num>(T)>() {}\n\
         void main() {\n\
         \x20 func1<exp2>();\n\
         }\n",
    );
    let hits = texts(&report, "type_argument_not_matching_bounds");
    assert!(
        hits.iter()
            .any(|m| m.contains("'T Function<T extends num>(T)'")),
        "expected the binder-verbatim bound, got {hits:?} ({:?})",
        all_rows(&report)
    );
}

/// The trailing-call application depth (DND1): `self.member()()` invokes
/// the merged member's function-typed RESULT. The merged column for
/// `A<FutureOr<Object?> Function()> ⊔ B<dynamic Function()>` is
/// `Object? Function()`, so the trailing call reads `Object?` — the
/// `takesNum(x)` argument row names it.
///
/// The send-close fold computes the merged face for a chained send
/// (`consume_send` asks `operand_static_ty` → `callee_return_ty` — the
/// merged column — and applies the return once), stamping the applied
/// `Object?` back onto the callee row. `expression_row_ty`'s
/// intent-Invoke arm reads that stamp: the stored type is exactly
/// `callable_result_of(callee_return_ty)` — one application deeper than
/// the callee's face — so the arm answers the row's own `ty` instead of
/// re-deriving the un-applied column (#3990).
///
/// The `implements B<dynamic Function()>` clause argument parses faithful
/// now that `scan_type_args`'s keyword arms carry the `Function()` tail
/// (#3994): DND1's substitution environment instantiates, `self.member()`
/// resolves the merged `Object? Function()` column, and the stamped
/// (`Object?`) and unstamped faces both ride it.
#[test]
fn routed_trailing_call_applies_the_merged_function_column() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         class B<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         void takesNum(num x) {}\n\
         class DND1 extends A<FutureOr<Object?> Function()> implements B<dynamic Function()> {\n\
         \x20 void test() {\n\
         \x20   var self = this;\n\
         \x20   var x = self.member()();\n\
         \x20   takesNum(x);\n\
         \x20 }\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert!(
        atna.iter().any(|m| m.contains("'Object?'")),
        "the trailing call must read the merged `Object?`, got {atna:?} ({:?})",
        all_rows(&report)
    );
    assert!(
        !atna.iter().any(|m| m.contains("Function()")),
        "the argument must not still name the un-applied function column, got {atna:?}"
    );
}

/// The creation-argument capture reads the argument's OWN static type (M4,
/// routed from #3917): `const C.subtyped(a)` over `B<U> this.a` with an
/// `A<int>` argument. The failed inference's i2b fallback substitutes
/// `U := dynamic`, and the verdict must ask `A<int>` -> `B<dynamic>` —
/// `A` is `B`'s SUPERTYPE, and the relation never unfolds the target's
/// supertype to accept the source through it.
///
/// Routed: the argument face is still the const capture's downward
/// `B<dynamic>` — the `rebound_read_face`/`read_type_at` layering lands
/// (the sibling walks' shape) but neither answers for a const argument
/// read, and the local's declared slot is itself polluted: the const
/// evaluator's `bind_declared_type` writes the DOWNWARD face into
/// `var_types` (`const A<int> a` stores `B<dynamic>`), so no read face can
/// recover `A<int>` without the const evaluator keeping the written
/// annotation. Owner: the const capture's declared-slot write
/// (`bind_declared_type` under a const initializer).
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn routed_creation_argument_reads_the_arguments_own_face() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 const A();\n\
         }\n\
         class B<S> extends A<S> {\n\
         \x20 const B();\n\
         }\n\
         class C<U> {\n\
         \x20 final A<U> a;\n\
         \x20 const C.subtyped(B<U> this.a);\n\
         }\n\
         void main() {\n\
         \x20 const A<int> a = const B<int>();\n\
         \x20 const C.subtyped(a);\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert!(
        atna.iter()
            .any(|m| m.contains("'A<int>'") && m.contains("'B<dynamic>'")),
        "the i2b fallback's argument ask must report `A<int>` -> `B<dynamic>`, \
         got {atna:?} ({:?})",
        all_rows(&report)
    );
}

/// A matching creation argument stays silent — the outranking must not
/// fabricate rows on the accepted pair.
///
/// Routed: `const C<int>.subtyped(a)` over `const A<int> a` still asks
/// `A<dynamic>` -> `A<int>`. The named-selector creation's instantiation
/// does not reach the argument face: `creation_call_site_ty` returns `None`
/// over the written `<...>` group by design, and `creation_row_ty` has
/// published nothing for the const spelling, so `map` is empty and the
/// formal keeps its raw `U` spelling — which `instantiated_head_ty` filled
/// to `A<dynamic>`. Owner: invocation.rs's named-creation instantiation
/// read (`check_named_creation_arguments`'s `instantiation` chain).
#[test]
fn creation_argument_matching_face_stays_silent() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 const A();\n\
         }\n\
         class C<U> {\n\
         \x20 final A<U> a;\n\
         \x20 const C.subtyped(this.a);\n\
         }\n\
         void main() {\n\
         \x20 const A<int> a = const A<int>();\n\
         \x20 const C<int>.subtyped(a);\n\
         }\n",
    );
    assert!(
        texts(&report, "argument_type_not_assignable").is_empty(),
        "the matching pair stays silent, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

/// The callee-shape matrix cluster: a tear-off's declared return drives the
/// send result, so a chained call applies it once — the `f()()` shape on a
/// function-returning function reads the INNER return, never the function
/// type.
#[test]
fn chained_call_applies_the_declared_return_once() {
    let report = report_ops(
        "int Function() inner() => () => 1;\n\
         void takesNum(num x) {}\n\
         void m() {\n\
         \x20 takesNum(inner()());\n\
         }\n",
    );
    assert!(
        texts(&report, "argument_type_not_assignable").is_empty(),
        "`inner()()` reads `int`, which fits `num`; got {:?} ({:?})",
        texts(&report, "argument_type_not_assignable"),
        all_rows(&report)
    );
}

/// The callee-shape matrix cluster: the same chained shape still reports a
/// genuine mismatch — `String` against `num`.
#[test]
fn chained_call_reports_a_foreign_return() {
    let report = report_ops(
        "String Function() inner() => () => '';\n\
         void takesNum(num x) {}\n\
         void m() {\n\
         \x20 takesNum(inner()());\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert_eq!(
        atna.len(),
        1,
        "`inner()()` reads `String`, which refuses `num`; got {atna:?} ({:?})",
        all_rows(&report)
    );
    assert!(
        atna[0].contains("'String'"),
        "the row names the applied return, got {atna:?}"
    );
}

/// The callee-shape matrix cluster: the arity faces on a bare function head
/// (`f(1, 2)` against `void f(int)`) report `extra_positional_arguments`
/// at the surplus argument.
#[test]
fn bare_call_surplus_positional_reports() {
    let report = report_ops(
        "void f(int a) {}\n\
         void m() {\n\
         \x20 f(1, 2);\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"extra_positional_arguments"),
        "expected extra_positional_arguments, got {names:?} ({:?})",
        rows(&report)
    );
}

/// The callee-shape matrix cluster: a short bare call reports
/// `not_enough_positional_arguments` naming the missing count.
#[test]
fn bare_call_short_positional_reports() {
    let report = report_ops(
        "void f(int a, int b) {}\n\
         void m() {\n\
         \x20 f(1);\n\
         }\n",
    );
    let text = texts(&report, "not_enough_positional_arguments");
    assert_eq!(
        text.len(),
        1,
        "expected one not_enough row, got {text:?} ({:?})",
        all_rows(&report)
    );
}
