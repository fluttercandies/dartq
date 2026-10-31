//! #3157 pins for the `unnecessary_statements` residual #3148 fold-C handed
//! back, measured against the pinned `dart analyze` 3.13.7 oracle
//! (`pkg/linter/test/rules/unnecessary_statements_test.dart`).
//!
//! Three faces had under-reported:
//!
//! - the empty-literal statement (`[];`, `<dynamic, dynamic>{};`) — the
//!   parser seals the empty list literal away, so the statement row carries
//!   no token range and the window must be read off its byte span;
//! - the member-selector extent (`c.g`, `C().g`, `() => c.g`) — the bridge's
//!   `DotAccess` marker anchors at the member name, so the containment root
//!   walk missed it and the whole statement stayed silent;
//! - the cascade section (`c..m`) — a method section's static type is the
//!   method's own signature (a function type), never its declared return
//!   type.
//!
//! Every expected span below is the oracle's `line:column|length` row for the
//! same shape.

use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// One row for `source`: it opens at the `nth` `needle` token and spans
/// `len` UTF-16 units, the oracle's `reportAtNode` extent.
fn assert_row(source: &str, needle: &str, nth: usize, len: u32) {
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("unnecessary_statements"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(lints[0].message, "Unnecessary statement.", "{source:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, needle, nth)),
        "{source:?}"
    );
    assert_eq!(
        lints[0].range.end - lints[0].range.start,
        len,
        "{source:?}"
    );
}

/// No row for `source`.
fn assert_silent(source: &str) {
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("unnecessary_statements"));
    assert!(lints.is_empty(), "{source:?}: {lints:?}");
}

// —— the empty-literal statement ——————————————————————————————————————
// Oracle `test_listLiteral` 3:3|2 and `test_inIfBody` 4:5|2.

#[test]
fn us_empty_list_literal_statement_reports_at_the_brackets() {
    assert_row("void f() {\n  [];\n}\n", "[]", 0, 2);
}

#[test]
fn us_empty_list_literal_in_block_body_reports_at_the_brackets() {
    assert_row("void f() {\n  if (1 == 2) {\n    [];\n  }\n}\n", "[]", 0, 2);
}

/// Oracle `test_mapLiteral` 3:3|20 — the whole `<>{}` literal, not the
/// braces alone.
#[test]
fn us_map_literal_statement_reports_the_whole_literal() {
    assert_row(
        "void f() {\n  <dynamic, dynamic>{};\n}\n",
        "<",
        0,
        20,
    );
}

/// Oracle `test_inForLoopUpdaters` 3:18|2 — an updater's empty literal is
/// an expression statement's expression (upstream visits updaters).
#[test]
fn us_for_updater_empty_list_literal_reports_at_the_brackets() {
    assert_row("void f() {\n  for (; 1 == 2; []) {}\n}\n", "[]", 0, 2);
}

// —— the member-selector extent ————————————————————————————————————
// Oracle `test_instanceField2` 3:3|3 — the whole `x.g`, not the member.

#[test]
fn us_instance_field_read_reports_the_whole_selector() {
    assert_row(
        "void f(C v) {\n  v.g;\n}\nclass C {\n  int g = 1;\n}\n",
        "v",
        1,
        3,
    );
}

/// Oracle `test_instanceField` 3:3|5 — the invocation receiver chain.
#[test]
fn us_instance_field_read_on_an_invocation_receiver() {
    assert_row(
        "void f() {\n  C().g;\n}\nclass C {\n  int g = 1;\n}\n",
        "C",
        0,
        5,
    );
}

/// Oracle `test_methodTearoff_returned_inFunctionLiteral` 3:3|24 — the
/// function literal's body carries the selector, and the parameter list's
/// `)` is content (not a follower).
#[test]
fn us_function_literal_body_selector_reports_the_whole_literal() {
    assert_row(
        "void f(C x) {\n  () => x.g;\n}\nclass C {\n  int g = 1;\n}\n",
        "(",
        1,
        9,
    );
}

// —— the cascade section —————————————————————————————————————————————
// Oracle `test_methodTearoff_cascaded` 3:15|7 — the section opens at `..`.

#[test]
fn us_cascade_method_tearoff_reports_the_section() {
    assert_row(
        "void f(C c) {\n  c..m;\n}\nclass C {\n  void m() {}\n}\n",
        "..",
        0,
        4,
    );
}

/// A cascade getter that does not return a function type has a clear
/// effect (`:194` gates on `staticType is FunctionType`).
#[test]
fn us_cascade_field_section_stays_silent() {
    assert_silent("void f(C c) {\n  c..g;\n}\nclass C {\n  int g = 1;\n}\n");
}

/// Oracle `test_methodTearoff` 3:3|18 — a method read as a value is a
/// tear-off, reported over the whole selector.
#[test]
fn us_method_tearoff_on_an_invocation_receiver() {
    assert_row(
        "void f() {\n  C().m;\n}\nclass C {\n  void m() {}\n}\n",
        "C",
        0,
        5,
    );
}

/// Oracle `test_constructorTearoff_named` 3:3|12 — a static member read
/// through a class reference reports over the whole selector.
#[test]
fn us_static_field_read_on_a_class_reference() {
    assert_row(
        "void f() {\n  C.now;\n}\nclass C {\n  static int now = 1;\n}\n",
        "C",
        0,
        5,
    );
}

/// An assignment is a clear effect — the selector-extent widening must not
/// drag it into the report set.
#[test]
fn us_selector_assignment_stays_silent() {
    assert_silent("void f(C x) {\n  x.g = 1;\n}\nclass C {\n  int g = 1;\n}\n");
}

/// An origin getter read has no effect to report and nothing to discard
/// (`:129-176` keeps the getter face silent) — the fencing case for the
/// widened selector root.
#[test]
fn us_instance_getter_read_stays_silent() {
    assert_silent("void f(C c) {\n  c.g;\n}\nclass C {\n  int get g => 1;\n}\n");
}

#[test]
fn us_top_level_getter_stays_silent() {
    assert_silent("void f() {\n  g;\n}\nint get g => 1;\n");
}
