//! #3797 — the context-shaped receiver faces of the
//! `type_argument_not_matching_bounds` prune.
//!
//! #3744's prune re-decides every emitted bounds row against the
//! pass-2 receiver re-derivation; this ticket widens the faces the
//! re-verdict retries. The frozen push-time face cannot spell two
//! receiver shapes the oracle stamps differently:
//!
//! * **await** — `(await x)..expectStaticType<Exactly<…>>()`: the oracle
//!   checks against `flatten(operand)`; the frozen face is the operand's
//!   own type, so a user Future-like (`SubFuture implements Future<int>`,
//!   a bound-chained type parameter) fires a fabricated row checking
//!   `Exactly<SubFuture>` where the oracle checks `Exactly<int>` and
//!   stays silent (the `derive_future_type`/`await` census family).
//! * **context-pushed composite** — `contextB1((c ? a : b)..m<T>())`,
//!   `contextX((x ??= y)..m<T>())`: inference-update-3 types the
//!   composite `T` when `UP <: S` else `S = greatestClosure(K)`, `K` the
//!   composite's downward context. The frozen face is the context-free
//!   `UP`, so the send checks `Exactly<A>` where the oracle checks
//!   `Exactly<B1<Object?>>` and stays silent (the
//!   `conditional_expression`/`if_null_assignment` census families).
//!
//! Every candidate re-decides the WHOLE instantiated-bound verdict — a
//! wrong guess keeps the row, so only the oracle's own `T <: S == false`
//! gate (proven, not inferred from arm subtypings) releases the `S` face.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`
//! over a package fixture — each pin names the verdict it encodes.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// The co19 `StaticType`/`Exactly<T>` extension the census cases use.
const EXPECT_HELPER: &str = "typedef Exactly<T> = T Function(T);\n\
     extension StaticType<T> on T {\n\
     \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
     }\n";

/// The `type_argument_not_matching_bounds` rows of `report`.
fn bounds_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The context-channel fixture family the census cases ride: `A`/`B1`/
/// `B2`/`C1`/`C2` from `type_inference/local_type_inference` — `B1<T>`
/// is the `contextB1` formal's schema, `C1<T>`/`C2<T>` the two arms
/// whose UP is `A` (both subtype `B1<Object?>` without either subtyping
/// the other).
const CONDITIONAL_FIXTURE: &str = "class A {}\n\
     class B1<T> {}\n\
     class B2<T> {}\n\
     class C1<T> implements B1<T> {}\n\
     class C2<T> implements B1<T> {}\n\
     void contextB1(B1<Object?> x) {}\n";

/// `(c ? C1<int>() : C2<double>())` under `contextB1` — the oracle's
/// `S` arm: `T = UP(C1<int>, C2<double>)` (analyzer's named-interface
/// UP = `A`), `T <: S` fails (`A` is not `<: B1<Object?>`), the
/// composite stamps `S = greatestClosure(B1<Object?>) = B1<Object?>`,
/// and `Exactly<B1<Object?>>` conforms — oracle silent.
#[test]
fn conditional_receiver_under_context_stays_silent() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}{CONDITIONAL_FIXTURE}void f(bool b) {{\n\
         \x20 contextB1(\n\
         \x20   (b ? C1<int>() : C2<double>())\n\
         \x20     ..expectStaticType<Exactly<B1<Object?>>>(),\n\
         \x20 );\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// The same composite with NO downward context keeps the `T` branch:
/// `var x = (c ? C1<int>() : C2<double>())` types `A`, so
/// `Exactly<B1<Object?>>` does NOT conform — the oracle fires.
/// Currently 0 rows observed: the emit-side face over a `?:` composite
/// receiver produces no keepable row here (the missing-side direction —
/// a row the oracle reports but pass 1/re-derivation never preserves).
/// Pinned `#[ignore]` as the documented residual; the keep-direction
/// control that DOES hold is `plain_receiver_keeps_the_frozen_verdict`.
#[test]
#[ignore = "missing-side residual: 0 rows over a no-context conditional receiver"]
fn conditional_receiver_without_context_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}{CONDITIONAL_FIXTURE}void f(bool b) {{\n\
         \x20 var x = (b ? C1<int>() : C2<double>())\n\
         \x20   ..expectStaticType<Exactly<B1<Object?>>>();\n\
         \x20 x;\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// A WRONG expectation under the same context keeps its row:
/// `T := B1<Object?>` makes the bound `Exactly<B1<Object?>>`, and
/// `Exactly<B1<int>>` does not conform — the oracle reports one row.
/// Same missing-side residual as the no-context twin: 0 rows observed
/// over this receiver shape, so the keep-direction pin parks.
#[test]
#[ignore = "missing-side residual: 0 rows over a conditional receiver, keep-direction unpins"]
fn conditional_receiver_wrong_expectation_still_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}{CONDITIONAL_FIXTURE}void f(bool b) {{\n\
         \x20 contextB1(\n\
         \x20   (b ? C1<int>() : C2<double>())\n\
         \x20     ..expectStaticType<Exactly<B1<int>>>(),\n\
         \x20 );\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// `(await x)..expectStaticType<Exactly<int>>()` where `x`'s face is a
/// type parameter bounded by a future: upstream `flatten` derives `X`'s
/// future type from its written bound (`X extends Future<int>` →
/// `int`); the frozen push-time face stayed `X` and fired
/// `Exactly<X>`. Oracle silent. (The user-Future-like arm — a class
/// `implements Future<E>` — cannot spell in this rig: the stub
/// `Future` is names-only, `implements` on it errors
/// `implements_non_class`; the bound-walk arm exercises the same
/// prune re-verdict.)
#[test]
fn await_future_bound_parameter_flattens_to_the_bound_element() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f<X extends Future<int>>(X x) async {{\n\
         \x20 (await x)..expectStaticType<Exactly<int>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// A WRONG expectation over the flattened face still reports:
/// `flatten(X) = int` makes the bound `Exactly<int>`, and
/// `Exactly<num>` does not conform — the oracle fires.
#[test]
fn await_flattened_wrong_expectation_still_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f<X extends Future<int>>(X x) async {{\n\
         \x20 (await x)..expectStaticType<Exactly<num>>();\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// `??=` under a context: the if-null family's THIRD arm keeps `T` when
/// the coerced right side does not fit the context (inference-update-3:
/// `T` if `T <: S`, else `S` if `T2' <: S`, else `T`). `x: A?`, `e2 = A()`
/// gives `T = A`, `S = B1<Object?>`; `T <! S` AND `T2' <! S` → the
/// composite stays `T`, so `Exactly<B1<Object?>>` does not conform.
/// Oracle 3.13.7: reports `'Exactly<B1<Object?>>' doesn't conform to the
/// bound 'Exactly<A>'` — one row (the pin previously asserted silence,
/// which the S-release-on-`T <! S`-alone produced).
#[test]
fn if_null_assign_receiver_under_context_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}{CONDITIONAL_FIXTURE}void f(A? x) {{\n\
         \x20 contextB1(\n\
         \x20   (x ??= A())\n\
         \x20     ..expectStaticType<Exactly<B1<Object?>>>(),\n\
         \x20 );\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// Plain `??` under the same context: oracle 3.13.7 REPORTS one row
/// (`'Exactly<B1<Object?>>' doesn't conform to the bound 'Exactly<Object>'`)
/// — `J' = NonNull(T1) = A`, `T2 = B1<Object?>` does not fit `J'`, `T2' <! S`,
/// so the composite stays `T`. dartq is silent here because pass 1 never
/// emitted: `receiver_face_ungrounded` (invocation.rs) abstains over a
/// `??` whose join is not staged. That is the MISSING-side seam, not a
/// prune question — parked `#[ignore]` until the emit learns this face.
#[test]
#[ignore = "missing-side residual: the emit abstains over an unstaged if-null join"]
fn if_null_receiver_under_context_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}{CONDITIONAL_FIXTURE}void f(A? x) {{\n\
         \x20 contextB1(\n\
         \x20   (x ?? B1<Object?>())\n\
         \x20     ..expectStaticType<Exactly<B1<Object?>>>(),\n\
         \x20 );\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// A non-composite, non-await control: a bare typed receiver carries
/// the frozen face — `x: A` sending `Exactly<B1<Object?>>` fires (the
/// emit's own verdict, nothing to reshape). Keeps exactly one row.
#[test]
fn plain_receiver_keeps_the_frozen_verdict() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}{CONDITIONAL_FIXTURE}void f(A x) {{\n\
         \x20 x.expectStaticType<Exactly<B1<Object?>>>();\n\
         }}\n"
    ));
    assert!(
        code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "{:#?}",
        rows(&report)
    );
}
