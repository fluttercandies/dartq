//! The #3085 pins — a `var` declarator whose initializer is a `Map`
//! index read binds the read's own static type, the nullable `V?`, so the
//! downstream promotions (a switch arm's write, a `!= null` guard) decide
//! on the real face instead of the `Unknown` slot.
//!
//! Upstream the local's declared type IS the initializer's static type:
//! `operator []` on `Map<K, V>` answers `V?` (upstream Dart SDK source,
//! sdk/lib/core/map.dart:288), and the SDK's own code runs on exactly that
//! inference — `var reference = _referenceByIndex[index]`
//! (pkg/analyzer/lib/src/summary2/reference.dart:680) with the switch arms
//! at `:684-707` writing it back to `Reference`, and
//! `var argumentValue = _argumentValueMap[baseParameter]`
//! (pkg/analyzer/lib/src/dart/constant/evaluation.dart:3335) with the
//! `argumentValue == null` guard at `:3341` promoting it away.
//!
//! Oracle provenance: `dart analyze` 3.13.1 machine format. Every fixture
//! below was probed against the real binary first: the two promotion
//! shapes report nothing, the unguarded read reports one
//! `unchecked_use_of_nullable_value`.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// The switch-arm write promotion (reference.dart:680): `var reference =
/// _referenceByIndex[index]` binds `Reference?`, every arm writes a
/// `Reference`, so the member read after the switch carries the promoted
/// non-null face — no row at all (oracle: zero rows).
#[test]
fn map_index_binds_nullable_then_switch_arm_promotes() {
    let source = "class Reference {\n\
                  \x20 int get id => 0;\n\
                  }\n\
                  final Map<int, Reference> _referenceByIndex = {0: Reference()};\n\
                  int f(int index, int tag) {\n\
                  \x20 var reference = _referenceByIndex[index];\n\
                  \x20 switch (tag) {\n\
                  \x20   case 1:\n\
                  \x20     reference = Reference();\n\
                  \x20   default:\n\
                  \x20     reference = Reference();\n\
                  \x20 }\n\
                  \x20 return reference.id;\n\
                  }\n";
    assert_silent(&report_ops(source));
}

/// The `!= null` guard promotion (evaluation.dart:3335/:3341): the local
/// binds `DartObjectImpl?`, the guard's fall-through arm is non-null, so
/// the member read below it is silent (oracle: zero rows).
#[test]
fn map_index_binds_nullable_then_guard_promotes() {
    let source = "class DartObjectImpl {\n\
                  \x20 int get value => 0;\n\
                  }\n\
                  final Map<String, DartObjectImpl> _argumentValueMap = {\n\
                  \x20 'x': DartObjectImpl(),\n\
                  };\n\
                  class Parameter {\n\
                  \x20 Parameter(this.type);\n\
                  \x20 final int type;\n\
                  }\n\
                  class Config {\n\
                  \x20 Config(int arg);\n\
                  }\n\
                  void f(Parameter parameter) {\n\
                  \x20 var argumentValue = _argumentValueMap['x'];\n\
                  \x20 if (argumentValue == null) {\n\
                  \x20   return;\n\
                  \x20 }\n\
                  \x20 Config(argumentValue.value);\n\
                  }\n";
    assert_silent(&report_ops(source));
}

/// The counter-face: with NO promotion standing, the bound `Box?` reports
/// the unchecked read exactly like the oracle's own map-inferred local
/// (oracle: one `unchecked_use_of_nullable_value` at the member read).
#[test]
fn unguarded_nullable_index_read_reports_unchecked() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  final Map<int, Box> _boxes = {0: Box()};\n\
                  int g(int i) {\n\
                  \x20 var un = _boxes[i];\n\
                  \x20 return un.size;\n\
                  }\n";
    let report = report_ops(source);
    assert!(
        code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "the bound V? face reaches the read: {:?}",
        rows(&report)
    );
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|name| **name == "unchecked_use_of_nullable_value")
            .count(),
        1,
        "one row, at the member read: {:?}",
        rows(&report)
    );
}

/// The `!`-asserted twin keeps the face it always carried: `_boxes[i]!`
/// decides nullness, the local binds `Box` and the read is silent
/// (oracle: zero rows).
#[test]
fn asserted_nullable_index_read_stays_silent() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  final Map<int, Box> _boxes = {0: Box()};\n\
                  int g(int i) {\n\
                  \x20 var un = _boxes[i]!;\n\
                  \x20 return un.size;\n\
                  }\n";
    assert_silent(&report_ops(source));
}

/// The `List<E>` control: the element face is non-nullable, so the same
/// window binds it unchanged and the member read stays silent (oracle:
/// zero rows).
#[test]
fn list_index_element_stays_non_nullable() {
    let source = "class Box {\n\
                  \x20 int get size => 0;\n\
                  }\n\
                  final List<Box> _list = [];\n\
                  int g(int i) {\n\
                  \x20 var one = _list[i];\n\
                  \x20 return one.size;\n\
                  }\n";
    assert_silent(&report_ops(source));
}
