//! The #3305 pins: the `duplicate_definition` scope matrix, pinned on the
//! shapes the fold already answers 1:1 with the pinned SDK
//! (`conformance/status.tsv` `diagnostic duplicate_definition 889 604 285`).
//!
//! Upstream keeps ONE rule set per scope kind (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/duplicate_definition_verifier.dart` and
//! `member_duplicate_definition_verifier.dart`):
//!
//! - the UNIT scope (`checkUnit`, verifier `:116-307`) keys getters and
//!   setters separately (`definedGetters` / `definedSetters`), reports the
//!   second declaration of a name, and does NOT exempt `_` — a top-level
//!   `const _` / `final _` / `var _` pair is a duplicate (a wildcard is a
//!   `_DuplicateIdentifierScope` notion only, i.e. statements, `for` heads,
//!   formal parameters and type parameters);
//! - the MEMBER scope (`_checkDuplicateIdentifier`, member verifier
//!   `:297-346`) merges a lone getter with a lone setter into a pair, reports
//!   every other re-claim, and DOES exempt a wildcard element
//!   (`fragment.element.isWildcardVariable`, `:303`) as well as `augment`
//!   members (`:307-309`), which neither check nor claim;
//! - the FORMAL-PARAMETER scope (`checkParameters`, verifier `:56-82`) shares
//!   one scope across positional and named formals, so a field formal
//!   `this.x` collides with a plain `x`; two colliding INITIALIZING formals
//!   are a different code (`duplicate_field_formal_parameter`), and a
//!   field-formal `_` is not a wildcard (`:444-457`);
//! - the TYPE-PARAMETER scope (`checkTypeParameters`, `:111-118`) is an
//!   ordinary `_DuplicateIdentifierScope`.
//!
//! Residual (measured against the conformance corpus, not pinned green here
//! because the owning emit site sits in a file this ticket may not touch):
//!
//! - a member declared inside a non-`augment` `augment` block never joins the
//!   base declaration's member scope — `class A { int foo; } augment class A
//!   { int foo; }` reports nothing where the oracle reports at the augment
//!   block's member (21 cases in `diagnostics/duplicate_definition/`, 22 in
//!   co19 `Augmentations/`). Upstream keys the scope on the MERGED element
//!   (`_getElementContext(fragment.element.firstFragment)`), so every
//!   augmentation of one element shares one `instanceScope`;
//!   `collector::member_dup_claim` keys it on the open container instead;
//! - the same per-unit blindness hides cross-unit part duplicates: a part
//!   file's top-level names never join the library's unit scope (5 cases in
//!   the home family, 2 in co19 `Parts-with-imports/`);
//! - the inverse direction, an over-report: a constructor field formal named
//!   `_` claims a duplicate where the oracle keeps it a wildcard
//!   (`test_parameters_constructor_field_second_wildcard`,
//!   `test_parameters_primaryConstructor_field_simple_wildcard`, co19
//!   `Wildcards/initializing_formals_A03_t01`).
//!
//! The emit surfaces for all three are `collector.rs` (capture-time scope and
//! log) and `diagnose.rs` (the fold's lead rows and the engine bake), so the
//! fix belongs to those files' owners; the pins below are the regression
//! guard they must keep green while closing the gap.

use super::test_rig::{report_ops, report_ops_wildcards};

/// No `duplicate_definition` row at all — the negative assertion for the
/// scope-isolation pins, which legitimately carry unrelated `unused_*` rows.
fn assert_no_dup(report: &crate::diagnose::FileReport) {
    let dups = dup_messages(report);
    assert!(dups.is_empty(), "expected no duplicate rows, got {dups:?}");
}

fn dup_messages(report: &crate::diagnose::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect()
}

/// The byte offset of every `duplicate_definition` row's name token, in
/// arrival order — the second declaration's own name.
fn dup_offsets(report: &crate::diagnose::FileReport) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.range.offset)
        .collect()
}

// —— unit scope ——

/// Two same-named top-level classes: the second reports at its own name
/// token (`checkUnit`'s `definedGetters` face).
#[test]
fn unit_class_repeat_reports_the_second() {
    let report = report_ops("class A {}\nclass B {}\nclass A {}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'A' is already defined.".to_string()],
        "the second top-level class collides"
    );
}

/// Two same-named top-level variables: one row, at the second declaration.
#[test]
fn unit_variable_repeat_reports_the_second() {
    let report = report_ops("var f = 1;\nvar f = 2;\n");
    assert_eq!(dup_messages(&report), vec!["The name 'f' is already defined.".to_string()]);
    let offsets = dup_offsets(&report);
    assert_eq!(offsets.len(), 1, "one row for the repeat: {offsets:?}");
}

/// A top-level variable's induced getter and a written getter of the same
/// name share `definedGetters` — the written one reports.
#[test]
fn unit_field_then_getter_reports() {
    let report = report_ops("var f = 1;\nint get f => 0;\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'f' is already defined.".to_string()],
        "a top-level getter collides with the variable's induced getter"
    );
}

/// A top-level getter/setter pair is legal: `definedGetters` and
/// `definedSetters` are separate maps.
#[test]
fn unit_getter_setter_pair_stays_silent() {
    let report = report_ops("int get g => 0;\nset g(int v) {}\n");
    assert_no_dup(&report);
}

/// Two top-level setters collide under the `name=` lookup spelling the
/// official renders for a setter element.
#[test]
fn unit_setter_repeat_reports_the_lookup_spelling() {
    let report = report_ops("set f(int v) {}\nset f(int v) {}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'f=' is already defined.".to_string()],
        "a repeated setter reports under its `name=` spelling"
    );
}

/// A `late final` top-level variable without an initializer still induces a
/// setter, so a following same-named setter collides on `f=`.
#[test]
fn unit_late_final_without_initializer_claims_its_setter() {
    let report = report_ops("late final int f;\nset f(int v) {}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'f=' is already defined.".to_string()],
        "the induced setter claim collides"
    );
}

/// Two same-named type aliases collide in the unit scope.
#[test]
fn unit_typedef_repeat_reports_the_second() {
    let report = report_ops("typedef A = int;\ntypedef A = String;\n");
    assert_eq!(dup_messages(&report), vec!["The name 'A' is already defined.".to_string()]);
}

/// A repeated top-level mixin/extension-type name collides like a class.
#[test]
fn unit_extension_type_repeat_reports_the_second() {
    let report = report_ops("extension type A(int it) {}\nextension type A(int it) {}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'A' is already defined.".to_string()]);
}

// —— member scope ——

/// Two same-named instance fields: the second field's induced accessor claims
/// collide with the first's pair. The oracle renders ONE row (its diagnostic
/// pipeline folds the getter and setter claims at the same span).
#[test]
fn member_field_repeat_reports_one_row() {
    let report = report_ops("class C {\n  int foo = 0;\n  int foo = 0;\n}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'foo' is already defined.".to_string()],
        "the repeated field reports once"
    );
}

/// A `final` field induces a getter only, so a following written getter of
/// the same name collides while a following setter does not.
#[test]
fn member_final_field_then_getter_reports() {
    let report = report_ops("class C {\n  final int foo = 0;\n  int get foo => 0;\n}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'foo' is already defined.".to_string()]);
}

#[test]
fn member_final_field_then_setter_stays_silent() {
    let report = report_ops("class C {\n  final int foo = 0;\n  set foo(int v) {}\n}\n");
    assert_no_dup(&report);
}

/// A method name collides with a written setter of the same bare name — the
/// member scope keys every fragment by its name token's lexeme.
#[test]
fn member_method_then_setter_reports() {
    let report = report_ops("class C {\n  void foo() {}\n  set foo(int v) {}\n}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'foo' is already defined.".to_string()]);
}

/// Operators collide under the operator's own lexeme.
#[test]
fn member_operator_repeat_reports_under_the_operator() {
    let report = report_ops(
        "class C {\n  int operator +(int o) => 0;\n  int operator +(int o) => 0;\n}\n",
    );
    assert_eq!(dup_messages(&report), vec!["The name '+' is already defined.".to_string()]);
}

/// A member `_` is an ordinary name, not a wildcard: the feature's other-
/// declarations rule leaves top-level and member names "unchanged", so the
/// second `_` member collides (`other_declarations_A01_t07`: a top-level `_`
/// repeat is an error). The member verifier's `isWildcardVariable` exemption
/// never fires for a field or method.
#[test]
fn member_wildcard_repeat_reports() {
    let report = report_ops("class C {\n  int _ = 0;\n  int _ = 1;\n}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name '_' is already defined.".to_string()]
    );
}

/// A repeated top-level `_` declaration collides for the same reason.
#[test]
fn unit_wildcard_repeat_reports() {
    let report = report_ops("const _ = 42;\nfinal _ = 43;\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name '_' is already defined.".to_string()]
    );
}

/// An `augment` member is neither checked nor claimed, so it never collides
/// with the declaration it augments (`member.augmentKeyword != null` skips
/// the constructor arm; `fragment.isAugmentation` bails the identifier check).
/// The sibling direction — a NON-`augment` member declared inside an `augment`
/// block, which the oracle DOES report against the base declaration — is the
/// residual recorded in the header.
#[test]
fn augment_member_stays_silent() {
    let report = report_ops(
        "class A {\n  int foo = 0;\n}\n\
         augment class A {\n  augment abstract int foo;\n}\n",
    );
    assert_no_dup(&report);
}

/// Two separate non-augment containers of the same name are two elements
/// with two member scopes: only the container-name duplicate reports, never a
/// member collision.
#[test]
fn duplicate_container_members_do_not_cross_collide() {
    let report = report_ops("class C {\n  int x = 0;\n}\nclass C {\n  int x = 0;\n}\n");
    assert_eq!(
        dup_messages(&report),
        vec!["The name 'C' is already defined.".to_string()],
        "only the class name collides"
    );
}

// —— formal-parameter scope ——

/// Two plain formals of the same name in one list collide.
#[test]
fn params_plain_repeat_reports() {
    let report = report_ops("void f(int a, int a) {}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'a' is already defined.".to_string()]);
}

/// The formal scope is shared across the positional and the named section: a
/// named `a` collides with a positional `a`.
#[test]
fn params_named_collides_with_positional() {
    let report = report_ops("void f(int a, {int? a}) {}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'a' is already defined.".to_string()]);
}

/// An optional positional and a required positional of the same name share
/// the scope too.
#[test]
fn params_optional_collides_with_required() {
    let report = report_ops("void f(int a, [int a = 0]) {}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'a' is already defined.".to_string()]);
}

/// A wildcard formal is exempt under the wildcard-variables feature.
#[test]
fn params_wildcard_repeat_stays_silent() {
    let report = report_ops("void f(int _, int _) {}\n");
    assert_no_dup(&report);
}

/// The same fixture under a pre-3.7 language version keeps `_` an ordinary
/// name, so the repeat collides (`_DuplicateIdentifierScope.isWildcard` is
/// feature-gated).
#[test]
fn params_wildcard_repeat_reports_pre_wildcards() {
    let report = report_ops_wildcards("void f(int _, int _) {}\n", false);
    assert_eq!(dup_messages(&report), vec!["The name '_' is already defined.".to_string()]);
}

/// Type parameters are an ordinary duplicate scope.
#[test]
fn type_params_repeat_reports() {
    let report = report_ops("class C<T, T> {}\n");
    assert_eq!(dup_messages(&report), vec!["The name 'T' is already defined.".to_string()]);
}

/// A type-parameter wildcard is exempt under the feature and reports without
/// it.
#[test]
fn type_params_wildcard_follows_the_feature() {
    let silent = report_ops("class C<_, _> {}\n");
    assert_no_dup(&silent);
    let reporting = report_ops_wildcards("class C<_, _> {}\n", false);
    assert_eq!(
        dup_messages(&reporting),
        vec!["The name '_' is already defined.".to_string()]
    );
}

// —— silent negatives: scopes never leak ——

/// A nested block may shadow an enclosing local — separate statement lists,
/// separate scopes.
#[test]
fn nested_block_shadow_stays_silent() {
    let report = report_ops("void g() {\n  var a = 1;\n  {\n    var a = 2;\n  }\n}\n");
    assert_no_dup(&report);
}

/// A parameter and a same-named local in the body are different scopes.
#[test]
fn parameter_and_body_local_stay_silent() {
    let report = report_ops("void g(int a) {\n  var a = 2;\n}\n");
    assert_no_dup(&report);
}

/// A member and a same-named local never collide.
#[test]
fn member_and_local_stay_silent() {
    let report = report_ops("class C {\n  int a = 0;\n  void g() {\n    var a = 1;\n  }\n}\n");
    assert_no_dup(&report);
}

/// A top-level name and a same-named class member never collide.
#[test]
fn top_level_and_member_stay_silent() {
    let report = report_ops("int a = 0;\nclass C {\n  int a = 0;\n}\n");
    assert_no_dup(&report);
}
