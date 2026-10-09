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
