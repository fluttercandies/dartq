//! `computeVariablePatternType` follower-set contract (official
//! `type_info.dart:392-411` riding on `computeType`'s follower decision,
//! :305-385). A bare identifier names a variable-pattern type only when
//! the follower is a name (`looksLikeName`, type_info_impl.dart:487), a
//! `?` before a name, or a `Function` group; every other follower
//! (`)`, `:`, `}`, `,`, `;`, operators) yields noType, so
//! `parsePrimaryPattern` (parser_impl.dart:11820-11894) reaches the
//! identifier branch and — refutable context, not `_`, no `.` — falls
//! through to a CONSTANT pattern. The noType decision is pure lookahead:
//! the compute layer emits no events, and `handle_type` fires only when a
//! typed variable pattern commits. On top, `TYPE as` / `TYPE when`
//! resolve to TYPE being the pattern itself (sdk#52199): the follower
//! `when` stays a guard instead of a variable-pattern name.

use super::collector::{Collector, Logged, setup};

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    p.listener
}

/// The events inside the outermost `(begin, pattern)` .. `(end, pattern)`
/// pair — the pattern subtree proper. Parameter/return types and the
/// scrutinee fire outside it, so negative assertions are scoped here.
fn pattern_window<'a>(c: &'a Collector, label: &str) -> &'a [Logged] {
    let start = c.log.iter().position(|e| *e == ("begin", "pattern")).unwrap_or_else(
        || panic!("{label}: no pattern events; log: {:?}", c.log),
    );
    let mut depth = 0usize;
    for (i, e) in c.log.iter().enumerate().skip(start) {
        if *e == ("begin", "pattern") {
            depth += 1;
        } else if *e == ("end", "pattern") {
            depth -= 1;
            if depth == 0 {
                return &c.log[start + 1..i];
            }
        }
    }
    panic!("{label}: unbalanced pattern events; log: {:?}", c.log);
}

/// Asserts the constant-pattern face: silent parse; inside the pattern
/// subtree a `constant_pattern` pair around the operand's send — a bare
/// identifier fires the official argument-less variant
/// (`handleSendWithoutArguments`, listener.dart:1368, from `parseSend`'s
/// short-circuit at parser_impl.dart:9433, which also skips the No*
/// events) — and no type or variable-pattern events (the noType path
/// never commits a type).
fn assert_constant_pattern(c: &Collector, label: &str) {
    assert!(c.errors.is_empty(), "{label}: {:?}", c.errors);
    let window = pattern_window(c, label);
    assert!(
        window.contains(&("begin", "constant_pattern")),
        "{label}: no begin constant_pattern; window: {:?} (full: {:?})",
        window,
        c.log
    );
    assert!(
        window.contains(&("end", "constant_pattern")),
        "{label}: no end constant_pattern; window: {:?} (full: {:?})",
        window,
        c.log
    );
    assert!(
        window.contains(&("handle", "send_without_arguments")),
        "{label}: no operand send; window: {:?} (full: {:?})",
        window,
        c.log
    );
    assert!(
        !window.contains(&("handle", "type")),
        "{label}: unexpected type event; window: {:?} (full: {:?})",
        window,
        c.log
    );
    assert!(
        !window.contains(&("handle", "declared_variable_pattern")),
        "{label}: parsed as a variable pattern; window: {:?} (full: {:?})",
        window,
        c.log
    );
    assert!(
        !window.contains(&("handle", "assigned_variable_pattern")),
        "{label}: parsed as an assigned variable pattern; window: {:?} (full: {:?})",
        window,
        c.log
    );
}

/// `if (x case int)`: the bare type literal before `)` is a constant
/// pattern, not a (nameless) variable pattern — the `)` follower is
/// officially noType (type_info.dart:305-385), so the identifier branch
/// falls through to the constant-expression parse (:11878-11893).
#[test]
fn bare_type_before_close_paren_is_constant_pattern() {
    let c = parse("void f(Object x) { if (x case int) {} }");
    assert_constant_pattern(&c, "if-case int)");
}

/// Same face at a switch-case label: the `:` follower is noType.
#[test]
fn bare_type_before_colon_is_constant_pattern() {
    let c = parse("void f(Object o) { switch (o) { case int: break; } }");
    assert_constant_pattern(&c, "switch case int:");
}

/// The `when` follower looks like a name, so `computeType` does compute a
/// type — but the sdk#52199 override (type_info.dart:398-407) resolves
/// `TYPE when` to TYPE being the pattern: constant pattern, `when` stays
/// the caller-parsed guard.
#[test]
fn type_when_resolves_to_constant_pattern_plus_guard() {
    let c = parse("void f(Object x, bool z) { if (x case int when z) {} }");
    assert_constant_pattern(&c, "if-case int when z");
}

/// Positive control: an identifier follower (`TYPE identifier`) still
/// computes a type — `case int y` is a typed variable pattern whose
/// commit emits `handle_type` plus the variable-pattern event, with no
/// constant-pattern events.
#[test]
fn name_follower_still_yields_a_typed_variable_pattern() {
    let c = parse("void f(Object x) { if (x case int y) { y; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(
        c.log.contains(&("handle", "type")),
        "no type event; log: {:?}",
        c.log
    );
    assert!(
        c.log.contains(&("handle", "declared_variable_pattern")),
        "no declared variable pattern; log: {:?}",
        c.log
    );
    assert!(
        !c.log.contains(&("begin", "constant_pattern")),
        "unexpected constant pattern; log: {:?}",
        c.log
    );
}

/// Positive control for the nullable face: `?` before a name keeps the
/// type (`simpleNullableType`, type_info.dart:364-375).
#[test]
fn nullable_type_follower_still_yields_a_typed_variable_pattern() {
    let c = parse("void f(Object x) { if (x case int? y) { y; } }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(
        c.log.contains(&("handle", "type")),
        "no type event; log: {:?}",
        c.log
    );
    assert!(
        c.log.contains(&("handle", "declared_variable_pattern")),
        "no declared variable pattern; log: {:?}",
        c.log
    );
    assert!(
        !c.log.contains(&("begin", "constant_pattern")),
        "unexpected constant pattern; log: {:?}",
        c.log
    );
}
