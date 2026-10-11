//! M0-gate wave-10 regression pins (#609 M0 singles sweep): the
//! constructor-invocation-arguments recovery anchor, the incomplete
//! const-dot-shorthand terminator, the bounded stack-overflow face, and
//! the legacy switch statement shape.
//!
//! The r5 gate-caliber rows that motivated this lane
//! (docs/audits/gate-m0/gate-caliber-r5) are stale on several of these
//! families (post-#410 statement work healed the switch shape; the
//! typedef-tail next.next EQ arm landed in 3b10ce3 with its own pins in
//! tests/declarations.rs), so every family here was re-derived against the
//! pinned oracle face before pinning. The stack-overflow pin locks the
//! ACCEPTED DIFFERENCE (#333 disclosure): the 3.13.1 oracle's
//! stack-overflow pathology surfaces three extra rows on
//! `targeted/stack_overflow.dart` (`expected_executable`, `expected_token`,
//! `unexpected_token`) that we do not replicate — dartq's bounded-depth
//! bailout is the contract, not the oracle's unbounded crash recovery.

use super::collector::setup;

/// Parses `source` with the latest feature set and returns the sorted
/// diagnostic code multiset.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut parser = setup(source);
    parser.parse_unit();
    let mut codes = parser.listener.error_codes();
    codes.sort();
    codes
}

fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

// —— constructor-invocation-arguments recovery (#609 gt-recovery) ——————

/// `const Foo;` — the missing-`(` recovery reports
/// `expectedAfterButGot('(')` (analyzer shared code `expected_token`) on
/// the LAST-CONSUMED token (`Foo`, official `parseConstructorInvocationArguments`
/// `token` parameter, parser_impl.dart:8884), splices an empty synthetic
/// argument group, and rides `parseArgumentsRest` over it — the `;`
/// follower survives as the declaration terminator.
#[test]
fn const_constructor_reference_reports_on_consumed_anchor() {
    let source = "var x = const Foo;\n";
    assert_codes(source, &["expected_token"], "const reference missing args");
    // The report anchors on `Foo` (offset 14), never on the `;` follower.
    let mut parser = setup(source);
    parser.parse_unit();
    assert_eq!(
        parser.listener.errors,
        [("expected_token".to_owned(), 14, 17)],
        "the missing-`(` report lands on the consumed identifier"
    );
}

/// `enum E { e<int> }` — the enum element is a verbatim copy of the
/// constructor-reference tail (parser_impl.dart:2948-2950), so the
/// missing-`(` recovery routes through
/// `parse_constructor_invocation_arguments`: one report on the
/// type-argument closer, an empty synthetic group, and the `}` closer is
/// NOT eaten — the enum closes with both elements intact.
#[test]
fn enum_element_type_arguments_recover_without_eating_closer() {
    assert_codes(
        "enum E {\n  e<int>,\n  other\n}\n",
        &["expected_token"],
        "enum element missing args keeps the follower",
    );
    let mut parser = setup("enum E {\n  e<int>,\n  other\n}\n");
    parser.parse_unit();
    parser.listener.assert_sequence(&[
        ("begin", "enum_declaration"),
        ("end", "enum_declaration"),
    ]);
}

/// `new Foo;` — the new-expression tail rides the same recovery: one
/// missing-`(` report on the constructor-reference end (the consumed
/// identifier), then the empty synthetic group; the `;` terminator
/// survives.
#[test]
fn new_expression_missing_args_recover_with_synthetic_args() {
    assert_codes(
        "void f() {\n  new Foo;\n}\n",
        &["expected_token"],
        "new expression missing args",
    );
}

// —— incomplete const dot shorthand (#609 dot-shorthand) ————————————————

/// `Color c = const .;` (corpus dot_shorthands/
/// const_no_identifier_recovery.dart): the pinned 3.13.1 oracle face is a
/// single `expected_token(';')` — the incomplete shorthand consumes the
/// follower `;` as its operand and the declaration terminator expect fires
/// once at the real block closer. No identifier expect may fire (the
/// 3.15 CFE template behavior is NOT the pinned face).
#[test]
fn incomplete_const_dot_shorthand_eats_terminator() {
    let mut parser = setup("void main() {\n  Color c = const .;\n}\n");
    parser.parse_unit();
    assert_eq!(
        parser.listener.error_codes(),
        ["expected_token"],
        "exactly the terminator expect, no identifier expect: {:?}",
        parser.listener.errors
    );
}

// —— stack_overflow difference surface (#333 accepted difference) ———————

/// The exact `targeted/stack_overflow.dart` shape (~700 nested parens):
/// dartq latches the bounded bailout at the nesting cap and terminates —
/// it must NEVER reproduce the oracle's unbounded-crash recovery rows
/// (`expected_executable` + `expected_token` + `unexpected_token`, the
/// accepted differential difference disclosed in #333).
#[test]
fn deep_paren_bomb_latches_bounded_bailout() {
    let source: &'static str = Box::leak(
        format!(
            "var x = {}1{};\n",
            "(".repeat(700),
            ")".repeat(700)
        )
        .into_boxed_str(),
    );
    let mut parser = setup(source);
    parser.parse_unit();
    assert!(parser.is_bailed(), "the nesting cap must latch the bailout");
    let codes = parser.listener.error_codes();
    assert!(
        codes.contains(&"stack_overflow".to_owned()),
        "the terminal diagnostic is the official stackOverflow code: {codes:?}"
    );
    assert!(
        codes.len() <= 4,
        "the bailout bounds total diagnostics on the bomb: {codes:?}"
    );
}

// —— legacy switch case constants (#609 legacy switch) ——————————————————

/// The `flow_analysis/assigned_variables/data/switch.dart` shape
/// (`@dart=2.19` legacy file): a plain constant `case 1:`, a `default:`
/// arm, and an assignment in the switch expression all parse clean on the
/// current tree (the r5 dartq-only rows predate the #410 statement fixes).
#[test]
fn legacy_switch_case_constants_parse_clean() {
    let mut parser = setup(
        "switchStatement(int a, int b, int c) {\n  switch (a = 0) {\n    case 1:\n      b = 0;\n      break;\n    default:\n      c = 0;\n      break;\n  }\n}\n",
    );
    parser.parse_unit();
    assert!(
        parser.listener.errors.is_empty(),
        "the legacy switch shape is diagnosis-free: {:?}",
        parser.listener.errors
    );
    parser.listener.assert_sequence(&[
        ("begin", "switch_statement"),
        ("end", "switch_statement"),
    ]);
}
