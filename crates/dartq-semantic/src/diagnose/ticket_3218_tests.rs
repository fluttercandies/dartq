//! The #3218 pins: the outside-library clause-error drive gate
//! (F9-A / S1). Upstream `error_verifier.dart` runs the whole
//! outside-library block (`_checkForBaseClassOrMixinImplemented`,
//! `_checkForInterfaceClassOrMixinSuperclass`, `_checkForFinalSupertype`,
//! `_checkForSealedSupertype`) only when every clause-error probe is
//! false — `_checkForExtendsDisallowedClass`,
//! `_checkForImplementsClauseErrorCodes`, `_checkForAllMixinErrorCodes`,
//! `_checkForNoGenerativeConstructorsInSuperclass` on the class drive
//! (:2746-2753), `_checkForOnClauseErrorCodes` +
//! `_checkForImplementsClauseErrorCodes` on the mixin drive
//! (:8168-8169). When a clause already reports a hard error
//! (`subtype_of_disallowed_type` / the core-`Enum` arms), the
//! `invalid_use_of_type_outside_library` rows stand down — dartq used to
//! emit both.
//!
//! The gate is deliberately under-gated for iteration 1: only the
//! disallowed-subtype arm (`isNonSubtypableClass`/`isDartCoreEnum` — the
//! same heads `subtype_of_disallowed_type` reports) is modeled.
//! `class_used_as_mixin` never gates upstream (it reports without setting
//! `problemReported`, error_verifier.dart:2917-2926);
//! `no_generative_constructors_in_superclass`, `mixin_instantiate`,
//! `mixin_inherits_not_from_object`, and the `*_deferred_class` arms are
//! unmodeled. Pass 5 (`BaseOrFinalTypeVerifier.checkElement`,
//! resolver-driven) is NOT gated — its `subtype_of_base_or_final_...`
//! rows still fire beside a clause error.
//!
//! The rig prelude declares `int`/`num`/`String`/`bool`/`double`/`Enum`
//! as USER classes, so those spellings shadow the restricted SDK faces
//! and never fire a clause error here — the fixtures key on the
//! names-only `dart:core` stub spellings (`Null`/`Record`/`Enum`) that
//! resolve to the `dart:*` face upstream's `isNonSubtypableClass` reads.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the conformance
//! `diagnostics/{extends,implements}_disallowed_class`,
//! `subtype_of_disallowed_type`,
//! `mixin_super_class_constraint_disallowed_class`, and the co19
//! Class-modifiers `*_t0x` corpora — every F9-A extra names a
//! non-subtypable SDK spelling (`int`/`String`/`Null`/`bool`/`double`/
//! `num`/`Record`/`Enum`/`FutureOr`).

use super::test_rig::{RigImport, report_ops, report_ops_imports, rows};
use crate::diagnose::types::FileReport;

/// The `invalid_use_of_type_outside_library` rows as `(offset, len,
/// message)`, sorted — the family's machine-row face.
fn outside_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut v: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_use_of_type_outside_library")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    v.sort();
    v
}

/// The `subtype_of_disallowed_type` rows as `(offset, message)`, sorted —
/// the clause-error rows the gate keys on.
fn disallowed_rows(report: &FileReport) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_disallowed_type")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    v.sort();
    v
}

/// One dependency library holding the modifier-carrying declarations.
fn dep(source: &'static str) -> RigImport<'static> {
    RigImport {
        uri: "lib.dart",
        prefix: None,
        source,
    }
}

// —— extends ————————————————————————————————————————————————————————————

/// `class X extends Null {}` reports the clause error and NO
/// outside-library row — upstream's `_checkForExtendsDisallowedClass`
/// already fired, so the whole block stands down.
#[test]
fn extends_null_reports_clause_error_and_gates_outside_library() {
    let fixture = "class X extends Null {}\n";
    let report = report_ops(fixture);
    let rows = disallowed_rows(&report);
    assert_eq!(rows.len(), 1, "clause rows: {:?}", self::rows(&report));
    assert_eq!(rows[0].1, "Classes can't extend 'Null'.");
    assert!(
        outside_rows(&report).is_empty(),
        "outside-library must be gated: {:?}",
        outside_rows(&report)
    );
}

// —— implements —————————————————————————————————————————————————————————

/// `class X implements Null {}` — `_checkForImplementsClauseErrorCodes`
/// fires and gates the outside-library block.
#[test]
fn implements_null_reports_clause_error_and_gates_outside_library() {
    let fixture = "class X implements Null {}\n";
    let report = report_ops(fixture);
    let rows = disallowed_rows(&report);
    assert_eq!(rows.len(), 1, "clause rows: {:?}", self::rows(&report));
    assert_eq!(rows[0].1, "Classes and mixins can't implement 'Null'.");
    assert!(
        outside_rows(&report).is_empty(),
        "outside-library must be gated: {:?}",
        outside_rows(&report)
    );
}

// —— on —————————————————————————————————————————————————————————————————

/// `mixin M on Null {}` — the mixin drive's `_checkForOnClauseErrorCodes`
/// fires and gates the outside-library block.
#[test]
fn on_null_reports_clause_error_and_gates_outside_library() {
    let fixture = "mixin M on Null {}\n";
    let report = report_ops(fixture);
    let rows = disallowed_rows(&report);
    assert_eq!(rows.len(), 1, "clause rows: {:?}", self::rows(&report));
    assert_eq!(
        rows[0].1,
        "'Null' can't be used as a superclass constraint."
    );
    assert!(
        outside_rows(&report).is_empty(),
        "outside-library must be gated: {:?}",
        outside_rows(&report)
    );
}

// —— with ———————————————————————————————————————————————————————————————

/// `class X with Record {}` — the `with` arm of
/// `_checkForAllMixinErrorCodes` fires `subtype_of_disallowed_type` on a
/// non-subtypable SDK spelling and gates the outside-library block.
#[test]
fn with_record_reports_clause_error_and_gates_outside_library() {
    let fixture = "class X with Record {}\n";
    let report = report_ops(fixture);
    let rows = disallowed_rows(&report);
    assert_eq!(rows.len(), 1, "clause rows: {:?}", self::rows(&report));
    assert_eq!(rows[0].1, "Classes can't mixin 'Record'.");
    assert!(
        outside_rows(&report).is_empty(),
        "outside-library must be gated: {:?}",
        outside_rows(&report)
    );
}

// —— positive control: a legal same-library supertype still reports ——————

/// A `final class F` in another library reports the interface-extends
/// row beside a `extends F` — no clause error fires, so the
/// outside-library block is not gated.
#[test]
fn extends_final_class_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nclass X extends F {}\n";
    let report = report_ops_imports(fixture, &[dep("final class F {}\n")]);
    let rows = outside_rows(&report);
    assert_eq!(rows.len(), 1, "rows: {:?}", self::rows(&report));
    assert_eq!(
        rows[0].2,
        "The class 'F' can't be extended outside of its library because it's a final class."
    );
}

// —— negative: pass 5 is NOT gated ——————————————————————————————————————

/// The subtype verifier (`BaseOrFinalTypeVerifier`) is resolver-driven
/// and sits outside the clause-error gate: a `base class B` supertype
/// still reports `subtype_of_base_or_final_is_not_base_final_or_sealed`
/// even when a clause error stands down the outside-library passes —
/// `class Y extends Null implements B {}` reports the `Null` clause
/// error, the pass-5 row on `B`, and no outside-library row.
#[test]
fn subtype_of_base_or_final_verifier_still_reports_beside_a_clause_error() {
    let fixture = "import 'lib.dart';\nclass Y extends Null implements B {}\n";
    let report = report_ops_imports(fixture, &[dep("base class B {}\n")]);
    // The `extends Null` clause error still fires (the gate suppresses
    // only the outside-library block, not the disallowed-subtype rows).
    let clause = disallowed_rows(&report);
    assert_eq!(clause.len(), 1, "clause rows: {:?}", rows(&report));
    assert_eq!(clause[0].1, "Classes can't extend 'Null'.");
    // Pass 5's subtype row still reports — the `B` implements face is
    // legal-modifier-carrying and the verifier is not gated.
    let subtype: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_base_or_final_is_not_base_final_or_sealed")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        subtype.len(),
        1,
        "subtype verifier must still report: {:?}",
        rows(&report)
    );
    // And the outside-library block stays gated by the `extends Null`
    // clause error — the `B` implements face produces no
    // `invalid_use_of_type_outside_library` row.
    assert!(
        outside_rows(&report).is_empty(),
        "outside-library must stay gated: {:?}",
        outside_rows(&report)
    );
}
