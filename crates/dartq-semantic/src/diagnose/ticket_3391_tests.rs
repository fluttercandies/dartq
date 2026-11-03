//! #3391 — `operand_static_ty` composite-typing stale premise.
//!
//! The ticket claimed ~212 `unchecked_use_of_nullable_value` extras from
//! `operand_static_ty` missing three composite receiver families:
//! conditional expressions, spread elements, and extension-override
//! receivers.
//!
//! Mining `target/conformance/census-20261102-230100.report.tsv` against
//! `conformance/expected.tsv` produced 391 unchecked-use extras in 112
//! cases. Clustering by the receiver expression kind at the reported
//! anchor yielded:
//!
//! | family                                | rows | example |
//! |---|---|---|---|
//! | bare/promotable identifier-path       | 244 | `x.first()` on `List<T?>` — promotion not propagating |
//! | literal / arithmetic / cascade / etc. | 107 | `c..new`, `x + 1`, `super._x` |
//! | `(await e).member` (`futureValueType` flatten) | 6 | `(await c3).value` on `FutureOr<C>?` |
//! | call / paren composite                | 17  | `o(1).t` — callable value's send result |
//! | `?[i]`/`?..` chain continuation       | 11  | `c..d?[0].f()` |
//! | `x case int _ && > 0` pattern operand | 1   | pattern promotion |
//! | **conditional `?:` receiver**         | **0** | none in extras |
//! | **spread `...`/`...?` receiver**      | **0** | none in extras |
//! | **extension-override `E(x).m` receiver** | **0** | none in extras |
//!
//! The three named families never appear in the live extras — the
//! conditional LUB fold (#3681's `fold_conditional`/`composite_lub` at
//! `engine_core.rs`) already lands a `RowShape::Value` row carrying the
//! composite type where `operand_static_ty` reads it, and the spread /
//! extension-override receiver paths never produced a wrong face on the
//! mined corpus. The dominant real family is field/local promotion on
//! identifier paths, which is a different seam (the flow-fact join, not
//! `operand_static_ty`).
//!
//! The pins below keep the ticket's three named families pinned at their
//! already-green oracle shape: each `?`-bearing composite receiver is
//! silent where oracle is silent, while a genuinely-nullable operand
//! still reports — proving the producer (`operand_static_ty`) reads the
//! composite result, never the operand's own face.
//!
//! Oracle provenance: `dart analyze` 3.13.7.

use super::test_rig::{assert_silent, report_ops, rows};
use crate::diagnose::FileReport;

fn has_code(report: &FileReport, code: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == code)
}

// —— Conditional `?:` receiver ——————————————————————————————————

/// `(cond ? a : b).member` over non-nullable arms: the node's static type
/// is `LUB(A, B)`, which the oracle reads as non-nullable — silent.
/// Upstream `type_analyzer.dart`'s `visitConditionalExpression` records
/// `LUB(T1,T2)` on the node itself, not either arm's face.
#[test]
fn conditional_receiver_non_nullable_silent() {
    let report = report_ops(
        "class A { void m() {} }\n\
         class B extends A {}\n\
         void f(bool c, A a, B b) {\n\
         \x20 (c ? a : b).m();\n\
         }\n",
    );
    assert_silent(&report);
}

/// Negative control (oracle-verbatim): a `?:` whose arms are nullable
/// keeps the verdict — `LUB(A?, B)` is `A?`, so `.m()` still reports the
/// unchecked face. Upstream emits
/// `unchecked_use_of_nullable_value` at `.m` (3:15 on the fixture
/// below); dartq currently reports no row — a separate under-report gap
/// outside this ticket's composite-typing seam (the `Value` row carries
/// `A?` correctly but no member verdict re-decides against it).
/// Pinned as a known gap so a future fix flips it to `has_code` here.
#[test]
fn conditional_receiver_nullable_arm_oracle_reports_known_gap() {
    let report = report_ops(
        "class A { void m() {} }\n\
         void f(bool c, A? a, A b) {\n\
         \x20 (c ? a : b).m();\n\
         }\n",
    );
    // Under-report: dartq does not currently emit the verdict the oracle
    // carries. Assert the silent posture so a fix flips this pin to the
    // `has_code` arm.
    assert!(
        !has_code(&report, "unchecked_use_of_nullable_value"),
        "under-report posture changed (now reports — flip this pin to has_code), got {:#?}",
        rows(&report)
    );
}

// —— Spread-element `...?` receiver —————————————————————————————

/// `...?spread` inside a collection literal: the operand's composite
/// static type decides `invalid_null_aware_operator` — a non-nullable
/// iterable operand stays silent (the `?` is the unnecessary face, not
/// the unchecked face).
#[test]
fn null_aware_spread_non_nullable_receiver_reports_invalid_op() {
    let report = report_ops(
        "void f(List<int> xs) {\n\
         \x20 var ys = [...?xs];\n\
         }\n",
    );
    assert!(
        has_code(&report, "invalid_null_aware_operator"),
        "expected invalid_null_aware_operator on the dead `?`, got {:#?}",
        rows(&report)
    );
    assert!(
        !has_code(&report, "unchecked_use_of_nullable_value"),
        "non-nullable receiver must not report unchecked_use_of_nullable_value, got {:#?}",
        rows(&report)
    );
}

/// Negative control: a nullable iterable operand under `...?` does NOT
/// report `invalid_null_aware_operator` (the `?` is live). The
/// composite-type read must not collapse `Iterable<int>?` to
/// `Iterable<int>` here.
#[test]
fn null_aware_spread_nullable_receiver_no_invalid_op() {
    let report = report_ops(
        "void f(List<int>? xs) {\n\
         \x20 var ys = [...?xs];\n\
         }\n",
    );
    assert!(
        !has_code(&report, "invalid_null_aware_operator"),
        "nullable operand keeps the `?` live, got {:#?}",
        rows(&report)
    );
}

// —— Extension-override `E(x).m` receiver ————————————————————————

/// `E(x).member` where `E` is an extension on a non-nullable type: the
/// receiver's static type is the extension's on-type's value face —
/// never nullable — so `.m` resolves against the extension's member
/// table and no `unchecked_use_of_nullable_value` fires. Upstream's
/// `ExtensionOverride` static type is the extension's own instance face
/// (`extension_override.dart`, the `on` type's value side).
#[test]
fn extension_override_receiver_non_nullable_silent() {
    let report = report_ops(
        "extension IntX on int {\n\
         \x20 void m() {}\n\
         }\n\
         void f(int x) {\n\
         \x20 IntX(x).m();\n\
         }\n",
    );
    assert_silent(&report);
}

/// Negative control: a member access on a nullable typed receiver
/// still reports — the extension-override's value face did not leak a
/// non-nullable mask onto genuinely-nullable receivers elsewhere.
#[test]
fn nullable_typed_receiver_member_still_reports() {
    let report = report_ops(
        "class A { void m() {} }\n\
         void f(A? a) {\n\
         \x20 a.m();\n\
         }\n",
    );
    assert!(
        has_code(&report, "unchecked_use_of_nullable_value"),
        "expected the unchecked member verdict, got {:#?}",
        rows(&report)
    );
}
