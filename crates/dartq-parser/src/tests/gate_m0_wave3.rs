//! M0-gate wave-3 regression pins (#440): the r5 dartq-only over-report
//! families (`docs/audits/gate-m0/gate-caliber-r5/report.json`), each
//! matching the official recovery decision order:
//!
//! * unnamed extensions: `extension<T> on ...` routes to the extension
//!   declaration when the `<...>` group is followed by `on` (official
//!   `parseTopLevelKeywordDeclaration`, `parser_impl.dart:771-782`); the
//!   top-level `<`-lookahead offset read the token after the `<`.
//! * typedef `=`-RHS recovery: a non-function type before a parameter
//!   list and bare `<typeArgs>(...);` splice in a missing `Function`
//!   (`parseTypedef`, `parser_impl.dart:1543-1610`); a reserved type head
//!   with `=` at the name position bails the typedef (`typedef void = ...;`).
//! * enum headers: the official five-stage `parseEnumHeaderOpt`
//!   (`parser_impl.dart:2737-2832`) — bounded gunk skips, duplicate
//!   `with`/`implements` clauses under the single `multiple_clauses` code,
//!   and out-of-order reporting.
//! * constant patterns: a `switch` expression is a syntactically valid
//!   pattern operand (the official fallthrough runs the full expression
//!   engine, `parser_impl.dart:11878-11890`).
//! * formal-parameter scans: `void Function(...)` inside type arguments
//!   (`skipTypeLen`'s `void` arm), inline function-typed parameters with
//!   type variables (`skip` is relative to the token *before* `<`), and
//!   scanner-synthetic `)` closers relocate to the current token
//!   (`moveSynthetic`, `parser_impl.dart:1977-1981`/:5035).
//! * unnamed factories: `factory (` with a clean modifier set takes
//!   `parseFactoryMethod(hasName: false)` (official :5456-5477).
//! * representation fields: the function-typed `expectedRepresentationField`
//!   verifier arm only runs with primary constructors disabled.
//! * assignment targets: a bare `super` LHS reports
//!   `missingAssignableSelector`, a bare `this` is assignable (analyzer
//!   `ast_builder.dart:3825-3841`).
//! * collection for-elements consume the `(` (`parseForLoopPartsStart`).

use super::collector::{Collector, latest_features, setup};

/// Sorted diagnostic-code multiset of one whole-unit parse.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

/// Sorted diagnostic-code multiset (scanner + parser) of one whole-unit
/// parse, for inputs the scanner itself flags (an unclosed `(` pins a
/// synthetic closer and reports `expectedToken`).
fn sorted_codes_with_scanner(source: &'static str) -> Vec<String> {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let mut codes: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_owned())
        .collect();
    let mut p = crate::Parser::new(text, table, latest_features(), Collector::default());
    p.parse_unit();
    codes.extend(p.listener.error_codes());
    codes.sort();
    codes
}

/// [`assert_codes`] over [`sorted_codes_with_scanner`].
fn assert_codes_with_scanner(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes_with_scanner(source);
    let expected: Vec<String> = {
        let mut e = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
        e.sort();
        e
    };
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

/// Asserts the exact diagnostic multiset (codes only, sorted).
fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = {
        let mut e = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
        e.sort();
        e
    };
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

fn assert_clean(source: &'static str, label: &str) {
    assert_codes(source, &[], label);
}

// —— unnamed extensions ———————————————————————————————————————————————————

/// `extension<T> on ...` (no name) is an extension declaration; the oracle
/// parses the whole file silently.
#[test]
fn unnamed_extension_with_type_parameters() {
    assert_clean(
        "class I<T> {}\nclass A<T> extends I<T> {}\n\nextension<T> on I<T> {\n  num get member {\n    return T == int ? 0.5 : 1;\n  }\n}\n\nextension<T> on A<T> {\n  void member(T t) {}\n}\n",
        "unnamed extensions",
    );
}

/// `extension<T> X on ...` keeps the member lane, exactly like the official
/// lookahead (the token after `>` must be `on`).
#[test]
fn named_extension_with_type_parameters_takes_member_lane() {
    assert_codes(
        "extension<T> X on List<T> {}\n",
        &["expected_token", "missing_function_parameters"],
        "named extension with type parameters",
    );
}

// —— typedef `=`-RHS recovery —————————————————————————————————————————————

/// `<returnType>? '(' ... ')';` assumes the type is the return type of a
/// missing `Function` (official :1543-1561).
#[test]
fn typedef_rhs_missing_function_before_params() {
    assert_codes(
        "typedef c = foo(int x);\n",
        &["expected_token"],
        "typedef return-type recovery",
    );
    assert_codes(
        "typedef e = foo<F>(int x);\n",
        &["expected_token"],
        "typedef return-type recovery with type arguments",
    );
}

/// `<typeArgs>(<params>);` inserts the missing `Function` before the `<`
/// (official :1564-1610).
#[test]
fn typedef_rhs_type_arguments_only_with_params() {
    assert_codes(
        "typedef f = <F>(int x);\n",
        &["expected_token"],
        "typedef type-argument recovery",
    );
    assert_codes(
        "typedef h = <F, G, H, I, J>(int x);\n",
        &["expected_token"],
        "typedef multi type-argument recovery",
    );
}

/// `<typeArgs>;` keeps the missing-parameter report on the group's closer
/// (official :1596-1605, pinned in wave 2 and kept green).
#[test]
fn typedef_rhs_type_arguments_only_semicolon() {
    assert_codes(
        "typedef i = <F, G, H, I, J>;\n",
        &["missing_typedef_parameters"],
        "typedef type arguments only",
    );
}

/// `typedef void = ...;` bails the typedef (`expectedToken(';')` on the
/// reserved head) and restarts the declaration parse at the `=`.
#[test]
fn typedef_reserved_head_with_equals_bails() {
    assert_codes(
        "typedef void = void Function();\n",
        &[
            "expected_executable",
            "expected_token",
            "missing_function_body",
        ],
        "typedef reserved head bail",
    );
}

/// The valid new-style shapes stay clean.
#[test]
fn typedef_valid_shapes_clean() {
    assert_clean(
        "typedef a = foo Function(int x);\n",
        "return-type function alias",
    );
    assert_clean("typedef b = Function(int x);\n", "bare function alias");
    assert_clean("typedef int = void Function();\n", "recovered-name alias");
}

// —— enum headers ————————————————————————————————————————————————————————

/// Gunk between and after clauses goes through the bounded skip with
/// `unexpectedToken` per token; the body is still found.
#[test]
fn enum_header_gunk_between_clauses() {
    assert_codes(
        "enum E implements Foo w { v }\n",
        &["unexpected_token"],
        "gunk after implements",
    );
    assert_codes(
        "enum E gunk1 with Foo gunk2 implements Bar { v }\n",
        &["unexpected_token", "unexpected_token"],
        "gunk around with",
    );
    assert_codes(
        "enum E gunk1 implements Bar gunk2 with Foo { v }\n",
        &[
            "unexpected_token",
            "unexpected_token",
            "out_of_order_clauses",
        ],
        "gunk with out-of-order with",
    );
}

/// Duplicate clauses report the single `multiple_clauses` code with the
/// clause kind interpolated (official `diag.multipleClauses`).
#[test]
fn enum_header_duplicate_clauses() {
    assert_codes(
        "enum E implements Foo implements Bar implements Bar2 { v }\n",
        &["multiple_clauses", "multiple_clauses"],
        "duplicate implements",
    );
    assert_codes(
        "enum E with Foo with Foo2 implements Bar implements Bar2 with Foo3 implements Bar3 { v }\n",
        &[
            "multiple_clauses",
            "multiple_clauses",
            "multiple_clauses",
            "multiple_clauses",
        ],
        "duplicate with and implements",
    );
    assert_codes(
        "enum E implements Bar implements Bar2 with Foo with Foo2 implements Bar3 with Foo3 { v }\n",
        &[
            "multiple_clauses",
            "out_of_order_clauses",
            "multiple_clauses",
            "multiple_clauses",
            "multiple_clauses",
        ],
        "wrong order and additional clauses",
    );
}

// —— constant patterns ———————————————————————————————————————————————————

/// `case switch (o) { _ => true }:` parses as a switch-expression operand;
/// only the semantic layer rejects it, so the parser stays silent.
#[test]
fn switch_expression_as_constant_pattern_operand() {
    assert_clean(
        "void f(Object? o) {\n  switch (o) {\n    case switch (o) { _ => true }: // E\n  }\n}",
        "switch expression pattern",
    );
    assert_clean(
        "void f(Object? o) {\n  switch (o) {\n    case const switch (o) { _ => true }: // E\n  }\n}",
        "const switch expression pattern",
    );
}

// —— formal-parameter scans —————————————————————————————————————————————————

/// `void Function(...)` as a type argument (with or without `?`, with or
/// without a function-typed parameter shape) parses silently.
#[test]
fn void_return_gft_inside_type_arguments() {
    assert_clean("void f(List<void Function(dynamic)>? g()) {}\n", "nullable");
    assert_clean("void f(List<void Function()> g()) {}\n", "non-nullable");
    assert_clean("void f(List<void Function()> g) {}\n", "plain parameter");
}

/// Inline function-typed parameters with type variables
/// (`f<T>()`) keep the shape detection (official `skip` is relative to the
/// token before `<`); a `const` modifier still reports only
/// `extraneous_modifier` (`representationFieldModifier`), never
/// `expected_representation_field`.
#[test]
fn inline_function_typed_parameter_with_type_variables() {
    assert_clean("extension type ET(f<T>()) {}\n", "type-variable formal");
    assert_codes(
        "extension type ET(const f<T>()) {}\n",
        &["extraneous_modifier"],
        "const-modified type-variable formal",
    );
    assert_codes(
        "extension type ET(const void f<T>()) {}\n",
        &["extraneous_modifier"],
        "void-returning const formal",
    );
}

/// `const f()` / `const void f()` in a representation field keep only the
/// `const` modifier report (the verifier's function-typed
/// `expectedRepresentationField` arm needs the feature disabled).
#[test]
fn const_modified_representation_field() {
    assert_codes(
        "extension type ET(const f()) {}\n",
        &["extraneous_modifier"],
        "const representation field",
    );
    assert_codes(
        "extension type ET(const void f()) {}\n",
        &["extraneous_modifier"],
        "const void representation field",
    );
}

// —— unnamed factories ———————————————————————————————————————————————————

/// `factory ()` with a clean modifier set parses as an unnamed factory:
/// bodies report `const_factory` for `const`, redirections stay silent,
/// and everything else is oracle-silent.
#[test]
fn unnamed_factory_heads() {
    assert_clean(
        "class C {\n  factory () => C();\n  factory () {}\n  factory () = C;\n  external const factory ();\n  const factory () = C;\n  void factory() => C();\n}\n",
        "unnamed factory shapes",
    );
    assert_codes(
        "class C {\n  const factory () => C();\n  const factory () {}\n}\n",
        &["const_factory", "const_factory"],
        "const unnamed factory bodies",
    );
}

// —— assignment targets ——————————————————————————————————————————————————

/// A bare `super` LHS reports `missingAssignableSelector`; a bare `this`
/// LHS is assignable (analyzer `handleAssignmentExpression`,
/// `ast_builder.dart:3825-3841`).
#[test]
fn initializer_assignment_targets() {
    assert_codes(
        "class C {\n  C() : super = 0;\n}\n",
        &["invalid_initializer", "missing_assignable_selector"],
        "super initializer target",
    );
    assert_codes(
        "class C {\n  C() : this = 0;\n}\n",
        &["expected_token", "missing_assignment_in_initializer"],
        "this initializer target",
    );
}

// —— collection for-elements —————————————————————————————————————————————

/// A for-element consumes its `(` (`parseForLoopPartsStart`), so for-in
/// collection elements parse like the statement form.
#[test]
fn collection_for_elements() {
    assert_clean(
        "void f(int x) {\n  return [\n    for (x in [0]) x,\n  ];\n}\n",
        "trailing comma for-in element",
    );
    assert_clean(
        "void f() {\n  [for (Object x = g(); x is int; x = g()) x];\n}\n",
        "classic for element",
    );
    assert_clean(
        "void f(int x) {\n  var l = [for (x in [0]) x,];\n}\n",
        "local for-in element",
    );
}

// —— unclosed formals keep the body ——————————————————————————————————————

/// A scanner-synthetic `)` for an unclosed `(` relocates to the current
/// token (`moveSynthetic`), so a later real `{...}` still opens the
/// declaration body.
#[test]
fn unclosed_formal_relocation() {
    assert_codes_with_scanner(
        "enum E( {}\n",
        &[
            "empty_named_parameter_list",
            "expected_token",
            "missing_enum_body",
            "missing_identifier",
        ],
        "unclosed paren relocates at the brace",
    );
}
