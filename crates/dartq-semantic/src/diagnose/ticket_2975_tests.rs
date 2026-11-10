//! The #2975 pins: the `invalid_null_aware_operator` verdict on
//! MEMBER/PROPERTY receivers (`_checkForUnnecessaryNullAware`'s
//! promoted-property arm, upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/error_verifier.dart`). A `?.`/`?..`
//! access whose receiver is a promoted field — `_g?.abs()`,
//! `this._f?.x`, `obj._f?.x` — reports the operator as unnecessary: the
//! receiver's flow-augmented static type is the promoted non-null face.
//!
//! The receiver never rides `null_aware_staged`: that staging gate keys
//! on `local_hits`, which only a bare LOCAL read enters — a
//! `RefTarget::Member` receiver never lands there. The member receiver
//! reaches `null_aware_members`/`member_reads` instead, and the flush's
//! promoted-property pass re-decides it against
//! [`FlowFact::PromotedPropertyRead`] through
//! `promotable_field_substitution` — the same eligibility verdict the
//! condition-face join asks (private, final, non-external,
//! unit-unconflicted, under the field-promotion grant).
//!
//! Oracle provenance: `dart analyze` 3.13.1 — a promoted field's `?.`
//! reports `STATIC_WARNING.INVALID_NULL_AWARE_OPERATOR` at the operator
//! (the `cascaded_field_promotion_*` inference-update-2 cases carry it).

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `if (_g != null) { _g?.toString(); }` — the promoted private final
/// field's `?.` is unnecessary. The implicit-`this` bare read carries the
/// `PromotedPropertyRead` row at `_g`'s own token. (`toString` resolves on
/// the `Object` root every promoted face reaches — the stub `int` declares
/// no named method, so an `abs()` tail would surface an unrelated
/// `undefined_method`.)
#[test]
fn implicit_this_promoted_field_null_aware_reports() {
    let report = report_ops(
        "class C { final int? _g = 0; void m() { if (_g != null) { _g?.toString(); } } }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `this._f?.x` — an explicit `this.` member receiver promotes the same
/// way; the `PromotedPropertyRead` anchor is the `_f` member token (the
/// receiver read's trailing member), re-decided through `member_reads`'s
/// end-keyed hop.
#[test]
fn explicit_this_promoted_field_null_aware_reports() {
    let report = report_ops(
        "class C { final int? _f = 0; void m() { if (_f != null) { this._f?.toString(); } } }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// `obj._f?.x` — the promoted field of ANOTHER object receiver: the
/// `obj._f` member read anchors `PromotedPropertyRead` at `_f`'s member
/// token; the `?.` on it is unnecessary.
#[test]
fn foreign_receiver_promoted_field_null_aware_reports() {
    let report = report_ops(
        "class C { final int? _f = 0; }\n\
         void m(C c) { if (c._f != null) { c._f?.toString(); } }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// A promoted field's cascade `?..` — `c._f` promoted, then
/// `c._f?..m()` — reports at the `?..` operator (the cascade kind's
/// `..` replacement text).
#[test]
fn cascade_section_promoted_field_null_aware_reports() {
    let report = report_ops(
        "class C { final C? _f = null; void m() {} }\n\
         void top(C c) { if (c._f != null) { c._f?..m(); } }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["invalid_null_aware_operator"],
        "{:?}",
        rows(&report)
    );
}

/// The negative pin: WITHOUT the null check the field stays `int?`, so
/// `_g?.abs()` is a legal null-aware access — no `invalid_null_aware`
/// row. The promoted-property arm must not fabricate a verdict the flow
/// never evidenced.
#[test]
fn unpromoted_field_null_aware_stays_silent() {
    let report = report_ops("class C { final int? _g = 0; void m() { _g?.toString(); } }\n");
    assert!(
        !code_names(&report).contains(&"invalid_null_aware_operator"),
        "{report:?}"
    );
}

/// A public (non-private) field is never field-promotable — the
/// `isPropertyPromotable` gate keeps `_g`'s sibling `g` from the
/// promoted-property arm even under `!= null`. (Upstream reports
/// `invalid_null_aware_operator` here only when the LOCAL promotion
/// stands; a public member name answers `NotPrivate`, so the property
/// re-decision stands down.)
#[test]
fn public_field_receiver_reports_nothing_via_property_arm() {
    let report = report_ops(
        "class C { final int? g = 0; void m() { if (g != null) { g?.toString(); } } }\n",
    );
    assert!(
        !code_names(&report).contains(&"invalid_null_aware_operator"),
        "{report:?}"
    );
}

/// A local receiver promoted through `!= null` still re-decides through
/// the `null_aware_staged` LOCAL path — the member pass must not
/// double-report it.
#[test]
fn local_receiver_still_reports_once() {
    let report = report_ops("class C { void m(int? x) { if (x != null) { x?.toString(); } } }\n");
    let names = code_names(&report);
    assert_eq!(
        names
            .iter()
            .filter(|n| **n == "invalid_null_aware_operator")
            .count(),
        1,
        "{names:?}"
    );
}
