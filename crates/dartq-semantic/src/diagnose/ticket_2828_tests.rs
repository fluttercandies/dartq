//! The #2828 dead-tail pins.
//!
//! * S6 — a Never-completing operand leaves the as-cast's type the first
//!   dead node, and that face is not initializer-only: a
//!   conditional-expression arm (`condition ? x1 : ((throw 0) as int)`)
//!   owns its own `flowEnd` close (official `flowEnd(elseExpression)` /
//!   `flowEnd(thenExpression)`, type_analyzer.dart:2588/2616), so the
//!   oracle reports the cast tail there too
//!   (`least_upper_bound_greatest_closure_1_test`, 22 entries over both
//!   arm positions; upstream dead_code_verifier.dart:253-279's first dead
//!   node with the covering close's end — the row runs from the cast
//!   type's head token through the arm's own closing token, `int)` /
//!   `C1<X1>)`).
//! * E5 — the `_`-named local-function `dead_code` face gates on the
//!   library's `wildcard_variables` feature (dead_code_verifier.dart:72-82
//!   `_wildCardVariablesEnabled`): a pre-3.7 library (`// @dart = 3.4`)
//!   treats `_` as an ordinary name and stays silent (the census's 5
//!   preWildcards rows).

use super::test_rig::{code_names, report_ops, report_ops_wildcards};

/// Both arm positions of the least-upper-bound corpus shape: the dead
/// cast tail reports in each arm that holds the cast, from the cast type's
/// head token to the arm's own end.
#[test]
fn dead_cast_tail_reports_in_conditional_arms() {
    let source = r#"
void main() {
  bool condition = true;
  int x1 = 1;
  var z1 = condition ? x1 : ((throw 0) as int);
  use(z1);
  var z4 = condition ? ((throw 0) as C2) : x1;
  use(z4);
}
class C2 {}
void use(Object? x) {}
"#;
    // The rig prepends its core prelude (ASCII), so rows index the joined
    // string; the snippets render against that.
    let full = format!("{}{source}", super::test_rig::CORE_PRELUDE);
    let report = report_ops(source);
    let dead: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect();
    assert_eq!(dead, ["int)", "C2)"], "{:?}", report.diagnostics);
}

/// The bare initializer face stays reported (the probed shape,
/// `var a = f() as int;` with a Never-typed operand).
#[test]
fn dead_cast_tail_initializer_face_stays() {
    let report = report_ops(
        r#"
Never f() => throw 0;
void main() {
  var a = f() as int;
  use(a);
}
void use(Object? x) {}
"#,
    );
    assert!(
        code_names(&report).contains(&"dead_code"),
        "{:?}",
        report.diagnostics
    );
}

/// A pre-3.7 library (`// @dart = 3.4`, the census's preWildcards cases)
/// treats `_` as an ordinary local-function name: no `dead_code` row.
#[test]
fn wildcard_local_fn_dead_code_silent_pre_wildcards() {
    let report = report_ops_wildcards(
        r#"
void main() {
  void _() {}
  _();
}
"#,
        false,
    );
    assert!(
        !code_names(&report).contains(&"dead_code"),
        "{:?}",
        report.diagnostics
    );
}

/// With the feature on (the modern default posture), an `_`-named local
/// function is dead code over its whole declaration, used or not.
#[test]
fn wildcard_local_fn_dead_code_emits_with_wildcards() {
    let report = report_ops_wildcards(
        r#"
void main() {
  void _() {}
  _();
}
"#,
        true,
    );
    assert!(
        code_names(&report).contains(&"dead_code"),
        "{:?}",
        report.diagnostics
    );
}
