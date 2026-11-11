//! Ticket #3861 — the written-`dynamic` creation face of the
//! `written_ty_invalid` gate (the `downwardsInferenceOnInstanceCreations`
//! cluster's 8 `invalid_assignment` rows), landed DYNAMIC-ONLY, plus the
//! instantiated-interface refinement the cluster's subclass rows need.
//!
//! Root cause, two layers:
//!
//! 1. The gate read every keyword spelling as unresolvable —
//!    `named_type_is_type` finds no declaration for `dynamic` (upstream's
//!    `dynamic` is `DynamicTypeImpl.instance`, a type with no element), so
//!    the whole written spelling poisoned through the recursion and the
//!    enclosing initializer ask went silent. Un-silencing the `dynamic`
//!    spelling alone answers the DIRECT instantiation rows
//!    (`A<int, dynamic>` / `A<dynamic, dynamic>`), the slot verdict
//!    normalizing in `Engine::slot_subtype`'s `written_dynamic_face`.
//! 2. The subclass rows (`B<String, dynamic>`, `C<dynamic>`,
//!    `D<num, dynamic>` through the declared `extends A<T, S>` chains) need
//!    more than the gate: the distinct-heads leg decided on the NAME reach
//!    alone, and the interface rule upstream decides over the INSTANTIATED
//!    supertype view (subtype.dart's interface block — `B<String, dynamic>`
//!    grounds `A<dynamic, String>`, whose `dynamic <: int` leg refuses).
//!    `Engine::instantiated_supertype_args_mismatch` builds the view
//!    through the strict snapshot and re-asks the ordinary covariance
//!    matrix; a view that never builds keeps the reach verdict.
//!
//! The oracle rows below are measured on oracle SDK 3.13.7
//! (`dart analyze --format=machine`) over exactly these fixtures; lengths
//! are the whole creation expressions.
//!
//! The written `void` spelling keeps the gate (`written_ty_invalid` still
//! poisons a `void` head), and the void-result emitter stays out of the
//! written-slot path — that channel is what traded the cluster for ten
//! thousand `use_of_void_result` extras in the reverted attempt. The final
//! pin freezes the measured void posture: the emitter never fires, the
//! single-argument `W<void>` shape already matches the oracle end to end,
//! and the two-argument `<int, void>` shape decides at the oracle's span
//! with a degraded source spelling (a capture face recorded as residual,
//! owned by the written-group capture, not this pin's contract).

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};

fn invalid_assignment_rows(report: &super::FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` inside the fixture source, shifted by the rig's
/// core prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The four class shapes the cluster's cases declare (`A` direct, `B`
/// through the swapped supertype `A<T, S>`, `C` through the `B<S, S>`
/// chain, `D` through `B<T, int>`).
const FIXTURE_HEAD: &str = r#"
class A<S, T> {
  S x;
  T y;
  A(this.x, this.y);
  A.named(this.x, this.y);
}
class B<S, T> extends A<T, S> {
  B(S y, T x) : super(x, y);
  B.named(S y, T x) : super.named(x, y);
}
class C<S> extends B<S, S> {
  C(S a) : super(a, a);
  C.named(S a) : super.named(a, a);
}
class D<S, T> extends B<T, int> {
  D(T a) : super(a, 3);
  D.named(T a) : super.named(a, 3);
}
"#;

/// The `A<int, dynamic>` / `A<dynamic, dynamic>` rows (case lines 39-40):
/// the direct instantiation's invariance leg asks `dynamic <: String` and
/// refuses, once per creation — the whole-creation span, oracle text.
#[test]
fn written_dynamic_direct_creation_reports_invalid_assignment() {
    let source = format!(
        "{FIXTURE_HEAD}\nvoid main() {{\n  A<int, String> a4 = new A<int, dynamic>(3, \"hello\");\n  A<int, String> a5 = new A<dynamic, dynamic>.named(3, \"hello\");\n}}\n"
    );
    let report = report_ops(&source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [
            (
                at(&source, "new A<int, dynamic>(3, \"hello\")"),
                31,
                "A value of type 'A<int, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                    .to_string()
            ),
            (
                at(&source, "new A<dynamic, dynamic>.named(3, \"hello\")"),
                41,
                "A value of type 'A<dynamic, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                    .to_string()
            ),
        ],
        "{:?}",
        rows(&report)
    );
}

/// The `B<String, dynamic>` / `B<dynamic, dynamic>` rows (case lines
/// 51-52): the supertype view maps `B<S, T>` to `A<T, S>`, so the refusal
/// rides the mapped arguments (`dynamic` against `int`) while the message
/// still spells the written creation type, not the supertype.
#[test]
fn written_dynamic_subclass_creation_reports_invalid_assignment() {
    let source = format!(
        "{FIXTURE_HEAD}\nvoid main() {{\n  A<int, String> a4 = new B<String, dynamic>(\"hello\", 3);\n  A<int, String> a5 = new B<dynamic, dynamic>.named(\"hello\", 3);\n}}\n"
    );
    let report = report_ops(&source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [
            (
                at(&source, "new B<String, dynamic>(\"hello\", 3)"),
                34,
                "A value of type 'B<String, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                    .to_string()
            ),
            (
                at(&source, "new B<dynamic, dynamic>.named(\"hello\", 3)"),
                41,
                "A value of type 'B<dynamic, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                    .to_string()
            ),
        ],
        "{:?}",
        rows(&report)
    );
}

/// The `C<dynamic>` rows (case lines 63-64): the two-step chain
/// `C<S> → B<S, S> → A<T, S>` grounds every parameter on the one written
/// `dynamic` argument; both constructor forms refuse against
/// `A<int, int>`.
#[test]
fn written_dynamic_chained_creation_reports_invalid_assignment() {
    let source = format!(
        "{FIXTURE_HEAD}\nvoid main() {{\n  A<int, int> a4 = new C<dynamic>(3);\n  A<int, int> a5 = new C<dynamic>.named(3);\n}}\n"
    );
    let report = report_ops(&source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [
            (
                at(&source, "new C<dynamic>(3)"),
                17,
                "A value of type 'C<dynamic>' can't be assigned to a variable of type 'A<int, int>'."
                    .to_string()
            ),
            (
                at(&source, "new C<dynamic>.named(3)"),
                23,
                "A value of type 'C<dynamic>' can't be assigned to a variable of type 'A<int, int>'."
                    .to_string()
            ),
        ],
        "{:?}",
        rows(&report)
    );
}

/// The `D<num, dynamic>` rows (case lines 75-76): only the second argument
/// is the keyword spelling (`D<S, T> → B<T, int> → A<int, dynamic>`), so
/// the refusal rides the one dynamic slot against `String` while the
/// resolvable `num` head stays untouched.
#[test]
fn written_dynamic_mixed_args_creation_reports_invalid_assignment() {
    let source = format!(
        "{FIXTURE_HEAD}\nvoid main() {{\n  A<int, String> a4 = new D<num, dynamic>(\"hello\");\n  A<int, String> a5 = new D<dynamic, dynamic>.named(\"hello\");\n}}\n"
    );
    let report = report_ops(&source);
    assert_eq!(
        invalid_assignment_rows(&report),
        [
            (
                at(&source, "new D<num, dynamic>(\"hello\")"),
                28,
                "A value of type 'D<num, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                    .to_string()
            ),
            (
                at(&source, "new D<dynamic, dynamic>.named(\"hello\")"),
                38,
                "A value of type 'D<dynamic, dynamic>' can't be assigned to a variable of type 'A<int, String>'."
                    .to_string()
            ),
        ],
        "{:?}",
        rows(&report)
    );
}

/// Control: the resolvable spellings never trip the gate — the same
/// creation with written `int`/`String` arguments assigns cleanly (the
/// case's `a2`-style line), so the un-silencing is the keyword face alone.
#[test]
fn resolvable_written_args_creation_stays_silent() {
    let source = format!(
        "{FIXTURE_HEAD}\nvoid main() {{\n  A<int, String> c0 = new A<int, String>(3, \"hello\");\n}}\n"
    );
    let report = report_ops(&source);
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "resolvable written args must not report, got {:?}",
        rows(&report)
    );
}

/// The void-side A/B control (the #3861 reverted-attempt lesson, frozen as
/// pins): the written `void` spelling must NEVER reach the void-result
/// emitter — that channel is what traded the cluster for ten thousand
/// `use_of_void_result` extras. The oracle verdicts on these cells
/// (measured on oracle SDK 3.13.7): both creations report
/// `invalid_assignment` and nothing else — `A<int, void>` → `A<int,
/// String>` at the whole-creation span (len 28), `W<void>` → `A<int, int>`
/// (len 14). The single-argument shape already matches the oracle
/// verdict-for-verdict; the two-argument `<int, void>` shape reports at
/// the oracle's span with a wrong source spelling (`A<dynamic, dynamic>` —
/// the `void` member degrades the written group to the raw fill), a
/// pre-existing capture face recorded as residual, not this pin's contract.
#[test]
fn written_void_creation_never_fires_the_void_result_emitter() {
    let source = r#"
class A<S, T> {
  S x;
  T y;
  A(this.x, this.y);
}
class W<S> {
  W(S a);
}
void main() {
  A<int, String> v1 = new A<int, void>(3, "hello");
  A<int, int> v2 = new W<void>(3);
}
"#;
    let report = report_ops(source);
    let names = code_names(&report);
    assert!(
        !names.contains(&"use_of_void_result"),
        "the void spelling must not reach the void-result emitter, got {:?}",
        rows(&report)
    );
    // The `W<void>` shape is oracle-exact end to end (verdict, span, text).
    assert_eq!(
        invalid_assignment_rows(&report)
            .iter()
            .find(|(_, len, _)| *len == 14)
            .map(|(offset, len, message)| (*offset, *len, message.clone())),
        Some((
            at(source, "new W<void>(3)"),
            14,
            "A value of type 'W<void>' can't be assigned to a variable of type 'A<int, int>'."
                .to_string()
        )),
        "{:?}",
        rows(&report)
    );
    // The `<int, void>` shape decides at the oracle's span (len 28) — the
    // verdict posture holds even where the source spelling degrades.
    assert!(
        invalid_assignment_rows(&report)
            .iter()
            .any(
                |(offset, len, _)| *offset == at(source, "new A<int, void>(3, \"hello\")")
                    && *len == 28
            ),
        "the <int, void> creation must decide at the oracle's span, got {:?}",
        rows(&report)
    );
}
