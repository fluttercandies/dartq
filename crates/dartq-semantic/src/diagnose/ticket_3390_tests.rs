//! The #3390 pins: the `??=` staged flush re-decides a pair the local
//! promotion table missed through the target row's declared read face —
//! upstream's second `readType` source. A local target reads its
//! flow-promoted type (`localVariableTypeProvider.getType`), but a member
//! target reads the getter/field's DECLARED return, promotion-free
//! (`setReadElement`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart:1754-1792; the compound
//! assignment feeds it to `_checkForDeadNullCoalesce`,
//! error_verifier.dart:444-449). The staged pairs carry only
//! local-keyed reads, so the flush's new arm re-decides them from the
//! target row's static type / the parse-recorded member entry face; the
//! member, `this`, and `super` shapes below pin the observable dead face
//! at the value span.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// Asserts one `dead_null_aware_expression` sits at `offset`, tolerating
/// the co-diagnostics a declared-but-unread local or the dead arm's own
/// `dead_code` row adds around it.
fn assert_dead_coalesce_at(report: &super::FileReport, offset: usize) {
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "dead_null_aware_expression"
                && d.range.offset as usize == offset),
        "no dead_null_aware_expression at {offset}: {:?}",
        rows(report)
    );
}

/// The offset of `needle`'s LAST occurrence inside `fixture` shifted past
/// the prelude — `report_ops` prepends
/// [`crate::diagnose::test_rig::CORE_PRELUDE`]. The last-occurrence read
/// keys the USE site when the fixture's declaration carries the same
/// spelling.
fn fixture_offset(fixture: &str, needle: &str) -> usize {
    crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.rfind(needle).unwrap()
}

/// `c.f ??= v` over a non-nullable field: the target's declared read face
/// (`int`) anchors the dead value — the oracle's `readType` answers the
/// field's declared type with no promotion row in play
/// (`setReadElement`'s variable face).
#[test]
fn member_field_target_non_null_reports_the_dead_value() {
    let fixture = "class C { int f = 0; }\nvoid f(C c, int v) {\n  c.f ??= v;\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "v;"));
}

/// `this.x ??= v` over a non-nullable field: the keyword-receiver read
/// resolves the same declared face, the dead value reports.
#[test]
fn this_member_target_non_null_reports_the_dead_value() {
    let fixture = "class C { int x = 0; void m() { this.x ??= 1; } }";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "1;"));
}

/// `c.p ??= v` over a non-nullable GETTER: the read face is the getter's
/// declared return (`setReadElement`'s `InternalGetterElement.returnType`
/// arm) — the staged join's member-entry face answers `return_ty`, and
/// the dead value reports.
#[test]
fn member_getter_target_non_null_reports_the_dead_value() {
    let fixture = "class C { String get p => ''; set p(Object? value) {} }\n\
                   void f(C c, String v) {\n  c.p ??= v;\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "v;"));
}

/// `super.x ??= v` over an inherited non-nullable field: the super read's
/// declared face anchors the dead value the same way.
#[test]
fn super_member_target_non_null_reports_the_dead_value() {
    let fixture = "class B { int x = 0; }\n\
                   class C extends B {\n  void m(int v) {\n    super.x ??= v;\n  }\n}";
    let report = report_ops(fixture);
    assert_dead_coalesce_at(&report, fixture_offset(fixture, "v;"));
}

/// The local control: an unpromoted nullable `x ??= v` stays silent — the
/// write's own promotion is the only anchor for a local target, and no
/// promotion stands here.
#[test]
fn local_nullable_target_stays_silent() {
    let fixture = "void f(int? x, int v) {\n  x ??= v;\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|&c| c == "dead_null_aware_expression"),
        "{:?}",
        rows(&report)
    );
}
