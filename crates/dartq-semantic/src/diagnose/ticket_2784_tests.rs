//! The #2784 augmentation-verifier pins: one exact oracle row per code
//! family (the span and message conventions of the pinned SDK's
//! augmentation checks — the `augment` keyword anchor everywhere except
//! the variable rows, which anchor at the variable's own name), plus the
//! resolved-shape silences and the multi-unit library walk.

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_augment, rows};
use super::*;

/// The augmentation-family rows of `report`, `code @offset:len message`.
fn aug_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("augmentation_") || d.code.name == "typedef_augmentation")
        .map(|d| {
            format!(
                "{} @{}:{} {}",
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message
            )
        })
        .collect()
}

/// The duplicate-constructor rows (the augment-ctor suppression pin).
fn duplicate_ctor_rows(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_constructor")
        .map(|d| d.code.name)
        .collect()
}

/// The absolute fixture offset of `needle` (searching from `after`), the
/// prelude the rig prepends included.
fn at(fixture: &str, needle: &str, after: usize) -> usize {
    CORE_PRELUDE.len() + fixture[after..].find(needle).unwrap() + after
}

/// The absolute fixture offset of `needle`, searching after the end of the
/// earlier match of `prev` at absolute offset `abs` (an `at` result).
fn at_abs(fixture: &str, needle: &str, prev: &str, abs: usize) -> usize {
    let local = abs - CORE_PRELUDE.len() + prev.len();
    CORE_PRELUDE.len() + local + fixture[local..].find(needle).unwrap()
}

/// Exactly one row of `name` at `offset:len` carrying `message`.
#[track_caller]
fn assert_aug_row(report: &FileReport, name: &str, offset: usize, len: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name
                && d.range.offset as usize == offset
                && d.range.length == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {name} @ {offset}:{len}; got {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

#[track_caller]
fn assert_aug_silent(report: &FileReport) {
    let family = aug_rows(report);
    assert!(family.is_empty(), "{family:#?}");
}

/// The class augmentation with no preceding declaration reports at the
/// `augment` keyword (upstream `_checkAugmentationWithoutDeclaration`'s
/// `errorToken`).
#[test]
fn without_declaration_class_anchors_at_the_augment_keyword() {
    let src = r#"
augment class A {}
"#;
    let report = report_ops(src);
    assert_aug_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
}

/// A variable augmentation anchors the same row at the variable's NAME
/// (upstream `visitTopLevelVariableDeclaration` passes `variable.name`).
#[test]
fn without_declaration_variable_anchors_at_the_name() {
    let src = r#"
augment int x = 0;
"#;
    let report = report_ops(src);
    assert_aug_row(
        &report,
        "augmentation_without_declaration",
        at(src, "x", 1),
        1,
        "The declaration being augmented doesn't exist.",
    );
}

/// A different-kind refusal spells both display names (upstream
/// `augmentationOfDifferentDeclarationKind.withArguments`).
#[test]
fn different_kind_row_names_both_kinds() {
    let src = r#"
class C {}
augment mixin C {}
"#;
    let report = report_ops(src);
    assert_aug_row(
        &report,
        "augmentation_of_different_declaration_kind",
        at(src, "augment", 0),
        7,
        "Can't augment a class with a mixin.",
    );
}

/// A named mixin application refuses every augment (upstream
/// `ClassFragmentImpl(isMixinApplication: true)`'s switch arm).
#[test]
fn mixin_application_class_refuses_augmentation() {
    let src = r#"
mixin M {}
class C = Object with M;
augment class C {}
"#;
    let report = report_ops(src);
    assert_aug_row(
        &report,
        "augmentation_of_mixin_application_class",
        at(src, "augment", 0),
        7,
        "Mixin application classes can't be augmented.",
    );
}

/// An `extends` clause repeating one any preceding fragment carries
/// reports at the augmentation's own `extends` keyword.
#[test]
fn extends_clause_already_present_anchors_at_extends() {
    let src = r#"
class B {}
class A extends B {}
augment class A extends B {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_extends_clause_already_present",
        at_abs(src, "extends", "augment", at(src, "augment", 0)),
        7,
        "The augmentation has an 'extends' clause, but an augmentation target already includes an 'extends' clause and it isn't allowed to be repeated or changed.",
    );
}

/// An extra header modifier anchors at its own keyword.
#[test]
fn modifier_extra_anchors_at_the_modifier() {
    let src = r#"
class A {}
augment base class A {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_modifier_extra",
        at(src, "base", 0),
        4,
        "The augmentation has the 'base' modifier that the declaration doesn't have.",
    );
}

/// A missing header modifier anchors at the `augment` keyword.
#[test]
fn modifier_missing_anchors_at_the_augment_keyword() {
    let src = r#"
base class A {}
augment class A {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_modifier_missing",
        at(src, "augment", 0),
        7,
        "The augmentation is missing the 'base' modifier that the declaration has.",
    );
}

/// A surplus type parameter anchors at its own name.
#[test]
fn type_parameter_count_surplus_anchors_at_the_name() {
    let src = r#"
class A {}
augment class A<T> {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_type_parameter_count",
        at_abs(src, "T", "augment", at(src, "augment", 0)),
        1,
        "The augmentation must have the same number of type parameters as the declaration.",
    );
}

/// No type-parameter list against a generic intro anchors at the
/// declaration's own name.
#[test]
fn type_parameter_count_missing_list_anchors_at_the_name() {
    let src = r#"
class A<T> {}
augment class A {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_type_parameter_count",
        at_abs(src, "A", "augment class ", at(src, "augment class ", 0)),
        1,
        "The augmentation must have the same number of type parameters as the declaration.",
    );
}

#[test]
fn type_parameter_name_mismatch_anchors_at_the_name() {
    let src = r#"
class A<T> {}
augment class A<U> {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_type_parameter_name",
        at_abs(src, "U", "augment", at(src, "augment", 0)),
        1,
        "The augmentation type parameter must have the same name as the corresponding type parameter of the declaration.",
    );
}

/// A written bound against an absent introductory bound reports (the
/// one-directional check) at the bound annotation.
#[test]
fn type_parameter_bound_mismatch_anchors_at_the_bound() {
    let src = r#"
class A<T> {}
augment class A<T extends num> {}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_type_parameter_bound",
        at_abs(src, "num", "augment", at(src, "augment", 0)),
        3,
        "The augmentation type parameter must have the same bound as the corresponding type parameter of the declaration.",
    );
}

/// A top-level function's return-type mismatch anchors at the written
/// return type (upstream `errorEntity: node.returnType ?? node.name`).
#[test]
fn return_type_mismatch_anchors_at_the_return_type() {
    let src = r#"
int f() => 0;
augment String f() => '';
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_return_type_mismatch",
        at_abs(src, "String", "augment", at(src, "augment", 0)),
        6,
        "The augmentation's return type 'String' must be the same as the introductory declaration's return type 'int'.",
    );
}

/// A variable's induced-getter mismatch reports the induced variant at the
/// variable's own name (upstream `errorEntity: variable.name`).
#[test]
fn induced_getter_return_mismatch_anchors_at_the_name() {
    let src = r#"
int x = 0;
augment String x = '';
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_return_type_mismatch",
        at_abs(src, "x", "augment", at(src, "augment", 0)),
        1,
        "The getter induced by this augmentation has return type 'String', but the getter being augmented has return type 'int'.",
    );
}

/// A non-final augmentation of a final variable induces a setter with no
/// preceding fragment — the setter variant of the without-declaration row
/// at the variable's name.
#[test]
fn final_intro_non_final_augmentation_reports_the_setter_row() {
    let src = r#"
final int x = 0;
augment int x = 0;
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_without_declaration",
        at_abs(src, "x", "augment", at(src, "augment", 0)),
        1,
        "This augmentation induces a setter, but no setter declaration named 'x' exists to augment.",
    );
}

/// The constructor's `const` pair anchors the missing row at the member's
/// own `augment` keyword.
#[test]
fn constructor_const_missing_anchors_at_the_member_augment_keyword() {
    let src = r#"
class C {
  const C();
}
augment class C {
  augment C();
}
"#;
    let report = report_ops(src);
    let member_augment = at_abs(src, "augment", "augment class C", at(src, "augment class C", 0));
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_modifier_missing",
        member_augment,
        7,
        "The augmentation is missing the 'const' modifier that the declaration has.",
    );
}

/// A surplus required positional anchors at the first surplus parameter's
/// name (upstream `formalParameterAtPositionalIndex(firstRequired)`).
#[test]
fn required_count_surplus_anchors_at_the_surplus_name() {
    let src = r#"
class C {
  C(int a);
}
augment class C {
  augment C(int a, int b);
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_required_positional_formal_parameter_count",
        at_abs(src, "b", "augment C(", at(src, "augment C(", 0)),
        1,
        "The augmentation has 2 required positional formal parameters, but the declaration has 1.",
    );
}

/// A missing required positional with no optional group anchors at the `)`
/// (the `leftDelimiter ??` fallback's entity arm).
#[test]
fn required_count_missing_anchors_at_the_right_paren() {
    let src = r#"
class C {
  C(int a);
}
augment class C {
  augment C();
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_required_positional_formal_parameter_count",
        at_abs(src, ")", "augment C(", at(src, "augment C(", 0)),
        1,
        "The augmentation has 0 required positional formal parameters, but the declaration has 1.",
    );
}

/// A surplus optional positional against an optional-less intro anchors at
/// the group's opening delimiter (upstream `leftDelimiter ?? ...`).
#[test]
fn optional_count_surplus_anchors_at_the_group_delimiter() {
    let src = r#"
class C {
  C(int a);
}
augment class C {
  augment C(int a, [int b]);
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_optional_positional_formal_parameter_count",
        at_abs(src, "[", "augment C(", at(src, "augment C(", 0)),
        1,
        "The augmentation has 1 optional positional formal parameters, but the declaration has 0.",
    );
}

#[test]
fn named_extra_anchors_at_the_parameter_name() {
    let src = r#"
class C {
  C(int a);
}
augment class C {
  augment C(int a, {int n});
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_named_formal_parameter_extra",
        at_abs(src, "n", "{int ", at(src, "{int ", 0)),
        1,
        "The augmentation has a named formal parameter 'n', but the declaration doesn't.",
    );
}

/// A missing named parameter anchors at the `)` (upstream
/// `formalParameterList.rightParenthesis`).
#[test]
fn named_missing_anchors_at_the_right_paren() {
    let src = r#"
class C {
  C({int n});
}
augment class C {
  augment C();
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_named_formal_parameter_missing",
        at_abs(src, ")", "augment C(", at(src, "augment C(", 0)),
        1,
        "The augmentation is missing the named formal parameter 'n' from the declaration.",
    );
}

#[test]
fn covariant_extra_anchors_at_the_keyword() {
    let src = r#"
class C {
  C(int a);
}
augment class C {
  augment C(covariant int a);
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_formal_parameter_modifier_extra",
        at_abs(src, "covariant", "augment C(", at(src, "augment C(", 0)),
        9,
        "The augmentation has the 'covariant' modifier on this formal parameter, but the declaration doesn't.",
    );
}

#[test]
fn covariant_missing_anchors_at_the_parameter_name() {
    let src = r#"
class C {
  C(covariant int a);
}
augment class C {
  augment C(int a);
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_formal_parameter_modifier_missing",
        at_abs(src, "a", "augment C(int ", at(src, "augment C(int ", 0)),
        1,
        "The augmentation is missing the 'covariant' modifier on this formal parameter that the declaration has.",
    );
}

#[test]
fn formal_type_mismatch_anchors_at_the_written_type() {
    let src = r#"
class C {
  C(int a);
}
augment class C {
  augment C(String a);
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_formal_parameter_type_mismatch",
        at_abs(src, "String", "augment C(", at(src, "augment C(", 0)),
        6,
        "The augmentation's formal parameter type 'String' must be the same as the declaration's formal parameter type 'int'.",
    );
}

#[test]
fn positional_name_mismatch_anchors_at_the_name() {
    let src = r#"
class C {
  C(int p1);
}
augment class C {
  augment C(int p2);
}
"#;
    let report = report_ops(src);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_positional_formal_parameter_name",
        at_abs(src, "p2", "augment C(", at(src, "augment C(", 0)),
        2,
        "The parameter name 'p2' must either match the name 'p1' from a preceding declaration or be '_'.",
    );
}

/// The parser-rejected typedef augmentation rides the capture log's row at
/// the `augment` keyword.
#[test]
fn typedef_augmentation_anchors_at_the_augment_keyword() {
    let src = r#"
class C {}
augment typedef C = int;
"#;
    let report = report_ops(src);
    assert_aug_row(
        &report,
        "typedef_augmentation",
        at(src, "augment", 0),
        7,
        "Type aliases can't be augmented.",
    );
}

/// An augment constructor merges into the introductory element's fragment
/// chain — never a second declaration of the selector (the
/// `duplicate_constructor` pass skips augment sites).
#[test]
fn augment_constructor_suppresses_duplicate_constructor() {
    let src = r#"
class C {
  C();
}
augment class C {
  augment C() {}
}
"#;
    let report = report_ops(src);
    assert!(
        duplicate_ctor_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
    assert_aug_silent(&report);
}

/// A resolved, shape-identical augmentation is silent.
#[test]
fn resolved_shape_identical_augmentation_is_silent() {
    let report = report_ops(
        r#"
class A {}
augment class A {}
int f() => 0;
augment int f() => 0;
"#,
    );
    assert_aug_silent(&report);
}

/// The cross-unit walk: an augmentation in a second file of the same
/// library resolves against the first file's introductory declarations and
/// reports in its own unit.
#[test]
fn cross_unit_extends_clause_already_present_reports_in_the_augmenting_unit() {
    let main = r#"
class B {}
class A extends B {}
"#;
    let aug = r#"
augment class A extends B {}
"#;
    let report = report_ops_augment(main, aug);
    assert_eq!(aug_rows(&report).len(), 1, "{:#?}", aug_rows(&report));
    assert_aug_row(
        &report,
        "augmentation_extends_clause_already_present",
        aug.find("extends").unwrap(),
        7,
        "The augmentation has an 'extends' clause, but an augmentation target already includes an 'extends' clause and it isn't allowed to be repeated or changed.",
    );
}

/// The cross-unit resolved shape is silent.
#[test]
fn cross_unit_resolved_augmentation_is_silent() {
    let report = report_ops_augment("class A {}\n", "augment class A {}\n");
    assert_aug_silent(&report);
}
