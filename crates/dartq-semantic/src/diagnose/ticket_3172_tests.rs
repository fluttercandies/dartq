//! The #3172 Lane D `dead_code` pins: the null-aware dead chain over a
//! definitely unassigned local (`_verifyUnassignedSimpleIdentifier`,
//! upstream Dart SDK source
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:467-505). The gate:
//! the operator is `?`/`?.`/`?..`, the target's `staticType` keeps the
//! `question` nullability suffix, the target's `unParenthesized` shape is
//! a `SimpleIdentifier` resolving a promotable element, and the flow
//! facts mark it definitely unassigned — then the row anchors at
//! `operator.offset` and ends at the outermost access's end (the
//! `while (parent is MethodInvocation | PropertyAccess | IndexExpression)`
//! climb, :492-497). An assigned (or `Null`/parameter/dynamic) receiver
//! never reaches this face — the `Null`-receiver dead continuations ride
//! the reachability facts, not this join.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// `i?.member` over an unassigned `int? i` reports `?.member` — the
/// operator through the (single) access's end.
#[test]
fn ticket_3172_unassigned_property_access() {
    let fixture = "void f() {\n  int? i;\n  i?.sign;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.sign").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 6, "{:?}", rows(&report));
}

/// The method-invocation climb covers the send's `)` — `i?.truncate()`
/// reports `?.truncate()`.
#[test]
fn ticket_3172_unassigned_method_invocation() {
    let fixture = "void f() {\n  int? i;\n  i?.truncate();\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.truncate").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 12, "{:?}", rows(&report));
}

/// A chained method call climbs to the OUTERMOST access — `i?.m().m()`
/// reports `?.m().m()` whole, not `?.m()`.
#[test]
fn ticket_3172_unassigned_method_invocation_method_invocation() {
    let fixture = "void f() {\n  int? i;\n  i?.truncate().truncate();\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.truncate").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 24, "{:?}", rows(&report));
}

/// A mixed chain climbs through the property access —
/// `i?.truncate().sign` reports `?.truncate().sign`.
#[test]
fn ticket_3172_unassigned_method_invocation_property_access() {
    let fixture = "void f() {\n  int? i;\n  i?.truncate().sign;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.truncate").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 17, "{:?}", rows(&report));
}

/// A two-link property chain climbs through both — `i?.sign.sign`
/// reports `?.sign.sign` (upstream
/// test_unassigned_propertyAccess_propertyAccess).
#[test]
fn ticket_3172_unassigned_property_access_property_access() {
    let fixture = "void f() {\n  int? i;\n  i?.sign.sign;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.sign").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 11, "{:?}", rows(&report));
}

/// `unParenthesized` (:486): `(i)?.sign` unwraps the group and roots on
/// the inner `i` read — reports `?.sign`, upstream
/// test_unassigned_propertyAccess.
#[test]
fn ticket_3172_unassigned_parenthesized_property_access() {
    let fixture = "void f() {\n  int? i;\n  (i)?.sign;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.sign").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 6, "{:?}", rows(&report));
}

/// `(i)?.sign.sign` — the unwrapped root plus the parent climb:
/// `?.sign.sign` whole (upstream
/// test_unassigned_propertyAccess_propertyAccess).
#[test]
fn ticket_3172_unassigned_parenthesized_property_access_property_access() {
    let fixture = "void f() {\n  int? i;\n  (i)?.sign.sign;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?.sign").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 11, "{:?}", rows(&report));
}

/// `l?[0]` over an unassigned `List<int>?` reports `?[0]` — the `?`
/// token through the bracket's own `]` (upstream
/// test_unassigned_indexExpression).
#[test]
fn ticket_3172_unassigned_index_expression() {
    let fixture = "void f() {\n  List<int>? l;\n  l?[0];\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?[0]").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 4, "{:?}", rows(&report));
}

/// `l?[0][0]` — the index-continuation climb grows the dead span through
/// the outer `[0]`: `?[0][0]` whole (upstream
/// test_unassigned_indexExpression_indexExpression).
#[test]
fn ticket_3172_unassigned_index_expression_index_expression() {
    let fixture = "void f() {\n  List<List<int>>? l;\n  l?[0][0];\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?[0]").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 7, "{:?}", rows(&report));
}

/// `l?[0]?[1]` — the second `?[` is the same climb (the inner `?[`'s
/// parent is an IndexExpression): one row at `?[0]?[1]`, no second row
/// on the outer operator (the outer `?[`'s own target is the inner
/// IndexExpression, not a SimpleIdentifier).
#[test]
fn ticket_3172_unassigned_index_index_aware() {
    let fixture = "void f() {\n  List<List<int>>? l;\n  l?[0]?[1];\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?[0]").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 8, "{:?}", rows(&report));
}

/// `i?..sign` — a single-section cascade: `verifyCascadeExpression`
/// checks `node.target` (`i`) against the first section's `?..`
/// operator; the cascade node's own end closes the span.
#[test]
fn ticket_3172_unassigned_cascade_property_access() {
    let fixture = "void f() {\n  int? i;\n  i?..sign;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?..sign").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 7, "{:?}", rows(&report));
}

/// `i?..sign..isEven` — two bare sections keep the `first is
/// PropertyAccess` gate (`first.operator` is the `?..`): the cascade
/// grows to its own end (upstream
/// test_unassigned_cascadeExpression_propertyAccess).
#[test]
fn ticket_3172_unassigned_cascade_two_sections() {
    let fixture = "void f() {\n  int? i;\n  i?..sign..isEven;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?..sign").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 15, "{:?}", rows(&report));
}

/// `i?..toInt()..isEven` — a method-invocation first section keeps the
/// `first is MethodInvocation` gate (upstream
/// test_unassigned_cascadeExpression_methodInvocation).
#[test]
fn ticket_3172_unassigned_cascade_method_invocation() {
    let fixture = "void f() {\n  int? i;\n  i?..toInt()..isEven;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?..toInt").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 18, "{:?}", rows(&report));
}

/// `l?..[0]..length` — an index-expression first section keeps the
/// `first is IndexExpression` gate (upstream
/// test_unassigned_cascadeExpression_indexExpression).
#[test]
fn ticket_3172_unassigned_cascade_index_expression() {
    let fixture = "void f() {\n  List<int>? l;\n  l?..[0]..length;\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?..[0]").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 14, "{:?}", rows(&report));
}

/// `l?..[0][1]` — the index continuation inside the cascade's first
/// section keeps `first` an IndexExpression whose `period` is the `?..`:
/// the bare-body gate holds and the row grows to `?..[0][1]`.
#[test]
fn ticket_3172_unassigned_cascade_index_continuation() {
    let fixture = "void f() {\n  List<List<int>>? l;\n  l?..[0][1];\n}";
    let report = report_ops(fixture);
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    let offset =
        crate::diagnose::test_rig::CORE_PRELUDE.len() + fixture.find("?..[0]").unwrap();
    assert_eq!(dead[0].range.offset as usize, offset, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 10, "{:?}", rows(&report));
}

/// `i?..toInt().bitLength` — a plain `.` continuation at the cascade
/// section's end changes `first.operator` to the INNER `.`
/// (`verifyCascadeExpression`'s `first.operator` is `?..` only while the
/// first section stays one bare selector): the chain dies and no
/// `dead_code` row lands.
#[test]
fn ticket_3172_cascade_chained_body_stays_silent() {
    let report = report_ops("void f() {\n  int? i;\n  i?..toInt().bitLength;\n}");
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}

/// An ASSIGNED nullable receiver never reaches this face — `int? i = 0;
/// i?.sign;` is live code (the `isDefinitelyUnassigned` gate).
#[test]
fn ticket_3172_assigned_receiver_stays_silent() {
    let report = report_ops("void f() {\n  int? i = 0;\n  i?.sign;\n}");
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}

/// A `dynamic`-typed unassigned receiver stays silent — `dynamic`'s
/// `nullabilitySuffix` is `none` (upstream Dart SDK source
/// pkg/analyzer/lib/src/dart/element/type.dart:59), so the `question`
/// suffix gate fails and `var i; i?.m;` reports nothing here.
#[test]
fn ticket_3172_dynamic_receiver_stays_silent() {
    let report = report_ops("void f() {\n  var i;\n  i?.m;\n}");
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}
