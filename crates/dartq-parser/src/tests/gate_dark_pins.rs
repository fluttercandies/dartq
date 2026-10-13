//! Dark-syntax-code pin gates: one presence pin per frozen m0-syntax
//! diagnostic code that the harness fixture-coverage manifest records as a
//! documented gap (no targeted fixture, no annotation mention in the tier
//! corpora, no scanner-golden face — 88 codes total).
//!
//! Data source: the harness fixture-coverage manifest (syntax tier
//! `documented_gap` rows). Its note column splits the dark set in two:
//!
//! * never nailed (38 recorded, 37 pinned here + 1 recorded dead) — no
//!   recorded differential exposure anywhere. Snippets are derived from the
//!   dartq emission sites (the official decision-order mirrors in this
//!   crate) or, where noted, from the official SDK recovery corpus.
//! * exposure only (50) — the only recorded exposure is incidental diff
//!   entries in the differential regression nails, i.e. dartq provably emits
//!   the code on those corpus shapes. Snippets mirror those nail-proven
//!   shapes, minimized to one declaration.
//!
//! `representation_field_trailing_comma` is the one dark code without a pin
//! here: it is recorded dead at the pinned oracle (no analyzer emitter,
//! probed silent) and its must-stay-dead pin already lives in
//! `emitter_w1.rs`.
//!
//! Assertion shape: presence of the code in the whole-unit diagnostic
//! stream ([`setup`] + `parse_unit`, collector channel). Exact spans and
//! full streams stay owned by the wave gates; these gates own per-code
//! visibility.

use super::collector::setup;

/// Whole-unit parse, diagnostic-code sequence only.
fn codes_of(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    p.listener.error_codes()
}

/// Presence pin: `code` must fire at least once on `source`.
fn assert_code_fires(source: &'static str, code: &str) {
    let got = codes_of(source);
    assert!(
        got.iter().any(|c| c == code),
        "`{code}` must fire on {source:?}; got: {got:?}"
    );
}

/// Shared snippet: `implements <literal>` misparse — the oracle reports a
/// body-shaped row and a member-shaped row on one stream, so one snippet
/// pins two dark codes.
const IMPLEMENTS_LITERAL: &str = "class X implements 3 {}\n";

/// Shared snippet: top-level send-assignment statement — a member head with
/// no type and no modifier, pinning two dark codes.
const TOP_LEVEL_SEND_ASSIGN: &str = "x.y = 42;\n";

/// Shared snippet: assignment to a non-assignable primary — the LHS check
/// reports the missing selector and the non-assignable target together.
const ASSIGN_TO_LITERAL: &str = "void f() { 1 = 2; }\n";

/// Shared snippet: pattern assignment with a `when` field (reserved in
/// pattern position) and a fresh `a` field — the assignment-context name
/// checks pin two dark codes on one stream.
const PATTERN_ASSIGN_WHEN: &str = "void f() { (when, a) = y; }\n";

// —— never nailed (37 pins; snippets from emission-site analysis) —————————

/// `base enum` — enums take no class modifiers (`base` is reported in the
/// dispatch scan before the enum body parses).
#[test]
fn dark_syntax_never_nailed_base_enum() {
    assert_code_fires("base enum E { a }\n", "base_enum");
}

/// Written-out binary operator (`and`) at a binary position recovers with
/// the written-out-operator report (gate_m0's `issue_26810` family face).
#[test]
fn dark_syntax_never_nailed_binary_operator_written_out() {
    assert_code_fires("void f() { var x = a and b; }\n", "binary_operator_written_out");
}

/// `class` keyword in a class body: `recoverFromInvalidMember` reports and
/// skips the member-shaped nested declaration.
#[test]
fn dark_syntax_never_nailed_class_in_class() {
    assert_code_fires("class C { class D {} }\n", "class_in_class");
}

/// `:` where the for-in `in` belongs (official `parseForIn` colon face).
#[test]
fn dark_syntax_never_nailed_colon_in_place_of_in() {
    assert_code_fires("void f() { for (var x : xs) {} }\n", "colon_in_place_of_in");
}

/// `const` constructor with a real block body (official `_verifyConstructor`:
/// `;` bodies stay clean, `{}` reports at the brace).
#[test]
fn dark_syntax_never_nailed_const_constructor_with_body() {
    assert_code_fires("class C { const C() {} }\n", "const_constructor_with_body");
}

/// Constructor with a return type: the name matching the enclosing class
/// makes the member a constructor, and the type prefix reports.
#[test]
fn dark_syntax_never_nailed_constructor_with_return_type() {
    assert_code_fires("class C { int C() => 1; }\n", "constructor_with_return_type");
}

/// `default` arm in a switch EXPRESSION (official :11819-11825: report and
/// parse the wildcard-shaped remainder).
#[test]
fn dark_syntax_never_nailed_default_in_switch_expression() {
    assert_code_fires(
        "void f(x) { var y = switch (x) { default => 1 }; }\n",
        "default_in_switch_expression",
    );
}

/// Empty named-fields list in a record type `({})`.
#[test]
fn dark_syntax_never_nailed_empty_record_type_named_fields_list() {
    assert_code_fires(
        "void f() { ({}) x = (a: 1); }\n",
        "empty_record_type_named_fields_list",
    );
}

/// Misspelled `extend` reads as `extends` with the expected-instead report
/// (official lexeme-based gate over `isKeywordOrIdentifier`).
#[test]
fn dark_syntax_never_nailed_expected_instead() {
    assert_code_fires("class C extend A {}\n", "expected_instead");
}

/// `extends` on an extension type: the clause is always invalid.
#[test]
fn dark_syntax_never_nailed_extension_type_extends() {
    assert_code_fires("extension type E(int x) extends A {}\n", "extension_type_extends");
}

/// `with` on an extension type: the clause is always invalid.
#[test]
fn dark_syntax_never_nailed_extension_type_with() {
    assert_code_fires("extension type E(int x) with A {}\n", "extension_type_with");
}

/// `this` formal outside a constructor parameter list (error_verifier
/// `field_initializer_outside_constructor` at the `this` keyword).
#[test]
fn dark_syntax_never_nailed_field_initializer_outside_constructor() {
    assert_code_fires("void f(this.x) {}\n", "field_initializer_outside_constructor");
}

/// `covariant final` field pair (official `_endClassFields` validation).
#[test]
fn dark_syntax_never_nailed_final_and_covariant() {
    assert_code_fires(
        "class C { covariant final int x = 0; }\n",
        "final_and_covariant",
    );
}

/// `covariant late final` WITH an initializer — the full quartet the
/// official verifier singles out.
#[test]
fn dark_syntax_never_nailed_final_and_covariant_late_with_initializer() {
    assert_code_fires(
        "class C { covariant late final int x = 0; }\n",
        "final_and_covariant_late_with_initializer",
    );
}

/// `final enum` — enums take no class modifiers.
#[test]
fn dark_syntax_never_nailed_final_enum() {
    assert_code_fires("final enum E { a }\n", "final_enum");
}

/// Getter in constructor-name position (`get C()` where `C` names the
/// enclosing class): the constructor head reports the accessor.
#[test]
fn dark_syntax_never_nailed_getter_constructor() {
    assert_code_fires("class C { get C() => 1; }\n", "getter_constructor");
}

/// Getter with a formal parameter list (official `parseGetterOrFormalParameters`).
#[test]
fn dark_syntax_never_nailed_getter_with_parameters() {
    assert_code_fires("class C { int get x() => 1; }\n", "getter_with_parameters");
}

/// Pattern assignment binding a reserved pattern name (`when`): the
/// assignment-context variable-name check fires.
#[test]
fn dark_syntax_never_nailed_illegal_pattern_assignment_variable_name() {
    assert_code_fires(
        PATTERN_ASSIGN_WHEN,
        "illegal_pattern_assignment_variable_name",
    );
}

/// Pattern variable named `when`/`as` in a declaration context: the
/// declaration-context variable-name check fires.
#[test]
fn dark_syntax_never_nailed_illegal_pattern_variable_name() {
    assert_code_fires("void f() { var (when) = y; }\n", "illegal_pattern_variable_name");
}

/// `interface enum` — enums take no class modifiers.
#[test]
fn dark_syntax_never_nailed_interface_enum() {
    assert_code_fires("interface enum E { a }\n", "interface_enum");
}

/// Unary suffix (`?`) applied to a logical-or pattern whose last primary is
/// a refutable identifier (constant pattern): the unary-context guard
/// reports (official `isLastPatternAllowedInsideUnaryPattern`).
#[test]
fn dark_syntax_never_nailed_invalid_inside_unary_pattern() {
    assert_code_fires(
        "void f(x) { switch (x) { case a || b?: break; } }\n",
        "invalid_inside_unary_pattern",
    );
}

/// `covariant` in an extension member's parameters (official :2221 face;
/// extensions never take covariant formals).
#[test]
fn dark_syntax_never_nailed_invalid_use_of_covariant_in_extension() {
    assert_code_fires(
        "extension X on int { void m(covariant int a) {} }\n",
        "invalid_use_of_covariant_in_extension",
    );
}

/// `late` on a pattern variable declaration (`late var (a, b) = ...`): the
/// statement-side pattern head reports the keyword.
#[test]
fn dark_syntax_never_nailed_late_pattern_variable_declaration() {
    assert_code_fires(
        "void f() { late var (a, b) = (1, 2); }\n",
        "late_pattern_variable_declaration",
    );
}

/// `new List<int>[...]` — the typed-list literal must not keep the `new`
/// (official :8724 face, constructor-reference branch).
#[test]
fn dark_syntax_never_nailed_literal_with_class_and_new() {
    assert_code_fires(
        "void f() { var x = new List<int>[1]; }\n",
        "literal_with_class_and_new",
    );
}

/// `new {}` — a bare `new` before a map-shaped literal drops the keyword
/// with the literal-with-new report (official :8731-8761).
#[test]
fn dark_syntax_never_nailed_literal_with_new() {
    assert_code_fires("void f() { var x = new {}; }\n", "literal_with_new");
}

/// Accessor named like the enclosing class WITHOUT a parameter list: not a
/// constructor, so the member-name check reports (official
/// `_verifyDeclaration` face).
#[test]
fn dark_syntax_never_nailed_member_with_class_name() {
    assert_code_fires("class C { get C => 1; }\n", "member_with_class_name");
}

/// `enum E;` — no body: the block-body verifier maps the enum flavor to
/// `missing_enum_body` (pseudoSharedCode `expectedEnumBody`).
#[test]
fn dark_syntax_never_nailed_missing_enum_body() {
    assert_code_fires("enum E;\n", "missing_enum_body");
}

/// `with` clause on a mixin: always invalid (mixins take `on` clauses).
#[test]
fn dark_syntax_never_nailed_mixin_with_clause() {
    assert_code_fires("mixin M with A {}\n", "mixin_with_clause");
}

/// `final late` order violation: the modifier walk reports the later
/// keyword against the earlier one (official modifier-order table).
#[test]
fn dark_syntax_never_nailed_modifier_out_of_order() {
    assert_code_fires("class C { final late int x = 0; }\n", "modifier_out_of_order");
}

/// Two `extends` types in one class header (official
/// `parseClassExtendsSeenExtendsClause` comma face).
#[test]
fn dark_syntax_never_nailed_multiple_extends_clauses() {
    assert_code_fires("class C extends A, B {}\n", "multiple_extends_clauses");
}

/// Two `implements` clauses in one class header (header-recovery duplicate
/// check).
#[test]
fn dark_syntax_never_nailed_multiple_implements_clauses() {
    assert_code_fires(
        "class C implements A implements B {}\n",
        "multiple_implements_clauses",
    );
}

/// `var (a, b) = ...` at TOP LEVEL: pattern variable declarations are
/// function/method-only; the top-level member scan reports and skips.
#[test]
fn dark_syntax_never_nailed_pattern_variable_declaration_outside_function_or_method() {
    assert_code_fires(
        "var (a, b) = (1, 2);\n",
        "pattern_variable_declaration_outside_function_or_method",
    );
}

/// `sealed enum` — enums take no class modifiers.
#[test]
fn dark_syntax_never_nailed_sealed_enum() {
    assert_code_fires("sealed enum E { a }\n", "sealed_enum");
}

/// Setter in constructor-name position (`set C(v)` where `C` names the
/// enclosing class): the constructor head reports the accessor.
#[test]
fn dark_syntax_never_nailed_setter_constructor() {
    assert_code_fires("class C { set C(v) {} }\n", "setter_constructor");
}

/// `var` keyword on a declaration-context variable pattern (`var (var a)`):
/// the inner keyword is reported (official `parseVariablePattern`).
#[test]
fn dark_syntax_never_nailed_variable_pattern_keyword_in_declaration_context() {
    assert_code_fires(
        "void f() { var (var a) = (1, 2); }\n",
        "variable_pattern_keyword_in_declaration_context",
    );
}

/// `void<int>` — the void type rejects type arguments but consumes them
/// (official `VoidType.parseType` recovery).
#[test]
fn dark_syntax_never_nailed_void_with_type_arguments() {
    assert_code_fires("void<int> x;\n", "void_with_type_arguments");
}

/// Setter with a parameter list whose count is not 1 (official
/// `_verifyDeclaration` setter arity face).
#[test]
fn dark_syntax_never_nailed_wrong_number_of_parameters_for_setter() {
    assert_code_fires(
        "class C { set x(a, b) {} }\n",
        "wrong_number_of_parameters_for_setter",
    );
}

/// `break` outside a loop, no label (official `parseBreakStatement`;
/// regression-nail shape, `keyword_named_class_methods_2`).
#[test]
fn dark_syntax_exposure_only_break_outside_of_loop() {
    assert_code_fires("void f() { break; }\n", "break_outside_of_loop");
}

/// `continue` outside a loop, no label (official `parseContinueStatement`;
/// regression-nail shape, `keyword_named_class_methods_2`).
#[test]
fn dark_syntax_exposure_only_continue_outside_of_loop() {
    assert_code_fires("void f() { continue; }\n", "continue_outside_of_loop");
}

/// Annotations on type arguments are reported and skipped (official
/// skipMetadata probe in the type-argument group; nail shape
/// `issue_22314`).
#[test]
fn dark_syntax_exposure_only_annotation_on_type_argument() {
    assert_code_fires(
        "class C {\n  m() => new A<@foo C>();\n}\n",
        "annotation_on_type_argument",
    );
}

/// A space between an annotation name and its argument list before a
/// declaration keyword (official :9473/:9480 faces; nail shape
/// `record/metadata.dart`).
#[test]
fn dark_syntax_exposure_only_annotation_space_before_parenthesis() {
    assert_code_fires(
        "@foo (\"x\")\nclass C {}\n",
        "annotation_space_before_parenthesis",
    );
}

/// `const factory` with a real body (official `parseFactoryMethod`
/// handleConstFactory face; external and redirecting bodies bypass it).
#[test]
fn dark_syntax_exposure_only_const_factory() {
    assert_code_fires("class C { const factory C() => 0; }\n", "const_factory");
}

/// `const` on a plain method (official `_verifyDeclaration`: getters,
/// setters and methods can't be const).
#[test]
fn dark_syntax_exposure_only_const_method() {
    assert_code_fires("class C { const void m() {} }\n", "const_method");
}

/// Duplicated `required` in a named group (official modifier walk;
/// regression-nail shape, `use_required_in_non_nnbd`).
#[test]
fn dark_syntax_exposure_only_duplicated_modifier() {
    assert_code_fires(
        "void foo({required required x}) {}\n",
        "duplicated_modifier",
    );
}

/// Empty record literal with a comma `(,)` (official
/// `recordLiteralZeroFieldsWithTrailingComma`; nail shape
/// `record_and_commas`).
#[test]
fn dark_syntax_exposure_only_empty_record_literal_with_comma() {
    assert_code_fires("void f() { var x = (,); }\n", "empty_record_literal_with_comma");
}

/// Record type `(,)` — empty with a trailing comma (official
/// record-type verifier; nail shape `record_and_commas`).
#[test]
fn dark_syntax_exposure_only_empty_record_type_with_comma() {
    assert_code_fires("void f() { (,) x = (); }\n", "empty_record_type_with_comma");
}

/// Chained equality operands (official per-tier rejection in the binary
/// parse; nail shape `function_reference_following_token`).
#[test]
fn dark_syntax_exposure_only_equality_cannot_be_equality_operand() {
    assert_code_fires(
        "void f() { x == y == z; }\n",
        "equality_cannot_be_equality_operand",
    );
}

/// `implements <literal>`: the body-shaped row of the misparse
/// (regression-nail shape, `blindspot_type_literals`).
#[test]
fn dark_syntax_exposure_only_expected_body() {
    assert_code_fires(IMPLEMENTS_LITERAL, "expected_body");
}

/// `implements <literal>`: the member-shaped row of the same misparse
/// (regression-nail shape, `blindspot_type_literals`).
#[test]
fn dark_syntax_exposure_only_expected_class_member() {
    assert_code_fires(IMPLEMENTS_LITERAL, "expected_class_member");
}

/// Top-level junk token: the invalid-declaration face (official
/// `parseTopLevelMember` recovery; `;` takes the unexpected-token face
/// instead).
#[test]
fn dark_syntax_exposure_only_expected_executable() {
    assert_code_fires(")\n", "expected_executable");
}

/// Reserved word in an identifier position recovers with the keyword-name
/// report (official `ensureIdentifierPotentiallyRecovered`; wave gates pin
/// the exact spans, this owns visibility).
#[test]
fn dark_syntax_exposure_only_expected_identifier_but_got_keyword() {
    assert_code_fires("int class = 42;\n", "expected_identifier_but_got_keyword");
}

/// `covariant` heading a class declaration: the header modifier walk
/// reports it as extraneous (official declaration-modifier gate).
#[test]
fn dark_syntax_exposure_only_extraneous_modifier() {
    assert_code_fires("covariant class C {}\n", "extraneous_modifier");
}

/// Assignment to a non-assignable primary reports the non-assignable target
/// (official :6671 LHS check; regression-nail shape,
/// `patternVariableDeclarationStatement_disallowsConst`).
#[test]
fn dark_syntax_exposure_only_illegal_assignment_to_non_assignable() {
    assert_code_fires(ASSIGN_TO_LITERAL, "illegal_assignment_to_non_assignable");
}

/// Assignment to a non-assignable primary reports the missing selector on
/// the same LHS check (official :6671; nail shape
/// `patternVariableDeclarationStatement_disallowsConst`).
#[test]
fn dark_syntax_exposure_only_missing_assignable_selector() {
    assert_code_fires(ASSIGN_TO_LITERAL, "missing_assignable_selector");
}

/// Constant-pattern identifier named `when` (guard keyword) in a refutable
/// context (official `parsePrimaryPattern`; nail shape
/// `constant_identifier_namedWhen`).
#[test]
fn dark_syntax_exposure_only_illegal_pattern_identifier_name() {
    assert_code_fires(
        "void f(x) { switch (x) { case when: break; } }\n",
        "illegal_pattern_identifier_name",
    );
}

/// Explicit `const` prefix on a plain literal constant pattern (official
/// over-collection diagnostics; nail shape `const_patterns_binary`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_const_prefix() {
    assert_code_fires(
        "void f(x) { switch (x) { case const 1: break; } }\n",
        "invalid_constant_const_prefix",
    );
}

/// Binary operator in an implicit constant pattern below selector
/// precedence (official :7507-7543; nail shape `const_patterns_binary`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_pattern_binary() {
    assert_code_fires(
        "void f(x) { switch (x) { case 1 + 2: break; } }\n",
        "invalid_constant_pattern_binary",
    );
}

/// Duplicated `const` prefix in a constant pattern (official
/// duplicate-const face; nail shape `const_patterns`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_pattern_duplicate_const() {
    assert_code_fires(
        "void f(x) { switch (x) { case const const Object(): break; } }\n",
        "invalid_constant_pattern_duplicate_const",
    );
}

/// Explicitly-const empty record literal in a constant pattern (official
/// :8389-8394; nail shape `const_patterns`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_pattern_empty_record_literal() {
    assert_code_fires(
        "void f(x) { switch (x) { case const (): break; } }\n",
        "invalid_constant_pattern_empty_record_literal",
    );
}

/// Generic type instantiation as a constant pattern (official
/// generic-constant face; nail shape `const_patterns`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_pattern_generic() {
    assert_code_fires(
        "void f(x) { switch (x) { case G<int>: break; } }\n",
        "invalid_constant_pattern_generic",
    );
}

/// Negation of a non-numeric-literal operand in a constant pattern
/// (official numeric-literal-only rule; nail shape `const_patterns`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_pattern_negation() {
    assert_code_fires(
        "void f(x) { switch (x) { case -y: break; } }\n",
        "invalid_constant_pattern_negation",
    );
}

/// Unary `!` in a constant pattern (official parsePrimary checks; nail
/// shape `const_patterns`).
#[test]
fn dark_syntax_exposure_only_invalid_constant_pattern_unary() {
    assert_code_fires(
        "void f(x) { switch (x) { case !false: break; } }\n",
        "invalid_constant_pattern_unary",
    );
}

/// Constructor whose name does not match the enclosing class (official
/// name-mismatch face; nail shape `new_dot_name`).
#[test]
fn dark_syntax_exposure_only_invalid_constructor_name() {
    assert_code_fires(
        "class C {\n  factory.named();\n}\n",
        "invalid_constructor_name",
    );
}

/// `super = 0` as a constructor initializer entry — not a valid initializer
/// (official initializer-entry verifier; nail shape `issue_48411_prime_1`).
#[test]
fn dark_syntax_exposure_only_invalid_initializer() {
    assert_code_fires(
        "class C {\n  C() : super = 0;\n}\n",
        "invalid_initializer",
    );
}

/// `operator =` — the assignment operator is not user-definable (official
/// `parseOperatorName`; nail row on the `=` of an initializer recovery).
#[test]
fn dark_syntax_exposure_only_invalid_operator() {
    assert_code_fires(
        "class C {\n  operator =(x) {}\n}\n",
        "invalid_operator",
    );
}

/// Scanner-channel pin: a string part's content ending in a lone `\`
/// (before the newline) reports the escape-started face. The plain
/// [`setup`] rig asserts scanner-clean, so this goes through the scanner
/// directly.
#[test]
fn dark_syntax_exposure_only_invalid_unicode_escape_started() {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan("var s = 'a\\\n;\n", config, file);
    let got: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_owned())
        .collect();
    assert!(
        got.iter().any(|c| c == "invalid_unicode_escape_started"),
        "lone backslash before the newline must report the escape-started \
         face; got: {got:?}"
    );
}

/// Class-named constructor reference used as a typed list literal — the
/// literal face drops the class head (official :8856-8862; nail shape
/// `issue_45251_list`).
#[test]
fn dark_syntax_exposure_only_literal_with_class() {
    assert_code_fires(
        "final x = List<List<int>>[];\n",
        "literal_with_class",
    );
}

/// Constructor initializer entry without an assignment (`this = 0` shape
/// degrades; official :4873-4881 recovery; nail shape
/// `issue_48411_prime_1`).
#[test]
fn dark_syntax_exposure_only_missing_assignment_in_initializer() {
    assert_code_fires(
        "class C {\n  C() : this = 0;\n}\n",
        "missing_assignment_in_initializer",
    );
}

/// `try` with neither `catch` nor `finally` (official `parseTryStatement`
/// only-try face).
#[test]
fn dark_syntax_exposure_only_missing_catch_or_finally() {
    assert_code_fires("void f() { try {} }\n", "missing_catch_or_finally");
}

/// Top-level member head with no type and no modifier — the type-less
/// rider reports on the member head (official `parseFields` face; nail
/// shape `ambiguous_builder_01`).
#[test]
fn dark_syntax_exposure_only_missing_const_final_var_or_type() {
    assert_code_fires(TOP_LEVEL_SEND_ASSIGN, "missing_const_final_var_or_type");
}

/// Top-level function-shaped member without a parameter list (official
/// `missing_parameters` function flavor; nail shape `ambiguous_builder_01`).
#[test]
fn dark_syntax_exposure_only_missing_function_parameters() {
    assert_code_fires(TOP_LEVEL_SEND_ASSIGN, "missing_function_parameters");
}

/// `null = 0` as a constructor initializer entry — the entry head is not an
/// identifier, so the synthetic-assignment recovery reports (official
/// :4905-4915; nail shape `issue_48411_prime_1`).
#[test]
fn dark_syntax_exposure_only_missing_initializer() {
    assert_code_fires(
        "class C {\n  C() : null = 0;\n}\n",
        "missing_initializer",
    );
}

/// Operator-token head in an initializer context recovers as an operator
/// declaration missing the keyword (official `parseInvalidOperatorDeclaration`;
/// SDK recovery corpus `issue_39230`).
#[test]
fn dark_syntax_exposure_only_missing_keyword_operator() {
    assert_code_fires(
        "class C {\n  C()\n    /\n    : super();\n}\n",
        "missing_keyword_operator",
    );
}

/// Method-shaped member with an `=>` body and no parameter list (official
/// `missing_parameters` method flavor; nail shape `blindspot_accessors`).
#[test]
fn dark_syntax_exposure_only_missing_method_parameters() {
    assert_code_fires(
        "class C {\n  int x => 1;\n}\n",
        "missing_method_parameters",
    );
}

/// `typedef void = ...` — no alias name: the `=` falls to the typedef
/// parameter recovery (official :1913-1918; nail shape
/// `keyword_named_typedefs`).
#[test]
fn dark_syntax_exposure_only_missing_typedef_parameters() {
    assert_code_fires(
        "typedef void = void Function();\n",
        "missing_typedef_parameters",
    );
}

/// Extension type without a representation declaration: the primary-
/// constructor parameter parse reports the missing list (official
/// extension-type header face; nail shape
/// `extension_type_primary_constructor_error`).
#[test]
fn dark_syntax_exposure_only_missing_primary_constructor_parameters() {
    assert_code_fires(
        "extension type E {}\n",
        "missing_primary_constructor_parameters",
    );
}

/// Duplicate `implements` clause on an ENUM (the enum clause loop reports
/// the enum-flavored multipleClauses; nail shape `issue_48371`).
#[test]
fn dark_syntax_exposure_only_multiple_clauses() {
    assert_code_fires(
        "enum E implements Foo implements Bar {\n  v\n}\n",
        "multiple_clauses",
    );
}

/// Named function expression (`void g() => 1` in expression position):
/// official `parseNamedFunctionRest` reports the name (nail shape
/// `const_patterns`).
#[test]
fn dark_syntax_exposure_only_named_function_expression() {
    assert_code_fires(
        "void f() {\n  var cb = void g() => 1;\n}\n",
        "named_function_expression",
    );
}

/// `with` clause before `implements` on an enum (official clause-order
/// recovery; nail shape `issue_48371`).
#[test]
fn dark_syntax_exposure_only_out_of_order_clauses() {
    assert_code_fires(
        "enum E implements Foo with Bar {\n  v\n}\n",
        "out_of_order_clauses",
    );
}

/// Pattern assignment declaring a typed fresh variable (`int _`): fresh
/// declarations are assignment-only-illegal (official assignment-context
/// check; nail shape `wildcard_inPatternAssignment_usingType`).
#[test]
fn dark_syntax_exposure_only_pattern_assignment_declares_variable() {
    assert_code_fires(
        "void f() {\n  [a, int _] = y;\n}\n",
        "pattern_assignment_declares_variable",
    );
}

/// `const (1)` — the const prefix forces record interpretation, and one
/// positional field without a trailing comma reports (official
/// :7649-7689; nail shape `keyword_named_class_methods_2`).
#[test]
fn dark_syntax_exposure_only_record_literal_one_positional_no_trailing_comma() {
    assert_code_fires(
        "void f() { const (1); }\n",
        "record_literal_one_positional_no_trailing_comma",
    );
}

/// Record type `(int)` — one positional field without a trailing comma
/// (official record-type verifier; nail shape `keyword_named_class_methods_2`).
#[test]
fn dark_syntax_exposure_only_record_type_one_positional_no_trailing_comma() {
    assert_code_fires(
        "void f() {\n  (int) x = (1,);\n}\n",
        "record_type_one_positional_no_trailing_comma",
    );
}

/// `=` body on a NON-factory constructor: only factories redirect
/// (official body-kind check; nail shape `issue_48411_prime_1`).
#[test]
fn dark_syntax_exposure_only_redirection_in_non_factory_constructor() {
    assert_code_fires(
        "class C {\n  C() = D;\n}\n",
        "redirection_in_non_factory_constructor",
    );
}

/// `var` representation field on an extension type (official
/// `_checkForExtensionTypeRepresentationErrorCodes`; nail shape
/// `declaring_parameter_error`).
#[test]
fn dark_syntax_exposure_only_representation_field_modifier() {
    assert_code_fires(
        "extension type ET(var a) {}\n",
        "representation_field_modifier",
    );
}

/// `??` in a constant pattern: the un-consumed operator surfaces as the
/// unexpected-token face (official constant-expression tail; nail shape
/// `const_patterns_binary`).
#[test]
fn dark_syntax_exposure_only_unexpected_token() {
    assert_code_fires(
        "void f(x) { switch (x) { case 1 ?? 2: break; } }\n",
        "unexpected_token",
    );
}

/// A junk range before a clause boundary skips with the range face
/// (official `recoverySmallLookAheadSkipTokens`; nail shape `issue_48371`).
#[test]
fn dark_syntax_exposure_only_unexpected_tokens() {
    assert_code_fires(
        "enum E w Foo {\n  v\n}\n",
        "unexpected_tokens",
    );
}

/// `var` + type on a primary-constructor formal (official formal-parameter
/// var face; nail shape `class_primary_constructor_error`).
#[test]
fn dark_syntax_exposure_only_var_and_type() {
    assert_code_fires(
        "class C({var int a}) {}\n",
        "var_and_type",
    );
}
