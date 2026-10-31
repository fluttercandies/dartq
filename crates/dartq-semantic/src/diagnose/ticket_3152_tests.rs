//! #3152 `unused_local_variable` family pins: the oracle-exact gates this
//! ticket re-cut — the destructuring `duplicate_definition` double-add
//! retraction, the annotation-mention read credit, the same-scope-duplicate
//! first-declaration binding (`LocalScope.add`'s `??=`), the pattern-
//! assignment `var d` declaration, the same-line `// ignore:` code match,
//! and the switch-expression arm-binder window. Every expectation is a
//! verbatim oracle 3.13.7 row.

use super::pins::{report, unused_local_rows};

/// The `duplicate_definition` rows of `report`, as (offset, length) pairs.
fn duplicate_rows(source: &str) -> Vec<(u32, u32)> {
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The rows of `code` in `report`, as (offset, length) pairs.
fn code_rows(source: &str, code: &str) -> Vec<(u32, u32)> {
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// A destructuring `PatternVariableDeclaration` never reports
/// `duplicate_definition` for its own binders — the statement registers
/// each name into the enclosing statement-list scope exactly once
/// (`checkStatements`' `PatternVariableDeclarationStatement` arm). A
/// self-coincident row is the engine's double-add, not a redeclaration.
/// Oracle: `var (a, b) = (0, 1);` reports `unused_local_variable` on both
/// binders and NO `duplicate_definition`.
#[test]
fn destructure_decl_no_self_duplicate() {
    let source = "void f() {\n\
                  \x20 var (a, b) = (0, 1);\n\
                  }\n";
    assert_eq!(duplicate_rows(source), Vec::<(u32, u32)>::new());
    // Both binders are unused (unusedLocalVariable on a and b).
    //   offsets: `a` @17, `b` @19.
    assert_eq!(unused_local_rows(source), vec![(17, 1), (19, 1)]);
}

/// A single-element record pattern destructures and reads the binder —
/// no `duplicate_definition`, no `unused_local_variable` on `a`.
/// Oracle: `var (a,) = (0,); a;` reports nothing for the binder.
#[test]
fn destructure_single_used_no_duplicate() {
    let source = "void f() {\n\
                  \x20 var (a,) = (0,);\n\
                  \x20 a;\n\
                  }\n";
    assert_eq!(duplicate_rows(source), Vec::<(u32, u32)>::new());
    assert_eq!(unused_local_rows(source), Vec::<(u32, u32)>::new());
}

/// A nested destructuring registers every leaf once — `var (a, [b, c])`
/// reads `c`, so `a` and `b` report unused, and NO binder reports a
/// duplicate.
/// Oracle: `a` @21 +1, `b` @26 +1 unused; `c` @29 read — no row.
#[test]
fn destructure_nested_no_duplicate() {
    let source = "void f() {\n\
                  \x20 var (a, [b, c]) = (0, []);\n\
                  \x20 c;\n\
                  }\n";
    assert_eq!(duplicate_rows(source), Vec::<(u32, u32)>::new());
    // `a` @17, `b` @21 unused; `c` @24 read.
    assert_eq!(unused_local_rows(source), vec![(17, 1), (21, 1)]);
}

/// A destructuring inside a nested function-literal body registers its own
/// binders once even though the outer destructuring is also open — the
/// inner scope is disjoint.
/// Oracle: `var (a, b) = () { var (c, d) = (0,1); return (c,d); }();`
/// reports `a`/`b` unused and no `duplicate_definition` for `c`/`d`.
#[test]
fn destructure_nested_closure_scope_no_duplicate() {
    let source = "void f() {\n\
                  \x20 var (a, b) = () {\n\
                  \x20   var (c, d) = (0, 1);\n\
                  \x20   return (c, d);\n\
                  \x20 }();\n\
                  }\n";
    assert_eq!(duplicate_rows(source), Vec::<(u32, u32)>::new());
    // `a` @21, `b` @24 unused; `c`/`d` returned — used.
    assert_eq!(unused_local_rows(source), vec![(17, 1), (19, 1)]);
}

/// The all-underscores binder in a destructuring is not a wildcard (the
/// wildcard gate is exact `_` only, unused_local_elements_verifier.dart
/// :985-994) — `var (__) = (1);` reports `unused_local_variable` AND, with
/// the double-add retracted, no `duplicate_definition`.
/// Oracle: `__` @21 +2 unused.
#[test]
fn destructure_dunder_binder_unused_no_duplicate() {
    let source = "f() {\n\
                  \x20 var (__) = (1);\n\
                  }\n";
    assert_eq!(duplicate_rows(source), Vec::<(u32, u32)>::new());
    assert_eq!(unused_local_rows(source), vec![(12, 2)]);
}

/// A list-pattern destructuring with an all-underscores binder: the name
/// still reports `unused_local_variable` (not a wildcard) and no
/// `duplicate_definition`. Oracle: `var [__] = [1];` → `__` @20 +2.
#[test]
fn list_pattern_dunder_unused_no_duplicate() {
    let source = "f() {\n\
                  \x20 var [__] = [1];\n\
                  }\n";
    assert_eq!(duplicate_rows(source), Vec::<(u32, u32)>::new());
    assert_eq!(unused_local_rows(source), vec![(12, 2)]);
}

/// A genuine same-scope redeclaration still reports `duplicate_definition`
/// — the retraction only drops the self-coincident double-add, never a
/// real second declaration.
/// Oracle: `var i = 0; var i = 0;` → duplicate_definition at the SECOND
/// `i` and `unused_local_variable` on the first (a read binds the first
/// declaration; the duplicate never receives one).
#[test]
fn genuine_redeclaration_still_reports() {
    let source = "void f() {\n\
                  \x20 var i = 0;\n\
                  \x20 var i = 0;\n\
                  }\n";
    // Second `i` @27 is the duplicate_definition anchor.
    assert_eq!(duplicate_rows(source), vec![(27, 1)]);
    // Both `i` decls report unused (neither is read).
    assert_eq!(unused_local_rows(source), vec![(15, 1), (27, 1)]);
}

/// The first-declaration binding (upstream `LocalScope.add`'s `??=`): a
/// read after a redeclaration resolves to the FIRST declaration, so the
/// duplicate's `unused_local_variable` still reports while the first is
/// spared. Oracle `d1`: `var i=0; var i=0; print(i);` → only the second
/// `i` is unused.
#[test]
fn duplicate_read_binds_first_declaration() {
    let source = "void f() {\n\
                  \x20 var i = 0;\n\
                  \x20 var i = 0;\n\
                  \x20 print(i);\n\
                  }\n\
                  void print(Object? o) {}\n";
    // The first `i` (@15) is read; only the duplicate (@27) is unused.
    assert_eq!(unused_local_rows(source), vec![(27, 1)]);
}

/// An annotation mention credits a local read — the oracle's
/// `visitNamedType`/identifier visit marks the annotation's name used
/// (unused_local_elements_verifier.dart `_visitNameExpression` via
/// `visitNode`). Oracle: `@a` on a sibling local reads `a`.
#[test]
fn annotation_head_credits_local_read() {
    let source = "void f() {\n\
                  \x20 var a = 0;\n\
                  \x20 @a\n\
                  \x20 var b = 0;\n\
                  }\n";
    // `a` is read by the `@a` annotation — only `b` reports.
    assert_eq!(unused_local_rows(source), vec![(31, 1)]);
}

/// A same-line `// ignore: unused_field` must NOT suppress an
/// `unused_local_variable` row — the ignore matches on the diagnostic's
/// own code (upstream `IgnoreInfo` per-name match, ignore_info.dart
/// :150-163), so a wrong-code trailing ignore leaves the row live.
/// Oracle: `var x = 0; // ignore: unused_field` still reports
/// `unused_local_variable` on `x`.
#[test]
fn wrong_code_same_line_ignore_does_not_suppress() {
    let source = "void f() {\n\
                  \x20 var x = 0; // ignore: unused_field\n\
                  }\n";
    assert_eq!(unused_local_rows(source), vec![(15, 1)]);
}

/// The matching `// ignore: unused_local_variable` DOES suppress.
#[test]
fn matching_code_same_line_ignore_suppresses() {
    let source = "void f() {\n\
                  \x20 var x = 0; // ignore: unused_local_variable\n\
                  }\n";
    assert_eq!(unused_local_rows(source), Vec::<(u32, u32)>::new());
}

/// A switch-expression arm's `var` binder is its own local element — an
/// unused one reports `unused_local_variable`, and the logical-or split
/// keeps each spelling its own element (three `var a`s → three rows).
/// Oracle: `var a || var a || var a => 0` reports all three binders.
#[test]
fn switch_expression_or_binders_each_report() {
    let source = "void f() {\n\
                  \x20 var z = switch (0) {\n\
                  \x20   var a || var a || var a => 0,\n\
                  \x20   _ => 1,\n\
                  \x20 };\n\
                  \x20 print(z);\n\
                  }\n\
                  void print(Object? o) {}\n";
    // Three `var a` binders at offsets 40, 49, 58 — all
    // unused, none spared (each is its own element).
    let rows = unused_local_rows(source);
    assert_eq!(rows, vec![(40, 1), (49, 1), (58, 1)]);
}

/// A single switch-expression arm binder that is read by its arm body
/// reports nothing. Oracle: `var a => a + 1` — no unused row.
#[test]
fn switch_expression_read_binder_quiet() {
    let source = "void f() {\n\
                  \x20 var z = switch (0) {\n\
                  \x20   var a => a + 1,\n\
                  \x20   _ => 1,\n\
                  \x20 };\n\
                  \x20 print(z);\n\
                  }\n\
                  void print(Object? o) {}\n";
    assert_eq!(unused_local_rows(source), Vec::<(u32, u32)>::new());
}

/// A `var d` binder inside a pattern assignment declares nothing for the
/// flow (the oracle reports `pattern_assignment_declares_variable`), but
/// upstream's `visitDeclaredVariablePattern` still creates the element —
/// and the name resolves to no prior local (`undefined_identifier` on a
/// later read). The `var d` itself reports `unused_local_variable`.
/// Oracle `[a, var d] = y` (a, d not prior-declared): `unused_local_variable`
/// on `d` @binder; `a`'s own unused reports on `var a` decl.
#[test]
fn pattern_assignment_var_binder_reports_unused() {
    let source = "void f(List<int> y) {\n\
                  \x20 var a = 0;\n\
                  \x20 [a, var d] = y;\n\
                  }\n";
    // `a` decl @26 unused; `d` binder @42 unused (its own element).
    let rows = unused_local_rows(source);
    assert!(rows.contains(&(26, 1)), "a decl unused: {rows:?}");
    assert!(rows.contains(&(42, 1)), "var d binder unused: {rows:?}");
}
