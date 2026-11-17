//! Ticket #4343 pins: the pre-generic-metadata CALL face of the
//! `could_not_infer` cluster — #4337's edge-3 handoff. Every expectation
//! below is 3.13.7-oracle-probed (`dart analyze --format=machine`, probe
//! fixtures rebuilt this lane under /tmp — volatile by design, the rows
//! quoted verbatim here).
//!
//! * **The face**: a call whose inference solves a parameter from a
//!   generic function tear-off ARGUMENT (`g(f)` over `void g<T>(T x)`)
//!   lands the parameter on a type that still carries its own type
//!   parameters — illegal as a type argument while the diagnosing
//!   library's language version predates `generic-metadata` (enabledIn
//!   2.14). Upstream's independent arm reports it at the CALLEE NAME
//!   (`errorEntity` is `node.function` — the identifier alone, never
//!   the whole send): `g` for a bare callee, `mk` for a
//!   return-consuming one, the SELECTOR `m` for `C().m(f)`, the local
//!   `h` for a nested function callee. (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/element/generic_inferrer.dart's
//!   `_reportErrors` generic-metadata gate, fed by
//!   invocation_inference_helper.dart:74-75 — the engine's settled
//!   `generic_metadata_enabled` flag, never a comment re-parse.)
//! * **One row per solved parameter** (`g<T, U>(f, f)` reports T then U
//!   at the same span — probed order is parameter order).
//! * **The candidate is the JOINED full type**: two same-shape tear-off
//!   arguments fold through the official function LUB — FIRST
//!   contributor's parameter names win (`g2(f, h2)` reports
//!   `void Function<X>()`, `g2(h2, f)` reports `void Function<Y>()`,
//!   probed both orders). A shape the join rejects stays silent
//!   (`g2(f<X>(X), h2<Y>())` — probed clean), and so does any
//!   non-tear-off competitor on the same parameter (`g2(f, 1)` and the
//!   generic-plus-erased pair `g2(f<X>(), h3)` — probed clean: upstream
//!   counts-disagree LUB answers the bare `Function` interface).
//! * **Bounded spellings**: the candidate text carries the tear-off's
//!   own bounds (`void Function<X extends N>()` has type parameters
//!   `[X extends N]` over a declared `class N` — probed; the bare rig
//!   has no dart:core), and a BOUNDED CALL parameter reports the
//!   Tried-to-infer row and this gate's row together, Tried first
//!   (probed unsorted machine order). The Tried row's own detail text
//!   belongs to the pre-existing constraint-failure face and keeps its
//!   current spelling; this file pins the gate's row exactly.
//! * **Silence faces**: a NON-generic tear-off (a legal candidate) and
//!   the modern default (the feature on) stay clean — probed. Explicit
//!   type arguments never build an inferrer (probed clean on the
//!   oracle); the engine's ATNA walk still rejects the tear-off against
//!   the instantiated formal there — a pre-existing
//!   instantiate-to-bounds divergence in a different seam, so that pin
//!   holds the gate's posture only (no `could_not_infer` row).
//! * **Under-report seams (stopped at)**: a LOCAL function callee
//!   reports at its own name on the oracle (probed) — the engine's
//!   callee-kind coverage does not yet feed a local declaration's
//!   parameters into the argument-inference seam; the pin is deferred.
//!   A BOUNDED CALL parameter reports the Tried-to-infer row and this
//!   gate's row together on the oracle (probed) — the engine's
//!   constraint-failure seam rewinds the tear-off round at the gather's
//!   bound gate (the settled #3794-era posture) and the ATNA walk
//!   answers instead, so both rows are missing; that pin is deferred
//!   with it.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::report_ops_bare;
use super::*;

/// `(code, anchored text, message)` rows in source order.
fn rows(report: &FileReport, full: &str) -> Vec<(&'static str, String, String)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
                d.message.to_string(),
            )
        })
        .collect()
}

/// The gate's row text for the unadorned `void f<X>()` candidate — the
/// most common probe shape, quoted once.
const PLAIN_DETAIL: &str = "Couldn't infer type parameter 'T'. Inferred candidate type \
     void Function<X>() has type parameters [X], but a function with \
     type parameters cannot be used as a type argument.";

/// `g(f)` over `void g<T>(T x)` under `// @dart=2.12` — the corpus row
/// shape (#4337's handoff probe `g` span len 1): one row at the callee
/// NAME, oracle-word-for-word.
#[test]
fn bare_name_reports_at_callee() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<T>(T x) {}\n\
                   main() {\n\
                   \x20 g(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![("could_not_infer", "g".to_string(), PLAIN_DETAIL.to_string())],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `// @dart=2.13` still predates the 2.14 enable — same row, same span
/// (the gate is the version, not the comment's presence; #4337's
/// envelope flip, probed again on the call face this lane).
#[test]
fn dart_2_13_reports() {
    let fixture = "// @dart=2.13\n\
                   void f<X>() {}\n\
                   void g<T>(T x) {}\n\
                   main() {\n\
                   \x20 g(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![("could_not_infer", "g".to_string(), PLAIN_DETAIL.to_string())],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// The modern default keeps `generic-metadata` on — the candidate is a
/// legal type argument and the file stays silent (probed: a `^3.0.0`
/// package default is clean; only a pre-2.14 override or package
/// default reports).
#[test]
fn modern_default_stays_silent() {
    let fixture = "void f<X>() {}\n\
                   void g<T>(T x) {}\n\
                   main() {\n\
                   \x20 g(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A callee whose parameter feeds a generic RETURN (`T mk<T>(T x)`)
/// anchors at the longer callee name — the `mk` probe (span len 2); the
/// candidate stays the ARGUMENT's type, never the return's.
#[test]
fn returned_generic_reports_at_callee() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   T mk<T>(T x) => x;\n\
                   main() {\n\
                   \x20 mk(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "mk".to_string(),
            PLAIN_DETAIL.to_string()
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Two solved parameters, two rows at the same span, PARAMETER order
/// (probed unsorted: T then U).
#[test]
fn two_parameters_report_two_rows_in_order() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<T, U>(T x, U y) {}\n\
                   main() {\n\
                   \x20 g(f, f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    let detail = |parameter: &str| {
        format!(
            "Couldn't infer type parameter '{parameter}'. Inferred candidate type \
             void Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
        )
    };
    assert_eq!(
        rows(&report, fixture),
        vec![
            ("could_not_infer", "g".to_string(), detail("T")),
            ("could_not_infer", "g".to_string(), detail("U")),
        ],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// The method-call face anchors at the SELECTOR — `C().m(f)` reports at
/// `m` (probed span `m` len 1, never the receiver chain).
#[test]
fn method_callee_reports_at_selector() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   class C {\n\
                   \x20 void m<T>(T x) {}\n\
                   }\n\
                   main() {\n\
                   \x20 C().m(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![("could_not_infer", "m".to_string(), PLAIN_DETAIL.to_string())],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A LOCAL function callee (`void h<T>(T x) {}` inside `main`, then
/// `h(f)`) reports at `h` on the oracle (probed this lane) — the engine's
/// callee-kind coverage does not yet carry a local declaration's
/// parameters into the argument-inference seam, so the pin is deferred:
/// an under-report seam, not a posture divergence (everything the gate
/// needs exists once the local callee feeds the seam).

/// A named formal carries the same face (`g(x: f)` over
/// `void g<T>({T? x})` — probed).
#[test]
fn named_formal_reports() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<T>({T? x}) {}\n\
                   main() {\n\
                   \x20 g(x: f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![("could_not_infer", "g".to_string(), PLAIN_DETAIL.to_string())],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Explicit type arguments never build an inferrer — the oracle's send
/// stays fully clean (probed; the non-generic function type is a legal
/// type argument pre-2.14). The engine's assignability face still
/// rejects the generic tear-off against the instantiated formal there
/// (a pre-existing instantiate-to-bounds divergence in the ATNA walk,
/// a different seam — not this gate): this pin holds the gate's own
/// posture, no `could_not_infer` row.
#[test]
fn explicit_type_arguments_stay_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<T>(T x) {}\n\
                   main() {\n\
                   \x20 g<void Function()>(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        rows(&report, fixture)
            .iter()
            .all(|(code, ..)| *code != "could_not_infer"),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A NON-generic tear-off is a legal candidate — the face keys on the
/// candidate's own type parameters, never the tear-off shape (probed:
/// `g(q)` over a plain `void q()` stays clean).
#[test]
fn non_generic_tear_off_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void q() {}\n\
                   void g<T>(T x) {}\n\
                   main() {\n\
                   \x20 g(q);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Only the parameter the tear-off feeds reports — the non-generic
/// formal's argument solves nothing (`g(f, null)` over
/// `void g<T>(T x, void y)` — probed one row; the bare rig has no
/// dart:core, so the filler formal is `void`).
#[test]
fn mixed_formals_report_only_the_tear_off_parameter() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<T>(T x, void y) {}\n\
                   main() {\n\
                   \x20 g(f, null);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![("could_not_infer", "g".to_string(), PLAIN_DETAIL.to_string())],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A non-tear-off competitor on the SAME parameter keeps the face
/// silent — the oracle's solution is the LUB of the lowers, which is
/// not the tear-off (`g2(f, 1)` over `void g2<T>(T x, T y)` — probed
/// clean).
#[test]
fn tear_off_and_int_same_parameter_stay_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g2<T>(T x, T y) {}\n\
                   main() {\n\
                   \x20 g2(f, 1);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Two same-shape tear-offs fold through the function LUB — the FIRST
/// contributor's parameter names win (`g2(f, h2)` reports `X` —
/// probed).
#[test]
fn same_shape_join_keeps_first_names() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void h2<Y>() {}\n\
                   void g2<T>(T x, T y) {}\n\
                   main() {\n\
                   \x20 g2(f, h2);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "g2".to_string(),
            PLAIN_DETAIL.to_string()
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// The reversed order reports the SECOND declaration's name — first
/// CONTRIBUTOR, not first declaration (`g2(h2, f)` reports `Y` —
/// probed).
#[test]
fn reversed_join_keeps_first_contributor() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void h2<Y>() {}\n\
                   void g2<T>(T x, T y) {}\n\
                   main() {\n\
                   \x20 g2(h2, f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "g2".to_string(),
            "Couldn't infer type parameter 'T'. Inferred candidate type \
             void Function<Y>() has type parameters [Y], but a function with \
             type parameters cannot be used as a type argument."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A join the shape gate rejects stays silent — different formal counts
/// (`f<X>(X)` vs `h2<Y>()` on one parameter — probed clean; the
/// official LUB answers the bare `Function` interface, on which the
/// gate cannot fire).
#[test]
fn join_reject_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>(X x) {}\n\
                   void h2<Y>() {}\n\
                   void g2<T>(T x, T y) {}\n\
                   main() {\n\
                   \x20 g2(f, h2);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// The candidate text carries the tear-off's own BOUNDS — both in the
/// type (`void Function<X extends N>()`) and in the parameter list
/// (`[X extends N]` — probed; the element face's bracket list is
/// name-only, this call face spells the element display string; the
/// bare rig has no dart:core, so the bound is a declared class).
#[test]
fn bounded_tear_off_parameter_spells_bound() {
    let fixture = "// @dart=2.12\n\
                   class N {}\n\
                   void f<X extends N>() {}\n\
                   void g<T>(T x) {}\n\
                   main() {\n\
                   \x20 g(f);\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "g".to_string(),
            "Couldn't infer type parameter 'T'. Inferred candidate type \
             void Function<X extends N>() has type parameters [X extends N], \
             but a function with type parameters cannot be used as a type argument."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}
