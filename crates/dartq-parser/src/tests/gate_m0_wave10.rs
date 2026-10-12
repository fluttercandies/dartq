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

use super::collector::{Collector, setup};

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
    parser
        .listener
        .assert_sequence(&[("begin", "enum_declaration"), ("end", "enum_declaration")]);
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
/// const_no_identifier_recovery.dart): the pinned 3.13.1 oracle faces
/// (trend-w4 gate raw output + live probe) are `missing_identifier` on the
/// follower `;` (kept unconsumed, real-token anchor) and
/// `expected_token(';')` at the dot (the terminator's walked-back anchor
/// over the synthetic operand). The earlier single-row pin came from a
/// stale probe dump.
#[test]
fn incomplete_const_dot_shorthand_missing_identifier() {
    let mut parser = setup("void main() {\n  Color c = const .;\n}\n");
    parser.parse_unit();
    assert_eq!(
        parser.listener.error_codes(),
        ["missing_identifier", "expected_token"],
        "identifier expect on the follower + terminator expect at the dot: {:?}",
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
    let source: &'static str =
        Box::leak(format!("var x = {}1{};\n", "(".repeat(700), ")".repeat(700)).into_boxed_str());
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
    parser
        .listener
        .assert_sequence(&[("begin", "switch_statement"), ("end", "switch_statement")]);
}

// —— #839 r5/r6-triage batch: single-point re-verification pins ————————————
//
// Each shape was re-probed live against `dart analyze --format=machine`
// 3.13.1 before pinning (probe rows in docs/audits/p839-r6-triage.md); the
// r5 gate-caliber table predates the W3/W4 listener work, so several of its
// rows were stale. A face that already matched the oracle is pinned here to
// keep it matched; no production code changed for a dead divergence.

/// The `keyword_named_*_methods_2.dart` face: a statement keyword in
/// EXPRESSION position (`return break(x-1) + 1;`) routes through
/// `ExpressionIdentifierContext.ensureIdentifier` — `looksLikeStatementStart`
/// splices a synthetic name in front of the keyword (`missing_identifier`)
/// and the keyword survives to the statement loop, which re-parses it as a
/// break/continue/try statement (`break_outside_of_loop` via the loop guard,
/// `missing_catch_or_finally` via `only_try`, `expected_body` via
/// `ensureBlock(TryStatement)`; probed rows 28:10/53:10/313:10 in
/// `keyword_named_top_level_methods_2.dart`).
#[test]
fn expression_position_statement_keywords_reparse_as_statements() {
    assert_codes(
        "void f() {\n  return break(x-1) + 1;\n}\n",
        &[
            "break_outside_of_loop",
            "expected_token",
            "expected_token",
            "missing_identifier",
        ],
        "break in expression position",
    );
    assert_codes(
        "void f() {\n  return continue(x-1) + 1;\n}\n",
        &[
            "continue_outside_of_loop",
            "expected_token",
            "expected_token",
            "missing_identifier",
        ],
        "continue in expression position",
    );
    assert_codes(
        "void f() {\n  return try(x-1) + 1;\n}\n",
        &[
            "expected_body",
            "expected_token",
            "missing_catch_or_finally",
            "missing_identifier",
        ],
        "try in expression position",
    );
}

/// `set s;` — the missing-`(` recovery splices a SYNTHETIC empty group, and
/// the official setter arity check suppresses the row when the group's `(` is
/// synthetic (`ast_builder.dart:6475 !leftParenthesis.isSynthetic`). The
/// stale-count hazard is pinned with a two-parameter predecessor: the
/// synthetic group must reset the count AND suppress, not ride the stale
/// value. Both arms mirror each other (top-level `ast_builder.dart:6257`,
/// class member :6457). `set s() {}` — real empty parens — still reports.
#[test]
fn setter_missing_formals_suppress_arity_on_synthetic_group() {
    assert_codes(
        "void a(int x, int y) {}\n\nset s;\n",
        &["missing_function_body", "missing_function_parameters"],
        "top-level `set s;` after a two-parameter member",
    );
    assert_codes(
        "class C {\n  void m(int x, int y) {}\n  set s2;\n}\n",
        &["missing_method_parameters"],
        "class `set s2;` after a two-parameter method",
    );
    assert_codes(
        "class D {\n  set s3() {}\n}\n",
        &["wrong_number_of_parameters_for_setter"],
        "class `set s3() {}` real empty parens still report",
    );
}

/// Doubled `required` reports `extraneous_modifier` on the FIRST keyword
/// even inside a named group, plus `duplicated_modifier` on the repeat
/// (probed 3.13.1 rows 1:9+1:18 named, 3:8+3:17 positional); a lone
/// `required` in a named group stays silent.
#[test]
fn doubled_required_reports_extraneous_on_first_in_named_group() {
    assert_codes(
        "void a({required required x}) {}\n",
        &["duplicated_modifier", "extraneous_modifier"],
        "named doubled required",
    );
    assert_codes(
        "void c(required required x) {}\n",
        &["duplicated_modifier", "extraneous_modifier"],
        "positional doubled required",
    );
    assert_codes(
        "void d({required x}) {}\n",
        &[],
        "lone named required stays silent",
    );
}

/// A function-typed formal with a `var`/`final` prefix reports BOTH
/// `extraneous_modifier` and `function_typed_parameter_var` on the keyword
/// (probed 3.13.1: `void g(var g()) {}` rows 8:8 len3 twice).
#[test]
fn function_typed_parameter_var_also_reports_extraneous_modifier() {
    assert_codes(
        "void g(var g()) {}\n",
        &["extraneous_modifier", "function_typed_parameter_var"],
        "var function-typed formal",
    );
    assert_codes(
        "void h(final g()) {}\n",
        &["extraneous_modifier", "function_typed_parameter_var"],
        "final function-typed formal",
    );
}

/// #891: top-level `factory Foo.bar();` — the `factory` arm reports
/// `factory_top_level_declaration` and hands the stream back at `Foo`
/// (`force_advance` = the official consume-on-invalidation posture), and the
/// unit loop re-parses the cascade: `Foo.bar` rides the missing-`(`
/// synthetic group (`missing_function_parameters` on the name), the `.`-led
/// junk body reports `missing_function_body`, the `.` re-enters as an
/// invalid declaration (`expected_executable`), and `bar();` parses as a
/// fresh member whose `;` body reports the second `missing_function_body`
/// (probed 3.13.1: 5 rows for the `;` shape, 4 with a real `{}` body).
#[test]
fn top_level_factory_cascade_reparses_the_tail() {
    // (expectations in the helper's sorted order; dartq's emission order —
    // the factory row first, then the re-parse cascade — matches the
    // oracle's machine-output order, probed 3.13.1.)
    assert_codes(
        "factory Foo.bar();\n",
        &[
            "expected_executable",
            "factory_top_level_declaration",
            "missing_function_body",
            "missing_function_body",
            "missing_function_parameters",
        ],
        "top-level factory with `;` tail",
    );
    assert_codes(
        "factory Foo.bar() {}\n",
        &[
            "expected_executable",
            "factory_top_level_declaration",
            "missing_function_body",
            "missing_function_parameters",
        ],
        "top-level factory with `{}` tail",
    );
}

// —— dot-shorthands gate (#908) ———————————————————————————————————————————
//
// The feature ships in 3.10 (`enabledIn: '3.10.0'`); the shared parser has no
// flag for it, the analyzer's `AstBuilder` reports `experiment_not_enabled`
// from its listener handlers (`ast_builder.dart:4129`/`4143`/`1456`). Head
// and context share the dot token and the analyzer dedupes identical
// same-offset errors, so the probed 3.13.1 oracle face (files pinned below
// 3.10 via `// @dart`) is one report per head anchor plus one at the `const`
// keyword of a const shorthand.

/// Parses `source` at language version `major.minor` and returns the
/// diagnostic (code, start, end) rows in report order.
fn rows_at(source: &'static str, major: u32, minor: u32) -> Vec<(String, u32, u32)> {
    let (table, text) = super::collector::scan_ok(source);
    let features =
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(major, minor),
        ));
    let mut parser = crate::Parser::new(text, table, features, Collector::default());
    parser.parse_unit();
    parser
        .listener
        .errors
        .iter()
        .map(|(c, s, e)| (c.clone(), *s, *e))
        .collect()
}

/// The default (latest) face stays silent: every existing dot-shorthand pin
/// runs at >= 3.10, so the gate must not perturb it.
#[test]
fn dot_shorthand_gate_is_silent_at_latest_version() {
    assert_eq!(
        rows_at("enum E { a }\nvoid f() { var x = E.a == .a; }\n", 3, 15),
        []
    );
    assert_eq!(rows_at("enum E { a }\nfinal x = const .a;\n", 3, 15), []);
    // Plain selectors are never shorthand heads at any version.
    assert_eq!(rows_at("void f() { var x = foo.bar; }\n", 3, 9), []);
}

/// `.a` at 3.9: exactly one `experiment_not_enabled` report, anchored at the
/// dot token (the deduped head+context anchor, oracle probe 3.13.1:
/// `ERROR|SYNTACTIC_ERROR|EXPERIMENT_NOT_ENABLED|…|4|14|1`).
#[test]
fn dot_shorthand_gate_reports_once_at_the_dot_below_3_10() {
    let source = "enum E { a }\nvoid f() {\n  if (E.a == .a) {}\n}\n";
    let dot = source.rfind(".a").expect("shorthand dot") as u32;
    assert_eq!(
        rows_at(source, 3, 9),
        vec![("experiment_not_enabled".to_string(), dot, dot + 1)],
        "one report at the dot, nothing else"
    );
}

/// `const .a` at 3.9: the head anchor at the dot plus the const-keyword
/// anchor (`endConstDotShorthand`, `ast_builder.dart:1456`; oracle probe:
/// `…|3|11|5|This requires the 'dot-shorthands' language feature to be
/// enabled.` + `…|3|17|1|` same text).
#[test]
fn const_dot_shorthand_gate_reports_dot_and_const_below_3_10() {
    let source = "enum E { a }\nfinal x = const .a;\n";
    let const_kw = source.find("const").expect("const keyword") as u32;
    let dot = source.rfind(".a").expect("shorthand dot") as u32;
    assert_eq!(
        rows_at(source, 3, 9),
        vec![
            ("experiment_not_enabled".to_string(), dot, dot + 1),
            ("experiment_not_enabled".to_string(), const_kw, const_kw + 5),
        ],
        "dot head anchor + const keyword anchor"
    );
}
