//! The #3848 pins: the `duplicate_field_formal_parameter` row contract.
//!
//! The formal-parameter duplicate scope splits the row in two: when BOTH
//! colliding formals are initializing formals (`this.x`), the verifier
//! returns `diag.duplicateFieldFormalParameter` instead of
//! `diag.duplicateDefinition` (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/duplicate_definition_verifier.dart:447-450` —
//! `_FormalParameterDuplicateIdentifierScope.getDiagnostic`); a pair with at
//! least one plain formal keeps `duplicate_definition`. Both rows are built
//! by `duplicateDefinitionForNodes` (`diagnostic_factory.dart:114-131`):
//! ERROR at the SECOND formal's name token (length 1) with the shared
//! "first definition" context, and only the code and problem text differ
//! (`messages.yaml:5230-5236`, `compileTimeError`, `hasPublishedDocs: true`).
//!
//! Measured against the pinned 3.13.7 oracle (`dart analyze
//! --format=machine`), on `class A { int x = 0; int y = 0; int z = 0;
//! A(this.x, this.x); B(this.y, y){} C(z, z){} }`:
//!
//! ```text
//! ERROR|COMPILE_TIME_ERROR|DUPLICATE_FIELD_FORMAL_PARAMETER|<file>|4|12|1|...
//! ERROR|COMPILE_TIME_ERROR|DUPLICATE_DEFINITION|<file>|5|13|1|The name 'y' is already defined.
//! ERROR|COMPILE_TIME_ERROR|DUPLICATE_DEFINITION|<file>|6|8|1|The name 'z' is already defined.
//! ```
//!
//! This module pins the code half of the split — descriptor name/machine,
//! tier, severity, and the verbatim text. The row-level swap is not wired
//! yet: the capture-time duplicate log carries no field-formal bit
//! (`crate::model::DuplicateDecl` has `name` + `range` only), and the
//! engine's bake-versus-fold dedup keys on the `duplicate_definition` name
//! (`semdiag.rs`, the `baked_ranges` drop), so both faces still report
//! `duplicate_definition` for the pair shape. The three-shape behavioral
//! pins belong to that plumbing change; this module keeps the descriptor
//! from drifting into a reused `duplicate_definition` name in the meantime.

use crate::codes::SemanticCode;
use dartq_diagnostics::{DiagnosticType, Severity};

use super::test_rig::report_ops_bare;

/// The official name (upstream registry, `analyzer-codes.tsv`).
const NAME: &str = "duplicate_field_formal_parameter";

/// The machine form (`DARTQ`-format code column / `dart analyze --format=machine`).
const MACHINE: &str = "DUPLICATE_FIELD_FORMAL_PARAMETER";

#[test]
fn descriptor_matches_the_official_row() {
    let info = SemanticCode::DuplicateFieldFormalParameter.code_info();
    assert_eq!(info.name, NAME);
    assert_eq!(info.machine, MACHINE);
    assert_eq!(
        SemanticCode::DuplicateFieldFormalParameter.dtype(),
        DiagnosticType::CompileTimeError
    );
    assert_eq!(
        SemanticCode::DuplicateFieldFormalParameter.severity(),
        Severity::Error
    );
}

#[test]
fn message_is_the_verbatim_oracle_text() {
    assert_eq!(
        SemanticCode::DuplicateFieldFormalParameter.message("x", ""),
        "The field 'x' can't be initialized by multiple parameters in the same constructor."
    );
}

/// The two codes are distinct rows for the same scope machinery: an entry
/// that reuses `duplicate_definition`'s name or text would make the pair
/// indistinguishable in every consumer (the engine's dedup, the census
/// triples, and the human report).
#[test]
fn the_pair_code_stays_distinct_from_duplicate_definition() {
    let pair = SemanticCode::DuplicateFieldFormalParameter.code_info();
    let plain = SemanticCode::DuplicateDefinition.code_info();
    assert_ne!(pair.name, plain.name);
    assert_ne!(pair.machine, plain.machine);
    assert_ne!(
        SemanticCode::DuplicateFieldFormalParameter.message("x", ""),
        SemanticCode::DuplicateDefinition.message("x", "")
    );
}

/// The pair row's geometry — the half of the split the current capture
/// already gets right and that the code swap must preserve: ERROR at the
/// SECOND formal's name token, length 1, one row per collision. The
/// evidence is the census family itself: all 29
/// `duplicate_field_formal_parameter` cases carry our `duplicate_definition`
/// extra at exactly the oracle row's span and length, i.e. only the code and
/// the text are wrong. The code assertion joins this pin when the swap lands.
#[test]
fn the_pair_row_lands_on_the_second_formal_name() {
    // ASCII-only fixture: byte offsets are the row's UTF-16 offsets too.
    const SOURCE: &str = "class A {\n  int x = 0;\n  A(this.x, this.x);\n}\n";
    let report = report_ops_bare(SOURCE);
    let dup_name = SemanticCode::DuplicateDefinition.code_info().name;
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == dup_name || row.code.name == NAME)
        .collect();
    assert_eq!(rows.len(), 1, "one row per colliding pair, got {rows:?}");
    let row = rows[0];
    let second_name = u32::try_from(SOURCE.rfind('x').expect("the fixture ends on the name"))
        .expect("fixture is short");
    assert_eq!(row.range.offset, second_name, "the SECOND formal's name");
    assert_eq!(row.range.length, 1);
    assert_eq!(row.severity, Severity::Error);
}
