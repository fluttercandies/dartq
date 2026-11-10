//! The #3302 pins — the assignment-pattern declaring binder face of the
//! `unused_local_variable` cluster (`Engine::report_pattern_assignment_binders`).
//!
//! Upstream's `parseVariablePattern` reports
//! `pattern_assignment_declares_variable` for a variable pattern that
//! carries `var`/`final` (or a written type) inside a pattern assignment,
//! yet still mints a `LocalVariableElement` for the binder. That element
//! never enters a scope — the surrounding block keeps resolving the outer
//! spelling — so `unused_local_elements_verifier` reports it as unused at
//! the binder's own range (oracle 3.13.7, probed against the pinned SDK):
//!
//! ```text
//! var a = 1;  (var a) = 2;  print(a);
//!             ^ reports unused_local_variable (and the error above it)
//! ```
//!
//! A same-spelled read before or after the assignment spares nothing
//! (the read binds another element), a single `_` stays silent under the
//! wildcard-variables feature, and a repeat of one spelling inside one
//! pattern reports once (the duplicate carries
//! `duplicate_variable_pattern` instead).
//!
//! The shapes below are the corpus population the face closes
//! (`co19/LanguageFeatures/Patterns/pattern_assignment_A04_t0{1,2,4,5}`,
//! `pattern_context_A0{1,4}`, and
//! `dart/resolution/pattern_assignment/PatternAssignmentResolutionTest`),
//! plus the silent negatives that keep a plain initializer, a `for` head,
//! a bare assigned variable and a declaration's own pattern head out of
//! the face.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unused_local_variable` rows of `source`, as (offset, length).
fn local_rows(source: &str) -> Vec<(u32, u32)> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The `(offset, length)` of the `nth` (0-based) occurrence of `name`,
/// shifted by the rig's prelude — every pin below spells its locals so each
/// occurrence is unambiguous.
fn nth(source: &str, name: &str, index: usize) -> (u32, u32) {
    let mut from = 0usize;
    let mut offset = 0usize;
    for _ in 0..=index {
        offset = source[from..].find(name).expect("name occurrence") + from;
        from = offset + name.len();
    }
    let prelude = CORE_PRELUDE.len() as u32;
    (offset as u32 + prelude, name.len() as u32)
}

/// The load-bearing face: `var` in a parenthesized pattern assignment
/// reports at the binder name, and the later same-spelled read spares
/// nothing (it binds the outer declaration).
#[test]
fn var_binder_in_pattern_assignment_reports() {
    let source = "void f() {\n\
                  \x20 var aaa = 1;\n\
                  \x20 (var aaa) = 2;\n\
                  \x20 print(aaa);\n\
                  }\n";
    assert_eq!(
        local_rows(source),
        vec![nth(source, "aaa", 1)],
        "{source:?}"
    );
}

/// A `final` binder declares the same element: the row lands on the name,
/// never on the keyword.
#[test]
fn final_binder_in_pattern_assignment_reports() {
    let source = "void f() {\n\
                  \x20 var bbb = 1;\n\
                  \x20 (final bbb) = 2;\n\
                  \x20 print(bbb);\n\
                  }\n";
    assert_eq!(
        local_rows(source),
        vec![nth(source, "bbb", 1)],
        "{source:?}"
    );
}

/// A written type is its own declaring head (`(int ccc) = 2;`).
#[test]
fn typed_binder_in_pattern_assignment_reports() {
    let source = "void f() {\n\
                  \x20 var ccc = 1;\n\
                  \x20 (int ccc) = 2;\n\
                  \x20 print(ccc);\n\
                  }\n";
    assert_eq!(
        local_rows(source),
        vec![nth(source, "ccc", 1)],
        "{source:?}"
    );
}

/// The parenthesized logical-and chain (co19 `pattern_assignment_A04_t01`):
/// the binders sit under the outer group and the chain's tail is a bare
/// identifier, so the span has to reach past `&&` to the group head.
#[test]
fn binder_in_logical_and_pattern_assignment_reports() {
    let source = "void f() {\n\
                  \x20 var r = ();\n\
                  \x20 var aaa = 42, bbb = 42;\n\
                  \x20 ((var aaa, _) && r) = (1, 2);\n\
                  \x20 ((aaa, var xxx) && r) = (1, 2);\n\
                  \x20 print(aaa);\n\
                  \x20 print(bbb);\n\
                  \x20 print(r);\n\
                  }\n";
    assert_eq!(
        local_rows(source),
        vec![nth(source, "aaa", 1), nth(source, "xxx", 0)],
        "{source:?}"
    );
}

/// The list-pattern assignment (co19 `pattern_assignment_A04_t02`): the
/// `var` head sits right after the bracket.
#[test]
fn list_pattern_binder_in_assignment_pattern_reports() {
    let source = "void f() {\n\
                  \x20 var aaa = 1, bbb = 2;\n\
                  \x20 [var aaa, _] = [1, 2];\n\
                  \x20 [aaa, var b1] = [1, 2];\n\
                  \x20 print(aaa);\n\
                  \x20 print(bbb);\n\
                  }\n";
    assert_eq!(
        local_rows(source),
        vec![nth(source, "aaa", 1), nth(source, "b1", 0)],
        "{source:?}"
    );
}

/// The object-pattern assignment (co19 `pattern_assignment_A04_t05`): a
/// named field's `var` head is a pattern-element position like any other.
#[test]
fn object_pattern_binder_in_assignment_pattern_reports() {
    let source = "class Square {\n\
                  \x20 final int areaAsInt;\n\
                  \x20 final int sizeAsInt;\n\
                  \x20 Square(this.areaAsInt, this.sizeAsInt);\n\
                  }\n\
                  void f() {\n\
                  \x20 var aaa = 1;\n\
                  \x20 Square(areaAsInt: var aaa, sizeAsInt: _) = Square(1, 1);\n\
                  \x20 print(aaa);\n\
                  }\n";
    assert_eq!(
        local_rows(source),
        vec![nth(source, "aaa", 1)],
        "{source:?}"
    );
}

/// One spelling repeated inside one pattern reports once — the first
/// binder; the repeat already answers `duplicate_variable_pattern`.
#[test]
fn repeated_spelling_reports_the_first_binder_only() {
    let source = "void f() {\n\
                  \x20 var qq = 0;\n\
                  \x20 (var qq, var qq) = (1, 2);\n\
                  \x20 print(qq);\n\
                  }\n";
    assert_eq!(local_rows(source), vec![nth(source, "qq", 1)], "{source:?}");
}

/// A lone `_` is a wildcard variable: the arm spares it exactly as the
/// frame close does for every other local.
#[test]
fn wildcard_binder_never_reports() {
    let source = "void f() {\n\
                  \x20 (var _) = 1;\n\
                  }\n";
    assert_eq!(local_rows(source), Vec::new(), "{source:?}");
}

/// The trailing same-line ignore reaches the face: the row is staged
/// before the ignition pass, so the comment entry spares it.
#[test]
fn trailing_ignore_spares_the_binder() {
    let source = "void f() {\n\
                  \x20 var aaa = 1;\n\
                  \x20 (var aaa) = 2; // ignore: unused_local_variable\n\
                  \x20 print(aaa);\n\
                  }\n";
    assert_eq!(local_rows(source), Vec::new(), "{source:?}");
}

/// Silent negatives: a plain initializer, a `for` head's initializer and a
/// bare assigned variable are not assignment-pattern binders — none of
/// them may gain a row from this face.
#[test]
fn non_pattern_assignments_stay_silent() {
    let source = "void f() {\n\
                  \x20 var aaa = 1;\n\
                  \x20 var bbb = 2;\n\
                  \x20 aaa = bbb;\n\
                  \x20 (aaa) = bbb;\n\
                  \x20 for (var i = 0; i < aaa; i++) {}\n\
                  \x20 print(aaa);\n\
                  \x20 print(bbb);\n\
                  }\n";
    assert_eq!(local_rows(source), Vec::new(), "{source:?}");
}

/// A declaration's pattern head binds through the all-or-nothing group
/// (a `PatternVariableDeclaration`), not through this face: the used
/// group reports nothing, and the face adds nothing on top.
#[test]
fn declaration_pattern_head_is_not_this_face() {
    let source = "void f() {\n\
                  \x20 var (aaa, bbb) = (1, 2);\n\
                  \x20 print(aaa);\n\
                  \x20 print(bbb);\n\
                  }\n";
    assert_eq!(local_rows(source), Vec::new(), "{source:?}");
}
