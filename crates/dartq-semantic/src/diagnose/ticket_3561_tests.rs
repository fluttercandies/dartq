//! The #3561 pins: the *already-correct* half of the two language-version
//! gates this ticket names, locked at their exact boundaries.
//!
//! # The contract
//!
//! Both faces are gated by the enclosing library's language version. dartq is
//! missing the reporting arms; what is correct today — and what these pins
//! lock — is the other half: the versions at which the analyzed spelling is
//! legal and must stay silent.
//!
//! 1. `private-named-parameters` (`enabledIn: '3.12.0'`, upstream Dart SDK
//!    source, `tools/experimental_features.yaml`). `_checkPrivateOptionalParameter`
//!    (upstream Dart SDK source, `pkg/analyzer/lib/src/generated/error_verifier.dart:7872-7923`)
//!    reports only when the feature is off, or when the private name has no
//!    field or no public spelling behind it. From 3.12 on a named field formal
//!    like `C({this._m})` is legal: the oracle prints no rows for the fixture
//!    below at 3.12 or at the latest version. The feature's parser stance is
//!    deliberately absent (`Feature::PrivateNamedParameters` is in the
//!    version-blind arm of `dartq_parser::features`): the grammar is the same
//!    in every version, only the analysis rows are gated.
//! 2. `obsolete_colon_for_default_value` vs `deprecated_colon_for_default_value`
//!    (`_checkForColonDefaultValue`, upstream Dart SDK source,
//!    `pkg/analyzer/lib/src/dart/resolver/best_practices_verifier.dart:957-972`):
//!    the selection is `languageVersion.major < 3` → the deprecated INFO row,
//!    else the obsolete ERROR row. 3.0 belongs to the obsolete side and may not
//!    drift back to the deprecated row.
//!
//! The reporting arms (the 3.11-and-earlier `experiment_not_enabled` /
//! `private_optional_parameter` / `private_named_non_field_parameter` rows and
//! the 2.x `deprecated_colon_for_default_value` row) live in
//! `diagnose/listener.rs` and need their own `SemanticCode` entries in
//! `codes.rs`; they are not part of this pin file.

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops_bare};

/// A private named field formal that is fully legal from 3.12 on. Untyped on
/// purpose: the fixture is analyzed without the rig's core prelude (the
/// `// @dart` marker has to sit on the first line), and the private-parameter
/// gate does not read the formal's type, so no builtin spelling is needed. The
/// getter reads the field, which keeps unused-member faces out of the report.
const PRIVATE_FIELD_FORMAL: &str = "class C {\n\
     var _m;\n\
     C({this._m});\n\
     get current => _m;\n\
     }\n";

/// The `enabledIn` boundary itself: 3.12 is the first version where a
/// `_`-prefixed named field formal is legal (oracle:
/// `dart analyze --format machine` on `// @dart=3.12` + the fixture prints no
/// rows, rc 0), so dartq reports nothing there either.
#[test]
fn private_named_field_formal_at_3_12_is_silent() {
    let report = report_ops_bare(&format!("// @dart=3.12\n{PRIVATE_FIELD_FORMAL}"));
    assert_silent(&report);
}

/// The shipped default posture: a 3.13+ library keeps the same silence — the
/// gate must stay quiet on the whole enabled side of 3.12, not just at the
/// boundary version.
#[test]
fn private_named_field_formal_at_latest_language_version_is_silent() {
    let report = report_ops_bare(PRIVATE_FIELD_FORMAL);
    assert_silent(&report);
}

/// `major >= 3` keeps the obsolete ERROR face at the colon: the oracle at
/// `// @dart=3.0` reports `obsolete_colon_for_default_value` at the colon
/// (line 2, column 10 → offset 22 of the fixture) with the verbatim message
/// and the "equal sign" correction, and never the deprecated INFO row.
#[test]
fn colon_default_at_language_3_0_keeps_the_obsolete_face() {
    let report = report_ops_bare("// @dart=3.0\nvoid f({x: 0}) {}\n");
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "obsolete_colon_for_default_value")
        .unwrap_or_else(|| panic!("rows: {:?}", code_names(&report)));
    assert_eq!(
        row.message,
        "Using a colon as the separator before a default value is no longer supported."
    );
    assert_eq!(
        row.correction.as_deref(),
        Some("Try replacing the colon with an equal sign.")
    );
    assert_eq!(row.range.offset as usize, 22);
    assert_eq!(row.range.length as usize, 1);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "deprecated_colon_for_default_value"),
        "rows: {:?}",
        code_names(&report)
    );
}
