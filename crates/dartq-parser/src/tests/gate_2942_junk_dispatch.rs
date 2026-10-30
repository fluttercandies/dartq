//! #2942 parser residual pack: junk-name dispatch follower + the
//! member-level `None if` arm's report side.
//!
//! Item 1 — the top-level method-vs-fields follower read was one token
//! off when the head-scan spliced a synthetic name. Upstream's dispatch
//! (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:4327-4341)
//! reads `next.next` where `next` is the NAME — with a head-scan
//! synthetic occupying the name slot, the follower IS the junk token
//! itself (`void () {}` → `(` → method; `void 5() {}` → `5` → fields).
//! dartq read `look(1)` past the junk, inverting both routings and
//! double-reporting `missing_identifier` on the `void 5() {}` method
//! tail (the head-scan row plus the tail's consume-in-doubt ensure).
//! The synthetic is now threaded into `parse_top_level_method`, matching
//! upstream's `name` argument and the IDENTIFIER fast path of
//! `ensureIdentifierPotentiallyRecovered` (parser_impl.dart:4575-4579 →
//! the unconditional `handleIdentifier` at :4109): one TLF event, no
//! re-report.
//!
//! Item 2 — the member-level `None if` arm (the
//! `recoverFromInvalidMember` → `parseFields` fallback's own junk-name
//! insert) fired the slot event but reported nothing. Upstream
//! `FieldDeclarationIdentifierContext.ensureIdentifier`
//! (identifier_context_impl.dart:487-510) reports on BOTH arms: the
//! keep-set branch's `insertSyntheticIdentifier(token, this)` passes no
//! `message`, so `context.recoveryTemplate` — `expectedIdentifier`,
//! `PseudoSharedCode.missingIdentifier` — fires (parser_impl.dart:
//! 4016-4019; the `:491` "no error reported" TODO is stale). The arm's
//! earlier probed-silence note cited faces (`late ;`, `int ;`, `int }`)
//! that do not reach it — `late`/`int` heads are keyword-or-identifier
//! (the `None` arm's `ensure_identifier`) and `}`/EOF heads are claimed
//! by `expectedClassMember` upstream of the call — so the probe premise
//! could not be re-derived from source; the source-traced behavior
//! wins. The arm now reports `missing_identifier` at the junk and splits
//! keep-set (insert before, junk stays for the grammar) from
//! consume-in-doubt (synthetic spliced after the junk), mirroring the
//! official decision order.
//!
//! Pinned: `void () {}` routes method (one TLF event, one
//! `missing_identifier`); `void 5() {}` routes fields (TLV event, single
//! `missing_identifier`, no TLF shadow); member-level `int ;`/`int 5;`
//! carry the keep/consume reports; named controls unchanged.

use super::collector::{Collector, setup};
use crate::parser::Parser;

/// Whether a ZERO-length identifier event fired in `context` at
/// `target`'s offset inside `needle` (same shape as the #2941 gate).
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

/// Whether an identifier event fired for `target` inside `needle`'s span
/// in `context` (the named-declaration control shape).
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

// —— item 1: the junk-name dispatch follower ————————————————————————————

/// `void () {}`: the `(` junk occupies the follower slot upstream — the
/// method path runs with the head-scan synthetic as `name` (one TLF
/// event on it, the head-scan's single `missing_identifier` row, no
/// tail re-report).
#[test]
fn junk_paren_name_routes_method() {
    let source = "void () {}";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "(", "(", "TopLevelFunctionDeclaration"),
        "the head-scan synthetic must carry the TLF event: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "TopLevelFunctionDeclaration"), 1);
    assert_eq!(
        ident_count(c, "TopLevelVariableDeclaration"),
        0,
        "{:?}",
        c.idents
    );
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
}

/// `void 5() {}`: the `5` junk is the follower upstream — the fields
/// path runs with the head-scan synthetic as `name` (one TLV event, one
/// `missing_identifier`); the `5 () {}` residue reparses as invalid
/// declarations behind the member loop. No TLF shadow.
#[test]
fn junk_literal_name_routes_fields() {
    let source = "void 5() {}";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "5", "5", "TopLevelVariableDeclaration"),
        "the head-scan synthetic must carry the TLV event: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "TopLevelVariableDeclaration"), 1);
    assert_eq!(
        ident_count(c, "TopLevelFunctionDeclaration"),
        0,
        "{:?}",
        c.idents
    );
    // Head-scan `missing_identifier` is the only name-slot row — the old
    // double-report (head + method tail) is gone.
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
}

/// `var = 42;`: the real-name control for the fields side — unchanged.
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

/// `void f() {}`: the real-name control for the method side — unchanged.
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

// —— item 2: the member-level `None if` arm reports ——————————————————————

/// `class C { void ; }`: the `void` type commits itself (unlike `int`,
/// which needs an identifier after it — `int ;` parses `int` as the
/// NAME and never reaches this arm), so `;` arrives at the fields name
/// slot through `recoverFromInvalidMember` → `parseFields`. `;` is in
/// the keep-set — the synthetic inserts BEFORE it, `missing_identifier`
/// fires at the `;` (upstream `expectedIdentifier` via
/// `context.recoveryTemplate`), and the slot event still rides the
/// synthetic.
#[test]
fn member_semicolon_junk_reports_missing_identifier() {
    let source = "class C { void ; }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, ";", ";", "FieldDeclaration"),
        "the inserted synthetic must fire its slot event: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "FieldDeclaration"), 1);
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
}

/// `class C { void 5; }`: `5` is outside the keep-set — consume-in-doubt,
/// the synthetic splices after the junk, and `missing_identifier` fires
/// at the `5`. The `;` then terminates the field cleanly (no
/// `expected_token` shadow).
#[test]
fn member_literal_junk_reports_missing_identifier() {
    let source = "class C { void 5; }";
    let mut p = setup(source);
    p.parse_unit();
    let c = &p.listener;
    assert!(
        synthetic_named(c, source, "5;", ";", "FieldDeclaration"),
        "the spliced synthetic must fire its slot event at the `5` end: {:?}",
        c.idents
    );
    assert_eq!(ident_count(c, "FieldDeclaration"), 1);
    assert_eq!(code_count(c, "missing_identifier"), 1, "{:?}", c.errors);
    assert_eq!(code_count(c, "expected_token"), 0, "{:?}", c.errors);
}

/// `class C { static int x; }`: the named member control — unchanged.
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
