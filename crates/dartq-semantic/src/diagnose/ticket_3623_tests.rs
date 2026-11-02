//! The #3623 pins (UNDEF): two fabrication families on the
//! `undefined_getter` / `undefined_identifier` faces that this ticket
//! traced to decisions inside the row-resolution pass.
//!
//! # What the cluster pass found
//!
//! Census-11 (`target/conformance/census-20261102-122042.report.tsv`) over
//! the two faces carries 17 `Undefined name '_'.` EXTRA rows that all sit
//! in wildcard-variables corpus cases whose enclosing container DECLARES a
//! member named `_` (`var _ = 2;`, `static const _ = 42;`). The member
//! face used to report those reads as a blanket "a wildcard `_` never
//! resolves" rule; the oracle is silent there — upstream skips `_`
//! declarations only in the LEXICAL scopes (`LocalScope` /
//! `FormalParameterScope` / `TypeParameterScope`, scope.dart:108-125,
//! 547-557, 862-872), while the class-member tables carry `_` like any
//! private name, so the bare read reaching the member face resolves to the
//! real member. The two largest members of that family
//! (`non_binding/class_members_test`, `non_binding/
//! top_level_typedef_test`'s `CConst`) are pinned below.
//!
//! Beside it, the `new`-selector face rendered every read/write miss
//! against `InvalidType`. Upstream reports against the receiver's RESOLVED
//! static type (`resolveProperty` hands `receiverType` down;
//! type_property_resolver.dart:78-83 short-circuits `name == 'new'` before
//! the dynamic-bounded silence), so `d.new` over a `dynamic` variable
//! names 'dynamic' while only a receiver whose own resolution failed
//! keeps `InvalidType`. Both are pinned.
//!
//! Oracle provenance: `dart analyze` 3.13.7 (the pinned SDK) — the
//! wildcard member shapes were probed against that binary for this ticket
//! (a class field `_` plus a bare read reports nothing, instance and
//! static-const alike), and the `d.new` / `Undefined.new` pair matches the
//! corpus expectations verbatim.

use super::test_rig::{code_names, report_ops};

/// The messages of `report`'s rows carrying `code`, in emission order.
fn rows_of(report: &super::FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// Every `undefined_identifier` message of `report`, in emission order.
fn identifier_rows(report: &super::FileReport) -> Vec<String> {
    rows_of(report, "undefined_identifier")
}

/// Every `undefined_getter` message of `report`, in emission order.
fn getter_rows(report: &super::FileReport) -> Vec<String> {
    rows_of(report, "undefined_getter")
}

/// An instance field `_` binds the bare spelling: the wildcard locals,
/// type parameters, and pattern binders around the read declare nothing
/// (scope.dart's lexical-scope skips), so the read falls through to the
/// member face and resolves to the field — silently, for reads, writes,
/// and the wildcard formal's default alike (the `non_binding/
/// class_members_test` `InstanceMember` shape, probed 3.13.7).
#[test]
fn wildcard_bare_read_resolves_to_instance_underscore_field() {
    let report = report_ops(
        "class InstanceMember {\n  var _ = 2;\n  void member<_, _>() {\n    int _ = _;\n    int _ = 2;\n    var (_, _) = (3, '4');\n    _;\n    _ = 4;\n    int foo<_>([int _ = 5]) => _;\n    foo();\n  }\n}\n",
    );
    assert!(
        identifier_rows(&report).is_empty(),
        "{:?}",
        identifier_rows(&report)
    );
    assert!(
        getter_rows(&report).is_empty(),
        "{:?}",
        getter_rows(&report)
    );
}

/// The static twin (`non_binding/class_members_test` `StaticMember`): a
/// `static const _` member resolves the bare read — including inside a
/// const initializer's RHS — exactly like the instance field.
#[test]
fn wildcard_bare_read_resolves_to_static_underscore_member() {
    let report = report_ops(
        "class StaticMember {\n  static const _ = 2;\n  void member<_, _>() {\n    int _ = _;\n    const _ = _ - 1;\n    _;\n  }\n}\n",
    );
    assert!(
        identifier_rows(&report).is_empty(),
        "{:?}",
        identifier_rows(&report)
    );
    assert!(
        getter_rows(&report).is_empty(),
        "{:?}",
        getter_rows(&report)
    );
}

/// The negative that keeps the miss face honest: with NO `_` binding at
/// any scope level the bare read still reports — the removed member-face
/// rule must not take the lexical-chain miss with it (the
/// `CConst`-without-a-member shape; the `class C<_>` and parameter twins
/// are pinned in the #3297 file).
#[test]
fn wildcard_bare_read_without_any_binding_still_reports() {
    let report = report_ops("class C {\n  void m() {\n    _;\n  }\n}\n");
    assert_eq!(
        identifier_rows(&report),
        vec!["Undefined name '_'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// A receiver whose static type is `dynamic` names 'dynamic' in the
/// `new`-selector miss — the oracle reports against the resolved
/// receiverType (type_property_resolver.dart's `name == 'new'` arm runs
/// before the dynamic-bounded silence), never `InvalidType`.
#[test]
fn new_selector_miss_names_the_dynamic_receiver_type() {
    let report = report_ops("f(dynamic d) {\n  d.new;\n}\n");
    assert_eq!(
        getter_rows(&report),
        vec!["The getter 'new' isn't defined for the type 'dynamic'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// The unresolved-receiver twin keeps `InvalidType`: only a receiver whose
/// own resolution failed reads as the oracle's invalid type, so the
/// display's fallback posture is unchanged for that face.
#[test]
fn new_selector_miss_keeps_invalid_type_for_unresolved_receiver() {
    let report = report_ops("f() {\n  var x = Undefined.new;\n}\n");
    assert_eq!(
        getter_rows(&report),
        vec!["The getter 'new' isn't defined for the type 'InvalidType'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}
