//! The #3840 `unused_element` pins: the self-reference exclusion and the
//! write-only top-level variable.
//!
//! Oracle contract (upstream Dart SDK source 3.13.7,
//! `pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart`):
//!
//! * `_useIdentifierElement` ignores a reference to an element from inside
//!   that element's own declaration — `identical(element, _enclosingClass)`
//!   (:493) and `identical(element, _enclosingExec)` (:494). `_enclosingExec`
//!   is set by the three declaration visits that own a body
//!   (`visitFunctionDeclaration` :214-220, `visitMethodDeclaration`
//!   :287-292, `visitTopLevelGetterDeclaration` :390-395), so a recursive
//!   `_f(p - 1)` inside `_f`, a `return _g;` inside `get _g`, and a
//!   `_s = v;` inside `set _s` all leave the declaration UNUSED. Probed
//!   against `dart analyze` 3.13.7: every shape below reports exactly one
//!   `unused_element` row at the declared name.
//! * A pure write credits the SETTER accessor element, never the variable
//!   (`_useAssignmentTargetElement` → `_useIdentifierElement(node.write
//!   .element)`, :464-479), while `_visitTopLevelVariableElement`
//!   (:1319-1325) joins the VARIABLE element — so `int _a = 1;` written and
//!   never read reports (:1319-1325 against the write path). A read (with
//!   `readCountsAsUse`, :494-520) credits the variable and stays silent.
//!
//! Both directions are pinned: every reporting shape has a keep-honest twin
//! that must stay silent, so the exclusion cannot widen into a blanket
//! "declarations are never credited".
//!
//! Rig: [`crate::diagnose::test_rig::report_ops`], the single-file rig that
//! mounts the implicit `dart:core` library every real Dart unit has. The
//! import-free `pins::report` rig cannot express the sibling-member shape:
//! without the core library the bare `_g` read inside `m()` resolves
//! `RefTarget::Missing` (the engine reports `undefined_identifier` beside
//! the `unused_element`), so the sibling-twin assertion was unreachable
//! there for a reason that has nothing to do with the exclusion under test.
//! Probed both ways: the same source analyzed through `report_ops` is
//! silent, exactly as the oracle is.

use crate::diagnose::test_rig::report_ops as report;

/// The `unused_element` rows of `report`, as (offset, message) pairs — the
/// message carries the `{0}` displayName the assertions key on.
fn unused_element_rows(source: &str) -> Vec<(u32, String)> {
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

/// The rows naming `name` (the diagnostic renders it as `'<name>'`).
fn rows_naming(source: &str, name: &str) -> Vec<(u32, String)> {
    let needle = format!("'{name}'");
    unused_element_rows(source)
        .into_iter()
        .filter(|(_, message)| message.contains(&needle))
        .collect()
}

fn assert_reports(name: &str, source: &str) {
    let rows = rows_naming(source, name);
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one unused_element row for {name}: {rows:?} in {source:?}"
    );
}

fn assert_silent_about(name: &str, source: &str) {
    let rows = rows_naming(source, name);
    assert!(
        rows.is_empty(),
        "expected no unused_element row for {name}: {rows:?} in {source:?}"
    );
}

/// A getter that reads itself inside its own body is unused — the
/// `UnusedElementTest.test_getter_notUsed_referenceFromItself` shape.
#[test]
fn ticket_3840_getter_self_reference_reports() {
    let source = "class A {\n\
                  \x20 get _g {\n\
                  \x20   return _g;\n\
                  \x20 }\n\
                  }\n";
    assert_reports("_g", source);
}

/// A method that calls itself inside its own body is unused — the
/// `test_method_notUsed_referenceFromItself` shape.
#[test]
fn ticket_3840_method_self_reference_reports() {
    let source = "class A {\n\
                  \x20 _m() {\n\
                  \x20   _m();\n\
                  \x20 }\n\
                  }\n";
    assert_reports("_m", source);
}

/// A top-level function that only calls itself is unused — the
/// `test_topLevelFunction_notUsed_referenceFromItself` shape.
#[test]
fn ticket_3840_top_level_function_self_reference_reports() {
    let source = "_f(p) {\n\
                  \x20 _f(p - 1);\n\
                  }\n";
    assert_reports("_f", source);
}

/// A setter that writes itself inside its own body is unused — the
/// `test_setter_notUsed_referenceFromItself` shape.
#[test]
fn ticket_3840_setter_self_reference_reports() {
    let source = "set _s(v) {\n\
                  \x20 _s = v;\n\
                  }\n";
    assert_reports("_s", source);
}

/// A private top-level variable written and never read is unused — the
/// `test_topLevelVariable_notUsed` shape. The write credits the setter
/// accessor element only.
#[test]
fn ticket_3840_write_only_top_level_variable_reports() {
    let source = "var _a = 1;\n\
                  f() {\n\
                  \x20 _a = 2;\n\
                  }\n";
    assert_reports("_a", source);
}

/// Keep-honest twin: the same variable READ once is used.
#[test]
fn ticket_3840_read_top_level_variable_is_silent() {
    let source = "var _b = 1;\n\
                  f() {\n\
                  \x20 return _b;\n\
                  }\n";
    assert_silent_about("_b", source);
}

/// Keep-honest twin: a written top-level SETTER is the element the write
/// credits, so it never reports — the two faces the pure-write split
/// separates (variable vs accessor).
#[test]
fn ticket_3840_written_top_level_setter_is_silent() {
    let source = "set _s(v) {}\n\
                  f() {\n\
                  \x20 _s = 1;\n\
                  }\n";
    assert_silent_about("_s", source);
}

/// Keep-honest twin: a private getter read from a SIBLING member is used —
/// the reference sits outside the getter's own body, so the exclusion must
/// not claim it.
#[test]
fn ticket_3840_getter_read_from_sibling_member_is_silent() {
    let source = "class A {\n\
                  \x20 get _g {\n\
                  \x20   return 1;\n\
                  \x20 }\n\
                  \x20 m() {\n\
                  \x20   return _g;\n\
                  \x20 }\n\
                  }\n";
    assert_silent_about("_g", source);
}

/// Keep-honest twin: a private top-level function called from ANOTHER
/// executable is used — the call lies outside the callee's own extent.
#[test]
fn ticket_3840_function_called_elsewhere_is_silent() {
    let source = "_f(p) {\n\
                  \x20 return p;\n\
                  }\n\
                  main() {\n\
                  \x20 _f(1);\n\
                  }\n";
    assert_silent_about("_f", source);
}
