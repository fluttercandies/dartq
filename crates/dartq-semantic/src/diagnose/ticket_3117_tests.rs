//! The #3117 pins: the member-level `duplicate_definition` combination
//! matrix of `member_duplicate_definition_verifier.dart` — the
//! `_checkDuplicateIdentifier` scope entries (:297-346), the
//! induced-accessor claim set of a field declaration (getter always,
//! setter only while `fieldElement.setter` exists — `final` and `const`
//! fields declare none, `late final` declares one only while the
//! declarator carries no initializer, upstream :118-149), and the
//! per-ELEMENT scoping of the duplicate map
//! (`_getElementContext(firstFragment)` — the scopes key on the
//! declaration's own element, never on the spelling).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/dd3116
//! combination battery (field × getter × setter × method × constructor
//! pairs, static × instance, enum/mixin/extension/extension-type
//! containers, `==`/`[]`/`-`/`hashCode` members, wildcard `_` members).
//!
//! Three collector-side gaps this file pins:
//!
//! 1. A `const` field group is final for the induced-accessor face — the
//!    oracle's `FieldFragmentImpl.isFinal` covers `const`, so `static
//!    const int x` claims only the getter and a same-scope `static set x`
//!    merges into the getter/setter pair with NO diagnostic (upstream
//!    :320-334 pair merge). The collector's group finality matched only
//!    the `final` keyword and wrongly claimed a setter.
//! 2. A declaration whose two induced-accessor claims both collide
//!    produces two IDENTICAL upstream diagnostics at the same name token
//!    (the getter check and the setter check each report — `int x; int
//!    x;` is one emitted row). The reporter's identical-row absorption
//!    keeps only the first (upstream emits the `_checkDuplicateIdentifier`
//!    pair per variable; the pipeline dedupes the same code/span/message
//!    row) — the collector now records a back-to-back identical
//!    (name, range) repeat once.
//! 3. The duplicate scope lives on the ELEMENT, not the name
//!    (upstream `_InstanceElementContext` keyed on
//!    `fragment.element.firstFragment`, :49-54/:657-677): a second
//!    non-augment container of the same name is a fresh element with a
//!    fresh scope — `class C { int x; } class C { int x; }` reports the
//!    class-name duplicate only, never a member collision — while an
//!    `augment` member never claims at all (:307-309). The collector's
//!    name-keyed cross-declaration scope merge mis-served both.

use super::test_rig::{assert_silent, report_ops, report_ops_wildcards};

fn dup_messages(report: &crate::diagnose::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| d.message.to_string())
        .collect()
}

fn dup_rows(report: &crate::diagnose::FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

// —— the identical-row absorption: two induced claims, one emitted row ——

/// A non-final field declares its induced getter AND setter into the
/// scope (upstream :128-149); the second field's two
/// `_checkDuplicateIdentifier` calls each collide, producing two
/// same-span rows the reporter dedupes — the oracle emits exactly one
/// `duplicate_definition` at the second name.
#[test]
fn field_field_repeat_reports_once() {
    let report = report_ops(
        "class C {\n\
         \x20 int x = 0;\n\
         \x20 int x = 1;\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "second field's getter+setter claims collapse to one row: {rows:?}"
    );
    assert_eq!(
        rows[0].2,
        "The name 'x' is already defined.".to_string(),
        "message names the field: {rows:?}"
    );
}

/// The same absorption inside ONE declaration list — `int x = 0, x = 1;`
/// is a single upstream row at the second declarator's name.
#[test]
fn field_declarator_list_repeat_reports_once() {
    let report = report_ops(
        "class C {\n\
         \x20 int x = 0, x = 1;\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "within-list field repeat emits once: {rows:?}"
    );
}

/// A method then a field of the same name: the field's getter claim and
/// setter claim each collide with the method entry — one emitted row at
/// the field's name.
#[test]
fn method_then_field_reports_once() {
    let report = report_ops(
        "class C {\n\
         \x20 void m() {}\n\
         \x20 int m = 0;\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(rows.len(), 1, "method then field emits one row: {rows:?}");
}

/// A setter then a getter merges into the pair (upstream :320-334); a
/// field after the pair collides on BOTH claims — one emitted row.
#[test]
fn field_after_getter_setter_pair_reports_once() {
    let report = report_ops(
        "class C {\n\
         \x20 int get x => 0;\n\
         \x20 set x(int v) {}\n\
         \x20 int x = 2;\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "field after the pair emits one row: {rows:?}"
    );
}

/// The wildcard `_` field is NOT a wildcard variable upstream (the
/// member fragment's `isWildcardVariable` is false — `_` members are
/// ordinary names) and reports the same single absorbed row.
#[test]
fn wildcard_field_repeat_reports_once() {
    let report = report_ops(
        "class C {\n\
         \x20 int _ = 0;\n\
         \x20 int _ = 1;\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "wildcard member repeat emits one row: {rows:?}"
    );
    assert_eq!(
        rows[0].2,
        "The name '_' is already defined.".to_string(),
        "message names the wildcard member: {rows:?}"
    );
}

/// Absorption is span-keyed, not name-keyed: two DISTINCT same-name
/// members at different tokens each emit (the getter after the pair and
/// the setter after the pair are two rows, never one).
#[test]
fn getter_setter_after_occupied_pair_each_report() {
    let report = report_ops(
        "class C {\n\
         \x20 int m = 0;\n\
         \x20 int get m => 0;\n\
         \x20 set m(int v) {}\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        2,
        "the getter claim and the setter claim each report: {rows:?}"
    );
    assert!(
        rows[0].0 != rows[1].0,
        "the two rows anchor at distinct name tokens: {rows:?}"
    );
}

// —— the `const` group's getter-only claim ——

/// `static const` declares no setter (the const field's element is final
/// upstream — `FieldFragmentImpl.isFinal`), so a same-scope `static set`
/// merges into the pair silently.
#[test]
fn static_const_field_then_static_setter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 static const int x = 0;\n\
         \x20 static set x(int v) {}\n\
         }\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "const field + setter merges the pair"
    );
}

/// A same-scope `static get` after the const field still collides on the
/// getter claim — the getter-only claim is real.
#[test]
fn static_const_field_then_static_getter_reports() {
    let report = report_ops(
        "class C {\n\
         \x20 static const int x = 0;\n\
         \x20 static int get x => 1;\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "const field's getter claim collides: {rows:?}"
    );
}

/// An instance `const` field is likewise getter-only: an instance
/// `set` merges (the `static`/`instance` split is
/// `conflicting_static_and_instance`'s own arm — the const claim set is
/// what this pin locks).
#[test]
fn const_instance_field_then_setter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 final int x = 0;\n\
         \x20 set x(int v) {}\n\
         }\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "final field + setter merges the pair"
    );
}

// —— `late final` declares a setter only while uninitialized ——

/// `late final int x;` (no initializer) declares its setter — a
/// same-scope `set x` collides on the SECOND setter claim.
#[test]
fn late_final_uninitialized_then_setter_reports() {
    let report = report_ops(
        "class C {\n\
         \x20 late final int x;\n\
         \x20 set x(int v) {}\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "uninitialized late-final + setter collides: {rows:?}"
    );
}

/// `late final int x = 0;` (initialized) declares no setter — the same
/// `set x` merges silently.
#[test]
fn late_final_initialized_then_setter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 late final int x = 0;\n\
         \x20 set x(int v) {}\n\
         }\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "initialized late-final + setter merges"
    );
}

// —— the scope lives on the element, not the name ——

/// A duplicate-named class declares a fresh element upstream
/// (`firstFragment` keys the scope): the second class's own `int x`
/// never collides with the first class's — only the class-name
/// `duplicate_definition` reports.
#[test]
fn duplicate_named_class_member_scope_is_fresh() {
    let report = report_ops(
        "class C { int x = 0; }\n\
         class C { int x = 0; }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(rows.len(), 1, "only the class name duplicates: {rows:?}");
    assert_eq!(
        rows[0].2,
        "The name 'C' is already defined.".to_string(),
        "the reported duplicate is the class name: {rows:?}"
    );
}

/// The fresh scope still catches the second declaration's own
/// member-internal collisions — `class C { int x; } class C { int x;
/// void x() {} }` reports the class name AND the second class's own
/// `x` method-against-field collision.
#[test]
fn duplicate_named_class_own_member_repeat_reports() {
    let report = report_ops(
        "class C { int x = 0; }\n\
         class C { int x = 0; void x() {} }\n",
    );
    let messages = dup_messages(&report);
    assert_eq!(
        messages.len(),
        2,
        "class-name dup + second class's own member dup: {messages:?}"
    );
    assert!(
        messages.contains(&"The name 'C' is already defined.".to_string()),
        "class-name dup present: {messages:?}"
    );
    assert!(
        messages.contains(&"The name 'x' is already defined.".to_string()),
        "second class's own member dup present: {messages:?}"
    );
}

/// A getter in the first class and a setter in the second (same name)
/// never merge across the element boundary — the scope pair is
/// per-element (upstream :320-334 entries never cross `firstFragment`s).
#[test]
fn getter_setter_across_duplicate_classes_never_merge() {
    let report = report_ops(
        "class C { int get x => 0; }\n\
         class C { set x(int v) {} }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(rows.len(), 1, "only the class-name dup reports: {rows:?}");
    assert_eq!(
        rows[0].2,
        "The name 'C' is already defined.".to_string(),
        "the lone row is the class name: {rows:?}"
    );
}

// —— the top-level `_reportedTokens` gate ——

/// A second top-level variable's GETTER check reports at its name token
/// and absorbs the variable's own SETTER check
/// (`_checkDuplicateFragmentIdentifier`'s `_reportedTokens` — upstream
/// duplicate_definition_verifier.dart:308-352): `int x; int x;` emits
/// `x` only, and the second variable's induced `x=` never joins
/// `definedSetters`.
#[test]
fn top_var_repeat_reports_getter_only() {
    let report = report_ops(
        "int x = 0;\n\
         int x = 1;\n",
    );
    let messages = dup_messages(&report);
    assert_eq!(
        messages,
        vec!["The name 'x' is already defined.".to_string()],
        "second var reports the getter face only: {messages:?}"
    );
}

/// The absorbed setter check also keeps `definedSetters` clean for the
/// NEXT declaration: `int x; int x; set x;` — the second variable never
/// claimed `x=`, so `set x` still collides against the FIRST variable's
/// induced setter and reports `x=`.
#[test]
fn top_var_repeat_then_setter_reports_setter_once() {
    let report = report_ops(
        "int x = 0;\n\
         int x = 1;\n\
         set x(int v) {}\n",
    );
    let messages = dup_messages(&report);
    assert_eq!(
        messages,
        vec![
            "The name 'x' is already defined.".to_string(),
            "The name 'x=' is already defined.".to_string(),
        ],
        "getter dup then the setter's own definedSetters dup: {messages:?}"
    );
}

/// A variable after a real setter claims its getter cleanly (the setter
/// only ever joined `definedSetters`), then its own setter check
/// collides on `x=` — one row at the variable's name token, named by
/// the setter lookup spelling (upstream `lookupName` renders `x=`).
#[test]
fn top_setter_then_var_reports_setter_spelling() {
    let report = report_ops(
        "set x(int v) {}\n\
         int x = 0;\n",
    );
    let messages = dup_messages(&report);
    assert_eq!(
        messages,
        vec!["The name 'x=' is already defined.".to_string()],
        "the variable's setter claim reports the x= face: {messages:?}"
    );
}

/// Three repeats: each later variable reports its getter at its own
/// token (the gate is per token, not per name).
#[test]
fn top_var_triple_reports_two_getter_rows() {
    let report = report_ops(
        "int x = 0;\n\
         int x = 1;\n\
         int x = 2;\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        2,
        "the second and third variables each report once: {rows:?}"
    );
    assert!(
        rows[0].0 != rows[1].0,
        "the two rows anchor at distinct name tokens: {rows:?}"
    );
}

/// A top-level `_` variable is an ordinary name under checkUnit (the
/// wildcard gate lives on the statement scopes, never the unit's
/// `_checkDuplicateFragmentIdentifier`) — the repeat reports `_` once
/// (getter face only, same `_reportedTokens` absorption).
#[test]
fn top_wildcard_var_repeat_reports_getter_only() {
    let report = report_ops_wildcards(
        "int _ = 0;\n\
         int _ = 1;\n",
        /* wildcards enabled */ true,
    );
    let messages = dup_messages(&report);
    assert_eq!(
        messages,
        vec!["The name '_' is already defined.".to_string()],
        "top-level `_` repeat reports once: {messages:?}"
    );
}

// —— matrix reassertions the pins keep green ——

/// The getter/setter pair itself stays silent (the transition's
/// `_ScopeEntryGetterSetterPair` arm, upstream :320-331).
#[test]
fn getter_then_setter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 int get x => 0;\n\
         \x20 set x(int v) {}\n\
         }\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "getter/setter pair merges"
    );
}

/// The reverse order merges the same way (`SetterElement` previous
/// accepts a `GetterFragment`, upstream :326-331).
#[test]
fn setter_then_getter_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 set x(int v) {}\n\
         \x20 int get x => 0;\n\
         }\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "setter/getter pair merges"
    );
}

/// A unary/binary `operator -` pair is not a redeclaration (the
/// 0-arity/1-arity split keeps two distinct members) — the minus
/// exemption keeps this whole file silent.
#[test]
fn operator_minus_arity_split_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 int operator -() => 0;\n\
         \x20 int operator -(int o) => 0;\n\
         }\n",
    );
    assert!(
        dup_messages(&report).is_empty(),
        "unary/binary minus split is legal"
    );
}

/// An unnamed extension's members claim in their own scope (the
/// container carries no finished-scope name): `extension on int { void
/// m() {} void m() {} }` still reports the method repeat.
#[test]
fn unnamed_extension_member_repeat_reports() {
    let report = report_ops(
        "extension on int {\n\
         \x20 void m() {}\n\
         \x20 void m() {}\n\
         }\n",
    );
    let rows = dup_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "unnamed extension member repeat reports: {rows:?}"
    );
}
