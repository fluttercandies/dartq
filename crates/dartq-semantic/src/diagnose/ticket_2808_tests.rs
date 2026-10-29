//! The #2808 pins: an applicable nullable-`on` extension resolves a member
//! invocation on a nullable receiver safely — no
//! `unchecked_use_of_nullable_value` row. The corpus face is the co19
//! `expectStaticType` helper, an imported NAMED generic
//! `extension StaticType<T> on T` whose `on` type has no nullability of its
//! own: `applicableTo` instantiates the extended type from the receiver
//! (`T := ET<Never>?`), so the application over the nullable receiver is
//! safe (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! extension_member_resolver.dart's `applicableTo` against the nullable
//! receiver type).

use super::test_rig::{RigImport, report_ops, report_ops_imports, rows};
use super::*;

fn unchecked_count(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .count()
}

/// The corpus shape (co19
/// `static_analysis_extension_types_A34_t02.dart`): `ET<Never>? et;
/// et.expectStaticType<Exactly<ET<Never>?>>()` through an imported
/// `extension StaticType<T> on T` — silent (B1's 67 cases / 692 entries).
#[test]
fn imported_own_param_on_type_extension_member_invocation_stays_silent() {
    let report = report_ops_imports(
        r#"
extension type ET<X>(X _) {}
void f() {
  ET<Never>? et1;
  et1.expectStaticType<ET<Never>?>();
  ET<Never>? et2;
  et2.expectStaticType<ET<Never>?>();
}
"#,
        &[RigImport {
            uri: "package:t/static_type_helper.dart",
            prefix: None,
            source: r#"
extension StaticType<T> on T {
  T expectStaticType<R>() {
    return this;
  }
}
"#,
        }],
    );
    assert_eq!(
        unchecked_count(&report),
        0,
        "an own-parameter `on` face instantiates its nullability from the receiver: {:?}",
        rows(&report)
    );
}

/// The same face declared in-file answers the same way (the capture latch
/// arms without the import machinery).
#[test]
fn in_file_own_param_on_type_extension_member_invocation_stays_silent() {
    let report = report_ops(
        r#"
extension type ET<X>(X _) {}
extension StaticType<T> on T {
  T expectStaticType<R>() {
    return this;
  }
}
void f() {
  ET<Never>? et1;
  et1.expectStaticType<ET<Never>?>();
}
"#,
    );
    assert_eq!(unchecked_count(&report), 0);
}

/// A NAMED nullable-`on` extension answers the same way (the written `?`
/// face beside the instantiated one).
#[test]
fn named_nullable_on_extension_member_invocation_stays_silent() {
    let report = report_ops(
        r#"
class C {}
extension Helper on C? {
  void m() {}
}
void f() {
  C? c;
  c.m();
}
"#,
    );
    assert_eq!(unchecked_count(&report), 0);
}

/// The true-positive guard: a genuine instance member on a nullable
/// receiver keeps the row — the detector must not go blind.
#[test]
fn nullable_receiver_instance_member_still_reports() {
    let report = report_ops(
        r#"
class C {
  void m() {}
}
void f() {
  C? c;
  c.m();
}
"#,
    );
    assert_eq!(unchecked_count(&report), 1);
}

/// The non-null-requiring extension face keeps the unsafe row (the
/// `UnsafeSingle` posture over `extension E on C` reached through `C?`).
#[test]
fn non_nullable_on_extension_reports_unsafe() {
    let report = report_ops(
        r#"
class C {}
extension E on C {
  void m() {}
}
void f() {
  C? c;
  c.m();
}
"#,
    );
    assert_eq!(unchecked_count(&report), 1);
}

/// A bounded own-parameter face stays unsafe on a nullable receiver: the
/// concrete bound rejects the nullable instantiation (`applicableTo` fails
/// the inference), so the flag must not claim safety there.
#[test]
fn bounded_own_param_on_type_extension_reports_unsafe() {
    let report = report_ops(
        r#"
class C {}
extension E<T extends C> on T {
  void m() {}
}
void f() {
  C? c;
  c.m();
}
"#,
    );
    assert_eq!(unchecked_count(&report), 1);
}

/// An imported UNNAMED extension never applies: unnamed extensions are
/// visible only in their declaring library (the language's scoping rule —
/// the import scope keys on names), so the member stays unresolved and the
/// nullable receiver keeps the oracle's unchecked row.
#[test]
fn imported_unnamed_extension_never_applies() {
    let report = report_ops_imports(
        r#"
extension type ET<X>(X _) {}
void f() {
  ET<Never>? et1;
  et1.expectStaticType<ET<Never>?>();
}
"#,
        &[RigImport {
            uri: "package:t/static_type_helper.dart",
            prefix: None,
            source: r#"
extension on Object? {
  void expectStaticType<T>() {}
}
"#,
        }],
    );
    assert_eq!(unchecked_count(&report), 1);
}

/// The imported true-positive guard: a non-null `on` helper through the
/// same edge keeps the unsafe row (the cross-file face carries its written
/// nullability).
#[test]
fn imported_non_nullable_on_extension_reports_unsafe() {
    let report = report_ops_imports(
        r#"
class C {}
void f() {
  C? c;
  c.m();
}
"#,
        &[RigImport {
            uri: "package:t/helper.dart",
            prefix: None,
            source: r#"
extension E on C {
  void m() {}
}
"#,
        }],
    );
    assert_eq!(unchecked_count(&report), 1);
}
