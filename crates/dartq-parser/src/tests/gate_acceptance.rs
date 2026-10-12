//! Acceptance-gate P0 pins (#914): the valid-syntax families the first gate
//! run flagged, each anchored to a 3.13.1 oracle probe and the official
//! parser sources.
//!
//! * `varAndType` on primary-constructor declaring parameters: the official
//!   gate reports `typeAfterVar` only when
//!   `memberKind != MemberKind.PrimaryConstructor` (parser_impl.dart:2283-
//!   2298; probed 3.13.1: `class C(var int a) {}` is clean — at every
//!   language version, the analyzer never version-gates the syntax — while
//!   `void f(var int x) {}` reports).
//! * record types as type arguments: the `<(` group decision rides the
//!   record-content probe (`_checkIfRecordTypeParenthesisAreRecovered`,
//!   type_info_impl.dart:882-943). Every position below is probed clean on
//!   the 3.13.1 oracle (`List<(int, int)>` as a formal/named-formal/
//!   return/field/top-level type, nested `List<List<(int, int)>>`, the
//!   extends/implements/with/on clauses, type-parameter bounds, typedefs,
//!   invocations, literals, the trailing-comma `(int,)` and empty `()`
//!   records, and metadata-carrying fields); `(int)` — one positional
//!   field, no comma — is rejected by the walk's tail verdict
//!   (:937-941).

use super::collector::{Collector, latest_features, scan_ok};
use crate::Parser;

/// Sorted diagnostic-code multiset of one whole-unit parse.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let (table, text) = scan_ok(source);
    let mut p = Parser::new(text, table, latest_features(), Collector::default());
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

/// Asserts the exact diagnostic multiset (codes only, sorted).
fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

/// Asserts a clean parse (zero diagnostics).
fn assert_clean(source: &'static str, label: &'static str) {
    assert_codes(source, &[], label);
}

// —— varAndType on primary-constructor declaring parameters ————————————————

/// `var` + a type in declaring parameters is legal modern syntax (probed
/// 3.13.1: zero rows; official gate keys on the member kind,
/// parser_impl.dart:2283-2298).
#[test]
fn primary_constructor_var_and_type_clean() {
    assert_clean("class C(var int a) {}\n", "declaring var+type");
}

/// Both declaring parameters of the acceptance-corpus shape stay clean
/// (primary_constructors.dart:14 carried two var_and_type gate rows).
#[test]
fn primary_constructor_multi_var_params_clean() {
    assert_clean(
        "class Point(var int x, var int y);\n",
        "declaring var+type x2",
    );
}

/// A normal formal position keeps reporting `varAndType` (probed 3.13.1;
/// the oracle adds `extraneousModifier`, the documented :2271-2277
/// reduction this parser pins).
#[test]
fn normal_formal_var_and_type_reports() {
    assert_codes(
        "void f(var int x) {}\n",
        &["var_and_type"],
        "normal formal var+type",
    );
}

/// A member formal position keeps reporting `varAndType` too.
#[test]
fn member_formal_var_and_type_reports() {
    assert_codes(
        "class C {\n  void m(var int x) {}\n}\n",
        &["var_and_type"],
        "member formal var+type",
    );
}

// —— record types as type arguments —————————————————————————————————————————

/// Formal-parameter position — the patterns.dart:137 gate row (`int
/// sumPairs(List<(int, int)> pairs)`).
#[test]
fn record_type_arg_formal_parameter_clean() {
    assert_clean(
        "int sumPairs(List<(int, int)> pairs) => 0;\n",
        "formal param",
    );
}

/// Named-group formal position (probed 3.13.1: `{List<(int, int)>? n}`).
#[test]
fn record_type_arg_named_formal_clean() {
    assert_clean("void f({List<(int, int)>? x}) {}\n", "named formal");
}

/// Return-type and top-level/field/local variable positions.
#[test]
fn record_type_arg_type_positions_clean() {
    assert_clean("List<(int, int)> f() => const [];\n", "return type");
    assert_clean("List<(int, int)> g = const [];\n", "top-level variable");
    assert_clean("class H {\n  List<(int, int)> f = const [];\n}\n", "field");
    assert_clean(
        "void main() {\n  List<(int, int)> x = const [];\n}\n",
        "local variable",
    );
}

/// Nested shape through a compound closer (probed 3.13.1).
#[test]
fn record_type_arg_nested_clean() {
    assert_clean("List<List<(int, int)>> f() => const [];\n", "nested shape");
}

/// Class-clause positions: extends, implements, with, on, and a
/// type-parameter bound (each probed 3.13.1 with zero syntax rows).
#[test]
fn record_type_arg_class_clauses_clean() {
    assert_clean(
        "class A<T> {}\nclass B extends A<(int, int)> {}\n",
        "extends clause",
    );
    assert_clean(
        "class A<T> {}\nclass B implements A<(int, String)> {}\n",
        "implements clause",
    );
    assert_clean(
        "class A<T> {}\nmixin M<T> {}\nclass C extends A<int> with M<(int, int)> {}\n",
        "with clause",
    );
    assert_clean("extension E on List<(int, int)> {}\n", "on clause");
    assert_clean(
        "class B<T extends List<(int, int)>> {}\n",
        "type-parameter bound",
    );
}

/// Typedef positions: alias RHS, named-argument RHS, and a record type as
/// a generalized-function-type parameter.
#[test]
fn record_type_arg_typedefs_clean() {
    assert_clean("typedef Alias = List<(int, int)>;\n", "typedef alias");
    assert_clean(
        "typedef Named = Map<String, (int, int)>;\n",
        "typedef named record arg",
    );
    assert_clean(
        "typedef Fn = void Function((int, int));\n",
        "typedef gft record param",
    );
}

/// Invocation type arguments and typed collection literals.
#[test]
fn record_type_arg_invocation_and_literal_clean() {
    assert_clean(
        "class A<T> {\n  A();\n}\nA<(int, int)> make() => A<(int, int)>();\n",
        "invocation type args",
    );
    assert_clean(
        "void main() {\n  final x = <(int, int)>[];\n  final y = <String, (int, int)>{};\n}\n",
        "typed literals",
    );
}

/// Trailing-comma single-field and empty record types as type arguments
/// (probed 3.13.1: `List<(int,)>` and `List<()>` are clean; the walk's
/// `hasComma` verdict keeps them).
#[test]
fn record_type_arg_trailing_comma_and_empty_clean() {
    assert_clean("List<(int,)> f() => const [];\n", "trailing-comma record");
    assert_clean("void f(List<(int,)> x) {}\n", "trailing-comma formal");
    assert_clean("List<()> f() => const [];\n", "empty record");
}

/// Metadata over a record-type field (probed 3.13.1: zero syntax rows —
/// only the resolver's undefined-annotation face, not a parser row).
#[test]
fn record_type_arg_metadata_field_clean() {
    assert_clean(
        "typedef Meta = List<(@meta int, int)>;\n",
        "metadata record field",
    );
}

/// The walk's tail verdict: one positional field with no comma is a
/// parenthesized type, not a record (type_info_impl.dart:937-941), so the
/// `<` group is not a type-argument group and the parse derails exactly as
/// the oracle does.
#[test]
fn parenthesized_type_as_single_type_arg_reports() {
    assert_codes(
        "void f(List<(int)> x) {}\n",
        &["expected_token"],
        "single-field no-comma type arg",
    );
}

// —— record types as collection-literal type arguments (#922) ———————————————

/// Typed collection literals over record-type arguments, const and
/// non-const (probed 3.13.1: zero rows). The acceptance-gate row on
/// constants.dart:54 was read off a release binary predating the #914
/// probe rewrite; the parse entry (`parseLiteralListSetMapOrFunction`,
/// parser_impl.dart:8772 — `computeTypeParamOrArg` with
/// `inDeclaration = true`) was already wired through the same probe.
#[test]
fn record_type_arg_collection_literals_clean() {
    // The exact acceptance-corpus line.
    assert_clean(
        "const canonicalPairs = <(int, int)>{(1, 2), (3, 4)};\n",
        "const set literal, record type args",
    );
    // All four record shapes as the literal's type argument.
    assert_clean(
        "var a = <(int, int)>[];\nvar b = <(int,)>[];\nvar c = <()>[];\nvar d = <(int, String, {bool b})>[];\n",
        "record shapes as literal type args",
    );
    // The same shapes under `const` variable initializers.
    assert_clean(
        "const e = <(int, int)>[];\nconst f = <(int,)>[];\nconst g = <()>[];\nconst h = <(int, String, {bool b})>[];\n",
        "const record shapes",
    );
    // Nested generics and record keys/values in maps.
    assert_clean(
        "var i = <List<(int, int)>>[];\nvar j = <Map<(int, int), String>>[];\nvar k = <(int, int), String>{(1, 2): 'a'};\n",
        "nested and map shapes",
    );
    // A `const <...>` expression prefix rides the same entry.
    assert_clean("var l = const <(int, int)>[];\n", "const-expression prefix");
}

/// Record-returning generalized function types as literal type arguments
/// (`computeRecordTypeGFT`/`computeRecordTypeQuestionGFT`,
/// type_info_impl.dart:982/:1019: computeRest from the `)` walks the
/// `Function` suffix chain; probed 3.13.1: zero rows). The scan-side
/// skip length covers the chain so the `<` group closes on its own `>`
/// and the follower decision never mistakes the `Function` formals for a
/// function-literal body.
#[test]
fn record_type_arg_gft_suffix_clean() {
    assert_clean(
        "var a = <(int, int) Function()>[];\nvar b = <(int, int)? Function()>[];\n",
        "record-return GFT type args",
    );
    assert_clean(
        "var c = <((int, int) Function(), int)>[];\nvar d = <({int a}) Function()>[];\n",
        "record field/return combinations",
    );
}

/// The tail verdict in the literal type-argument position: `<(int)>` is no
/// type-argument group, so `<` derails as a relational operator exactly as
/// the oracle (probed 3.13.1: EXPECTED_TOKEN `Expected to find '['` at the
/// `<` plus the comparison-chain row; the oracle's undefined-operator row
/// is resolver-tier, not a parser-face code).
#[test]
fn parenthesized_type_as_literal_type_arg_reports() {
    assert_codes(
        "var j = <(int)>[];\n",
        &["equality_cannot_be_equality_operand", "expected_token"],
        "single-field no-comma literal type arg",
    );
}

// —— type-lookahead family: recovered method type args, GFT suffix
// chains, `void<...>` anchoring (#940) ——————————————————————————————————————

/// Exact diagnostic rows (code, start, end in UTF-16) of one whole-unit
/// parse, in emission order.
fn parse_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut p = Parser::new(text, table, latest_features(), Collector::default());
    p.parse_unit();
    p.listener.errors.clone()
}

/// A group scan that recovered is a misjudged relational chain, never type
/// arguments: `computeMethodTypeArguments` keeps the group only under
/// `mayFollowTypeArgs && !typeArg.recovered` (type_info.dart:485-491).
/// 3.13.1 oracle probe of `f<int>(a < b);` inside `g`: ZERO syntax rows —
/// the sole oracle row is the resolver-tier ARGUMENT_TYPE_NOT_ASSIGNABLE —
/// and exactly one `f<int>` application event pair (the old face stacked
/// expected_token/expected_type_name rows on spurious type-argument
/// parses).
#[test]
fn method_type_args_recovered_scan_is_relational_arg() {
    let source = "void f<T>(T t) {}\nvoid g(int a, int b) {\n  f<int>(a < b);\n}\n";
    assert_eq!(parse_rows(source), vec![], "zero syntax rows");
    let (table, text) = scan_ok(source);
    let mut p = Parser::new(text, table, latest_features(), Collector::default());
    p.parse_unit();
    let applied = p
        .listener
        .log
        .iter()
        .filter(|e| *e == &("begin", "type_arguments"))
        .count();
    assert_eq!(applied, 1, "exactly the f<int> application group");
}

/// computeRest loops the whole `Function <...>? (...) ?` suffix chain
/// (type_info_impl.dart:1094-1133), so the scan-side skip length of
/// `Function() Function()` covers every link. 3.13.1 oracle probe of the
/// chain as a record field in a literal type argument: zero rows.
#[test]
fn gft_chain_function_arm_group_scan_clean() {
    assert_eq!(
        parse_rows("var x = <(Function() Function(),)>[];\n"),
        vec![],
        "chained Function suffix inside a record field"
    );
}

// —— `<(` literal type-argument family: computeRecordType's `!required`
// follower veto at the group guard + the missing-comma continuations
// (#941) ————————————————————————————————————————————————————————————————————

/// The `<(` guard runs `computeType(required: false)` and commits the group
/// only for an unrecovered record/GFT-record shape (type_info.dart:454-468).
/// The `computeRecordType` `!required` follower veto
/// (type_info_impl.dart:771-860) rejects a record whose follower after `)`
/// (and an optional `?`) is an identifier not followed by a declaration
/// boundary — `>`, the type-arg closer, is never in the acceptance set — so
/// `<(...)` with junk shapes is not type arguments at all and `<` derails
/// into the typed-literal recovery (expectedButGot('[') at the `<`, the
/// live-probed `parenthesized_type_as_literal_type_arg_reports` sibling).
/// Each shape then rides the relational/record-literal cascade below.
///
/// Faces are machinery-derived (every row from a live-validated link; the
/// decision order mirrors _fe_analyzer_shared-91.0.0 == 3.15-main byte for
/// byte); the serial-window oracle probe re-confirms them end to end.
///
/// * A `<(int, int) x>` — veto; `[<`-derail, walk-back `;` at `)`, junk
///   member `x` (no-type field + walk-back), unit restarts on `>`, `[]`, `;`.
/// * B `<(int, int) async>` — veto; the paren-group follower `async` reads
///   as a function literal (`parseParenthesizedExpression...`: async arm),
///   the body is missing at `>` and the comparison-chain row lands there.
/// * C `<(int, int) Function>` — veto (pseudo-keyword `Function` is not the
///   raw-identifier kind `looksLikeTypeParamOrArg` reads); record literal,
///   then the junk-member restart cascade.
#[test]
fn lt_paren_follower_veto_derailed_faces() {
    assert_eq!(
        parse_rows("var x = <(int, int) x>[];\n"),
        vec![
            ("expected_token".to_string(), 8, 9),
            ("expected_token".to_string(), 18, 19),
            ("missing_const_final_var_or_type".to_string(), 20, 21),
            ("expected_token".to_string(), 20, 21),
            ("expected_executable".to_string(), 21, 22),
            ("expected_executable".to_string(), 22, 24),
            ("unexpected_token".to_string(), 24, 25),
        ],
        "veto shape A: junk identifier x after the record"
    );
    assert_eq!(
        parse_rows("var x = <(int, int) async>[];\n"),
        vec![
            ("expected_token".to_string(), 8, 9),
            ("missing_function_body".to_string(), 25, 26),
            ("equality_cannot_be_equality_operand".to_string(), 25, 26),
        ],
        "veto shape B: async follower reads as a function literal"
    );
    assert_eq!(
        parse_rows("var x = <(int, int) Function>[];\n"),
        vec![
            ("expected_token".to_string(), 8, 9),
            ("expected_token".to_string(), 18, 19),
            ("missing_const_final_var_or_type".to_string(), 20, 28),
            ("expected_token".to_string(), 20, 28),
            ("expected_executable".to_string(), 28, 29),
            ("expected_executable".to_string(), 29, 31),
            ("unexpected_token".to_string(), 31, 32),
        ],
        "veto shape C: bare Function follower"
    );
}

/// A record-return GFT with a real identifier follower commits through the
/// guard (computeRest's `!required` veto reads the token after the `()?`
/// suffix), so D parses as type arguments and the junk `x` is a missing
/// comma: `looksLikeTypeParamOrArg` (raw IDENTIFIER kind + identifier/`,`
/// /closer follower) routes parseUnexpectedEnd's entry into parseMissingComma
/// (type_info_impl.dart:1462-1467), which anchors expectedButGot(',') on the
/// junk token and keeps parsing `x, int` as arguments. The scan-side twin
/// (type_info_impl.dart:1399-1404) keeps the group unrecovered, so
/// `f<...>(...)` method-type-args acceptance stays available.
#[test]
fn record_gft_missing_comma_anchors_on_the_junk() {
    assert_eq!(
        parse_rows("var x = <(int, int) Function() x, int>[];\n"),
        vec![("expected_token".to_string(), 31, 32)],
        "veto shape D: missing comma at x, then x and int parse as args"
    );
}

/// Guard-rejection controls. n4 `(int) Function()?`: the single positional
/// field without a comma fails the record content probe (the recovered flag
/// of the official `_checkIfRecordTypeParenthesisAreRecovered` tail), so the
/// group is rejected and `<` derails into the shared cascade. F6
/// `List<(int) Function()>?`: the inner `<(` is rejected the same way, the
/// outer group's lone argument is bare `List`, and parseUnexpectedEnd walks
/// to the scanner-paired real outer `>` (start.endGroup is non-null) — one
/// expectedAfterButGot('>') row anchored on the last consumed argument, then
/// a clean `[]` literal; F6c shows the tail row when the derailed remainder
/// reaches the statement end. V4/V5/V6 stay clean: real record shapes with
/// `,`/`>`/`?` followers pass the veto.
#[test]
fn record_type_args_guard_rejection_controls() {
    assert_eq!(
        parse_rows("var x = <(int) Function()?>[];\n"),
        vec![
            ("expected_token".to_string(), 8, 9),
            ("expected_token".to_string(), 13, 14),
            ("missing_function_body".to_string(), 25, 26),
            ("expected_executable".to_string(), 25, 26),
            ("expected_executable".to_string(), 26, 27),
            ("expected_executable".to_string(), 27, 29),
            ("unexpected_token".to_string(), 29, 30),
        ],
        "n4: single-field record derails the literal"
    );
    assert_eq!(
        parse_rows("var x = <List<(int) Function()>?>[];\n"),
        vec![("expected_token".to_string(), 9, 13)],
        "F6: inner rejection leaves one walk-to-endGroup row"
    );
    assert_eq!(
        parse_rows("var x = <List<(int, int)>[]>;\n"),
        vec![
            ("expected_token".to_string(), 24, 25),
            ("expected_token".to_string(), 28, 29),
        ],
        "F6b: valid inner record, junk [ tail after the outer group"
    );
    assert_eq!(
        parse_rows("var x = <List<(int) Function()>[]>;\n"),
        vec![
            ("expected_token".to_string(), 9, 13),
            ("expected_token".to_string(), 34, 35),
        ],
        "F6c: derailed tail reports the missing [ at the ;"
    );
    assert_eq!(parse_rows("var x = <(int, int)>[];\n"), vec![], "V4");
    assert_eq!(
        parse_rows("var x = <(int, int), String>[];\n"),
        vec![],
        "V5"
    );
    assert_eq!(parse_rows("var x = <(int, int)?>[];\n"), vec![], "V6");
}

/// The `void` arm rides the same computeRest chain through its
/// `skip_type_len` delegation to the Function arm. 3.13.1 oracle probes:
/// s3a/s3c zero rows; s3b's only row is the compile-time-tier
/// EXPECTED_ONE_LIST_TYPE_ARGUMENTS (resolver face, outside this parser's
/// syntax rows).
#[test]
fn gft_chain_void_arm_group_scan_clean() {
    assert_eq!(
        parse_rows("var x = <void Function() Function()?>[];\n"),
        vec![],
        "nullable chained void-return GFT type arg"
    );
    assert_eq!(
        parse_rows("var x = <void Function() Function()?, int>[];\n"),
        vec![],
        "chained GFT plus second type arg"
    );
    assert_eq!(
        parse_rows("List<void Function() Function()?> x = [];\n"),
        vec![],
        "chained GFT in a type annotation"
    );
}

/// VoidType.parseType reports `voidWithTypeArguments` on `token.next` — the
/// `<` after `void` (type_info_impl.dart:446) — and the group-derail
/// recovery anchors `expectedAfterButGot('>')` on the last consumed type
/// argument's end (parseUnexpectedEnd, type_info_impl.dart:1607-1730).
/// 3.13.1 oracle rows, machine format:
///
/// * `void<int>? y;` — MISSING_IDENTIFIER (1|10|1) at the `?` from the
///   member-name slot, VOID_WITH_TYPE_ARGUMENTS (1|5|1) at the `<`, the
///   adjacent EXPECTED_TOKEN ';' (1|9|1), EXPECTED_EXECUTABLE (1|10|1) and
///   MISSING_CONST_FINAL_VAR_OR_TYPE (1|12|1) row anchors match too.
/// * `var x = <void<int>?, int>[];` — VOID_WITH_TYPE_ARGUMENTS (1|14|1) at
///   the inner `<` plus EXPECTED_TOKEN (1|18|1) on the inner `>`.
#[test]
fn void_with_type_arguments_rows_anchor_after_void() {
    assert_eq!(
        parse_rows("void<int>? y;\n"),
        vec![
            ("missing_identifier".to_string(), 9, 10),
            ("void_with_type_arguments".to_string(), 4, 5),
            ("expected_token".to_string(), 8, 9),
            ("expected_executable".to_string(), 9, 10),
            ("missing_const_final_var_or_type".to_string(), 11, 12),
        ],
        "void<int>? y; face"
    );
    assert_eq!(
        parse_rows("var x = <void<int>?, int>[];\n"),
        vec![
            ("void_with_type_arguments".to_string(), 13, 14),
            ("expected_token".to_string(), 17, 18),
        ],
        "void<int> as a derail type argument"
    );
}
