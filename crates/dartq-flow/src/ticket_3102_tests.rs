//! #3102 — the merged member's `default` keyword row.
//!
//! The parser folds every head that shares one body into a single member
//! (`case A: case B: default: <body>`), so a member that carries case heads
//! ships no head event for its trailing `default` — `Ev::CaseBegin` names a
//! keyword only for a default-headed member, and `Ev::CaseEnd` is the first
//! event past the shared body that even mentions it. The official driver
//! visits each head with `switchStatement_beginAlternative` →
//! `handleSwitchBeforeAlternative` → (`handleDefault`, whose `flowEnd`
//! reports at `node.keyword`) before the body runs (upstream Dart SDK
//! source, type_analyzer.dart:2253-2272; dead_code_verifier.dart:237-246),
//! so a `default` head entered on an exhausted chain rows its own keyword.
//! The clause's chain after its case heads is that head's entry state.
//!
//! Every expectation below is the `dart analyze` 3.13.7 machine-format A/B
//! over the `dead_code` fixtures `/tmp`-rooted in #3096, re-expressed as the
//! exact source text each row covers.

use crate::ticket_3096_tests::dead_of;

/// Oracle `5:5|7`: the or-pattern head `int() || 0` covers every `int`, so
/// the `default` head is dead beside the dead `case 1:` member. The `|| 0`
/// row is the logical-or operand row (`3:16|4`) the or-frame already pins.
#[test]
fn merged_default_head_after_exhausting_or_head_rows_keyword() {
    let source = "\
void f(int x) {
  switch (x) {
    case int() || 0:
    case 1:
    default:
      break;
  }
}
";
    assert_eq!(
        dead_of(source),
        vec![
            "|| 0".to_string(),
            "case".to_string(),
            "default".to_string()
        ]
    );
}

/// Oracle `4:5|7`: `int()` covers the `int` scrutinee, so the merged
/// member's `default` head is dead on entry.
#[test]
fn merged_default_head_after_covering_object_head_rows_keyword() {
    let source = "\
void f(int x) {
  switch (x) {
    case int():
    default:
      break;
  }
}
";
    assert_eq!(dead_of(source), vec!["default".to_string()]);
}

/// Oracle `4:5|4` + `5:5|7`: the case head is dead already, and the
/// `default` head behind it rows its own keyword.
#[test]
fn merged_default_head_after_dead_case_head_rows_both_keywords() {
    let source = "\
void f(int x) {
  switch (x) {
    case int():
    case 1:
    default:
      break;
  }
}
";
    assert_eq!(
        dead_of(source),
        vec!["case".to_string(), "default".to_string()]
    );
}

/// Oracle `4:5|7`: a wildcard head matches every value, so the merged
/// member's `default` head is the dead one.
#[test]
fn merged_default_head_after_wildcard_head_rows_keyword() {
    let source = "\
void f(Object x) {
  switch (x) {
    case _:
    default:
      break;
  }
}
";
    assert_eq!(dead_of(source), vec!["default".to_string()]);
}

/// Oracle: no rows — none of the case heads exhausts `int`, so the merged
/// member's `default` head stays live.
#[test]
fn live_merged_default_head_stays_silent() {
    let source = "\
void f(int x) {
  switch (x) {
    case 0:
    case 1:
    default:
      break;
  }
}
";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// Oracle: no rows — a default-headed member of its own group stays live,
/// and the group after it is not reached (`return;`).
#[test]
fn live_default_group_stays_silent() {
    let source = "\
void f(int x) {
  switch (x) {
    case 0:
      return;
    default:
      break;
  }
}
";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// The keyword row fires exactly once: the merged member's own `default`
/// head is live here, so the row comes from the next case-headed group's
/// opener (the deferral #3171 pins), never twice.
#[test]
fn merged_default_head_rows_once_with_a_following_group() {
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
    default:
      break;
    case 2:
      break;
  }
}
";
    assert_eq!(
        dead_of(source),
        vec![
            "default".to_string(),
            "case".to_string(),
            "break;".to_string()
        ]
    );
}

/// Oracle `5:5|7` + `6:7|6`: the default-headed group after an exhausting
/// case group is dead on entry (its `default` keyword and its body row).
#[test]
fn dead_default_group_after_exhausting_case_rows_keyword_and_body() {
    let source = "\
void f(int x) {
  switch (x) {
    case int():
      break;
    default:
      break;
  }
}
";
    assert_eq!(
        dead_of(source),
        vec!["default".to_string(), "break;".to_string()]
    );
}

/// Oracle `3:5|7`, `5:5|4`, `6:7|6`, `7:5|4`, `8:7|6`: the first group's
/// live `default` rows its keyword once the later groups are dead, and each
/// later member rows its own keyword — the accounting must not re-report
/// the carried `default_kw` at the third member.
#[test]
fn live_default_rows_once_across_two_dead_case_groups() {
    let source = "\
void f(int x) {
  switch (x) {
    default:
      break;
    case 1:
      break;
    case 2:
      break;
  }
}
";
    assert_eq!(
        dead_of(source),
        vec![
            "default".to_string(),
            "case".to_string(),
            "break;".to_string(),
            "case".to_string(),
            "break;".to_string()
        ]
    );
}
