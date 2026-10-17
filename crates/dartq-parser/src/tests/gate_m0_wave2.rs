//! M0-gate wave-2 regression pins (#333): the r4 oracle-only gap families
//! (found by the r4 gate-caliber differential run), each mirroring an
//! official recovery template:
//!
//! * extension-type representation recovery: `expectedRepresentationField`
//!   (analyzer `ErrorVerifier`
//!   `_checkForExtensionTypeRepresentationErrorCodes`,
//!   error_verifier.dart:5356-5460), `representationFieldModifier`,
//!   `covariant` consumed silently in primary-constructor formals (3.13
//!   oracle shape — no augmentation gating), and
//!   `fieldInitializerOutsideConstructor` (error_verifier.dart:5558-5597).
//! * dot-access operand recovery: `doDotExpression` (`ast_builder.dart:918`)
//!   reports the missing name after `.`/`?.` and inserts a synthetic.
//! * record/const-pattern pins: `empty_record_literal_with_comma`,
//!   `invalidConstantPatternEmptyRecordLiteral` (parser_impl.dart:8389-8394),
//!   `record_literal_one_positional_no_trailing_comma` via the const
//!   statement re-entry (`parseExpressionStatementOrConstDeclaration`), and
//!   the named-function-expression recovery in const-pattern position.
//! * header recovery: the bounded `recoverySmallLookAheadSkipTokens`
//!   (parser_impl.dart:2861-2898) skip reports, the enum reserved-name
//!   synthetic, extension-type clause counting (`multiple_implements_clauses`),
//!   and the typedef old-style no-name recovery.
//! * setter arity (`AstBuilder` `_ensureSetterFormalParameter`,
//!   `ast_builder.dart:6457`) and `var`+type in formals (`varAndType`).

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
/// parse, for inputs with unmatched groups (an unclosed `(` in a broken for
/// header parks a synthetic closer; the parser reports the pair post-parse).
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
    let mut collector = Collector::default();
    let mut p = crate::Parser::new(text, table, latest_features(), &mut collector);
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

// —— representation recovery —————————————————————————————————————————————

/// `var` (with a type following) in the representation field reports
/// `representationFieldModifier` (oracle `ET(covariant var a)`).
#[test]
fn representation_field_modifier_only() {
    assert_codes(
        "extension type ET(covariant var a) {}\n",
        &["representation_field_modifier"],
        "covariant var representation",
    );
}

/// `covariant` without `var` in a primary-constructor formal reports ONE
/// `invalid_covariant_modifier_in_primary_constructor` row on the
/// `covariant` token (probed 3.13.1: `extension type ET(covariant a b) {}`
/// and `class P(covariant int x) {}` both yield exactly that row; only
/// `covariant var` is legal, and that shape is the
/// `representation_field_modifier` face instead).
#[test]
fn representation_field_covariant_reports() {
    assert_codes(
        "extension type ET(covariant a b) {}\n",
        &["invalid_covariant_modifier_in_primary_constructor"],
        "covariant representation",
    );
}

/// `var` + type in ordinary member formals reports the oracle pair
/// (probed 3.13.1 on `extension type ET(int i) { ET(var a b) {} }`:
/// `extraneous_modifier` + `var_and_type`, both on the `var` — the
/// constructor member's formals are non-primary).
#[test]
fn var_and_type_in_member_formals() {
    assert_codes(
        "extension type ET(int i) { ET(var a b) {} }\n",
        &["extraneous_modifier", "var_and_type"],
        "var and type member formal",
    );
}

// —— dot-access / selector operand recovery ——————————————————————————————

/// A non-name operand after `.`/`?.` reports the missing identifier and
/// inserts a synthetic (official `doDotExpression` in `AstBuilder`,
/// `ast_builder.dart:918`).
#[test]
fn dot_access_operand_missing_identifier() {
    assert_codes(
        "void f(dynamic c) {\n  c?.[1];\n}\n",
        &["missing_identifier"],
        "null-aware dot operand",
    );
}

// —— patterns / records ——————————————————————————————————————————————————

/// Reserved words in pattern-element position report the missing identifier
/// and continue (official pattern-element `ensureIdentifier`). The
/// delimited loops then recover per the official decision order: the list
/// loop's per-iteration no-progress skip consumes the blocker; the
/// parenthesized loop leaves it for `ensureCloseParen` (expected `)` at the
/// blocker, `parser_impl.dart:12229-12231` + `:5028`); the map loop's
/// stale-`next` skip leaves the first entry's blocker for the `,`
/// recovery, which re-parses it as the next key (official
/// `syntheticIdentifier_inside*` corpus).
#[test]
fn pattern_reserved_element_positions() {
    assert_codes(
        "void f(Object? x) {\n  switch (x) {\n    case [if]:\n  }\n}\n",
        &["missing_identifier"],
        "list pattern element",
    );
    // The map loop's recovery re-parses the blocker as the next key; the
    // first separator row (`,`) anchors on the blocker, the `:` row on the
    // token the entry recovery lands behind — the map's `}` (the #2024
    // separator-anchor rule; frozen 0c9fda4b anchors the separator
    // recovery row on the token following the recovery point).
    let map_src = "void f(Object? x) {\n  switch (x) {\n    case {0: if}:\n  }\n}\n";
    let mut p = setup(map_src);
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![
            ("missing_identifier".to_string(), 48, 50),
            ("expected_token".to_string(), 48, 50),
            ("expected_token".to_string(), 50, 51),
        ],
        "{map_src}"
    );
    assert_codes(
        "void f(Object? x) {\n  switch (x) {\n    case (if):\n  }\n}\n",
        &["expected_token", "missing_identifier"],
        "record pattern field",
    );
}

/// A top-level pattern statement recovers through the declaration lane
/// (`[if];` with no variable keyword).
#[test]
fn pattern_statement_reserved_head() {
    assert_codes(
        "[if];\n",
        &[
            "expected_executable",
            "expected_executable",
            "expected_identifier_but_got_keyword",
            "expected_token",
            "missing_const_final_var_or_type",
            "unexpected_token",
        ],
        "reserved pattern statement",
    );
}

/// Assignment RHS pattern recovery (`var x = [if];` / `var x = (if);`).
#[test]
fn pattern_rhs_reserved_head() {
    assert_codes(
        "var x = [if];\n",
        &["expected_token", "missing_identifier"],
        "list literal reserved element",
    );
    assert_codes(
        "var x = (if);\n",
        // Official ExpressionIdentifierContext.ensureIdentifier
        // (identifier_context_impl.dart:411): a statement-start keyword is
        // left for the statement parser — expectedIdentifier + synthetic —
        // then ensureCloseParen reports the missing `)` (:1917).
        &["expected_token", "missing_identifier"],
        "parenthesized reserved operand",
    );
}

/// `const (...) = x;` re-enters the statement parse at the `const`
/// (official `parseExpressionStatement(start)`), so the parenthesized
/// operand is a const literal, not a pattern assignment — the assignment
/// target check (`AstBuilder.handleAssignmentExpression`,
/// ast_builder.dart:3831-3837) reports `missingAssignableSelector` for the
/// non-assignable const record on top of the statement-level
/// `illegalAssignmentToNonAssignable` (oracle:
/// `patterns/patternVariableDeclarationStatement_disallowsConst.dart`).
#[test]
fn const_statement_assignment_targets() {
    assert_codes(
        "void f() { const (_) = x; }\n",
        &[
            "illegal_assignment_to_non_assignable",
            "missing_assignable_selector",
            "record_literal_one_positional_no_trailing_comma",
        ],
        "const parenthesized assignment",
    );
    assert_codes(
        "void f() { const (a, b) = x; }\n",
        &[
            "illegal_assignment_to_non_assignable",
            "missing_assignable_selector",
        ],
        "const record assignment",
    );
}

/// A named function expression in const-pattern position keeps the official
/// `namedFunctionExpression` report (`case const void fun() {}:`).
#[test]
fn const_pattern_named_function_expression() {
    assert_codes(
        "void f(Object? x) {\n  switch (x) {\n    case const void fun() {}:\n  }\n}\n",
        &["named_function_expression"],
        "const pattern function expression",
    );
}

// —— header recovery —————————————————————————————————————————————————————

/// `enum` followed by a foreign declaration starter: the bounded skip
/// refuses at the next top-level declaration (`recoverySmallLookAheadSkip`
/// and `looksLikeStartOfNextTopLevelDeclaration` — bail only, no report:
/// the official null-return paths `parser_impl.dart:2870/2882` report
/// nothing; success requires a `lookFor` boundary, :2872-2874), the
/// reserved head gets a synthetic name, and the missing body is reported.
/// The oracle emits no `unexpected_token` on this shape — the 3.13.1
/// differential lists it dartq-only for the twin fixture
/// (`issue_48371_prime3.dart`, gate-caliber-r5) — the row this pin used
/// to carry was the old port's success-on-bail bug.
#[test]
fn enum_header_reserved_head_skip_refusal() {
    assert_codes(
        "enum\nclass A {\n  void foo(int x) {}\n}\n",
        &["missing_enum_body", "missing_identifier"],
        "enum foreign header",
    );
}

/// A typedef with no name at all: the 3.13 oracle bails the whole typedef
/// with `expectedToken(';')` anchored on the reserved type head
/// (`typedef void = ...;`) and restarts the declaration parse at the `=`;
/// the reparse yields the top-level-member and alias-shape reports.
/// `typedef <reserved head> = ...`: the reserved type head bails the
/// typedef with `expected_token(';')` on the head, then the `=`-position
/// name slot reports `missing_identifier` + `missing_typedef_parameters`
/// before the synthetic `()` splices in, and the declaration loop's member
/// reparse of the `=` mints `expected_executable` (probed 3.13.1,
/// `typedef void = void Function();`: exactly these five rows).
#[test]
fn typedef_old_style_no_name() {
    assert_codes(
        "typedef void = void Function();\n",
        &[
            "expected_executable",
            "expected_token",
            "missing_function_body",
            "missing_identifier",
            "missing_typedef_parameters",
        ],
        "typedef missing name",
    );
}

/// Type arguments alone after `=` recover as a missing `Function` alias and
/// report the missing parameter list on the group's closer (official
/// parseTypedef :1596-1605).
#[test]
fn typedef_type_arguments_only() {
    assert_codes(
        "typedef i = <F, G, H, I, J>;\n",
        &["missing_typedef_parameters"],
        "typedef type arguments only",
    );
}

/// A built-in keyword in type-parameter position reports
/// `expectedIdentifierButGotKeyword` and is consumed as the name (oracle
/// `typedef F<Function> = int;`).
#[test]
fn typedef_type_parameter_built_in_keyword() {
    assert_codes(
        "typedef F<Function> = int;\n",
        &["expected_identifier_but_got_keyword"],
        "Function type parameter",
    );
}

// —— setters ——————————————————————————————————————————————————————————————

/// A setter takes exactly one required positional parameter; the report
/// anchors on the setter name (`_ensureSetterFormalParameter`).
#[test]
fn setter_parameter_count() {
    assert_codes(
        "class Foo {\n  set Foo() {\n  }\n}\n",
        &[
            "member_with_class_name",
            "wrong_number_of_parameters_for_setter",
        ],
        "zero-parameter setter",
    );
}

/// Constructor-recovered setter shapes keep `setterConstructor` and do NOT
/// cascade the arity report (the oracle reports the arity once, on the
/// genuine setter member).
#[test]
fn setter_constructor_shapes_no_arity_cascade() {
    assert_codes(
        "class Foo {\n  set Foo() : initializer = true {\n  }\n}\n",
        &["setter_constructor"],
        "setter with initializer",
    );
    assert_codes(
        "class Foo {\n  set Foo.x() {\n  }\n}\n",
        &["setter_constructor"],
        "qualified setter name",
    );
}

// —— for-header stall —————————————————————————————————————————————————————

/// An unclosed `(` in a for header can never complete: the enclosing
/// block's `}` is reported as the stall point
/// (`parseForLoopPartsRest` :10369) and the header parses on to the
/// scanner-pinned closer. The recovery re-visits the same tokens, so the
/// repeated rows share anchors — the oracle's downstream dedupe surfaces
/// one row per (code, anchor) pair.
#[test]
fn for_header_stall_on_block_close() {
    assert_codes_with_scanner(
        "main() {\n  for (var\n}\n",
        &[
            "expected_token",
            "expected_token",
            "missing_identifier",
            "missing_identifier",
            "unexpected_token",
        ],
        "for header dies at block closer",
    );
}

/// The same stall fires for an empty incomplete header.
#[test]
fn for_header_stall_empty_parts() {
    assert_codes_with_scanner(
        "void f() {\n  for (;;\n}\n",
        &[
            "expected_token",
            "expected_token",
            "missing_identifier",
            "unexpected_token",
        ],
        "empty for parts at block closer",
    );
}

/// An incomplete local declaration OUTSIDE a for header does not cascade
/// the stall report (the block closer is the enclosing block's own).
#[test]
fn incomplete_local_declaration_no_stall() {
    assert_codes(
        "void f() {\n  var x\n}\n",
        &["expected_token"],
        "dangling local declaration",
    );
}

// —— annotation spacing ———————————————————————————————————————————————————

/// A space before the annotation's argument list reports
/// `annotationSpaceBeforeParenthesis` (official `parseArgumentsOptMetadata`).
#[test]
fn annotation_space_before_parenthesis() {
    assert_codes(
        "@metadata (x, y)\nclass C {}\n",
        &["annotation_space_before_parenthesis"],
        "spaced annotation arguments",
    );
}
