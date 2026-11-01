//! The #3304 pins — the switch-closure half of
//! `body_might_complete_normally`.
//!
//! The oracle's `checkForBodyMayCompleteNormally` reads
//! `isReachable` of the block's end (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/type_analyzer.dart:647-652), so a
//! function whose tail is a switch is silent exactly when the switch's
//! post state is unreachable. That question is the scrutinee type's
//! `isAlwaysExhaustive` (type_system.dart:812-888) — the resolved face
//! of it lives in `resolved_ops.rs`, whose declaration arm
//! (`always_exhaustive_decl`) is pinned here.
//!
//! The face these pins drive is the RESOLVED one: with the adapter
//! mounted (`test_rig` mounts it the way the engine does), the flow's
//! `switch_statement_end` asks `TypeOps::is_always_exhaustive`, so a
//! declaration the resolved walk cannot classify keeps the tail live and
//! reports the completion row.
//!
//! The extension-type arm is the one this ticket closed: upstream
//! recurses into `type.extensionTypeErasure`
//! (`ExtensionTypeElement` arm, type_system.dart:862-868), so
//! `extension type W(E _)` over an enum `E` closes a switch exactly like
//! `E` itself. Before the arm existed the resolved face answered `false`
//! for every extension type — the corpus `LanguageFeatures/Extension-types`
//! family (`exhaustiveness_cast_*`) reported the tail completion the
//! oracle does not.
//!
//! Not pinned here, and deliberately: `unnecessary_type_check`'s verdict
//! and emission (`stage_type_check`/`resolved_subtype` in
//! `diagnose/engine_core.rs`) and this code's own return-type matrix
//! (`body_may_complete_report` in `diagnose/scopes_bodies.rs`) live
//! outside this module's files; the cluster table for both codes ships
//! with the ticket.

use crate::diagnose::test_rig::report_ops;

/// Whether the fixture reports `body_might_complete_normally` at all —
/// the ERROR twin is the one every non-nullable `int` fixture asks for.
fn reports_completion(source: &str) -> bool {
    report_ops(source).diagnostics.iter().any(|d| {
        d.code.name == "body_might_complete_normally"
            || d.code.name == "body_might_complete_normally_nullable"
    })
}

/// The ticket's dominant shape: a switch over an extension type whose
/// representation is an enum closes the tail
/// (`isAlwaysExhaustive(extensionTypeErasure)`, type_system.dart:862-868).
/// Both constants are covered, so no value of `W` survives the switch.
#[test]
fn extension_type_over_enum_switch_closes_the_tail() {
    let source = "\
enum E { a, b }
extension type W(E e) implements E {}
int f(W w) {
  switch (w) {
    case E.a: return 1;
    case E.b: return 2;
  }
}
";
    assert!(
        !reports_completion(source),
        "an extension type over an enum is always-exhaustive: {:?}",
        report_ops(source).diagnostics
    );
}

/// The chained representation (`W2(W1 _)` over `W1(E _)`) erases one hop
/// at a time — the walk recurses through the registry, not just the
/// written head.
#[test]
fn chained_extension_type_over_enum_switch_closes_the_tail() {
    let source = "\
enum E { a, b }
extension type W1(E e) implements E {}
extension type W2(W1 w) implements W1 {}
int f(W2 w) {
  switch (w) {
    case E.a: return 1;
    case E.b: return 2;
  }
}
";
    assert!(
        !reports_completion(source),
        "a chained extension type erases to its enum: {:?}",
        report_ops(source).diagnostics
    );
}

/// Silent-negative control: an extension type over an open representation
/// (`List<int>`) is NOT always-exhaustive, so a partial switch over it
/// still reports the completion — the erasure arm must not answer `true`
/// for every extension type.
#[test]
fn extension_type_over_open_representation_keeps_the_completion() {
    let source = "\
extension type W(List<int> l) {}
int f(W w) {
  switch (w) {
    case [1]: return 1;
  }
}
";
    assert!(
        reports_completion(source),
        "an open representation keeps the tail live: {:?}",
        report_ops(source).diagnostics
    );
}

/// Silent-negative control: a representation head that names no
/// declaration (`extension type W(Missing _)`) erases nowhere — the
/// under-report arm, never a false closure.
#[test]
fn extension_type_over_unresolved_representation_keeps_the_completion() {
    let source = "\
extension type W(Missing m) {}
int f(W w) {
  switch (w) {
    case 1: return 1;
  }
}
";
    assert!(
        reports_completion(source),
        "an unresolved representation keeps the tail live: {:?}",
        report_ops(source).diagnostics
    );
}

/// Regression control beside the new arm: the plain enum closure the
/// existing walk already answered stays answered (the enum's own constant
/// face, type_system.dart:825-831).
#[test]
fn plain_enum_switch_still_closes_the_tail() {
    let source = "\
enum E { a, b }
int f(E e) {
  switch (e) {
    case E.a: return 1;
    case E.b: return 2;
  }
}
";
    assert!(
        !reports_completion(source),
        "an enum splits completely: {:?}",
        report_ops(source).diagnostics
    );
}

/// Regression control beside the new arm: a partially covered switch over
/// an always-exhaustive scrutinee reports the MISSING-CASE row only —
/// never the completion. The oracle's own pin
/// (`test_enum_method_nonNullable_blockBody_switchStatement_notNullable_notExhaustive`,
/// upstream Dart SDK source, pkg/analyzer/test/src/diagnostics/
/// body_might_complete_normally_test.dart:53-68) expects
/// `nonExhaustiveSwitchStatement` alone in the pattern-enabled language
/// version, and the completion row only in the pre-patterns branch — so an
/// answer that adds the completion here would be the over-report this
/// control refuses.
#[test]
fn partially_covered_enum_switch_reports_the_missing_case_alone() {
    let source = "\
enum E { a, b }
int f(E e) {
  switch (e) {
    case E.a: return 1;
  }
}
";
    let report = report_ops(source);
    assert!(
        !reports_completion(source),
        "the oracle keeps the completion row out of a pattern-enabled \
         non-exhaustive switch: {:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "non_exhaustive_switch_statement"),
        "the missing-case row is the one the switch reports: {:?}",
        report.diagnostics
    );
}
