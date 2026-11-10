//! The #3173 Lane C `dead_code` pins: the assignability switch-chain —
//! statement members entered dead and the never-matching constant — and
//! the switch-expression arm rows (upstream Dart SDK source
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart, `flowEnd`'s
//! SwitchMember arm :237-241 — `deadCode.at(node.keyword)` when the member
//! itself is the first dead node — and the member trim
//! `node.statements.last` :275-279 when a body statement is).
//!
//! Chain law: a member whose unguarded pattern covers the whole scrutinee
//! (`case int() || 0`, `case _`) absorbs the match — every member after
//! it (up to the `default` member, whose own chain the flow layer owns)
//! reports its keyword ONLY: the member is the first dead node, so its
//! body statements die inside the same dead range. A constant-root case
//! disjoint from the scrutinee is the inverse — the member stays
//! reachable and the body span reports. A switch expression's dead arm
//! is one dead node: the whole `pattern => body` span.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

fn dead_ranges(source: &str) -> Vec<(usize, usize)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            let offset = d.range.offset as usize - crate::diagnose::test_rig::CORE_PRELUDE.len();
            (offset, offset + d.range.length as usize)
        })
        .collect()
}

/// The absorbing case's successor reports its `case` keyword only —
/// `case 1:` in `case int() || 0: … case 1:` is the member-dead face
/// (`test_deadPattern_switchStatement_nextCases` pins `case` tokens).
#[test]
fn ticket_3173_statement_chain_reports_keyword_only() {
    let source = "void f(int x) {\n  switch (x) {\n    case int() || 0:\n      break;\n    case 1:\n      break;\n  }\n}";
    let dead = dead_ranges(source);
    let kw = source.find("case 1").unwrap();
    assert!(
        dead.contains(&(kw, kw + 4)),
        "case 1 keyword row missing: {dead:?}"
    );
    // The member's body rows are the flow layer's call — `visitNode`'s
    // dead-interval poll opens on the statements inside the dead member
    // and `flowEnd(member)` trims to `statements.last` when a body
    // statement latched first; this lane's product is the `case` keyword
    // row alone, so the body span stays unpinned here.
}

/// A guarded absorber never starts the chain — `when` keeps the match
/// undecidable, so the later member stays live.
#[test]
fn ticket_3173_guarded_head_starts_no_chain() {
    let source = "void f(int x) {\n  switch (x) {\n    case int() when x > 0:\n      break;\n    case 1:\n      break;\n  }\n}";
    let dead = dead_ranges(source);
    let kw = source.find("case 1").unwrap();
    assert!(
        !dead.iter().any(|&(s, e)| s == kw && e == kw + 4),
        "guarded head must not chain: {dead:?}"
    );
}

/// `case null:` over a non-nullable scrutinee is the never-match face —
/// the member stays reachable and only its body reports.
#[test]
fn ticket_3173_never_matching_constant_reports_body_only() {
    let source = "void f(int x) {\n  switch (x) {\n    case null:\n      break;\n    case 1:\n      break;\n  }\n}";
    let dead = dead_ranges(source);
    let body = source.find("break;").unwrap();
    assert!(
        dead.contains(&(body, body + 6)),
        "never-match body row missing: {dead:?}"
    );
    // The member head stays live — no `case null` keyword row.
    let kw = source.find("case null").unwrap();
    assert!(
        !dead.iter().any(|&(s, _)| s == kw),
        "never-match must not report the keyword: {dead:?}"
    );
}

/// The chain stops at the `default` member — a `default` before a later
/// `case` cuts the chain (the `default` member's own dead rows belong to
/// the flow layer), so the trailing `case` past `default:` reports its
/// keyword and the chain still reaches it through the cases-only index.
#[test]
fn ticket_3173_chain_reaches_case_before_default() {
    let source = "void f(int x) {\n  switch (x) {\n    case int() || 0:\n      break;\n    case 1:\n      break;\n    default:\n      break;\n  }\n}";
    let dead = dead_ranges(source);
    let kw = source.find("case 1").unwrap();
    assert!(
        dead.contains(&(kw, kw + 4)),
        "case 1 keyword row missing: {dead:?}"
    );
}

/// A switch expression's dead arm reports the whole `pattern => body`
/// span — `int() => 1` after the absorbing `int() || 0 => 0`
/// (`test_deadPattern_switchExpression_logicalOrPattern_nextCases`
/// underlines the arm whole).
#[test]
fn ticket_3173_expression_chain_reports_whole_arm() {
    let source = "Object f(int x) {\n  return switch (x) {\n    int() || 0 => 0,\n    int() => 1,\n    _ => 2,\n  };\n}";
    let dead = dead_ranges(source);
    // `int() => 1` — pattern start through the body's `1`.
    let start = source.find("int() => 1").unwrap();
    let end = source.find("=> 1").unwrap() + 4;
    assert!(
        dead.contains(&(start, end)),
        "whole-arm row missing ({start}..{end}): {dead:?}"
    );
    // `_ => 2` — the trailing arm, `}`-bounded.
    let start2 = source.find("_ => 2").unwrap();
    let end2 = source.find("=> 2").unwrap() + 4;
    assert!(
        dead.contains(&(start2, end2)),
        "last-arm row missing ({start2}..{end2}): {dead:?}"
    );
}

/// A plain disjoint-constant switch stays silent — no absorbing case, no
/// never-matching constant, nothing dead.
#[test]
fn ticket_3173_plain_switch_stays_silent() {
    let report = report_ops(
        "void f(int x) {\n  switch (x) {\n    case 1:\n      break;\n    case 2:\n      break;\n  }\n}",
    );
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}
