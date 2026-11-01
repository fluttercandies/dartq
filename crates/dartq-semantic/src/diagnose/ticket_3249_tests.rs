//! The #3249 pins: the member-access and index `void`-receiver faces and
//! the record-literal field face of `use_of_void_result`. A `void`
//! receiver's value is consumed by the access, so the oracle reports there
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! property_element_resolver.dart — `_resolve`'s `targetType is VoidType`
//! arm anchors the member's own name for a read/write, `_reportUnresolved
//! Index` anchors the bracket pair, and a member *invocation* takes
//! error_verifier.dart `visitAnonymousMethodInvocation`'s
//! `checkForUseOfVoidResult(node.target2)` — the receiver node;
//! record_literal_resolver.dart:160 anchors the literal field).
//!
//! `void v() {}` + `var a = v();` types the local `void` (the oracle's
//! inferred slot), which is the receiver every fixture below consumes.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// The message the oracle spells for every face — parameterless
/// (messages.yaml:19638 `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// Asserts the fixture's only `use_of_void_result` row sits at `at` with
/// `length` (offsets relative to the fixture's own text). Sibling codes a
/// fixture's shape invites (`unused_element` over a receiver-typed class,
/// `unused_local_variable`) are tolerated — this family pins the void face.
fn assert_single_void(report: &super::FileReport, _fixture: &str, at: usize, length: u32) {
    let void: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert_eq!(void.len(), 1, "{:?}", rows(report));
    let offset = crate::diagnose::test_rig::CORE_PRELUDE.len() + at;
    assert_eq!(void[0].range.offset as usize, offset, "{:?}", rows(report));
    assert_eq!(void[0].range.length, length, "{:?}", rows(report));
}

/// A member read over a `void` local reports at the member's own name
/// (`.at(propertyName)`), never at the receiver.
#[test]
fn ticket_3249_member_read_void_receiver() {
    let fixture = "void v() {}\nclass A {\n  int bar = 0;\n}\nvoid main() {\n  var a = v();\n  a.bar;\n}";
    let report = report_ops(fixture);
    assert_single_void(&report, fixture, fixture.find(".bar").unwrap() + 1, 3);
}

/// The null-aware spelling is the same face — the `?.` merge does not
/// rescue a `void` receiver.
#[test]
fn ticket_3249_null_aware_member_read_void_receiver() {
    let fixture =
        "void v() {}\nclass A {\n  int bar = 0;\n}\nvoid main() {\n  var a = v();\n  a?.bar;\n}";
    let report = report_ops(fixture);
    assert_single_void(&report, fixture, fixture.find("?.bar").unwrap() + 2, 3);
}

/// A member write's TARGET reports at the member name as well
/// (`.at(propertyName)` on the write resolution); the assigned value keeps
/// its own faces.
#[test]
fn ticket_3249_member_write_void_receiver() {
    let fixture = "void v() {}\nvoid main() {\n  var a = v();\n  a.bar = 1;\n}";
    let report = report_ops(fixture);
    assert_single_void(&report, fixture, fixture.find(".bar").unwrap() + 1, 3);
}

/// A member *send* over a `void` receiver anchors at the receiver node —
/// `a` of `a.bar()` (`.at(target)`), length 1.
#[test]
fn ticket_3249_member_send_void_receiver() {
    let fixture = "void v() {}\nclass A {\n  void bar() {}\n}\nvoid main() {\n  var a = v();\n  a.bar();\n}";
    let report = report_ops(fixture);
    assert_single_void(&report, fixture, fixture.find("a.bar();").unwrap(), 1);
}

/// An index read over a `void` receiver anchors on the bracket pair
/// (`[0]`), the `_reportUnresolvedIndex` span.
#[test]
fn ticket_3249_index_read_void_receiver() {
    let fixture = "void v() {}\nvoid main() {\n  var a = v();\n  a[0];\n}";
    let report = report_ops(fixture);
    assert_single_void(&report, fixture, fixture.find("[0];").unwrap(), 3);
}

/// The index WRITE target reports the same bracket span.
#[test]
fn ticket_3249_index_write_void_receiver() {
    let fixture = "void v() {}\nvoid main() {\n  var a = v();\n  a[0] = 1;\n}";
    let report = report_ops(fixture);
    assert_single_void(&report, fixture, fixture.find("[0] =").unwrap(), 3);
}

/// A record literal's field value reports on the field node
/// (record_literal_resolver.dart:159 `.at(field)`) — the whole
/// `RecordLiteralNamedField` for a named field (`a: v`: the value's row
/// stops at the callee name), the expression itself for a positional one.
#[test]
fn ticket_3249_record_literal_field_void_value() {
    let named = "void v() {}\nvoid main() {\n  var r = (a: v());\n  print(r);\n}";
    let report = report_ops(named);
    assert_single_void(&report, named, named.find("a: v").unwrap(), 4);

    let positional = "void v() {}\nvoid main() {\n  var r = (v(), 1);\n  print(r);\n}";
    let report = report_ops(positional);
    assert_single_void(&report, positional, positional.find("v(),").unwrap(), 1);
}

/// The negative control: a non-`void` receiver keeps every access face
/// silent — the new hooks must not fire on ordinary members.
#[test]
fn ticket_3249_non_void_receiver_stays_silent() {
    let fixture = "class A {\n  int bar = 0;\n  void baz() {}\n}\nvoid main() {\n  A a = A();\n  a.bar;\n  a.bar = 1;\n  a.baz();\n}";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
}
