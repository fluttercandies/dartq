//! Declaration grammar tests (#18): top-level declarations, class members,
//! typedefs, enums, extensions, extension types, and their recovery paths.

use super::collector::setup;

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

// —— header-clause recovery ———————————————————————————————————————————————

#[test]
fn duplicate_extends_reports_once() {
    let c = parse("class C extends A extends B {}");
    assert_eq!(c.error_codes(), ["multiple_extends_clauses"]);
    c.assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

#[test]
fn implements_before_extends_reports_out_of_order() {
    let c = parse("class C implements I extends A {}");
    assert_eq!(c.error_codes(), ["out_of_order_clauses"]);
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
    let c = parse("class C { class D {} }");
    assert_eq!(c.error_codes(), ["class_in_class"]);
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("end", "class_declaration"),
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
    let c = parse("class C { void m([int a: 1]) {} }");
    assert_eq!(c.error_codes(), ["positional_parameter_with_equals"]);
}

#[test]
fn static_method_non_sync_setter_reports() {
    let c = parse("class C { static set g(int v) async {} }");
    assert_eq!(c.error_codes(), ["setter_not_sync"]);
}

#[test]
fn missing_class_body_recovers_at_eof() {
    let c = parse("class C");
    assert_eq!(c.error_codes(), ["expected_token"]);
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("handle", "no_class_body"),
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
    // The binary events prove the expression went through the full
    // engine (the retired diet skip emitted no expression events).
    c.assert_sequence(&[
        ("begin", "field_initializer"),
        ("begin", "binary_expression"),
        ("end", "binary_expression"),
        ("end", "field_initializer"),
    ]);
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
    // anchors on the head `x`. KNOWN DIVERGENCE (first-lane engine): the
    // oracle emits only the one report — its re-read consumes the queued
    // synthetic `<syn>(3)` as the assignment RHS (parser_impl.dart:4894,
    // issue_31192) — while the engine emits a trailing `missing_identifier`
    // at the `(` because the queued synthetic identifier is not consumed as
    // a primary. Left red as the repair pin; the primary report and its
    // anchor match the oracle.
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
    assert_eq!(c.error_codes(), ["expected_switch_expression_body"]);
    assert_eq!(c.errors[0].1, 9);
    assert_eq!(c.errors[0].2, 10);
}
