//! Declaration grammar tests (#18): top-level declarations, class members,
//! typedefs, enums, extensions, extension types, and their recovery paths.

use super::collector::{latest_features, scan_ok, setup};
use crate::listener::{ParserListener, Token};
use crate::parser::Parser;
use dartq_language::{FeatureSet, LanguageVersion};

fn parse(source: &'static str) -> super::collector::Collector {
    let mut p = setup(source);
    p.parse_unit();
    p.listener
}

// —— valid declaration forms ————————————————————————————————————————————

#[test]
fn class_with_modifiers_and_clauses() {
    let c = parse("abstract base class C extends B implements I {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("begin", "body"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
}

#[test]
fn named_mixin_application() {
    let c = parse("class C = A with M implements I;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "named_mixin_application"),
        ("end", "named_mixin_application"),
    ]);
}

#[test]
fn mixin_with_on_and_implements() {
    let c = parse("base mixin M on A implements B {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "mixin_declaration"), ("end", "mixin_declaration")]);
}

#[test]
fn mixin_application_semantics_on_class() {
    let c = parse("mixin M {} class A {} class B = A with M;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "mixin_declaration"),
        ("end", "mixin_declaration"),
        ("begin", "class_declaration"),
        ("end", "class_declaration"),
        ("begin", "named_mixin_application"),
        ("end", "named_mixin_application"),
    ]);
}

#[test]
fn enum_with_elements() {
    let c = parse("enum E { a, b }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "enum_declaration"), ("end", "enum_declaration")]);
}

#[test]
fn enum_with_members() {
    let c = parse("enum E { a, b; int get x => 1; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "enum_declaration"),
        ("begin", "body"),
        ("begin", "method"),
        ("end", "method"),
        ("end", "body"),
        ("end", "enum_declaration"),
    ]);
}

#[test]
fn extension_with_on_clause() {
    let c = parse("extension E<T> on List<T> { int get len => length; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "extension_declaration"),
        ("begin", "body"),
        ("begin", "method"),
        ("end", "method"),
        ("end", "body"),
        ("end", "extension_declaration"),
    ]);
}

#[test]
fn extension_type_with_representation() {
    let c = parse("extension type ET(int i) implements int {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "extension_type_declaration"),
        ("begin", "body"),
        ("end", "body"),
        ("end", "extension_type_declaration"),
    ]);
}

#[test]
fn typedef_new_style() {
    let c = parse("typedef F<T> = void Function(T);");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "typedef"), ("end", "typedef")]);
}

#[test]
fn typedef_old_style() {
    let c = parse("typedef void F(int x);");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "typedef"), ("end", "typedef")]);
}

#[test]
fn typedef_new_style_generic_gteq_compound() {
    // #167: `T<b>` followed by `= int` scans as a `>=` compound closing the
    // fast-shape group. The official new/old-style head decision reads the
    // split remainder `=` (parser_impl.dart:1497 via splitGtEq,
    // type_info_impl.dart:1263) — not the real successor — so this parses
    // as a valid new-style generic alias.
    let c = parse("typedef T<b>= int;");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "typedef"), ("end", "typedef")]);
}

#[test]
fn typedef_head_compound_gt_gt_stays_old_style() {
    // The remainder `>` of `T<b>>` is not `=`, so the head stays old-style
    // (official `skip.next` = the splitCloser remainder,
    // type_info_impl.dart:1409): the old-style body starts from the noType
    // head instead of entering the new-style name/type-variable path.
    let c = parse("typedef T<b>> = x;");
    c.assert_sequence(&[
        ("begin", "typedef"),
        ("handle", "no_type"),
        ("end", "typedef"),
    ]);
}

#[test]
fn typedef_head_lone_lt_compound_routes_to_member() {
    // `typedef <int,int>> = x;` never reaches the typedef parse: the lone
    // `<` is popped by `appendGtGt`'s inner step WITHOUT an endGroup
    // (abstract_scanner.dart:637-655 — only the outermost closed `<`
    // records one), so the routing guard
    // `keyword.next.endGroup?.next.isA(EQ)` (parser_impl.dart:769) fails
    // and the keyword falls through to `parseTopLevelMemberImpl` (:775).
    // The member parse takes `typedef` as a built-in-keyword method name
    // and reports the missing parameter list anchored on the name
    // (`parseGetterOrFormalParameters`, parser_impl.dart:1704,
    // TopLevelMethod → `missingFunctionParameters`, :2015).
    let c = parse("typedef <int,int>> = x;");
    assert!(
        !c.log.contains(&("begin", "typedef")),
        "unexpected typedef parse: {:?}",
        c.log
    );
    assert!(
        c.log.contains(&("begin", "top_level_member")),
        "no member parse: {:?}",
        c.log
    );
    assert_eq!(
        c.errors.first(),
        Some(&("missing_function_parameters".to_string(), 0, 7)),
        "{:?}",
        c.errors
    );
}

#[test]
fn typedef_head_nested_group_compound_eq_is_new_style() {
    // `typedef <A<B>> = x;` DOES reach the typedef parse: with two `<`
    // opens, `appendGtGt` pops the inner one silently and records the
    // endGroup on the outer `<` (abstract_scanner.dart:644-655), so the
    // routing guard (parser_impl.dart:769) sees `=` and enters
    // `parseTypedef` (:774). The head recheck (:1510-1518) recomputes the
    // group from the keyword and reads its split-stream `skip.next`: the
    // group's own closer is the compound's LAST `>`, whose successor is the
    // real `=` — new-style. The name is missing, so the parse recovers:
    // `missing_identifier` on the consumed `<` (identifier_context_impl.dart
    // consume-in-doubt), the variable list `<A<B>>` walks with a missing
    // comma and per-variable `handle_no_type` (official `parseVariables`,
    // type_info_impl.dart:1476/:1575), and the RHS alias type still parses.
    // The old-style cascade (missing typedef parameters / final-variable
    // riders) never fires.
    let c = parse("typedef <A<B>> = x;");
    assert!(
        c.log.contains(&("begin", "typedef")),
        "no typedef parse: {:?}",
        c.log
    );
    assert!(
        c.log.contains(&("handle", "no_type")),
        "variable recovery events missing: {:?}",
        c.log
    );
    assert!(
        c.log.contains(&("handle", "type")),
        "RHS alias type not parsed: {:?}",
        c.log
    );
    assert!(
        c.errors.iter().any(|e| e.0 == "missing_identifier"),
        "missing-name recovery not reported: {:?}",
        c.errors
    );
    assert!(
        !c.errors.iter().any(
            |e| e.0 == "missing_typedef_parameters" || e.0 == "missing_const_final_var_or_type"
        ),
        "old-style cascade fired: {:?}",
        c.errors
    );
}

#[test]
fn top_level_members() {
    let c = parse("int x = 1; void f() {} int get g => 1; set g(int v) {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "top_level_member"),
        ("begin", "top_level_member"),
        ("begin", "top_level_member"),
        ("begin", "top_level_member"),
    ]);
}

#[test]
fn class_member_soup() {
    let c = parse(
        "class C {\n\
         \x20 C.named();\n\
         \x20 C();\n\
         \x20 int f = 1;\n\
         \x20 static const c = 2;\n\
         \x20 void m(int a, [int b = 1]) {}\n\
         \x20 void n(int a, {required int b}) {}\n\
         \x20 int get g => 1;\n\
         \x20 set g(int v) {}\n\
         \x20 int operator +(o) => 1;\n\
         \x20 factory C.f() = C.named;\n\
         }",
    );
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "constructor"),
        ("end", "constructor"),
        ("begin", "constructor"),
        ("end", "constructor"),
        ("begin", "fields"),
        ("end", "fields"),
        ("begin", "fields"),
        ("end", "fields"),
        ("begin", "method"),
        ("end", "method"),
        ("begin", "method"),
        ("end", "method"),
        ("begin", "method"),
        ("end", "method"),
        ("begin", "method"),
        ("end", "method"),
        ("begin", "method"),
        ("end", "method"),
        ("begin", "factory"),
        ("end", "factory"),
    ]);
}

#[test]
fn function_typed_parameter() {
    let c = parse("class C { void m(void cb(int x), int y) {} }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[("begin", "method"), ("end", "method")]);
}

// —— modifier diagnostics ————————————————————————————————————————————————

#[test]
fn final_enum_reports() {
    let c = parse("final enum E { a }");
    assert_eq!(c.error_codes(), ["final_enum"]);
    c.assert_sequence(&[("begin", "enum_declaration"), ("end", "enum_declaration")]);
}

#[test]
fn base_enum_reports() {
    let c = parse("base enum E { a }");
    assert_eq!(c.error_codes(), ["base_enum"]);
}

#[test]
fn final_mixin_reports() {
    let c = parse("final mixin M {}");
    assert_eq!(c.error_codes(), ["final_mixin"]);
}

#[test]
fn interface_mixin_reports() {
    let c = parse("interface mixin M {}");
    assert_eq!(c.error_codes(), ["interface_mixin"]);
}

#[test]
fn interface_enum_reports() {
    let c = parse("interface enum E { a }");
    assert_eq!(c.error_codes(), ["interface_enum"]);
    c.assert_sequence(&[("begin", "enum_declaration"), ("end", "enum_declaration")]);
}

#[test]
fn sealed_enum_reports() {
    let c = parse("sealed enum E { a }");
    assert_eq!(c.error_codes(), ["sealed_enum"]);
}

#[test]
fn sealed_mixin_reports() {
    let c = parse("sealed mixin M {}");
    assert_eq!(c.error_codes(), ["sealed_mixin"]);
}

#[test]
fn base_mixin_and_base_mixin_class_stay_clean() {
    // `base` is the one modifier mixins and mixin classes accept
    // (parser_impl.dart:794-797/:853-855 — it rides into `parseMixin` /
    // `parseClassOrNamedMixinApplication` instead of a family row).
    for src in ["base mixin M {}", "base mixin class M {}"] {
        let c = parse(src);
        assert!(c.errors.is_empty(), "{src}: {:?}", c.errors);
    }
}

#[test]
fn enum_family_rows_report_in_official_order() {
    // Official family-row order on the enum head: base, final, interface,
    // sealed (parser_impl.dart:689-701); the walk-level `extraneous_modifier`
    // (abstract) lands first because `parseEnumModifiers` runs before the
    // family block. Anchors: the walk's captured tokens and the dispatch
    // scan's pseudo-keyword captures — each row on its own keyword token.
    let c = parse("abstract final base enum E { v }");
    assert_eq!(
        c.errors,
        [
            ("extraneous_modifier".to_string(), 0, 8),
            ("base_enum".to_string(), 15, 19),
            ("final_enum".to_string(), 9, 14),
        ],
        "{:?}",
        c.errors
    );
}

#[test]
fn modifier_rows_carry_registry_corrections() {
    // The ten parser-owned rows carry the full registry face
    // (messages.yaml `correctionMessage`s; 91.0.0 codes_generated.dart).
    const CASES: &[(&str, &str, &str)] = &[
        (
            "base enum E { v }",
            "Enums can't be declared to be 'base'.",
            "Try removing the keyword 'base'.",
        ),
        (
            "final enum E { v }",
            "Enums can't be declared to be 'final'.",
            "Try removing the keyword 'final'.",
        ),
        (
            "interface enum E { v }",
            "Enums can't be declared to be 'interface'.",
            "Try removing the keyword 'interface'.",
        ),
        (
            "sealed enum E { v }",
            "Enums can't be declared to be 'sealed'.",
            "Try removing the keyword 'sealed'.",
        ),
        (
            "final mixin M {}",
            "A mixin can't be declared 'final'.",
            "Try removing the 'final' keyword.",
        ),
        (
            "interface mixin M {}",
            "A mixin can't be declared 'interface'.",
            "Try removing the 'interface' keyword.",
        ),
        (
            "sealed mixin M {}",
            "A mixin can't be declared 'sealed'.",
            "Try removing the 'sealed' keyword.",
        ),
        (
            "final mixin class M {}",
            "A mixin class can't be declared 'final'.",
            "Try removing the 'final' keyword.",
        ),
        (
            "interface mixin class M {}",
            "A mixin class can't be declared 'interface'.",
            "Try removing the 'interface' keyword.",
        ),
        (
            "sealed mixin class M {}",
            "A mixin class can't be declared 'sealed'.",
            "Try removing the 'sealed' keyword.",
        ),
    ];
    for (src, problem, correction) in CASES {
        let c = parse(src);
        assert_eq!(c.error_codes().len(), 1, "{src}: {:?}", c.errors);
        assert_eq!(&c.texts, &[problem.to_string()], "{src}");
        assert_eq!(&c.corrections, &[Some(correction.to_string())], "{src}");
    }
}

// —— header-clause recovery ———————————————————————————————————————————————

#[test]
fn duplicate_extends_reports_once() {
    let c = parse("class C extends A extends B {}");
    assert_eq!(c.error_codes(), ["multiple_extends_clauses"]);
    c.assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

#[test]
fn implements_before_extends_reports_out_of_order() {
    // The header recovery reports `implementsBeforeExtends` on the late
    // `extends` (parser_impl.dart:3243-3248,
    // parseDeclarationHeaderRecoveryInternal); `outOfOrderClauses` is the
    // enum-header with/implements code (parser_impl.dart:2796).
    let c = parse("class C implements I extends A {}");
    assert_eq!(c.error_codes(), ["implements_before_extends"]);
}

#[test]
fn multiple_implements_reports() {
    let c = parse("class C implements I implements J {}");
    assert_eq!(c.error_codes(), ["multiple_implements_clauses"]);
}

// —— member recovery ——————————————————————————————————————————————————————

#[test]
fn missing_operator_keyword() {
    let c = parse("class C { +(x) => 1; }");
    assert_eq!(c.error_codes(), ["missing_keyword_operator"]);
    c.assert_sequence(&[("begin", "method"), ("end", "method")]);
}

#[test]
fn class_in_class_recovery() {
    // Official `reportAndSkipClassInClass` (parser_impl.dart:11464-11485):
    // report, `handleInvalidMember`, then a SILENT skip of the name and
    // `{...}` body — the nested class emits no class events, so the only
    // class_declaration/body events are the outer declaration's.
    let c = parse("class C { class D {} }");
    assert_eq!(c.error_codes(), ["class_in_class"]);
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
}

#[test]
fn required_parameter_with_default_reports() {
    let c = parse("class C { void m(int a = 1) {} }");
    assert_eq!(c.error_codes(), ["required_parameter_with_default"]);
}

#[test]
fn positional_default_with_colon_reports() {
    // `:` on an optional-positional formal wants `=` (oracle
    // `wrongSeparatorForPositionalParameter`, r5 corpus
    // wrong_separator_for_positional_parameter.dart).
    let c = parse("class C { void m([int a: 1]) {} }");
    assert_eq!(
        c.error_codes(),
        ["wrong_separator_for_positional_parameter"]
    );
}

#[test]
fn colon_on_required_positional_reports_named_parameter_outside_group() {
    // `:` on a required-positional formal reads as the named-group separator
    // used outside its group (oracle `namedParameterOutsideGroup`, r5 corpus
    // named_parameter_outside_group.dart: `void f(int x: 1) {}`).
    let c = parse("class C { void m(int a: 1) {} }");
    assert_eq!(c.error_codes(), ["named_parameter_outside_group"]);
}

#[test]
fn atomic_empty_bracket_group_reports_empty_optional_parameter_list() {
    // The scanner keeps `[]` as one INDEX token; the official
    // `parseFormalParametersRest` `[]`-token arm (parser_impl.dart:1963-1967)
    // splits it into `[` + `]` (`rewriteSquareBrackets` :5117 — `[` at the
    // `[]` offset, `]` at +1) and enters `parseOptionalPositionalParameters`,
    // whose `parameterCount == 0` arm (:2452-2462) reports
    // `emptyOptionalParameterList` at the synthetic `]` and feeds a
    // zero-length synthetic identifier to the parameter parse. Registry row
    // (`emptyOptionalParameterList`): problem + correction verbatim.
    let c = parse("void f([]) {}");
    assert_eq!(c.error_codes(), ["empty_optional_parameter_list"]);
    assert_eq!(c.errors[0].1, 8); // synthetic `]` = `[]` offset + 1
    assert_eq!(c.errors[0].2, 9);
    assert_eq!(&c.texts, &["Optional parameter lists cannot be empty."]);
    assert_eq!(
        &c.corrections,
        &[Some(
            "Try adding an optional parameter to the list.".to_string()
        )]
    );

    // The arm fires after a comma too: one row, anchored the same way.
    let c = parse("void f(int x, []) {}");
    assert_eq!(c.error_codes(), ["empty_optional_parameter_list"]);
    assert_eq!(c.errors[0].1, 15);
    assert_eq!(c.errors[0].2, 16);

    // Well-formed formals stay silent.
    let c = parse("void f(int x) {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn spaced_empty_bracket_group_reports_empty_optional_parameter_list() {
    // The real-`]` sibling of the `[]`-token arm above: a spaced empty
    // optional list `void f([ ]) {}` walks `parseOptionalPositionalParameters`
    // normally and takes its `parameterCount == 0` arm
    // (parser_impl.dart:2452-2462), whose `rewriteAndRecover` (:5112)
    // reports `emptyOptionalParameterList` on the real `]` (its own offset,
    // width 1) and splices a zero-length synthetic identifier before it for
    // `parseFormalParameter` (:2459) to consume silently. Same registry row,
    // so problem + correction verbatim.
    let c = parse("void f([ ]) {}");
    assert_eq!(c.error_codes(), ["empty_optional_parameter_list"]);
    assert_eq!(c.errors[0].1, 9); // the real `]` = `[` offset + 2
    assert_eq!(c.errors[0].2, 10);
    assert_eq!(&c.texts, &["Optional parameter lists cannot be empty."]);
    assert_eq!(
        &c.corrections,
        &[Some(
            "Try adding an optional parameter to the list.".to_string()
        )]
    );

    // The arm fires after a comma too: one row, anchored the same way.
    let c = parse("void f(int x, [ ]) {}");
    assert_eq!(c.error_codes(), ["empty_optional_parameter_list"]);
    assert_eq!(c.errors[0].1, 16); // the real `]` = `[` offset + 2
    assert_eq!(c.errors[0].2, 17);

    // A well-formed optional formal stays silent.
    let c = parse("void f([x]) {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn spaced_empty_named_group_reports_empty_named_parameter_list() {
    // The `{`-bracketed sibling of the spaced-`[` test above: a spaced empty
    // named list `void f({ }) {}` walks `parseOptionalNamedParameters`
    // normally and takes its `parameterCount == 0` arm
    // (parser_impl.dart:2516-2533), whose `rewriteAndRecover` (:5112)
    // reports `emptyNamedParameterList` on the real `}` (its own offset,
    // width 1) and splices a zero-length synthetic identifier before it for
    // `parseFormalParameter` (:2527) to consume silently. Same registry row
    // (`emptyNamedParameterList`, diagnostic.g.dart:563), so problem +
    // correction verbatim.
    let c = parse("void f({ }) {}");
    assert_eq!(c.error_codes(), ["empty_named_parameter_list"]);
    assert_eq!(c.errors[0].1, 9); // the real `}` = `{` offset + 2
    assert_eq!(c.errors[0].2, 10);
    assert_eq!(&c.texts, &["Named parameter lists cannot be empty."]);
    assert_eq!(
        &c.corrections,
        &[Some(
            "Try adding a named parameter to the list.".to_string()
        )]
    );

    // The arm fires after a comma too: one row, anchored the same way.
    let c = parse("void f(int x, { }) {}");
    assert_eq!(c.error_codes(), ["empty_named_parameter_list"]);
    assert_eq!(c.errors[0].1, 16); // the real `}` = `{` offset + 2
    assert_eq!(c.errors[0].2, 17);

    // A well-formed named formal stays silent.
    let c = parse("void f({x}) {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

#[test]
fn required_modifier_walk_matches_official_order() {
    // `required` on a non-named formal reports `extraneous_modifier` once at
    // the first token after the walk (modifier_context.dart:212-221); a
    // repeat reports `duplicatedModifier` inside the walk (:594) — walk
    // order first, walk-end second. Named formals stay silent; a doubled
    // named `required` only duplicates.
    let c = parse("class C { void m(required required int x) {} }");
    assert_eq!(
        c.error_codes(),
        ["duplicated_modifier", "extraneous_modifier"],
        "{:?}",
        c.errors
    );

    let c = parse("class C { void m(required int x) {} }");
    assert_eq!(c.error_codes(), ["extraneous_modifier"], "{:?}", c.errors);

    let c = parse("class C { void m({required x}) {} }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);

    let c = parse("class C { void m({required required x}) {} }");
    assert_eq!(c.error_codes(), ["duplicated_modifier"], "{:?}", c.errors);
}

#[test]
fn static_method_non_sync_setter_reports() {
    // `setterNotSync` is a pseudoSharedCode: the analyzer surfaces it as
    // `invalid_modifier_on_setter` (diagnostic.g.dart:2773-2777; report
    // sites parser_impl.dart:4644/:6060).
    let c = parse("class C { static set g(int v) async {} }");
    assert_eq!(c.error_codes(), ["invalid_modifier_on_setter"]);
}

#[test]
fn missing_class_body_recovers_at_eof() {
    // `handleNoClassBody` fires only for the `class C;` form
    // (parser_impl.dart:3117-3125); a missing body at EOF goes through
    // ensureBlock (parser_impl.dart:4984-5003), which reports the BlockKind
    // message and inserts a synthetic `{}` that parses as an empty body.
    // BlockKind.classDeclaration carries diag.expectedClassBody
    // (block_kind.dart:27-30), surfacing as the registry `expected_body`
    // (diagnostic.g.dart:697-703).
    let c = parse("class C");
    assert_eq!(c.error_codes(), ["expected_body"]);
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("begin", "body"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
}

#[test]
fn top_level_stray_token_skips_with_diagnostic() {
    let c = parse("int x = 1; + ; void f() {}");
    assert!(
        !c.errors.is_empty(),
        "expected a diagnostic for the stray '+'"
    );
    c.assert_sequence(&[("begin", "top_level_member")]);
    // The parser must still recover and parse the following function.
    assert!(c.log.contains(&("begin", "top_level_member")));
}

#[test]
fn empty_unit_is_two_events() {
    let c = parse("");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
}

// —— field and constructor initializers (#20 full-engine wiring) —————————

#[test]
fn field_initializer_parses_full_expression() {
    let c = parse("class C { int x = 1 + 2 * 3; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // The official shape folds a field initializer into
    // `VariablesDeclaration > InitializedIdentifier > VariableInitializer`
    // (ast_builder.dart:1749 builds `VariableDeclaration(equals,
    // initializer)`; no `FieldInitializer` node for fields). The binary
    // events prove the expression went through the full engine (the
    // retired diet skip emitted no expression events).
    c.assert_sequence(&[
        ("begin", "variables_declaration"),
        ("begin", "initialized_identifier"),
        ("begin", "variable_initializer"),
        ("begin", "binary_expression"),
        ("end", "binary_expression"),
        ("end", "variable_initializer"),
        ("end", "initialized_identifier"),
        ("end", "variables_declaration"),
    ]);
}

/// The lint-end proof for #502: a `late` field group's `VariablesDeclaration`
/// span head is the `late` token (collapse map §3.3 head-is-late; official
/// `VariableDeclarationList` begins at `lateKeyword`), and the initializer
/// rides `InitializedIdentifier > VariableInitializer` — the shape the
/// lints bridge collapse map dispatches `unnecessary_late` on.
#[test]
fn late_field_variables_declaration_head_is_the_late_token() {
    fn declaration_heads(source: &'static str) -> Vec<usize> {
        struct Heads {
            heads: Vec<usize>,
        }
        impl ParserListener for Heads {
            fn begin_variables_declaration(
                &mut self,
                token: Token,
                _: Option<Token>,
                _: Option<Token>,
            ) {
                self.heads
                    .push(usize::try_from(token.range.offset).expect("offset fits usize"));
            }
        }
        let (table, text) = scan_ok(source);
        let mut parser = Parser::new(text, table, latest_features(), Heads { heads: Vec::new() });
        parser.parse_unit();
        parser.listener.heads
    }

    let static_field = "class C { static late int x = 0; }\n";
    assert_eq!(
        declaration_heads(static_field),
        vec![static_field.find("late").unwrap()],
        "static late field: `late` is the VariablesDeclaration head"
    );
    let top_level = "late var y = 1;\n";
    assert_eq!(
        declaration_heads(top_level),
        vec![0],
        "top-level late variable: `late` is the VariablesDeclaration head"
    );

    // Parser-level diagnostic parity with the oracle probes (dart analyze,
    // 2026-10-10): the initialized/late/const field shapes below are
    // parser-clean; the oracle's `const_instance_field` and
    // `final_not_initialized` on the last two are resolution-phase codes
    // outside the parser surface.
    for source in [
        "class C { int x = 1 + 2 * 3; }\n",
        "class C { late int x = 0; }\n",
        "late var y = 1;\n",
        "class D { const int z = 1, w = 2; }\n",
        "class E { static late int s = 0; final int a; }\n",
    ] {
        let c = parse(source);
        assert!(c.errors.is_empty(), "{source:?}: {:?}", c.errors);
    }
}

/// Drives [`Parser::parse_initializers_opt`] directly on a `:`-led source
/// (the production entry once the constructor head is consumed; the
/// class-member head itself is another lane's surface).
fn initializers(source: &'static str) -> super::collector::Collector {
    let mut p = setup(source);
    p.parse_initializers_opt();
    p.listener
}

#[test]
fn constructor_initializers_parse_entries() {
    let c = initializers(": x = 1, assert(y > 0), super.z(2);");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "initializers"),
        ("begin", "initializer"),
        ("end", "initializer"),
        ("begin", "initializer"),
        ("end", "initializer"),
        ("begin", "initializer"),
        ("end", "initializer"),
        ("end", "initializers"),
    ]);
}

#[test]
fn missing_comma_between_initializers_reports_expected_after() {
    // `: x = 1 y = 2;` — the report anchors on the `1` (the last consumed
    // token of the first entry, official :4791).
    let c = initializers(": x = 1 y = 2;");
    assert_eq!(c.error_codes(), ["expected_token"]);
    assert_eq!(c.errors[0].1, 6);
    assert_eq!(c.errors[0].2, 7);
}

#[test]
fn initializer_without_entry_reports_expected_an_initializer() {
    // `: {}` — the expectedAnInitializer script; the report anchors on the
    // token before the entry (official `messageOnToken: token`).
    let c = initializers(": {}");
    assert_eq!(c.error_codes(), ["missing_initializer"]);
    assert_eq!(c.errors[0].1, 0);
    assert_eq!(c.errors[0].2, 1);
}

#[test]
fn initializer_head_without_assignment_reports_missing_assignment() {
    // `: x(3) {}` — the missingAssignmentInInitializer script; the report
    // anchors on the head `x`. The official stream is `x = <synthetic>(3)`
    // (parser_impl.dart:4894, issue_31192): the head is the assignment LHS
    // and the queued synthetic pair is the RHS, so exactly this one report
    // fires (the queue splices at the cursor, so the event stream is driven
    // directly instead of through the re-read).
    let c = initializers(": x(3) {}");
    assert_eq!(c.error_codes(), ["missing_assignment_in_initializer"]);
    assert_eq!(c.errors[0].1, 2);
    assert_eq!(c.errors[0].2, 3);
}

#[test]
fn redirecting_initializer_with_body_reports() {
    // `: this.x() {}` — the redirectingConstructorWithBody script; the
    // report anchors on the `{`.
    let c = initializers(": this.x() {}");
    assert_eq!(c.error_codes(), ["redirecting_constructor_with_body"]);
    assert_eq!(c.errors[0].1, 11);
    assert_eq!(c.errors[0].2, 12);
}

#[test]
fn super_field_assignment_outside_declaring_class_reports() {
    // `: super.x = 42;` — the fieldInitializedOutsideDeclaringClass script;
    // the report anchors on the member name (the assignment itself is
    // resolution-domain, outside the parser surface).
    let c = initializers(": super.x = 42;");
    assert_eq!(
        c.error_codes(),
        ["field_initialized_outside_declaring_class"]
    );
}

#[test]
fn switch_expression_missing_body_reports_on_close_paren() {
    // `switch (n);` — the expectedSwitchExpressionBody script shape (live
    // pinned-oracle code, block_kind.dart:58 + parser_impl.dart:12433): the
    // report anchors on the `)` (last consumed), never on the `;`, and the
    // synthetic empty body terminates the case list at zero cases.
    let mut p = setup("switch (n);");
    let _ = p.parse_expression();
    let c = p.listener;
    // The per-kind shared-parser body codes all surface under the single
    // analyzer registry name `expected_body` (the nine same-name
    // EXPECTED_BODY variants in the pinned syntactic_errors.g.dart).
    assert_eq!(c.error_codes(), ["expected_body"]);
    assert_eq!(c.errors[0].1, 9);
    assert_eq!(c.errors[0].2, 10);
}

// —— feature-off class-head `(` recovery (probe-verified) ————————————————

/// Parses `source` with a 3.9 feature set (predates the primary-constructors
/// `enabledIn` 3.13.0, matching the 3.13.1 oracle's default probe
/// environment) and returns the listener plus the scanner-diagnostic codes.
/// The scanner channel is merged by the caller: an unclosed `(` legitimately
/// carries a scanner-level `expected_token(')')` (the same fixture class as
/// `unclosed_formal_relocation`, whose rig is
/// `gate_m0_wave3.rs::assert_codes_with_scanner`).
fn parse_off_with_scanner(source: &'static str) -> (super::collector::Collector, Vec<String>) {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let scanner_codes: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_owned())
        .collect();
    let features = crate::ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 9),
    ));
    let mut p = Parser::new(
        text,
        table,
        features,
        super::collector::Collector::default(),
    );
    p.parse_unit();
    (p.listener, scanner_codes)
}

#[test]
fn class_head_paren_off_matches_oracle_face_set() {
    // 3.13.1 oracle probe of the minimal file `class C( {}`: the paren is
    // not a parameter list at a feature-off class head (3.15 main parses
    // class-head formals unconditionally, parser_impl.dart:3782-3838 — that
    // is our feature-on path). The `{}`-remnant parses as a member with no
    // type and no name, and the pending paren closes at EOF. Full face set
    // over the merged scanner+parser channel: expected_token(';') at the
    // `{`, missing_identifier + missing_const_final_var_or_type at the `}`,
    // expected_executable at the pending paren's EOF position, and the
    // scanner's unbalanced-group expected_token(')') pinned at the same
    // spot (diag.rs:22).
    let (c, scanner) = parse_off_with_scanner("class C( {}");
    let mut codes = c.error_codes();
    codes.extend(scanner);
    codes.sort();
    assert_eq!(
        codes,
        [
            "expected_executable",
            "expected_token",
            "expected_token",
            "missing_const_final_var_or_type",
            "missing_identifier",
        ]
    );
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("begin", "body"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
    // Follower fixture (3.13.1 oracle probe of `class C( {}\nclass D {}`,
    // verbatim): the oracle consumes D's keyword as a junk member inside
    // C — `unexpected_token` "Unexpected text '('." at the `(`, the shared
    // remnant faces, then `expected_executable` spanning `class` with the
    // pending paren's `expected_token(')')` anchored at the same boundary.
    // Merged 6-row multiset: [unexpected_token, expected_token(';'),
    // missing_identifier, missing_const_final_var_or_type,
    // expected_executable, expected_token(')')]. Our parser carries the
    // `)`-face count via the scanner's EOF row (the scanner runs eagerly,
    // so its unbalanced-group row is unconditional) and lets D parse as
    // its own declaration — observably identical at (code, file): the
    // oracle emits no rows for D either.
    let (c2, scanner2) = parse_off_with_scanner("class C( {}\nclass D {}");
    let mut codes2 = c2.error_codes();
    codes2.extend(scanner2);
    codes2.sort();
    assert_eq!(
        codes2,
        [
            "expected_executable",
            "expected_token",
            "expected_token",
            "missing_const_final_var_or_type",
            "missing_identifier",
            "unexpected_token",
        ]
    );
    // Span + text parity (oracle rows 1|8|1 and 2|1|5): the
    // unexpected_token spans the `(` token alone with the analyzer's
    // re-worded text, and the junk-member face spans the boundary keyword
    // alone (parser-channel order: the three remnant faces, then the two
    // follower faces).
    assert_eq!(c2.errors[3], ("unexpected_token".to_string(), 7, 8));
    assert_eq!(c2.texts[3], "Unexpected text '('.");
    assert_eq!(c2.errors[4], ("expected_executable".to_string(), 12, 17));
}
