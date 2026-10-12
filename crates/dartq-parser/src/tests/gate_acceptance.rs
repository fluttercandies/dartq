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
