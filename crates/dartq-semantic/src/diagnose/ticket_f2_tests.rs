//! The #3214 F2 pins: the `unchecked_use_of_nullable_value` verdict on a
//! MEMBER access whose receiver is a promoted PROPERTY read —
//! `c._f.expectStaticType<T>()`, `d._f.field`, `this._f.m()`. The receiver
//! never rode a local promotion (`c` stays `C`, not promoted); the flow's
//! `PromotedPropertyRead` fact stands at the FIELD's own member token, and
//! the use's `receiver_nullable` verdict owes the promoted-property
//! stand-down (`promotable_field_substitution`, the same eligibility gate
//! the #1409 family pins).
//!
//! The fold-time chained latch only arms a member row when its receiver
//! read was ITSELF already latched — a `x._f` read that folded over a
//! plain `x` takes no latch, so `.expectStaticType` over it kept the
//! written nullable face and over-reported. The pass-2 backfill latches
//! every member row whose receiver is a staged promoted-property read
//! (property_element_resolver.dart:2158-2166 — the official resolves the
//! use on the promoted static type).
//!
//! Oracle provenance: `dart analyze` 3.13.7 — a promoted private final
//! field's member use resolves on the promoted face; `int.m` reports
//! `undefined_method` naming `int`, never `unchecked_use_of_nullable_value`
//! (`inference_update_2/field_in_mixin_test.dart`,
//! `do_promote_abstract_getters_test.dart`).

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `if (c._f != null) c._f.m()` — the promoted field's use resolves on
/// `int`; a member `int` does not declare reports `undefined_method`
/// (naming `int`), NOT `unchecked_use_of_nullable_value`. This is the
/// corpus's dominant F2 shape (`field_in_mixin`, `do_promote_abstract_getters`
/// spell it `.expectStaticType` — an unresolved extension here, so a
/// member `int` lacks is the equivalent miss).
#[test]
fn promoted_field_member_use_reports_undefined_not_unchecked() {
    let report = report_ops(
        "class C { final int? _f = 0; }\n\
         void m(C c) { if (c._f != null) { c._f.m(); } }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "unchecked fired on promoted field receiver: {:?}",
        rows(&report)
    );
    assert_eq!(names, vec!["undefined_method"], "{:?}", rows(&report));
}

/// The implicit-`this` form — `if (_f != null) _f.m()` promotes the same
/// `PromotedPropertyRead` at `_f`'s token; the member use stands down.
#[test]
fn implicit_this_promoted_field_member_use_reports_undefined() {
    let report = report_ops(
        "class C { final int? _f = 0; void m() { if (_f != null) { _f.m(); } } }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        rows(&report)
    );
    assert_eq!(names, vec!["undefined_method"], "{:?}", rows(&report));
}

/// A member a promoted `int` DOES declare stays fully silent — the
/// receiver's non-null face reaches the Object root.
#[test]
fn promoted_field_declared_member_stays_silent() {
    let report = report_ops(
        "class C { final int? _f = 0; }\n\
         void m(C c) { if (c._f != null) { c._f.toString(); } }\n",
    );
    assert_silent(&report);
}

/// The negative pin: WITHOUT the null check the field stays `int?`, so
/// `c._f.m()` on the nullable receiver keeps its
/// `unchecked_use_of_nullable_value` verdict — the backfill must not
/// fabricate a stand-down the flow never evidenced.
#[test]
fn unpromoted_field_member_use_keeps_unchecked() {
    let report = report_ops(
        "class C { final int? _f = 0; }\n\
         void m(C c) { c._f.m(); }\n",
    );
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "unpromoted field receiver lost its unchecked verdict: {:?}",
        rows(&report)
    );
}

/// A PUBLIC (non-private) field is never field-promotable — `g`'s member
/// use under `!= null` keeps the nullable-receiver verdict. The
/// `isPropertyPromotable` gate must still reject it through the backfill.
#[test]
fn public_field_member_use_keeps_unchecked() {
    let report = report_ops(
        "class C { final int? g = 0; }\n\
         void m(C c) { if (c.g != null) { c.g.m(); } }\n",
    );
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "public field receiver stood down wrongly: {:?}",
        rows(&report)
    );
}
