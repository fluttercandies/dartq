//! The #3300 pins: the `unchecked_use_of_nullable_value` extension-acceptance
//! face — the dominant EXTRA cluster of the conformance board (the co19
//! `expectStaticType` helper shape: 692 extra entries over 67 solo cases,
//! 2026-10-29 co-failure decomposition, cluster B1).
//!
//! Oracle semantics (dart 3.13.7, the pinned conformance oracle;
//! `pkg/analyzer/lib/src/dart/resolver/type_property_resolver.dart:96-124`
//! and `nullable_dereference_verifier.dart`): the nullable dereference is
//! asked only for an access the member resolution routes through the
//! INSTANCE face. An applicable extension whose instantiated `on` type
//! accepts the receiver's nullable type resolves the access without
//! dereferencing anything, so the access reports nothing — the ordering of
//! the applicability test before the report is the whole face. Two on-type
//! families reach that verdict:
//!
//! * `extension E on C?` — the written nullable `on` type (`C? x; x.foo`
//!   resolves through `E` and stays silent, even when `C` declares `foo`);
//! * `extension StaticType<T> on T` — the unbounded `on` type instantiates
//!   to the nullable receiver itself (`ET<Never>? e; e.expectStaticType()`),
//!   the co19 helper shape the corpus sweeps thousands of times per file.
//!
//! The paired direction is the unsafe single: an extension over a
//! NON-nullable `on` type still resolves the member for a nullable receiver,
//! but the access IS a dereference — the oracle reports the shared code at
//! the selector, and these pins hold that row in place.

use super::test_rig::report_ops;
use crate::diagnose::FileReport;

/// The `unchecked_use_of_nullable_value` rows of `report`, sorted by offset
/// — `(offset, length, message)` (the conformance face without the file
/// column).
fn unchecked_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// The dominant shape (census B1): a nullable extension-type receiver
/// reaches the co19 `StaticType` helper, whose unbounded `on T` accepts the
/// nullable type — no dereference, no row. The plain `int?` receiver takes
/// the same unbounded on-type and stays silent for the same reason, so the
/// fixture exercises both instantiations of the one rule.
///
/// Ignored: whether the unbounded `on T` face answers `Single` (nullable
/// accepting) or the unsafe verdict is decided inside
/// `Engine::extension_application` (`diagnose/row_resolution.rs`), outside
/// this ticket's write scope. The acceptance gate in `diagnose.rs` drops any
/// row the member face already marked as extension-resolved, so this pin
/// flips green the moment the applicability verdict lands there; until then
/// it holds the oracle expectation (`dart analyze` 3.13.7 reports nothing on
/// the co19 `Extension-types`/`static_type_helper` shapes).
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn pin_unbounded_extension_on_accepts_nullable_receivers() {
    let report = report_ops(
        "abstract class Exactly<X> {}\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         extension type ET<X>(X _) {}\n\
         void main() {\n\
         \x20 ET<Never>? et1;\n\
         \x20 et1.expectStaticType<Exactly<ET<Never>?>>();\n\
         \x20 int? i;\n\
         \x20 i.expectStaticType<Exactly<int?>>();\n\
         }\n",
    );
    assert_eq!(
        unchecked_rows(&report),
        Vec::new(),
        "the unbounded `on T` accepts the nullable receiver — no dereference"
    );
}

/// The written nullable `on` type over a receiver whose class DECLARES the
/// member: the applicable extension wins over the unsafe instance face, so
/// `C? x; x.foo` is silent (the `n?.p` / `x.m` contrast the resolution arms
/// carry — `type_property_resolver.dart` consults the extensions against the
/// nullable type before it reports).
#[test]
fn pin_nullable_on_extension_wins_over_the_instance_face() {
    let report = report_ops(
        "class C {\n\
         \x20 int get foo => 0;\n\
         }\n\
         extension E on C? {\n\
         \x20 int get foo => 0;\n\
         }\n\
         void f(C? c) {\n\
         \x20 c.foo;\n\
         }\n",
    );
    assert_eq!(
        unchecked_rows(&report),
        Vec::new(),
        "the nullable `on` face resolved the access — no dereference"
    );
}

/// The unsafe single — the gate's boundary. An extension over a NON-nullable
/// `on` type resolves the member for a nullable receiver, but the access
/// dereferences the receiver: the oracle reports the property variant at the
/// selector, and no suppression may claim it.
#[test]
fn pin_non_nullable_on_extension_keeps_the_row() {
    let report = report_ops(
        "class C {\n\
         \x20 int get foo => 0;\n\
         }\n\
         extension E on C {\n\
         \x20 int get bar => 0;\n\
         }\n\
         void f(C? c) {\n\
         \x20 c.foo;\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "the unsafe single still reports: {rows:?}");
    assert!(
        rows[0]
            .2
            .contains("The property 'foo' can't be unconditionally accessed"),
        "the property variant: {rows:?}"
    );
}

/// A plain nullable receiver with no extension in sight: the property face
/// reports as before (the baseline the gate must not touch).
#[test]
fn pin_plain_nullable_receiver_still_reports() {
    let report = report_ops(
        "class C {\n\
         \x20 int get foo => 0;\n\
         \x20 void m() {}\n\
         }\n\
         void f(C? c) {\n\
         \x20 c.foo;\n\
         \x20 c.m();\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 2, "one property and one method row: {rows:?}");
    assert!(
        rows.iter()
            .any(|(_, _, m)| m.contains("The property 'foo'"))
            && rows.iter().any(|(_, _, m)| m.contains("The method 'm'")),
        "the two access variants: {rows:?}"
    );
}

/// The promotion silent-negative: a guarded receiver is promoted, so the
/// member send inside the guard never reports — the face the gate sits
/// beside, unchanged (a `C?` read the flow narrowed to `C`).
#[test]
fn pin_promoted_receiver_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 void m() {}\n\
         }\n\
         void f(C? c) {\n\
         \x20 if (c != null) {\n\
         \x20   c.m();\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        unchecked_rows(&report),
        Vec::new(),
        "the promoted receiver is non-null at the send"
    );
}

/// The nullable implicit `this` inside `extension on A?` keeps its row: the
/// extension's own `on` type makes `this` nullable, and a member declared on
/// the extended class is an unsafe dereference (the #2919 face, adjacent to
/// the acceptance rule and deliberately not absorbed by it).
#[test]
fn pin_nullable_this_keeps_its_member_row() {
    let report = report_ops(
        "class A {\n\
         \x20 void foo() {}\n\
         }\n\
         extension E on A? {\n\
         \x20 void baz() {\n\
         \x20   this.foo();\n\
         \x20 }\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "only this.foo() reports: {rows:?}");
    assert!(
        rows[0].2.contains("The method 'foo'"),
        "the method variant: {rows:?}"
    );
}
