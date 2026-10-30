//! #2932 parser: the nameless-declaration event faces. Official
//! `ensureIdentifier` fires `handleIdentifier` on the recovered identifier
//! UNCONDITIONALLY (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:4031-4040 — the
//! context template reports the recovery row and inserts the synthetic
//! first; the parser-level wrapper then fires the event at :4038 whatever
//! identifier came back), so every name-producing declaration context's
//! synthetic slot carries the event. The no-parens catch recovery's
//! synthetic parameter carries the CatchParameter event too:
//! `insertParens(includeIdentifier: true)` (token_stream_rewriter.dart:29)
//! makes the re-parse's name slot an identifier, whose event fires via
//! :4038 after the single `catch_syntax` row (:10741). Pinned here: one
//! synthetic name event per enabled declaration context, the catch slot's
//! event beside its single row, and the named-declaration order-stability
//! controls (the real name's event count is unchanged — exactly one
//! CatchParameter for `catch (e)`, no synthetic Typedef shadow).

use super::collector::{Collector, setup};
use crate::parser::Parser;

/// Whether a ZERO-length identifier event fired in `context` at `target`'s
/// offset inside `needle` (the recovery synthetic splices at the offending
/// token's own offset — `insertSyntheticIdentifier`'s empty lexeme).
fn synthetic_named(c: &Collector, source: &str, needle: &str, target: &str, context: &str) -> bool {
    let at = source.find(needle).unwrap_or_else(|| {
        panic!("needle {needle:?} absent from source");
    }) + needle
        .find(target)
        .unwrap_or_else(|| panic!("target {target:?} absent from needle {needle:?}"));
    let at = at as u32;
    c.idents
        .iter()
        .any(|(k, s, e, _)| k == context && *s == at && *e == at)
}

/// Whether an identifier event fired for `target` inside `needle`'s span in
/// `context` (the named-declaration control shape).
fn named_in(c: &Collector, source: &str, needle: &str, target: &str, context: &str) -> bool {
    let at = source.find(needle).unwrap_or_else(|| {
        panic!("needle {needle:?} absent from source");
    }) + needle
        .find(target)
        .unwrap_or_else(|| panic!("target {target:?} absent from needle {needle:?}"));
    let (start, end) = (at as u32, (at + target.len()) as u32);
    c.idents
        .iter()
        .any(|(k, s, e, _)| k == context && *s == start && *e == end)
}

/// Count of identifier events in `context`.
fn ident_count(c: &Collector, context: &str) -> usize {
    c.idents.iter().filter(|(k, _, _, _)| k == context).count()
}

/// Count of a structural event in the log.
fn count(c: &Collector, phase: &'static str, name: &'static str) -> usize {
    c.log
        .iter()
        .filter(|got| got.0 == phase && got.1 == name)
        .count()
}

/// Count of a diagnostic code.
fn code_count(c: &Collector, code: &str) -> usize {
    c.errors.iter().filter(|(k, _, _)| k == code).count()
}

// —— nameless typedef ———————————————————————————————————————————————————

/// `typedef = void Function();`: the missing name recovers synthetically
/// before the `=` (keep-set follower, branch-2 drain posture) and the slot
/// fires its TypedefDeclaration event — the parser_impl.dart:4038
/// unconditional fire the recovery used to swallow.
#[test]
fn nameless_typedef_fires_name_event() {
    let source = "typedef = void Function();";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "typedef =", "=", "TypedefDeclaration"),
        "the synthetic typedef name must fire its event: {:?}",
        c.idents
    );
    // The recovery report precedes the event upstream (template entry vs
    // the wrapper's :4038 fire) and stays the slot's only row.
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
}

// —— nameless catch —————————————————————————————————————————————————————

/// `on A catch }`: the no-parens recovery's synthetic parameter fires the
/// CatchParameter event (zero-length at the offending token), with the
/// clause's single `catch_syntax` row and the empty formals-list pair.
#[test]
fn nameless_catch_fires_catch_parameter_event() {
    let source = "void f() { try {} on A catch } }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "catch }", "}", "CatchParameter"),
        "the synthetic catch parameter must fire its event: {:?}",
        c.idents
    );
    assert_eq!(code_count(c, "catch_syntax"), 1, "{:?}", c.errors);
    // Two formal lists: the function's own `()` plus the catch recovery's.
    assert_eq!(count(c, "begin", "formal_parameters"), 2);
    assert_eq!(count(c, "end", "formal_parameters"), 2);
}

// —— named-declaration order-stability controls ——————————————————————————

/// `typedef F = int;`: the real name's event is the slot's only one — the
/// recovery faces above must not leak a synthetic shadow into the named
/// parse.
#[test]
fn named_typedef_emits_exactly_one_name_event() {
    let source = "typedef F = int;";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(c, source, "F", "F", "TypedefDeclaration"));
    assert_eq!(ident_count(c, "TypedefDeclaration"), 1, "{:?}", c.idents);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

/// `catch (e)`: exactly one CatchParameter event (the pre-scan slot) — the
/// no-parens change must not double-fire the normal path.
#[test]
fn named_catch_emits_exactly_one_catch_parameter_event() {
    let source = "void f() { try {} catch (e) {} }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(c, source, "(e)", "e", "CatchParameter"));
    assert_eq!(ident_count(c, "CatchParameter"), 1, "{:?}", c.idents);
}

// —— real-name stability over the remaining enabled contexts —————————————

// The TopLevelVariableDeclaration, FieldDeclaration, and
// LocalFunctionDeclaration synthetic slots have no reachable junk-peek
// face in the current callers: the fields' name slot routes junk to the
// head-scan's synthetic hand-in or the silent member fallback (both parse
// the name without an ensure call), and the local-function dispatches
// pre-verify an identifier head before the name slot. The arms stay in
// the gate (upstream fires unconditionally — parser_impl.dart:4038 — so
// any face that reaches the slot must carry the event); the pins here
// lock the REAL-name faces those contexts still own, whose event stream
// the tail restructure must not shift.

/// Top-level `int = 42;`: the head is consumed as the field's name (the
/// type is the missing face) and the TopLevelVariableDeclaration event
/// rides the real name.
#[test]
fn top_level_variable_real_name_event_unchanged() {
    let source = "int = 42;";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(
        c,
        source,
        "int",
        "int",
        "TopLevelVariableDeclaration"
    ));
    assert_eq!(ident_count(c, "TopLevelVariableDeclaration"), 1);
    assert_eq!(code_count(c, "missing_const_final_var_or_type"), 1);
}

/// Class `int 5;`: the FieldDeclaration event rides the real name `int`
/// (the member name-missing-type face).
#[test]
fn field_real_name_event_unchanged() {
    let source = "class C { int 5; }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(c, source, "int 5", "int", "FieldDeclaration"));
    assert_eq!(ident_count(c, "FieldDeclaration"), 1);
}

/// Local `int <> () {}`: the statement dispatch reads a local function
/// named `int` (the identifier-head pre-verification), so the
/// LocalFunctionDeclaration event rides the real name.
#[test]
fn local_function_real_name_event_unchanged() {
    let source = "void f() { int <> () {} }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(
        c,
        source,
        "int <>",
        "int",
        "LocalFunctionDeclaration"
    ));
    assert_eq!(ident_count(c, "LocalFunctionDeclaration"), 1);
}

// —— the reachable synthetic faces over the remaining contexts ———————————

/// `void () {}` in a class body: the method name slot's `(` follower takes
/// the branch-2 posture and the MethodDeclaration event fires.
#[test]
fn nameless_method_fires_name_event() {
    let source = "class C { void () {} }";
    let mut p = setup(source);
    p.parse_unit();
    assert!(
        synthetic_named(&p.listener, source, "void (", "(", "MethodDeclaration"),
        "{:?}",
        p.listener.idents
    );
}

/// Local `var = 3;`: the local-var walk consumes `var` as the modifier and
/// the name slot's `=` follower keeps the branch-2 drain posture — the
/// LocalVariableDeclaration event fires on the synthetic.
#[test]
fn nameless_local_variable_fires_name_event() {
    let source = "void f() { var = 3; }";
    let mut p = setup(source);
    p.parse_unit();
    assert!(
        synthetic_named(
            &p.listener,
            source,
            "var =",
            "=",
            "LocalVariableDeclaration"
        ),
        "{:?}",
        p.listener.idents
    );
}

/// `typedef R = ({int});`: the record-type named field's required name slot
/// fires the RecordFieldDeclaration event on its synthetic.
#[test]
fn nameless_record_field_fires_name_event() {
    let source = "typedef R = ({int});";
    let mut p = setup(source);
    p.parse_unit();
    assert!(
        synthetic_named(&p.listener, source, "int}", "}", "RecordFieldDeclaration"),
        "{:?}",
        p.listener.idents
    );
}
