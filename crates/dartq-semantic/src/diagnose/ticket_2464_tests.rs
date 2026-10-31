//! The #2464 pins: the statement-level `dead_code` anchor law, locked to
//! the `dart analyze` 3.13.x oracle triples.
//!
//! Upstream, `NullSafetyDeadCodeVerifier.flowEnd`
//! (upstream Dart SDK source, `pkg/analyzer/lib/src/error/dead_code_verifier.dart`,
//! `flowEnd`) reports `deadCode.atOffset(offset: firstDeadNode.offset,
//! length: node.end - firstDeadNode.offset)`: the row opens at the FIRST
//! NODE visited unreachable and spans through the construct that closes
//! the interval. Two consequences the fixtures below pin:
//!
//! * a dead then/else BLOCK opens at its `{` and runs through its `}`;
//! * a dead STATEMENT inside a live block opens at the statement's first
//!   token and covers the statement including its `;` — the loop/`throw`/
//!   `return`/`continue` tails below all report `length == 2` for `n;`; a
//!   nested block's tail instead runs on through that block's `}`, the
//!   flow window's own close (the matrix's `(141, 6)` row, `"6;\n  }"`).
//!
//! The ticket's third fixture (`try {} catch (e) { return; } finally { x; }`)
//! is pinned as a NEGATIVE: a `finally` body always executes, so the oracle
//! reports nothing there and neither may dartq. The pre-fix hypothesis
//! ("anchor at the expression or the semicolon") does not reproduce on this
//! tree — every row below was probed against the oracle and matched before
//! pinning.
//!
//! Anchors are asserted as `(offset, length)` triples: `dead_code` carries
//! a fixed message ("Dead code."), so the span IS the observable.

use crate::diagnose::test_rig::report_ops_bare;

/// The `dead_code` rows of `report` as `(offset, length)`, source order.
fn dead_rows(report: &super::FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "dead_code")
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// `if (true) { } else { ... }`: the else block is the first dead node, so
/// the row is the whole block — `{` at 34 through `}` at 45, length 12.
/// Oracle-probed (`dart analyze`, both sides identical).
#[test]
fn if_else_dead_block_anchors_at_block_brace() {
    let fixture = "void f() {\n  if (true) {\n  } else {\n    1;\n  }\n}";
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(34, 12)]);
}

/// `for (;;) { break; n; }`: the tail statement is dead, anchored at the
/// literal's token with its `;` inside the span (`length == 2`), not at the
/// loop's closing brace and not trimmed to the expression.
#[test]
fn loop_tail_dead_statement_anchors_at_statement_head() {
    let fixture = "void g() {\n  for (;;) {\n    break;\n    1;\n  }\n}";
    assert_eq!(dead_rows(&report_ops_bare(fixture)), [(39, 2)]);
}

/// `try {} catch (e) { return; } finally { n; }`: the `finally` body runs
/// on both the normal and the thrown path, so it is reachable and reports
/// NO row. Pinned as the negative the #2464 hypothesis got wrong.
#[test]
fn finally_body_after_returning_catch_stays_live() {
    let fixture =
        "void h() {\n  try {\n  } catch (e) {\n    return;\n  } finally {\n    1;\n  }\n}";
    assert_eq!(dead_rows(&report_ops_bare(fixture)), []);
}

/// The statement-tail anchor matrix — every `dead_code` row is the dead
/// statement's own `n;`, whatever terminates the preceding path:
///
/// * `throw 1; 2;` — throw tail (`24, 2`);
/// * `while (true) { break; 3; }` — break tail (`72, 2`);
/// * `return; 4;` — return tail in a void function (`104, 2`);
/// * a nested block whose `throw 5;` kills `6;` (`141, 6`) — the row runs
///   from the dead statement through the inner block's `}`, i.e.
///   `"6;\n  }"`, the flow window's own close rather than the statement's;
/// * `for (;;) { 7; continue; 8; }` — continue tail (`199, 2`).
#[test]
fn statement_tail_anchor_matrix_matches_oracle() {
    let fixture = "void a() {\n  throw 1;\n  2;\n}\n\
                   void b() {\n  while (true) {\n    break;\n    3;\n  }\n}\n\
                   void c() {\n  return;\n  4;\n}\n\
                   void d() {\n  {\n    throw 5;\n    6;\n  }\n}\n\
                   void e() {\n  for (;;) {\n    7;\n    continue;\n    8;\n  }\n}";
    // The offsets below are the oracle's for exactly this text: any edit to
    // the literal's whitespace moves them and fails loudly.
    assert_eq!(
        dead_rows(&report_ops_bare(fixture)),
        [(24, 2), (72, 2), (104, 2), (141, 6), (199, 2)]
    );
}
