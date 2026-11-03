//! #3687 pins — promotion completeness on member faces (census shape 4.1 /
//! 4.2 / 4.4).
//!
//! Census-12 family 4 (`unchecked_use_of_nullable_value`) carries 264
//! missing and 391 extra rows corpus-wide; 351 of the extras anchor on a
//! real source construct (the rest are `//^` expectation-marker lines the
//! census counts as rows). 371 of them are the over-emit side of one root:
//! a private final field read under its own null guard through a receiver
//! other than a plain local.
//!
//! The oracle's rule (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/field_promotability.dart` — "A field
//! is considered promotable if all of the following are true: it is final,
//! its name is private, the library doesn't contain any non-final fields,
//! concrete getters, or `noSuchMethod` getters with the same name"), read
//! through the flow layer's `PropertyTarget` identity
//! (`flow_analysis.dart` — `ExpressionPropertyTarget` for `c._f`,
//! `ThisPropertyTarget` for `_f`/`this._f`, `SuperPropertyTarget` for
//! `super._f`): **any stable expression receiver promotes**, not only
//! `this`. `if (c._foo != null) { c._foo.length; }` is therefore silent.
//!
//! The three sub-roots the census names:
//!
//! | shape | construct | extras |
//! |---|---|---|
//! | 4.1 | property access on a promoted non-`this` receiver | 157 |
//! | 4.2 | the same root on invocations | 151 |
//! | 4.4 | function invocation through a non-`this` private field | 63 |
//!
//! Every pin below is a silence control (the fix) or a genuine-error
//! control (a face that must keep reporting). The silence controls carry
//! the weight; the error controls fence the fix so it cannot degenerate
//! into "never report".

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};

/// Every `unchecked_use_of_nullable_value` message in report order.
fn unchecked_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "unchecked_use_of_nullable_value")
        .map(|row| row.message.to_string())
        .collect()
}

/// The single `unchecked_use_of_nullable_value` row's UTF-16 offset — the
/// exact-anchor assertion the genuine-error controls make. Panics with the
/// full row list when the report does not carry exactly one such row.
#[track_caller]
fn single_unchecked_offset(report: &FileReport) -> u32 {
    let hits: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "unchecked_use_of_nullable_value")
        .map(|row| row.range.offset)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one unchecked row, got {hits:?}: {:#?}",
        rows(report)
    );
    hits[0]
}

/// The UTF-16 offset of `needle` inside `source` — the span space every
/// `SourceRange` lives in.
#[track_caller]
fn offset_of(source: &str, needle: &str) -> u32 {
    let byte = source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture does not contain {needle:?}"));
    u32::try_from(source[..byte].encode_utf16().count()).expect("fixture offset fits u32")
}

/// The member-bearing receiver class every fixture uses.
const N_CLASS: &str = "class N { int get length => 0; void m() {} }\n";

// —— shape 4.1: property access on a promoted non-`this` receiver ——————

/// Silence control: a private final field read through a **non-`this`**
/// receiver under its own `!= null` guard resolves on the promoted static
/// type — the `.length` access is silent.
///
/// Oracle (`FieldPromotionTest/test_class_field_read_propertyAccess`,
/// `Records/type_promotion_A02_t01`): no rows.
#[test]
fn non_this_private_field_property_access_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final N? _n;\n  C(this._n);\n}}\n\
         void f(C c) {{\n  if (c._n != null) {{\n    c._n.length;\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Genuine-error control with the exact anchor: a **public** field is not
/// promotable (the oracle's `isNotPrivate` arm), so the use under the guard
/// still reports — at the unsafe use's member token.
///
/// Oracle (`FieldPromotionTest/test_class_field_notPrivate`): one row.
#[test]
fn non_this_public_field_read_still_reports() {
    let body = "class C {\n  final N? n;\n  C(this.n);\n}\n\
                void f(C c) {\n  if (c.n != null) {\n    c.n.m();\n  }\n}\n";
    let report = report_ops(&format!("{N_CLASS}{body}"));
    let anchor = offset_of(&format!("{CORE_PRELUDE}{N_CLASS}{body}"), "c.n.m();")
        + u32::try_from("c.n.".len()).unwrap();
    assert_eq!(
        single_unchecked_offset(&report),
        anchor,
        "the row anchors on the unsafe use's member token: {:#?}",
        rows(&report)
    );
}

// —— shape 4.2: the same root on invocations ——————————————————————

/// Silence control: a private final field invoked through a **non-`this`**
/// receiver under its own `!= null` guard is silent.
///
/// Oracle (`FieldPromotionTest/test_class_field_invocation_prefixedIdentifier_nullability`):
/// no rows.
#[test]
fn non_this_private_field_invocation_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final N? _n;\n  C(this._n);\n}}\n\
         void f(C c) {{\n  if (c._n != null) {{\n    c._n.m();\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Genuine-error control: a **private getter** is not promotable
/// (`addGetter` answers `isNotField`), so the guarded invocation still
/// reports — at the unsafe use's member token.
///
/// Oracle (`FieldPromotionTest/test_class_getter_read`): one row.
#[test]
fn non_this_private_getter_read_still_reports() {
    let body = "class C {\n  N? get _n => null;\n}\n\
                void f(C c) {\n  if (c._n != null) {\n    c._n.m();\n  }\n}\n";
    let report = report_ops(&format!("{N_CLASS}{body}"));
    let anchor = offset_of(&format!("{CORE_PRELUDE}{N_CLASS}{body}"), "c._n.m();")
        + u32::try_from("c._n.".len()).unwrap();
    assert_eq!(
        single_unchecked_offset(&report),
        anchor,
        "the row anchors on the unsafe use's member token: {:#?}",
        rows(&report)
    );
}

/// Genuine-error control: a private final field with **no** null guard is
/// not promoted, so the unguarded invocation still reports.
///
/// Oracle (probe 3.13.7: `c._n.m()` outside any guard reports).
#[test]
fn non_this_private_field_without_guard_still_reports() {
    let body = "class C {\n  final N? _n;\n  C(this._n);\n}\n\
                void f(C c) {\n  c._n.m();\n}\n";
    let report = report_ops(&format!("{N_CLASS}{body}"));
    let anchor = offset_of(&format!("{CORE_PRELUDE}{N_CLASS}{body}"), "c._n.m();")
        + u32::try_from("c._n.".len()).unwrap();
    assert_eq!(
        single_unchecked_offset(&report),
        anchor,
        "the unguarded row anchors on the member token: {:#?}",
        rows(&report)
    );
}

// —— shape 4.4: function invocation through a non-`this` private field ——

/// Silence control: a private final **function-typed** field invoked
/// through a non-`this` receiver under its own null guard is silent.
///
/// Oracle (`FieldPromotionTest/test_class_field_invocation_prefixedIdentifier_nullability`
/// — the `final void Function()? _foo` fixture): no rows.
#[test]
fn non_this_private_function_field_call_promotes() {
    let report = report_ops(
        "class C {\n  final void Function()? _foo;\n  C(this._foo);\n}\n\
         void f(C c) {\n  if (c._foo != null) {\n    c._foo();\n  }\n}\n",
    );
    assert_silent(&report);
}

/// Genuine-error control: the same function-typed field invoked with **no**
/// guard still reports — at the callee's member token.
///
/// Oracle (probe 3.13.7: `c._foo()` outside a guard is the function
/// variant of the shared code).
#[test]
fn non_this_private_function_field_unguarded_reports() {
    let body = "class C {\n  final void Function()? _foo;\n  C(this._foo);\n}\n\
                void f(C c) {\n  c._foo();\n}\n";
    let report = report_ops(body);
    assert_eq!(
        single_unchecked_offset(&report),
        offset_of(&format!("{CORE_PRELUDE}{body}"), "c._foo();")
            + u32::try_from("c.".len()).unwrap(),
        "the unguarded callee row anchors on the member token: {:#?}",
        rows(&report)
    );
}

// —— the guard forms the census names ————————————————————————————

/// Guard-form control: an **early-return guard** (`if (c._n == null)
/// return;`) promotes for the remainder of the block exactly like the
/// nested `if` form — the flow layer's negation branch is the same
/// promotion (`flow_analysis.dart` `_finishTypeTest`'s inverted arm).
///
/// Oracle: `if (c._n == null) return; c._n.length;` is silent.
#[test]
fn non_this_private_field_early_return_guard_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final N? _n;\n  C(this._n);\n}}\n\
         void f(C c) {{\n  if (c._n == null) {{\n    return;\n  }}\n  c._n.length;\n}}\n"
    ));
    assert_silent(&report);
}

/// Guard-form control: the promotion survives an `&&` conjunct —
/// `if (c._n != null && c._n.length > 0)` reads the RHS operand on the
/// promoted face.
///
/// Oracle: no rows.
#[test]
fn non_this_private_field_conjunct_guard_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final N? _n;\n  C(this._n);\n}}\n\
         void f(C c) {{\n  if (c._n != null && c._n.length > 0) {{\n    c._n.m();\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Guard-form control: a **`!` null assertion** promotes exactly like a
/// null test — `c._n!.length` on the same private final field.
///
/// Oracle: no rows.
#[test]
fn non_this_private_field_null_assert_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final N? _n;\n  C(this._n);\n}}\n\
         void f(C c) {{\n  c._n!.length;\n}}\n"
    ));
    assert_silent(&report);
}

/// Guard-form control (`this` chain): the **implicit-`this`** spelling of
/// the same field promotes off its own guard — the census's
/// `Records/type_promotion_A02_t01` shape, where a record-typed private
/// final field feeds a record-field read.
///
/// Oracle (`Records/type_promotion_A02_t01` lines 21-24): no rows.
#[test]
fn this_private_record_field_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final (N, N?)? _r;\n  C(this._r);\n  void t() {{\n\
         if (_r != null) {{\n      _r.$1.m();\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Guard-form control (bounded generic field type): `T? _t` with
/// `T extends N`, read through a **non-`this`** receiver.
///
/// Oracle: no rows.
#[test]
fn non_this_bounded_generic_field_read_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C<T extends N> {{\n  final T? _t;\n  C(this._t);\n}}\n\
         void f(C<N> c) {{\n  if (c._t != null) {{\n    c._t.length;\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Guard-form control (`super` chain, plain field type): the `super`
/// access form carries its own promotion slot (`SuperPropertyTarget`),
/// independent of `this`'s.
///
/// Oracle (`super_this_distinction_error_test`): a `super._n` guard makes
/// its own reads silent and leaves the `this` chain reporting (the two
/// independent chains; see the `this`-chain pin in `ticket_3633_tests`).
#[test]
fn super_plain_field_read_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base {{\n  final N? _n;\n  Base(this._n);\n}}\n\
         class D extends Base {{\n  D(super._n);\n  void f() {{\n\
         if (super._n != null) {{\n      super._n.length;\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Guard-form control (`super` chain, **instantiated ancestor**): the same
/// `super` guard where the field's declared type is the ancestor's own
/// parameter (`Base<T extends N>`, `D extends Base<N>`).
///
/// The climb hands back an INSTANTIATED ancestor entry — a fresh
/// projection the boundary's origin registration never saw — so this pin
/// fences [`Engine::member_read_resolution`]'s origin lookup against
/// bailing out and losing the promotion.
///
/// Oracle (`super_this_distinction_generic_test` — zero expected rows for
/// the whole fixture): no rows.
#[test]
fn super_bounded_generic_field_read_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base<T extends N> {{\n  final T? _t;\n  Base(this._t);\n}}\n\
         class D extends Base<N> {{\n  D(super._t);\n  void f() {{\n\
         if (super._t != null) {{\n      super._t.length;\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Genuine-error control beside the instantiated-ancestor pin: the same
/// `super` read **unguarded** still reports.
///
/// Oracle (`super_this_distinction_generic_test` encodes the guarded form
/// as silent; an unguarded read of a nullable field reports).
#[test]
fn super_bounded_generic_field_unguarded_reports() {
    let report = report_ops(&format!(
        "{N_CLASS}class Base<T extends N> {{\n  final T? _t;\n  Base(this._t);\n}}\n\
         class D extends Base<N> {{\n  D(super._t);\n  void f() {{\n\
         super._t.length;\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "the unguarded super read must report: {:#?}",
        rows(&report)
    );
}

// —— the adjacent shapes the census rows also land on ——————————————

/// Adjacent control: `extension E<T> on T` applied to a **nullable**
/// receiver. The oracle binds `T := T?` and the extension applies, so
/// `x.m()` on `T? x` is silent. This is the shape `package:expect`'s
/// `extension StaticType<T> on T` produces at every `expectStaticType`
/// call site, which the census counts under this family.
#[test]
fn extension_on_bare_type_parameter_nullable_receiver_is_silent() {
    let report = report_ops(
        "extension StaticType<T> on T { T expectStaticType<R>() => this; }\n\
         void f<T>(T? x) {\n  x.expectStaticType<int>();\n}\n",
    );
    assert_silent(&report);
}

/// The same adjacent control through a private-field receiver — the
/// `super_this_distinction_*` spelling (`_t` is `T?`).
#[test]
fn extension_on_type_parameter_private_field_receiver_is_silent() {
    let report = report_ops(
        "extension StaticType<T> on T { T expectStaticType<R>() => this; }\n\
         class C<T> {\n  final T? _t;\n  C(this._t);\n  void f() {\n\
         _t.expectStaticType<int>();\n  }\n}\n",
    );
    assert_silent(&report);
}
