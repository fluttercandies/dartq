//! The #2867 augmentation-signature-matching pins: one oracle-derived row
//! per broken face, each citing its conformance evidence. These are RED
//! until the face lands — the rows below are the oracle's answer, not the
//! current (pre-repair) emission.
//!
//! Root cause of the family: `MultiplexListener` dropped the
//! formal-parameter, initializer, and redirecting-factory events, so every
//! `augment` executable's `formals` drained empty — every parameter-check
//! row read "augmentation has 0 …" — and the accessor-family unification
//! (variable element owning its induced getter/setter chains) never ran.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The augmentation-family rows of `report`, `code @offset:len message`.
fn aug_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name.starts_with("augmentation_")
                || d.code.name.starts_with("constructor_already_complete")
                || d.code.name.starts_with("function_already_complete")
        })
        .map(|d| {
            format!(
                "{} @{}:{} {}",
                d.code.name, d.range.offset, d.range.length, d.message
            )
        })
        .collect()
}

/// The rows of `report` under one shared code name.
fn named_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

/// The absolute fixture offset of `needle` (prelude included).
fn at(fixture: &str, needle: &str, after: usize) -> usize {
    CORE_PRELUDE.len() + fixture[after..].find(needle).unwrap() + after
}

/// `at`, searching after the end of the earlier match of `prev` at `abs`.
fn at_abs(fixture: &str, needle: &str, prev: &str, abs: usize) -> usize {
    let local = abs - CORE_PRELUDE.len() + prev.len();
    CORE_PRELUDE.len() + local + fixture[local..].find(needle).unwrap()
}

/// Exactly one row of `name` at `offset:len` carrying `message`.
#[track_caller]
fn assert_row(report: &FileReport, name: &str, offset: usize, len: u32, message: &str) {
    let hits = named_rows(report, name)
        .into_iter()
        .filter(|d| d.range.offset as usize == offset && d.range.length == len)
        .count();
    assert_eq!(
        hits,
        1,
        "expected one {name} @{offset}:{len}; got {:#?}",
        rows(report)
    );
}

// ---------------------------------------------------------------------------
// P1 — aug-side formal capture (the mux forwards; `AugmentDecl.formals` no
// longer drains empty).
// ---------------------------------------------------------------------------

/// `void f(int _); augment void f(int x); augment void f(int _) {}`
///
/// Oracle: SILENT. `test_topLevelFunction_rP__wildcard__x__wildcard`
/// (AugmentationFormalParameterNameTest) expects no rows — a `_` wildcard
/// on either side skips the name check, and the count checks see the real
/// (non-zero) formals. Pre-repair emission: two spurious
/// `augmentation_required_positional_formal_parameter_count` rows reading
/// "augmentation has 0" (report.tsv `extra` column).
#[test]
fn wildcard_formals_on_both_sides_are_silent() {
    let src = r#"
void f(int _);
augment void f(int x);
augment void f(int _) {}
"#;
    let report = report_ops(src);
    assert!(aug_rows(&report).is_empty(), "{:#?}", aug_rows(&report));
}

/// A name mismatch surfaces ONLY the name row — the count row must not
/// also fire (the aug-side formals are non-empty once captured).
/// `test_topLevelFunction_rP__x__y` (AugmentationFormalParameterNameTest):
/// `void f(int x); augment void f(int y) {}` expects one
/// `augmentationPositionalFormalParameterName` at `y`.
#[test]
fn positional_name_mismatch_reports_only_the_name_row() {
    let src = r#"
void f(int x);
augment void f(int y) {}
"#;
    let report = report_ops(src);
    let count = named_rows(
        &report,
        "augmentation_required_positional_formal_parameter_count",
    );
    assert!(count.is_empty(), "{:#?}", rows(&report));
    assert_row(
        &report,
        "augmentation_positional_formal_parameter_name",
        at(src, "y", 1),
        1,
        "The parameter name 'y' must either match the name 'x' from a preceding declaration or be '_'.",
    );
}

/// `void f(int p); augment void f(int p) {}` — identical signature, silent.
#[test]
fn identical_top_level_signature_is_silent() {
    let src = r#"
void f(int p);
augment void f(int p) {}
"#;
    let report = report_ops(src);
    assert!(aug_rows(&report).is_empty(), "{:#?}", aug_rows(&report));
}

// ---------------------------------------------------------------------------
// P2 — accessor-family unification (the variable element owns its induced
// getter/setter chains; `augment get`/`set`/`field` resolve through it).
// ---------------------------------------------------------------------------

/// `static abstract int foo; augment static int get foo => 0; augment
/// static set foo(int _) {}`
///
/// Oracle: only `abstract_static_field`.
/// `test_class_staticField_abstract_completeAfterAugmentations`
/// (ExecutableBodyTest): the field's induced getter and setter chains
/// absorb both augments — no `different_declaration_kind`, no count row.
/// Pre-repair emission adds `augmentation_of_different_declaration_kind`
/// ("Can't augment a field with a getter") + a "has 0" count row.
#[test]
fn static_field_augments_its_induced_accessors() {
    let src = r#"
class A {
  static abstract int foo;
  augment static int get foo => 0;
  augment static set foo(int _) {}
}
"#;
    let report = report_ops(src);
    let diff = named_rows(&report, "augmentation_of_different_declaration_kind");
    assert!(diff.is_empty(), "{:#?}", rows(&report));
    let count = named_rows(
        &report,
        "augmentation_required_positional_formal_parameter_count",
    );
    assert!(count.is_empty(), "{:#?}", rows(&report));
    // `abstract_static_field` stays — the field itself is illegal.
    assert_row(
        &report,
        "abstract_static_field",
        at(src, "abstract", 1),
        8,
        "Static fields can't be declared 'abstract'.",
    );
}

/// `int? get foo => 0;` then `augment abstract final String? foo;`
///
/// Oracle: one induced-getter `augmentation_return_type_mismatch` only.
/// `test_class_instanceGetter_instanceField_int_String`
/// (AugmentationReturnTypeMismatchTest): the field augment induces a
/// getter of type `String?` against the declared getter's `int?` — the
/// induced variant row. Pre-repair emission adds
/// `augmentation_of_different_declaration_kind` + `final_not_initialized`.
#[test]
fn field_augmentation_induces_getter_return_mismatch() {
    let src = r#"
class A {
  int? get foo => 0;
}
augment class A {
  augment abstract final String? foo;
}
"#;
    let report = report_ops(src);
    let diff = named_rows(&report, "augmentation_of_different_declaration_kind");
    assert!(diff.is_empty(), "{:#?}", rows(&report));
    let fin = named_rows(&report, "final_not_initialized");
    assert!(fin.is_empty(), "{:#?}", rows(&report));
    assert_row(
        &report,
        "augmentation_return_type_mismatch",
        at_abs(src, "foo", "String?", at(src, "String?", 0)),
        3,
        "The getter induced by this augmentation has return type 'String?', but the getter being augmented has return type 'int?'.",
    );
}

/// A non-final augment field against a getter-only variable induces a
/// setter with no preceding fragment — the SETTER variant of
/// `augmentation_without_declaration` at the variable name.
/// (Upstream `visitFieldDeclaration` /
/// `_checkAugmentationWithoutDeclarationForInducedAccessors`: the induced
/// setter's `previousFragment` is null when no setter exists.)
#[test]
fn getter_only_variable_field_augment_reports_the_setter_row() {
    let src = r#"
class A {
  int get foo => 0;
}
augment class A {
  augment int foo = 0;
}
"#;
    let report = report_ops(src);
    assert_row(
        &report,
        "augmentation_without_declaration",
        at_abs(src, "foo", "augment int", at(src, "augment int", 0)),
        3,
        "This augmentation induces a setter, but no setter declaration named 'foo' exists to augment.",
    );
}
