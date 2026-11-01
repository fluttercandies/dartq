//! The #3430 pins: a type test's gated candidate must reach the receiver
//! face — the promoted type the flow analysis decides is the type the
//! receiver EXPRESSION is judged at, not only the property slot's
//! internal mark. `x._f is String` resolves a chained `x._f.m` on
//! `String` (upstream `property_element_resolver.dart` resolves the
//! member against the read's flow type; the flow row the read publishes
//! is what `read_type_at` answers).
//!
//! Oracle provenance (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart
//! analyze`, probed 2026-11-01):
//! * `x._f is String` over `final Object? _f` — `x._f.length` is SILENT
//!   (`String.length` resolves), `x._f.isEven` reports
//!   `undefined_getter` NAMING `String` (the promoted face), and
//!   `x._f + "s"` resolves `String.+(String)`;
//! * the same shape under a class body (`this._f`/`_f` is String`) and
//!   under an extension body (`extension E on A { _f is String }`)
//!   promotes identically;
//! * `x.pub is String` over a PUBLIC field never promotes — the oracle
//!   reports `unchecked_use_of_nullable_value` on `x.pub.length` with
//!   the `non_promo_public_field` context. That eligibility verdict is
//!   the semantic layer's `promotable_field_entry` gate: the flow
//!   over-tracks every promotable-SHAPED read (its `PromotedPropertyRead`
//!   already does) and the semantic decides — the pinned cases below
//!   only assert the promotable face.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// `(offset, length)` of `needle`'s first occurrence inside the FIXTURE
/// text, shifted past the prelude the rig mounts ahead of it.
fn at(source: &str, needle: &str) -> (u32, u32) {
    let found = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` missing"));
    (
        found as u32 + CORE_PRELUDE.len() as u32,
        needle.len() as u32,
    )
}

/// The rows of `code` as `(offset, length)`, source-ordered.
fn rows_of(source: &str, code: &str) -> Vec<(u32, u32)> {
    let report = report_ops(source);
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// `x._f is String` promotes the read to `String` — `x._f.length`
/// resolves `String.length`, so no diagnostic stands on the chain.
/// Oracle-verified: silent on 3.13.7.
#[test]
fn private_field_is_test_promotes_the_chained_read() {
    let source = "\
class A {\n\
\x20 final Object? _f;\n\
\x20 A(this._f);\n\
}\n\
void g(A x) {\n\
\x20 if (x._f is String) {\n\
\x20   x._f.length;\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "unexpected rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|row| (row.code.name.to_string(), row.range.offset, row.range.length))
            .collect::<Vec<_>>()
    );
}

/// The promoted face is the member-resolution type: `x._f.isEven` under
/// the same test reports `undefined_getter` at `isEven` — the oracle
/// names `String` (the promoted type), not `Object?` (the declared one).
/// A `non_null_face(Object?)` answer would resolve `isEven` on `Object`
/// and report nothing, or name the wrong receiver type.
#[test]
fn promoted_read_resolves_members_on_the_promoted_type() {
    let source = "\
class A {\n\
\x20 final Object? _f;\n\
\x20 A(this._f);\n\
}\n\
void g(A x) {\n\
\x20 if (x._f is String) {\n\
\x20   x._f.isEven;\n\
\x20 }\n\
}\n";
    assert_eq!(rows_of(source, "undefined_getter"), vec![at(source, "isEven")]);
}

/// `this._f` inside the declaring class body promotes through the bare
/// `_f` read — `_f.length` resolves on `String`, silent end to end.
/// Oracle-verified: silent on 3.13.7.
#[test]
fn this_private_field_is_test_promotes_inside_the_class_body() {
    let source = "\
class A {\n\
\x20 final Object? _f;\n\
\x20 A(this._f);\n\
\x20 void m() {\n\
\x20   if (_f is String) {\n\
\x20     _f.length;\n\
\x20   }\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "unexpected rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|row| (row.code.name.to_string(), row.range.offset, row.range.length))
            .collect::<Vec<_>>()
    );
}

/// The extension face: `extension E on A` bodies read `_f` through the
/// same `(this-receiver, _f)` slot — `_f is String` promotes it and the
/// chained `.length` resolves silent. Oracle-verified: silent on
/// 3.13.7.
#[test]
fn extension_body_private_field_is_test_promotes_the_read() {
    let source = "\
class A {\n\
\x20 final Object? _f;\n\
\x20 A(this._f);\n\
}\n\
extension E on A {\n\
\x20 void m() {\n\
\x20   if (_f is String) {\n\
\x20     _f.length;\n\
\x20   }\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "unexpected rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|row| (row.code.name.to_string(), row.range.offset, row.range.length))
            .collect::<Vec<_>>()
    );
}

/// The promoted face feeds operator resolution too — `x._f + "s"` under
/// `x._f is String` resolves `String.+(String)` (declared in the
/// prelude), silent. Under the `non_null_face(Object?)` fallback the
/// `+` lookup would stand on `Object` and report
/// `undefined_operator`. Oracle-verified: silent on 3.13.7.
#[test]
fn promoted_read_resolves_operators_on_the_promoted_type() {
    let source = "\
class A {\n\
\x20 final Object? _f;\n\
\x20 A(this._f);\n\
}\n\
void g(A x) {\n\
\x20 if (x._f is String) {\n\
\x20   x._f + \"s\";\n\
\x20 }\n\
}\n";
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "unexpected rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|row| {
                (
                    row.code.name.to_string(),
                    row.range.offset,
                    row.range.length,
                    row.message.to_string(),
                )
            })
            .collect::<Vec<_>>()
    );
}

/// The `as`-cast arm (upstream `tryPromoteForTypeCast`): `x._f as
/// String` promotes the slot exactly like the `is`-matched arm — the
/// following read resolves `.length` on `String`, silent.
/// Oracle-verified: `dart analyze` 3.13.7 reports nothing on this
/// fixture.
#[test]
fn private_field_as_cast_promotes_the_following_read() {
    let source = "\
class A {\n\
\x20 final Object? _f;\n\
\x20 A(this._f);\n\
}\n\
void g(A x) {\n\
\x20 x._f as String;\n\
\x20 x._f.length;\n\
}\n";
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "unexpected rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|row| (row.code.name.to_string(), row.range.offset, row.range.length))
            .collect::<Vec<_>>()
    );
}
