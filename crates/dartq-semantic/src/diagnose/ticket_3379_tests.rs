//! The #3379 pins — the `if-case` ELEMENT's pattern scope: a pattern
//! variable declared in `if (e case P)` inside a collection literal is in
//! scope in the guard and the then element only, exactly as the
//! statement form's `IfStatement` case-clause scope (upstream
//! `resolution_visitor.dart` `visitIfElement` runs the same
//! `_resolveGuardedPattern` arm as `visitIfStatement`, :569-583). The
//! element fires a disjoint parser event set (`beginIfControlFlow` /
//! `handleThenControlFlow` / `handleElseControlFlow` /
//! `endIfControlFlow`|`endIfElseControlFlow` — `literal_entry_info_impl.dart`),
//! so the statement-only wiring never armed: the condition's pattern
//! shadows accumulated unconsumed, and a shadowing binder permanently
//! overwrote the outer spelling's type for the rest of the executable.
//!
//! Oracle provenance (`dart analyze` 3.13.7): a read inside the then
//! element resolves the pattern variable; the same spelling in the else
//! element or after the element reads the outer declaration (or reports
//! `undefined_identifier` when there is none). A case binder never read
//! in its scope reports `unused_local_variable` at the binder.
//!
//! The two `#[ignore]`d shadowing pins are oracle-silent but stay routed:
//! the semantic restore lands them right (`resolve_local` skips the
//! retired binder, `var_types` answers the outer declaration), while the
//! flow layer's `ThenControlFlow` arm (dartq-flow `interp_expr.rs`)
//! never runs the if-case split the statement form's `ThenBegin` runs —
//! the binder's `bind_name` leaks into the condition-false state and the
//! promoted-read face re-decides the else read on the pattern type.
//! Routing: dartq-flow `ThenControlFlow` needs the `if_case_then` arm.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `[if (x case int y) y]` — the then element reads the pattern variable:
/// silent.
#[test]
fn if_case_element_variable_resolves_in_the_then_element() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x) => [if (x case int y) y];\n\
         }\n",
    );
    assert_silent(&report);
}

/// `[if (x case int y) 0 else y]` — the else element sees the complement:
/// `y` is out of scope there, and no outer `y` exists, so the bare read
/// reports `undefined_identifier`; the never-read binder reports
/// `unused_local_variable` (the oracle's pair at the same sites).
#[test]
fn if_case_element_variable_is_out_of_scope_in_the_else_element() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x) => [if (x case int y) 0 else y];\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["unused_local_variable", "undefined_identifier"],
        "{:?}",
        rows(&report)
    );
}

/// `[if (x case int y) y, y]` — after the element the case scope is
/// closed: the trailing bare `y` has no declaration to read.
#[test]
fn if_case_element_variable_is_out_of_scope_after_the_element() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x) => [if (x case int y) y, y];\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_identifier"],
        "{:?}",
        rows(&report)
    );
}

/// A pattern variable shadowing an outer binding in the then element:
/// `var y = ''` before the element, `if (x case int y)` rebinds `y` to
/// `int` for the then element — the `int` element lands in `List<int>`
/// (an `int`-typed read is what the binder's shadowing wins; the outer
/// `String` would report `invalid_assignment` against the element slot).
/// The outer `y` is then never read at all — its `unused_local_variable`
/// is the oracle's own row.
#[test]
fn if_case_element_binder_shadows_the_outer_binding_in_the_then_element() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x) {\n\
             var y = '';\n\
             return [if (x case int y) y];\n\
           }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["unused_local_variable"],
        "{:?}",
        rows(&report)
    );
}

/// An outer `y` survives a shadowing case binder: after the element the
/// same `y` reads the outer `String` again (`y.length` resolves
/// `String.length`; an `int y` would report `undefined_getter`). The
/// semantic scope lands this (the retired binder no longer answers the
/// read, `var_types` restores `String`); the flow layer's missing
/// if-case split still re-decides the read on `int` — `#[ignore]`d
/// until dartq-flow's `ThenControlFlow` runs `if_case_then`.
#[test]
#[ignore = "routed: dartq-flow interp_expr.rs ThenControlFlow lacks the if-case arm"]
fn if_case_element_shadowed_outer_binding_reads_again_after_the_element() {
    let report = report_ops(
        "class C {\n\
           int f(Object x) {\n\
             var y = '';\n\
             var t = [if (x case int y) y + 1];\n\
             return y.length + t.length;\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

/// The else element reads the OUTER `y`: `if (x case int y) 0 else
/// y.length` resolves `String.length` through the shadowed declaration
/// (the binder itself is never read, so `unused_local_variable` stands
/// alone — the oracle's single row). Same flow-layer routing as above.
#[test]
#[ignore = "routed: dartq-flow interp_expr.rs ThenControlFlow lacks the if-case arm"]
fn if_case_element_else_element_reads_the_shadowed_outer_binding() {
    let report = report_ops(
        "class C {\n\
           int f(Object x) {\n\
             var y = '';\n\
             var t = [if (x case int y) 0 else y.length];\n\
             return t.length;\n\
           }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["unused_local_variable"],
        "{:?}",
        rows(&report)
    );
}

/// A plain `if` element carries no case clause: `[if (b) 1 else 2]` —
/// the scope wiring must not fire on a condition that bound nothing (the
/// `if_condition_begin` clear plus the empty shadow list keep the
/// surrounding frame untouched).
#[test]
fn plain_if_element_is_unchanged() {
    let report = report_ops(
        "class C {\n\
           List<int> f(bool b) => [if (b) 1 else 2];\n\
         }\n",
    );
    assert_silent(&report);
}

/// A case clause that binds nothing still runs the scope pair: `[if (x
/// case int _) x as int]` — the wildcard declares no variable, so the
/// element is a plain guard (the `as` cast reads `x`, silent); a bare
/// `y` in the then element is an ordinary undefined read, not a pattern
/// variable.
#[test]
fn if_case_element_binding_nothing_leaves_the_then_element_plain() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x) => [if (x case int _) x as int];\n\
         }\n",
    );
    assert_silent(&report);
    let unresolved = report_ops(
        "class C {\n\
           List<int> f(Object x) => [if (x case int _) y];\n\
         }\n",
    );
    assert_eq!(
        code_names(&unresolved),
        vec!["undefined_identifier"],
        "{:?}",
        rows(&unresolved)
    );
}

/// A nested `if-case` element keeps its own case scope: the inner
/// element's `b` is out of scope in its own else arm while the outer
/// `a` stays in scope across the whole inner element (the outer then).
#[test]
fn nested_if_case_elements_keep_separate_scopes() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x, Object z) =>\n\
               [if (x case int a) if (z case int b) a + b else a];\n\
         }\n",
    );
    assert_silent(&report);
    // `b` in the inner else arm is out of scope (undefined read); the
    // never-read binder reports `unused_local_variable`.
    let escaped = report_ops(
        "class C {\n\
           List<int> f(Object x, Object z) =>\n\
               [if (x case int a) if (z case int b) a else b];\n\
         }\n",
    );
    assert_eq!(
        code_names(&escaped),
        vec!["unused_local_variable", "undefined_identifier"],
        "{:?}",
        rows(&escaped)
    );
}

/// An `else if` chain: `[if (x case int a) a else if (x case String s)
/// s.length else 0]` — the inner `if-case` element opens its own case
/// scope inside the outer's else entry; each arm reads only its own
/// binder. The outer `a` is out of scope inside the inner element's
/// arms, and the inner `s` never reaches the inner else: two
/// `undefined_identifier` reads plus the never-read `s` binder's
/// `unused_local_variable` — the oracle's three rows.
#[test]
fn if_case_element_else_if_chain_scopes_each_arm() {
    let report = report_ops(
        "class C {\n\
           List<int> f(Object x) =>\n\
               [if (x case int a) a else if (x case String s) s.length else 0];\n\
         }\n",
    );
    assert_silent(&report);
    let leaked = report_ops(
        "class C {\n\
           List<int> f(Object x) =>\n\
               [if (x case int a) a else if (x case String s) a else s.length];\n\
         }\n",
    );
    assert_eq!(
        code_names(&leaked),
        vec![
            "unused_local_variable",
            "undefined_identifier",
            "undefined_identifier"
        ],
        "{:?}",
        rows(&leaked)
    );
}
