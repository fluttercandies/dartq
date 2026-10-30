//! #2941 parser residual pack: the name-slot synthetic faces #2932's gate
//! did not reach.
//!
//! Item 1 — `parse_fields`'s synthetic hand-ins. Official `parseFields`
//! routes its `name` argument through `ensureIdentifierPotentiallyRecovered`
//! (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:4450-4454), which
//! fires `handleIdentifier` at :4109 UNCONDITIONALLY — on the head-scan's
//! inserted synthetic just as on a parsed name. Two port arms handed a
//! synthetic to `first_name` without the event: the top-level head-scan's
//! `Some(synthetic_name)` (`var = 42;` — the `<unnamed>` top-level variable
//! element exists upstream, e.g. ErrorParserTest's
//! test_expectedExecutable_topLevel_afterVoid `unused_element` row) and the
//! member-level junk-name insert (the `recoverFromInvalidMember` →
//! `parseFields` fallback — `class C { void ; }` carries the
//! FieldDeclaration event upstream). Both now fire `handle_identifier(span,
//! context)` where `context` is the slot's own resolution
//! (parser_impl.dart:4447-4449: `topLevelVariableDeclaration` for
//! `DeclarationKind.TopLevel`, `fieldDeclaration` otherwise).
//!
//! Item 2 — `TopLevelFunctionDeclaration` gate audit verdict: ADDED. (a)
//! The name slot of a top-level function reaches the synthetic tail today:
//! `parse_top_level_method`'s `ensure_identifier(TopLevelFunctionDeclaration)`
//! runs on whatever the dispatch left at the name slot — a junk token under
//! a method follower (`void 5() {}` — cursor on `5`, follower `(` at
//! look(1)) routes to the tail's consume-in-doubt branch. (b) The #2915
//! Face-A census (report_pre_wave5_1030_0828.tsv) carries the nameless
//! top-level-executable element face — `unused_element` on `<unnamed>`,
//! e.g. `parser/generated/error_parser/ErrorParserTest/
//! test_expectedExecutable_topLevel_afterVoid#0` (missing-side). Upstream
//! fires the event on the synthetic unconditionally (parser_impl.dart:4038),
//! so the context joins the gate.
//!
//! Same-invariant residual fixed alongside: the built-in declaration-head
//! arm in `ensure_identifier` (`augment mixin M {}` — `mixin` reparses as
//! the mixin head while the variable slot takes a synthetic) also returned
//! its synthetic without the slot event; upstream branch 1 of
//! `TopLevelDeclarationIdentifierContext.ensureIdentifier`
//! (identifier_context_impl.dart:1321-1332) feeds the same unconditional
//! :4038 fire.
//!
//! Pinned: TLV on the top-level head-scan synthetic, FieldDeclaration on
//! the member-level silent insert, TLF on the tail, TLV on the
//! built-in-head arm, and the named-declaration order-stability controls.

use super::collector::{Collector, setup};
use crate::parser::Parser;

/// Whether a ZERO-length identifier event fired in `context` at `target`'s
/// offset inside `needle` (same shape as the #2932 gate: recovery synthetics
/// splice at the offending token's own offset with an empty lexeme).
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

/// Count of a diagnostic code.
fn code_count(c: &Collector, code: &str) -> usize {
    c.errors.iter().filter(|(k, _, _)| k == code).count()
}

// —— item 1: the parse_fields synthetic hand-ins —————————————————————————

/// `var = 42;` at top level: the head-scan inserts the synthetic name in
/// front of `=` and reports `missing_identifier`; the hand-in now fires
/// TopLevelVariableDeclaration on it (parser_impl.dart:4109 unconditional
/// — upstream the `<unnamed>` field element exists, cf. the census's
/// unused_element `<unnamed>` face). Exactly one `missing_identifier` row:
/// the slot does not re-report.
#[test]
fn top_level_head_scan_synthetic_fires_tlv_event() {
    let source = "var = 42;";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "=", "=", "TopLevelVariableDeclaration"),
        "the head-scan synthetic must fire its slot event: {:?}",
        c.idents
    );
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
    assert_eq!(ident_count(c, "TopLevelVariableDeclaration"), 1);
}

/// `class C { void ; }`: a committed type followed by junk at the member
/// name slot routes through `recoverFromInvalidMember` → `parseFields`,
/// and `parse_fields`'s `None if` arm inserts the synthetic itself. The
/// same :4109 unconditional fire carries the FieldDeclaration event on it.
#[test]
fn member_junk_name_insert_fires_field_event() {
    let source = "class C { void ; }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "void ;", ";", "FieldDeclaration"),
        "the inserted synthetic must fire its slot event: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "FieldDeclaration"), 1);
}

/// `var x = 42;`: the named control — exactly one TLV event, on the real
/// name, and no errors.
#[test]
fn named_top_level_variable_event_unchanged() {
    let source = "var x = 42;";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(c, source, "x", "x", "TopLevelVariableDeclaration"));
    assert_eq!(ident_count(c, "TopLevelVariableDeclaration"), 1);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

/// `class C { static int x; }`: the named member control — exactly one
/// FieldDeclaration event on the real name.
#[test]
fn named_field_event_unchanged() {
    let source = "class C { static int x; }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(c, source, "x", "x", "FieldDeclaration"));
    assert_eq!(ident_count(c, "FieldDeclaration"), 1);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

// —— item 2: TopLevelFunctionDeclaration joins the gate ———————————————————

/// `void f ! () {}`: the `!`-after-name recovery still routes to the
/// method path (the dispatch's BANG skip reads past the real name's `!`
/// to `(`), `parse_top_level_method`'s ensure fires
/// TopLevelFunctionDeclaration on the real name, and
/// `parse_method_type_var` reports the `!` itself — the tail arm the
/// #2941 gate audit cited remains reachable through this shape (the
/// junk-name variant `void 5() {}` reroutes to the fields path under
/// #2942's follower fix — see gate_2942).
#[test]
fn nameless_top_level_function_fires_name_event() {
    let source = "void f ! () {}";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        named_in(c, source, "f", "f", "TopLevelFunctionDeclaration"),
        "the name event must fire on the real name: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "TopLevelFunctionDeclaration"), 1);
    assert_eq!(code_count(c, "unexpected_token"), 1, "{:?}", c.errors);
}

/// `void f() {}`: the named control — exactly one TLF event on the real
/// name, no synthetic shadow.
#[test]
fn named_top_level_function_event_unchanged() {
    let source = "void f() {}";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(named_in(c, source, "f", "f", "TopLevelFunctionDeclaration"));
    assert_eq!(ident_count(c, "TopLevelFunctionDeclaration"), 1);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

// —— same-invariant residual: the built-in declaration-head arm ———————————

/// `int mixin M {}` at top level: `mixin` reads as the next declaration's
/// head (the follower is not `=`/`,`/`;`), so the field slot takes the
/// spliced synthetic and `mixin` reparses as the mixin head. The arm now
/// fires TopLevelVariableDeclaration on it — the same unconditional fire
/// the template's returned identifier carries upstream
/// (identifier_context_impl.dart:1321-1332 → parser_impl.dart:4038).
#[test]
fn builtin_head_synthetic_fires_tlv_event() {
    let source = "int mixin M {}";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "mixin", "mixin", "TopLevelVariableDeclaration"),
        "the built-in-head synthetic must fire its slot event: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "TopLevelVariableDeclaration"), 1);
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
}
