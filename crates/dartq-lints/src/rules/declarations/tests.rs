use dartq_ast::NodeKind;

use crate::registry::EnableSet;
use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};
use dartq_parser::MemberKind;

/// `test/rules/avoid_return_types_on_setters_test.dart` `test_typedSet`.
#[test]
fn avoid_return_types_reports_a_typed_setter() {
    let source = "void set x(v) {}";
    // void0 set1 x2 (3 v4 )5 {6 }7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "set", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 6);
            b.begin(NodeKind::Block, 6);
            b.end(NodeKind::Block, 8);
            b.end(NodeKind::TopLevelMethod, 8);
        }),
    );
    let lints = f.run(&one_rule("avoid_return_types_on_setters"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary return type on a setter.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    assert_eq!(lints[0].range.end, utf16_at(&f, 0) + 4); // after `void`
}

#[test]
fn avoid_return_types_untyped_setter_passes() {
    let source = "set x(v) {}";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "set", 0));
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 5);
            b.begin(NodeKind::Block, 5);
            b.end(NodeKind::Block, 7);
            b.end(NodeKind::TopLevelMethod, 7);
        }),
    );
    assert!(f.run(&one_rule("avoid_return_types_on_setters")).is_empty());
}

/// Getters never report — only the `set` keyword qualifies.
#[test]
fn avoid_return_types_getter_passes() {
    let source = "int get x => 1;";
    // int0 get1 x2 =>3 14 ;5
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "get", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.leaf(NodeKind::LiteralInt, 4);
            b.end(NodeKind::TopLevelMethod, 6);
        }),
    );
    assert!(f.run(&one_rule("avoid_return_types_on_setters")).is_empty());
}

/// `test/rules/unnecessary_late_test.dart` `test_topLevelVariable`.
#[test]
fn unnecessary_late_reports_a_top_level_variable() {
    let source = "late var x = 1;";
    // late0 var1 x2 =3 14 ;5 — `late` is the span head (#174)
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 2);
            b.begin(NodeKind::VariableInitializer, 3);
            b.leaf(NodeKind::LiteralInt, 4);
            b.end(NodeKind::VariableInitializer, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::TopLevelFields, 6);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_late"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary 'late' modifier.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 0));
}

/// An uninitialized declaration keeps `late` meaningful (:51-54).
#[test]
fn unnecessary_late_uninitialized_passes() {
    let source = "late var x;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::InitializedIdentifier, 3);
            b.end(NodeKind::VariablesDeclaration, 3);
            b.end(NodeKind::TopLevelFields, 3);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_late")).is_empty());
}

/// Instance fields are excluded — lazy init is meaningful there.
#[test]
fn unnecessary_late_instance_field_passes() {
    let source = "class A { late var x = 1; }";
    // class0 A1 {2 late3 var4 x5 =6 17 ;8 }9
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 4);
            b.begin(NodeKind::InitializedIdentifier, 5);
            b.begin(NodeKind::VariableInitializer, 6);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 9);
            b.end(NodeKind::Fields, 9);
            b.end(NodeKind::DeclarationBody, 10);
            b.end(NodeKind::ClassDeclaration, 10);
            b.end(NodeKind::TopLevelDeclaration, 10);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_late")).is_empty());
}

/// The static-field face through the production bridge: the bridge's
/// `Fields` word 0 is the `MemberKind` discriminant and the `late`
/// head contract holds on real parses; the instance face on the same
/// parse path stays silent.
#[test]
fn unnecessary_late_static_field_reports_on_real_parse() {
    let f = Fixture::parse("class A {\n  static late var x = 1;\n}\n");
    let lints = f.run(&one_rule("unnecessary_late"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary 'late' modifier.");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "late", 0))
    );

    let f = Fixture::parse("class A {\n  late var x = 1;\n}\n");
    assert!(f.run(&one_rule("unnecessary_late")).is_empty());
}

/// Static fields report (`unnecessary_late.dart:36-45`).
#[test]
fn unnecessary_late_static_field_reports() {
    let source = "class A { late var x = 1; }";
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::StaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 4);
            b.begin(NodeKind::InitializedIdentifier, 5);
            b.begin(NodeKind::VariableInitializer, 6);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 9);
            b.end(NodeKind::Fields, 9);
            b.end(NodeKind::DeclarationBody, 10);
            b.end(NodeKind::ClassDeclaration, 10);
            b.end(NodeKind::TopLevelDeclaration, 10);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_late"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 3));
}

/// `test/rules/use_function_type_syntax_for_parameters_test.dart`
/// `test_functionTypedParameter`.
#[test]
fn function_type_syntax_reports_the_old_form() {
    let source = "f(void g()) {}";
    // f0 (1 void2 g3 (4 )5 )6 {7 }8
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::FunctionTypedFormalParameter, 2);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 2, 3);
            identifier(b, 3, 4);
            b.begin(NodeKind::FormalParameters, 4);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::FunctionTypedFormalParameter, 6);
            b.end(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::Block, 7);
            b.end(NodeKind::Block, 9);
            b.end(NodeKind::TopLevelMethod, 9);
        }),
    );
    let lints = f.run(&one_rule("use_function_type_syntax_for_parameters"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use the generic function type syntax to declare the parameter 'g'."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 2));
}

#[test]
fn function_type_syntax_generic_form_passes() {
    let source = "f(void Function() g) {}";
    // f0 (1 void2 Function3 (4 )5 g6 )7 {8 }9
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::FormalParameter, 6);
            b.patch(0, NONE);
            b.patch(1, 6);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 2, 6);
            b.end(NodeKind::FormalParameter, 7);
            b.end(NodeKind::FormalParameters, 8);
            b.begin(NodeKind::Block, 8);
            b.end(NodeKind::Block, 10);
            b.end(NodeKind::TopLevelMethod, 10);
        }),
    );
    assert!(
        f.run(&one_rule("use_function_type_syntax_for_parameters"))
            .is_empty()
    );
}

/// `test/rules/prefer_typing_uninitialized_variables_test.dart`
/// `test_untypedLocalWithoutInitializer`.
#[test]
fn prefer_typing_reports_an_untyped_local() {
    let source = "var x;";
    // var0 x1 ;2
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, 0);
            b.leaf(NodeKind::NoType, 0);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.end(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::VariablesDeclaration, 2);
            b.end(NodeKind::Block, 3);
        }),
    );
    let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "An uninitialized variable should have an explicit type annotation."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 1));
}

/// Fields carry the second official message variant
/// (`diagnostic.g.dart` `preferTypingUninitializedVariablesForField`).
#[test]
fn prefer_typing_reports_an_untyped_field_with_field_text() {
    let source = "class A { var x; }";
    // class0 A1 {2 var3 x4 ;5 }6
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 3);
            b.leaf(NodeKind::NoType, 3);
            b.begin(NodeKind::InitializedIdentifier, 4);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.end(NodeKind::DeclarationBody, 7);
            b.end(NodeKind::ClassDeclaration, 7);
            b.end(NodeKind::TopLevelDeclaration, 7);
        }),
    );
    let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "An uninitialized field should have an explicit type annotation."
    );
}

#[test]
fn prefer_typing_typed_declaration_passes() {
    let source = "int x;";
    // int0 x1 ;2
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 0, 1);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.end(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::VariablesDeclaration, 2);
            b.end(NodeKind::Block, 3);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
}

#[test]
fn prefer_typing_initialized_variable_passes() {
    let source = "var x = 1;";
    // var0 x1 =2 13 ;4
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.leaf(NodeKind::NoType, 0);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.leaf(NodeKind::LiteralInt, 3);
            b.end(NodeKind::VariableInitializer, 4);
            b.end(NodeKind::InitializedIdentifier, 4);
            b.end(NodeKind::VariablesDeclaration, 5);
            b.end(NodeKind::Block, 5);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
}

/// The keyword-only local face on a production parse — the `NoType` row
/// leaks before the declaration's window, so the untyped decision reads
/// the empty token run after `var` (`test_untypedLocalWithoutInitializer`
/// production shape).
#[test]
fn prefer_typing_keyword_local_reports_on_real_parse() {
    let source = "void f() {\n  var pending;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "An uninitialized variable should have an explicit type annotation."
    );
    let pending = tok(&f.source, &f.tokens, "pending", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, pending));
}

/// Typed declarations stay silent on the same production faces — a
/// keyword-anchored field row carries its type between the keyword and
/// the name.
#[test]
fn prefer_typing_typed_declarations_pass_on_real_parse() {
    let f = Fixture::parse("void f() {\n  int later;\n}\nclass A {\n  int x;\n}\n");
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
}

/// The for-each loop variable is a formal upstream
/// (`ForEachParts.loopVariable`), never a `VariableDeclarationList` —
/// the production row tiles inside the for node and stays silent. A
/// classic `for` init row keeps a real declaration-list face and
/// reports. The lint-gate wave1 over-reports (`prefer_foreach.dart`
/// ×5, `use_string_buffers.dart` ×2, `no_adjacent_strings_in_list.dart`
/// 20:19) are the for-each face.
#[test]
fn prefer_typing_for_each_loop_variable_passes() {
    let f = Fixture::parse(
        "void f(List<int> l, void Function(int) fn) {\n  for (final s in l) {\n    fn(s);\n  }\n}\n",
    );
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
    let f = Fixture::parse("var c = [for (var i in [1]) 'a'];\n");
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
    let f = Fixture::parse("void f() {\n  for (var i; i < 1; i++) {}\n}\n");
    assert_eq!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .len(),
        1
    );
}

/// A keyword-anchored field row with a written type stays silent — the
/// type sits between the storage keyword and the declarator name
/// (`final int i;`, the lint-gate `exhaustive_cases.dart` 15:13 face)
/// — while a keyword-only row still reports.
#[test]
fn prefer_typing_typed_keyword_field_passes() {
    let f = Fixture::parse("class E {\n  final int i;\n  const E._(this.i);\n}\n");
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
    let f = Fixture::parse("class A {\n  var x;\n}\n");
    assert_eq!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .len(),
        1
    );
}

/// The generalized-function-type parameter face: `final int inner` is
/// a formal upstream, never a declaration list — whatever recovery-row
/// silhouette the production tiling hangs for it inside the typedef
/// (`avoid_final_parameters.dart` 23:37), the written `final int` run
/// before the name answers the type read and stays silent.
#[test]
fn prefer_typing_function_type_parameter_passes() {
    let f = Fixture::parse("typedef F = void Function(final int inner);\n");
    assert!(
        f.run(&one_rule("prefer_typing_uninitialized_variables"))
            .is_empty()
    );
}

// —— prefer_generic_function_type_aliases —————————————————————————————

/// `test/rules/prefer_generic_function_type_aliases_test.dart`
/// `test_typedef_genericFunctionType` — the rewrite is spelled in the
/// correction.
#[test]
fn generic_function_alias_reports_the_old_form() {
    let source = "typedef int F(int x);";
    // typedef0 int1 F2 (3 int4 x5 )6 ;7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Typedef, 0);
            b.patch(0, tok(source, t, "typedef", 0));
            b.patch(1, NONE); // old form: no `=`
            b.leaf_span(NodeKind::Type, 1, 2);
            identifier(b, 2, 3);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 7);
            b.end(NodeKind::Typedef, 8);
        }),
    );
    let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use the generic function type syntax in 'typedef's."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try using the generic function type syntax ('int Function(int x)').".to_owned())
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 2));
}

/// A missing return type contributes nothing to the rewrite
/// (`returnType == null` → `''`).
#[test]
fn generic_function_alias_without_return_type() {
    let source = "typedef F(int x);";
    // typedef0 F1 (2 int3 x4 )5 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Typedef, 0);
            b.patch(0, tok(source, t, "typedef", 0));
            b.patch(1, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::FormalParameters, 2);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::Typedef, 7);
        }),
    );
    let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].correction,
        Some("Try using the generic function type syntax ('Function(int x)').".to_owned())
    );
}

/// Type parameters carry into the rewrite after `Function`.
#[test]
fn generic_function_alias_with_type_parameters() {
    let source = "typedef F<T>(T x);";
    // typedef0 F1 <2 T3 >4 (5 T6 x7 )8 ;9
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Typedef, 0);
            b.patch(0, tok(source, t, "typedef", 0));
            b.patch(1, NONE);
            identifier(b, 1, 2);
            b.leaf_span(NodeKind::TypeVariables, 2, 5);
            b.begin(NodeKind::FormalParameters, 5);
            b.end(NodeKind::FormalParameters, 9);
            b.end(NodeKind::Typedef, 10);
        }),
    );
    let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].correction,
        Some("Try using the generic function type syntax ('Function<T>(T x)').".to_owned())
    );
}

/// The `=` form is the recommended syntax — it never dispatches.
#[test]
fn generic_function_alias_new_form_passes() {
    let source = "typedef F = void Function();";
    // typedef0 F1 =2 void3 Function4 (5 )6 ;7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Typedef, 0);
            b.patch(0, tok(source, t, "typedef", 0));
            b.patch(1, tok(source, t, "=", 0));
            identifier(b, 1, 2);
            b.leaf_span(NodeKind::Type, 3, 7);
            b.end(NodeKind::Typedef, 8);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_generic_function_type_aliases"))
            .is_empty()
    );
}

// —— unnecessary_constructor_name —————————————————————————————————————

/// `test/rules/unnecessary_constructor_name_test.dart` `test_withNew`.
#[test]
fn constructor_name_reports_new() {
    let f = Fixture::parse("class A { A.new(); }");
    let lints = f.run(&one_rule("unnecessary_constructor_name"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary '.new' constructor name.");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
    );
}

/// An unnamed constructor has no name token to check; a differently
/// named constructor is not `.new`.
#[test]
fn constructor_name_unnamed_and_named_other_pass() {
    let f = Fixture::parse("class A { A(); }");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

    let f = Fixture::parse("class A { A.named(); }");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
}

/// Duplicate unnamed constructors already error (:45-52): with a
/// second unnamed (or `new`-spelled) constructor the bail fires.
/// Non-constructor members never count toward the bail — the flagged
/// count divergence (a field sibling no longer silences the report).
#[test]
fn constructor_name_duplicate_unnamed_bails() {
    let f = Fixture::parse("class A { A(); A.new(); }");
    assert!(
        f.run(&one_rule("unnecessary_constructor_name")).is_empty(),
        "two constructors named `new` — the duplicate error owns it"
    );

    let f = Fixture::parse("class A { int a = 0; A.new(); }");
    assert_eq!(
        f.run(&one_rule("unnecessary_constructor_name")).len(),
        1,
        "a field sibling is not an unnamed constructor"
    );
}

/// `Foo.new(1)` reports at the `new` name token through the production
/// bridge (the creation face reads the owner slice: the `.new` member
/// invocation over a bare receiver, `rules::slice`).
#[test]
fn creation_face_reports_dot_new() {
    let f = Fixture::parse("void f() { Foo.new(1); }\n");
    let lints = f.run(&one_rule("unnecessary_constructor_name"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
    );
}

/// A constructor tear-off (`Foo.new` without the invocation) is not an
/// instance creation, and a chained receiver over a non-prefix head
/// fails both faces — the documented conservative family.
#[test]
fn creation_face_tearoff_and_chained_pass() {
    let f = Fixture::parse("void f() { g(Foo.new); }\n");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

    let f = Fixture::parse("void f() { a.b.new(1); }\n");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
}

/// The prefixed instance-creation face: `p.C.new(…)` over an import
/// prefix of the unit reports at `.new` (the written import surface);
/// the same chain over a non-prefix head stays silent, and a
/// three-segment chain is not a type.
#[test]
fn creation_face_prefixed_receiver_reports() {
    let f = Fixture::parse("import 'kit.dart' as p;\nvoid f() {\n  p.C.new(1);\n}\n");
    let lints = f.run(&one_rule("unnecessary_constructor_name"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary '.new' constructor name.");
    let new_kw = tok(&f.source, &f.tokens, "new", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, new_kw));

    let f = Fixture::parse("void f() {\n  var local = 1;\n  local.b.new(1);\n}\n");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

    let f = Fixture::parse("import 'kit.dart' as p;\nvoid f() {\n  p.a.b.new(1);\n}\n");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
}

/// The primary-constructor face (:70-80): a `.new` primary name
/// reports; the unnamed primary has no name token.
#[test]
fn constructor_name_primary_faces() {
    let f = Fixture::parse("extension type E.new(int x) {}");
    let lints = f.run(&one_rule("unnecessary_constructor_name"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
    );

    let f = Fixture::parse("class C.new(int x);");
    assert_eq!(f.run(&one_rule("unnecessary_constructor_name")).len(), 1);

    let f = Fixture::parse("extension type E(int x) {}");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

    let f = Fixture::parse("extension type E.named(int x) {}");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
}

/// The extension-type exemption (:51-58): a secondary constructor
/// `.new` inside an extension type whose primary constructor is
/// unnamed stays — the qualified form is necessary there.
#[test]
fn constructor_name_extension_type_unnamed_primary_bails() {
    let f = Fixture::parse("extension type E(int x) { E.other(); E.new(); }");
    assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
}

/// The instance-creation face: `A.new()` (empty argument list) reports
/// at `.new`'s name, through the production bridge (the owner slice
/// reads the bare-receiver tear-off group behind the `.new` name).
#[test]
fn constructor_name_creation_reports_new() {
    let f = Fixture::parse("void f() { A.new(); }\n");
    let lints = f.run(&one_rule("unnecessary_constructor_name"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
    );
}

/// The prefixed creation face: `p.Foo.new(…)` over a written import
/// prefix reports — the official `_check` is purely lexical (the
/// constructor name token lexeme, no resolution gate), so the
/// erroneous `p.Missing.new(…)` reports on both sides alike.
#[test]
fn constructor_name_prefixed_creation_reports() {
    let f = Fixture::parse("import 'missing.dart' as p;\nvoid f() {\n  p.Missing.new(1);\n}\n");
    let lints = f.run(&one_rule("unnecessary_constructor_name"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
    );
}

// —— hash_and_equals ——————————————————————————————————————————————————

/// `test/rules/hash_and_equals_test.dart` `test_onlyEquals` — the
/// missing `hashCode` reports at `==`.
#[test]
fn hash_and_equals_reports_missing_hash_code() {
    let source = "class A { bool operator ==(o) => true; }";
    // class0 A1 {2 bool3 operator4 ==5 (6 o7 )8 =>9 true10 ;11 }12
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Method, 3);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::OperatorName, 4);
            b.patch(0, 5);
            b.patch(1, 0);
            b.end(NodeKind::OperatorName, 6);
            b.begin(NodeKind::FormalParameters, 6);
            b.end(NodeKind::FormalParameters, 9);
            b.begin(NodeKind::ExpressionBody, 9);
            b.patch(0, 9);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::LiteralBool, 10);
            b.end(NodeKind::ExpressionBody, 12);
            b.end(NodeKind::Method, 12);
            b.end(NodeKind::DeclarationBody, 13);
            b.end(NodeKind::ClassDeclaration, 13);
            b.end(NodeKind::TopLevelDeclaration, 13);
        }),
    );
    let lints = f.run(&one_rule("hash_and_equals"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Missing a corresponding override of 'hashCode'."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try overriding 'hashCode' or removing '=='.".to_owned())
    );
    // Upstream `reportAtToken(name)` spans the operator
    // (`test_equalsEquals_noHashCode` marks `[!==!]`).
    assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    assert_eq!(lints[0].range.end, utf16_at(&f, 5) + 2);
}

/// `test_onlyHashCode` — the getter form reports at the name.
#[test]
fn hash_and_equals_reports_missing_equals() {
    let source = "class A { int get hashCode => 0; }";
    // class0 A1 {2 int3 get4 hashCode5 =>6 07 ;8 }9
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Method, 3);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "get", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            identifier(b, 5, 6);
            b.begin(NodeKind::ExpressionBody, 6);
            b.patch(0, 6);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::ExpressionBody, 9);
            b.end(NodeKind::Method, 9);
            b.end(NodeKind::DeclarationBody, 10);
            b.end(NodeKind::ClassDeclaration, 10);
            b.end(NodeKind::TopLevelDeclaration, 10);
        }),
    );
    let lints = f.run(&one_rule("hash_and_equals"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Missing a corresponding override of '=='."
    );
    // The name spans whole (`test_hashCode_noEqualsEquals` marks
    // `[!hashCode!]`).
    assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    assert_eq!(lints[0].range.end, utf16_at(&f, 5) + 8);
}

/// `test_onlyGetHashCodeDeclaration` — the field form reports at the
/// variable name (the official `getFieldName` fallback).
#[test]
fn hash_and_equals_field_form_reports() {
    let source = "class A { final hashCode = 0; }";
    // class0 A1 {2 final3 hashCode4 =5 06 ;7 }8
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 3);
            b.begin(NodeKind::InitializedIdentifier, 4);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 9);
            b.end(NodeKind::ClassDeclaration, 9);
            b.end(NodeKind::TopLevelDeclaration, 9);
        }),
    );
    let lints = f.run(&one_rule("hash_and_equals"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 4));
}

#[test]
fn hash_and_equals_both_present_passes() {
    let source = "class A { bool operator ==(o) => true; int get hashCode => 0; }";
    // class0 A1 {2 bool3 operator4 ==5 (6 o7 )8 =>9 true10 ;11 int12
    // get13 hashCode14 =>15 016 ;17 }18
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Method, 3);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::OperatorName, 4);
            b.patch(0, 5);
            b.patch(1, 0);
            b.end(NodeKind::OperatorName, 6);
            b.begin(NodeKind::FormalParameters, 6);
            b.end(NodeKind::FormalParameters, 9);
            b.begin(NodeKind::ExpressionBody, 9);
            b.patch(0, 9);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::LiteralBool, 10);
            b.end(NodeKind::ExpressionBody, 12);
            b.end(NodeKind::Method, 12);
            b.begin(NodeKind::Method, 12);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "get", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 12, 13);
            identifier(b, 14, 15);
            b.begin(NodeKind::ExpressionBody, 15);
            b.patch(0, 15);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::LiteralInt, 16);
            b.end(NodeKind::ExpressionBody, 18);
            b.end(NodeKind::Method, 18);
            b.end(NodeKind::DeclarationBody, 19);
            b.end(NodeKind::ClassDeclaration, 19);
            b.end(NodeKind::TopLevelDeclaration, 19);
        }),
    );
    assert!(f.run(&one_rule("hash_and_equals")).is_empty());
}

#[test]
fn hash_and_equals_neither_present_passes() {
    let source = "class A { int x = 0; }";
    // class0 A1 {2 int3 x4 =5 06 ;7 }8
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 3);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 9);
            b.end(NodeKind::ClassDeclaration, 9);
            b.end(NodeKind::TopLevelDeclaration, 9);
        }),
    );
    assert!(f.run(&one_rule("hash_and_equals")).is_empty());
}

/// The production operator shape — the `OperatorName` row rides the
/// member's preceding sibling — reports at `==` (upstream
/// `test_onlyEquals`).
#[test]
fn hash_and_equals_operator_member_reports_on_real_parse() {
    let source = "class C {\n  @override\n  bool operator ==(Object other) => true;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("hash_and_equals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Missing a corresponding override of 'hashCode'."
    );
    let eq = tok(&f.source, &f.tokens, "==", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, eq));
}

/// The typed `hashCode` getter names through its declaration name on a
/// production parse (the written return type's identifier row must not
/// take the name slot) — upstream `test_onlyHashCode`.
#[test]
fn hash_and_equals_typed_hash_code_getter_reports_on_real_parse() {
    let source = "class C {\n  int get hashCode => 0;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("hash_and_equals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Missing a corresponding override of '=='."
    );
    let hash_code = tok(&f.source, &f.tokens, "hashCode", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, hash_code));
}

// —— avoid_init_to_null ———————————————————————————————————————————————

/// `test/rules/avoid_init_to_null_test.dart` `test_varInitializeNull` —
/// a `var x = null` local reports at the declaration.
#[test]
fn init_to_null_reports_a_local() {
    let source = "var x = null;";
    // var0 x1 =2 null3 ;4
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.leaf(NodeKind::LiteralNull, 3);
            b.end(NodeKind::VariableInitializer, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 5);
            b.end(NodeKind::Block, 5);
        }),
    );
    let lints = f.run(&one_rule("avoid_init_to_null"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Redundant initialization to 'null'.");
    assert_eq!(
        lints[0].correction,
        Some("Try removing the initializer.".to_owned())
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 1));
}

/// A typed variable reports; `final`/`const` declarations are exempt
/// (`!node.isConst && !node.isFinal`, :62-63).
#[test]
fn init_to_null_final_and_const_pass() {
    let typed = "int? x = null;";
    // int0 ?1 x2 =3 null4 ;5
    let f = Fixture::build(
        typed,
        unit_of(|b, _t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 0, 2);
            b.begin(NodeKind::InitializedIdentifier, 2);
            b.begin(NodeKind::VariableInitializer, 3);
            b.leaf(NodeKind::LiteralNull, 4);
            b.end(NodeKind::VariableInitializer, 6);
            b.end(NodeKind::InitializedIdentifier, 6);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Block, 6);
        }),
    );
    assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);

    for keyword in ["final", "const"] {
        let source = format!("{keyword} x = null;");
        // final0 x1 =2 null3 ;4
        let f = Fixture::build(
            &source,
            unit_of(|b, t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(&source, t, keyword, 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralNull, 3);
                b.end(NodeKind::VariableInitializer, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        assert!(
            f.run(&one_rule("avoid_init_to_null")).is_empty(),
            "{source}"
        );
    }
}

/// A field reports (`test_defaultNull` family).
#[test]
fn init_to_null_reports_a_field() {
    let source = "class A { var x = null; }";
    // class0 A1 {2 var3 x4 =5 null6 ;7 }8
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 4);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralNull, 6);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 9);
            b.end(NodeKind::ClassDeclaration, 9);
            b.end(NodeKind::TopLevelDeclaration, 9);
        }),
    );
    assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);
}

/// The parameter default-value face: `{int? x = null}` reports, a
/// non-nullable written type is a compile error upstream and stays
/// silent (`isNullable`, :87), a non-null default does not report.
#[test]
fn init_to_null_parameter_default() {
    let null_default = "f({int? x = null}) {}";
    // f0 (1 {2 int3 ?4 x5 =6 null7 }8 )9 {10 }11
    let f = Fixture::build(
        null_default,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::OptionalFormalParameters, 2);
            b.begin(NodeKind::FormalParameter, 3);
            b.patch(0, NONE);
            b.patch(1, 5);
            b.patch(2, 7);
            b.patch(3, 8);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 3, 5);
            b.end(NodeKind::FormalParameter, 8);
            b.end(NodeKind::OptionalFormalParameters, 9);
            b.end(NodeKind::FormalParameters, 10);
            b.begin(NodeKind::Block, 10);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::TopLevelMethod, 12);
        }),
    );
    let lints = f.run(&one_rule("avoid_init_to_null"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 5));

    let int_default = "f({int x = null}) {}";
    // f0 (1 {2 int3 x4 =5 null6 }7 )8 {9 }10
    let f = Fixture::build(
        int_default,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::OptionalFormalParameters, 2);
            b.begin(NodeKind::FormalParameter, 3);
            b.patch(0, NONE);
            b.patch(1, 4);
            b.patch(2, 6);
            b.patch(3, 7);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.end(NodeKind::FormalParameter, 7);
            b.end(NodeKind::OptionalFormalParameters, 8);
            b.end(NodeKind::FormalParameters, 9);
            b.begin(NodeKind::Block, 9);
            b.end(NodeKind::Block, 11);
            b.end(NodeKind::TopLevelMethod, 11);
        }),
    );
    assert!(
        f.run(&one_rule("avoid_init_to_null")).is_empty(),
        "`int x = null` is a compile error upstream"
    );

    let int_default = "f({int x = 1}) {}";
    // f0 (1 {2 int3 x4 =5 16 }7 )8 {9 }10
    let f = Fixture::build(
        int_default,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::OptionalFormalParameters, 2);
            b.begin(NodeKind::FormalParameter, 3);
            b.patch(0, NONE);
            b.patch(1, 4);
            b.patch(2, 6);
            b.patch(3, 7);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.end(NodeKind::FormalParameter, 7);
            b.end(NodeKind::OptionalFormalParameters, 8);
            b.end(NodeKind::FormalParameters, 9);
            b.begin(NodeKind::Block, 9);
            b.end(NodeKind::Block, 11);
            b.end(NodeKind::TopLevelMethod, 11);
        }),
    );
    assert!(f.run(&one_rule("avoid_init_to_null")).is_empty());
}

/// The variable-face nullable gate (:41/:65): a written non-nullable
/// type is a compile error upstream and stays silent; `dynamic` and the
/// tail-`?` spellings report.
#[test]
fn init_to_null_nullable_written_type_gate() {
    let f = Fixture::parse("void f() {\n  int x = null;\n}\n");
    assert!(f.run(&one_rule("avoid_init_to_null")).is_empty());

    let f = Fixture::parse("void f() {\n  int? x = null;\n}\n");
    assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);

    let f = Fixture::parse("void f() {\n  dynamic x = null;\n}\n");
    assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);

    // Fields carry the same gate: `int _x = null;` bails, `int? _x`
    // reports.
    let f = Fixture::parse("class C {\n  int _x = null;\n}\n");
    assert!(f.run(&one_rule("avoid_init_to_null")).is_empty());

    let f = Fixture::parse("class C {\n  int? _x = null;\n}\n");
    assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);
}

/// A non-null initializer never reports.
#[test]
fn init_to_non_null_passes() {
    let source = "var x = 1;";
    // var0 x1 =2 13 ;4
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.leaf(NodeKind::LiteralInt, 3);
            b.end(NodeKind::VariableInitializer, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 5);
            b.end(NodeKind::Block, 5);
        }),
    );
    assert!(f.run(&one_rule("avoid_init_to_null")).is_empty());
}

// —— prefer_function_declarations_over_variables ——————————————————————

/// `test/rules/prefer_function_declarations_over_variables_test.dart`
/// `test_local` — a final local initialized with a closure reports.
#[test]
fn function_variable_local_reports() {
    let source = "final f = () {};";
    // final0 f1 =2 (3 )4 {5 }6 ;7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "final", 0));
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.begin(NodeKind::FunctionExpression, 3);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 5);
            b.begin(NodeKind::Block, 5);
            b.end(NodeKind::Block, 7);
            b.end(NodeKind::FunctionExpression, 7);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Block, 8);
        }),
    );
    let lints = f.run(&one_rule("prefer_function_declarations_over_variables"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use a function declaration rather than a variable assignment to bind a function to a name."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 1));
}

/// A non-closure initializer passes.
#[test]
fn function_variable_non_closure_passes() {
    let source = "final f = 1;";
    // final0 f1 =2 13 ;4
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Block, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "final", 0));
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.leaf(NodeKind::LiteralInt, 3);
            b.end(NodeKind::VariableInitializer, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 5);
            b.end(NodeKind::Block, 5);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );
}

/// Fields and top-level variables report only when final (:42-49).
#[test]
fn function_variable_fields_require_final() {
    let field = |keyword: &str| {
        let source = format!("class A {{ {keyword} f = () {{}}; }}");
        // class0 A1 {2 var3 f4 =5 (6 )7 {8 }9 ;10 }11
        Fixture::build(
            &source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, tok(&source, t, keyword, 0));
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.begin(NodeKind::VariableInitializer, 5);
                b.begin(NodeKind::FunctionExpression, 6);
                b.begin(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::Block, 8);
                b.end(NodeKind::Block, 10);
                b.end(NodeKind::FunctionExpression, 10);
                b.end(NodeKind::VariableInitializer, 10);
                b.end(NodeKind::InitializedIdentifier, 10);
                b.end(NodeKind::VariablesDeclaration, 11);
                b.end(NodeKind::Fields, 11);
                b.end(NodeKind::DeclarationBody, 12);
                b.end(NodeKind::ClassDeclaration, 12);
                b.end(NodeKind::TopLevelDeclaration, 12);
            }),
        )
    };
    assert_eq!(
        field("final")
            .run(&one_rule("prefer_function_declarations_over_variables"))
            .len(),
        1
    );
    assert!(
        field("var")
            .run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );

    // Top-level: final reports.
    let top = "final f = () {};";
    // final0 f1 =2 (3 )4 {5 }6 ;7
    let f = Fixture::build(
        top,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(top, t, "final", 0));
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.begin(NodeKind::FunctionExpression, 3);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 5);
            b.begin(NodeKind::Block, 5);
            b.end(NodeKind::Block, 7);
            b.end(NodeKind::FunctionExpression, 7);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::TopLevelFields, 8);
        }),
    );
    assert_eq!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .len(),
        1
    );
}

// —— type_init_formals —————————————————————————————————————————————————

/// `test/rules/type_init_formals_test.dart` `test_optionalParameters` —
/// a typed `this.x` formal reports at the type when its field exists
/// with an equal written type (the element re-check, :41-46).
#[test]
fn typed_initializing_formal_reports() {
    let source = "class C { int x; C(int this.x); }";
    // class0 C1 {2 int3 x4 ;5 C6 (7 int8 this9 .10 x11 )12 ;13 }14
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 3);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::VariablesDeclaration, 4);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 4);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.begin(NodeKind::Constructor, 6);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 7);
            b.patch(3, NONE);
            identifier(b, 6, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::FormalParameter, 8);
            b.patch(0, 9); // the `this` receiver
            b.patch(1, 11);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 8, 9);
            b.end(NodeKind::FormalParameter, 13);
            b.end(NodeKind::FormalParameters, 13);
            b.end(NodeKind::Constructor, 14);
            b.end(NodeKind::DeclarationBody, 14);
            b.end(NodeKind::ClassDeclaration, 15);
            b.end(NodeKind::TopLevelDeclaration, 15);
        }),
    );
    let lints = f.run(&one_rule("type_init_formals"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Don't needlessly type annotate initializing formals."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try removing the type.".to_owned())
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 8));
}

/// The field-formal gate (:41-46): a nonexistent field and a
/// mismatched written type are compile errors upstream — both stay
/// silent.
#[test]
fn type_init_formals_erroneous_field_faces_pass() {
    // No field `x` in the class.
    let f = Fixture::parse("class C { C(int this.x); }");
    assert!(f.run(&one_rule("type_init_formals")).is_empty());

    // The written type disagrees with the field's.
    let f = Fixture::parse("class C { num x; C(int this.x); }");
    assert!(f.run(&one_rule("type_init_formals")).is_empty());
}

/// A matching field through a wider spelling reports on a real parse;
/// a `super.x` formal keeps the lexical face (cross-file element
/// identity — the L1 inheritance arm).
#[test]
fn type_init_formals_field_and_super_faces() {
    let f = Fixture::parse("class C {\n  int x;\n  C(int this.x);\n}\n");
    let lints = f.run(&one_rule("type_init_formals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let ty = tok(&f.source, &f.tokens, "int", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, ty));

    let f = Fixture::parse(
        "class S { int y; S(this.y); }\nclass C extends S {\n  C(int super.y);\n}\n",
    );
    let lints = f.run(&one_rule("type_init_formals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let ty = tok(&f.source, &f.tokens, "int", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, ty));
}

/// An untyped initializing formal and a typed plain formal pass
/// (`node.type == null` / not an initializing formal).
#[test]
fn untyped_and_plain_formals_pass() {
    let untyped = "class C { C(this.x); }";
    // class0 C1 {2 C3 (4 this5 .6 x7 )8 ;9 }10
    let f = Fixture::build(
        untyped,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Constructor, 3);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 4);
            b.patch(3, NONE);
            identifier(b, 3, 4);
            b.begin(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::FormalParameter, 5);
            b.patch(0, 5); // the `this` receiver
            b.patch(1, 7);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.end(NodeKind::FormalParameter, 8);
            b.end(NodeKind::FormalParameters, 9);
            b.end(NodeKind::Constructor, 10);
            b.end(NodeKind::DeclarationBody, 11);
            b.end(NodeKind::ClassDeclaration, 11);
            b.end(NodeKind::TopLevelDeclaration, 11);
        }),
    );
    assert!(f.run(&one_rule("type_init_formals")).is_empty());

    let plain = "f(int x) {}";
    // f0 (1 int2 x3 )4 {5 }6
    let f = Fixture::build(
        plain,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.begin(NodeKind::FormalParameter, 2);
            b.patch(0, NONE);
            b.patch(1, 3);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 2, 3);
            b.end(NodeKind::FormalParameter, 4);
            b.end(NodeKind::FormalParameters, 5);
            b.begin(NodeKind::Block, 5);
            b.end(NodeKind::Block, 7);
            b.end(NodeKind::TopLevelMethod, 7);
        }),
    );
    assert!(f.run(&one_rule("type_init_formals")).is_empty());
}

// —— recursive_getters —————————————————————————————————————————————————

/// `test/rules/recursive_getters_test.dart` `test_recursiveGetter` — a
/// getter returning its own name reports at the reference.
#[test]
fn recursive_getter_reports_the_self_reference() {
    let source = "int get x => x;";
    // int0 get1 x2 =>3 x4 ;5
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "get", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.begin(NodeKind::ExpressionBody, 3);
            b.patch(0, 3);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            identifier(b, 4, 5);
            b.end(NodeKind::ExpressionBody, 6);
            b.end(NodeKind::TopLevelMethod, 6);
        }),
    );
    let lints = f.run(&one_rule("recursive_getters"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "The getter 'x' recursively returns itself."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 4));
}

/// A different name passes, and a property access on another target is
/// never a self-reference (`isSelfReference`, :57-64).
#[test]
fn recursive_getter_other_names_pass() {
    let source = "int get x => y;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "get", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.begin(NodeKind::ExpressionBody, 3);
            b.patch(0, 3);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            identifier(b, 4, 5);
            b.end(NodeKind::ExpressionBody, 6);
            b.end(NodeKind::TopLevelMethod, 6);
        }),
    );
    assert!(f.run(&one_rule("recursive_getters")).is_empty());
}

/// The typed getter face on a production parse — the written return
/// type's identifier row must not take the getter-name slot, or
/// `int get level => level;` stays silent.
#[test]
fn recursive_getter_typed_getter_reports_on_real_parse() {
    let source = "class C {\n  int get level => level;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("recursive_getters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The getter 'level' recursively returns itself."
    );
    // The report anchors on the body reference.
    let read = tok(&f.source, &f.tokens, "level", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, read));
}

/// An if-case pattern variable captures the then-branch read (the
/// element identity: the read resolves to the pattern variable, not the
/// getter) — and a when-guard reference to it passes too.
#[test]
fn recursive_getter_if_case_then_branch_passes() {
    let plain = concat!(
        "class C {\n",
        "  int o = 0;\n",
        "  int get level {\n",
        "    if (o case var level) {\n",
        "      return level;\n",
        "    }\n",
        "    return 0;\n",
        "  }\n",
        "}\n",
    );
    let f = Fixture::parse(plain);
    assert!(f.run(&one_rule("recursive_getters")).is_empty());

    let guarded = concat!(
        "class C {\n",
        "  int o = 0;\n",
        "  int get level {\n",
        "    if (o case var level when level > 0) {\n",
        "      return level;\n",
        "    }\n",
        "    return 0;\n",
        "  }\n",
        "}\n",
    );
    let f = Fixture::parse(guarded);
    assert!(f.run(&one_rule("recursive_getters")).is_empty());
}

/// The binding reaches the then branch only: an else-branch read and a
/// read after the if statement still name the getter and report.
#[test]
fn recursive_getter_if_case_else_and_later_reads_report() {
    let else_read = concat!(
        "class C {\n",
        "  int o = 0;\n",
        "  int get level {\n",
        "    if (o case var level) {\n",
        "      return 0;\n",
        "    } else {\n",
        "      return level;\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let f = Fixture::parse(else_read);
    let lints = f.run(&one_rule("recursive_getters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The getter 'level' recursively returns itself."
    );

    let later_read = concat!(
        "class C {\n",
        "  int o = 0;\n",
        "  int get level {\n",
        "    if (o case var level) {\n",
        "      return 0;\n",
        "    }\n",
        "    return level;\n",
        "  }\n",
        "}\n",
    );
    let f = Fixture::parse(later_read);
    assert_eq!(f.run(&one_rule("recursive_getters")).len(), 1);
}

// —— unnecessary_getters_setters ———————————————————————————————————————

/// `test/rules/unnecessary_getters_setters_test.dart` — the trivial
/// private-field wrap reports at the getter name.
#[test]
fn trivial_getter_setter_pair_reports() {
    let source = "class C { int _x = 0; int get x => _x; set x(int v) { _x = v; } }";
    // class0 C1 {2 int3 _x4 =5 06 ;7 int8 get9 x10 =>11 _x12 ;13 set14 x15 (16 int17 v18 )19 {20 _x21 =22 v23 ;24 }25 }26
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            // int _x = 0;
            b.begin(NodeKind::Fields, 3);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            // int get x => _x;
            b.begin(NodeKind::Method, 8);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "get", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 8, 9);
            identifier(b, 10, 11);
            b.begin(NodeKind::ExpressionBody, 11);
            b.patch(0, 11);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            identifier(b, 12, 13);
            b.end(NodeKind::ExpressionBody, 14);
            b.end(NodeKind::Method, 14);
            // set x(int v) { _x = v; }
            b.begin(NodeKind::Method, 14);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "set", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::NoType, 14);
            identifier(b, 15, 16);
            b.begin(NodeKind::FormalParameters, 16);
            b.begin(NodeKind::FormalParameter, 16);
            b.patch(0, NONE);
            b.patch(1, 18);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 17, 18);
            identifier(b, 18, 19);
            b.end(NodeKind::FormalParameter, 20);
            b.end(NodeKind::FormalParameters, 20);
            b.begin(NodeKind::BlockBody, 20);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 20);
            b.begin(NodeKind::ExpressionStatement, 20);
            b.begin(NodeKind::Assignment, 20);
            b.patch(0, 22);
            identifier(b, 21, 22);
            identifier(b, 23, 24);
            b.end(NodeKind::Assignment, 25);
            b.end(NodeKind::ExpressionStatement, 25);
            b.end(NodeKind::Block, 26);
            b.end(NodeKind::BlockBody, 26);
            b.end(NodeKind::Method, 26);
            b.end(NodeKind::DeclarationBody, 26);
            b.end(NodeKind::ClassDeclaration, 27);
            b.end(NodeKind::TopLevelDeclaration, 27);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_getters_setters"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Unnecessary use of getter and setter to wrap a field."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 10));
}

/// The production pair face — a typed getter/setter wrap over a private
/// field reports at the getter name (the written return type's
/// identifier row must not take the name slot).
#[test]
fn trivial_getter_setter_pair_reports_on_real_parse() {
    let source = "class C {\n  int _size = 0;\n  int get size => _size;\n  \
set size(int input) {\n    _size = input;\n  }\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("unnecessary_getters_setters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary use of getter and setter to wrap a field."
    );
    let size = tok(&f.source, &f.tokens, "size", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, size));
}

/// The write-type agreement guard (`_checkForSimpleSetter`:
/// `expression.writeType != rightHandSide.staticType` bails) — a wrap
/// whose field and parameter types disagree stays silent
/// (upstream `test_necessary_differentType` family).
#[test]
fn getter_setter_disagreeing_types_pass() {
    // `dynamic` parameter over a `String?` field.
    let f = Fixture::parse(
        "class C {\n  String? _x;\n  dynamic get x => _x;\n  set x(dynamic value) {\n    _x = value;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());

    // `num` parameter over an `int` field.
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n  int get x => _x;\n  set x(num value) {\n    _x = value;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());

    // An unwritten parameter is `dynamic` — disagrees with the `int`
    // field spelling.
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n  int get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
}

/// The initializer-inference face of the write-type agreement: an
/// unwritten field reads the inferred type (`expression.writeType` is
/// a resolved product) — `var _x = 0;` is `int`, which disagrees with
/// the `dynamic` unwritten parameter (bails) and agrees with a written
/// `int` parameter (reports); an initializer-less field is `dynamic`
/// and a `null` literal infers `dynamic`, both agreeing with the
/// unwritten parameter.
#[test]
fn getter_setter_inferred_field_literal_faces() {
    // `var _x = 0;` + unwritten parameter: int != dynamic — silent.
    let f = Fixture::parse(
        "class C {\n  var _x = 0;\n  get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());

    // Same field, written `int` parameter: int == int — reports.
    let f = Fixture::parse(
        "class C {\n  var _x = 0;\n  get x => _x;\n  set x(int value) {\n    _x = value;\n  }\n}\n",
    );
    assert_eq!(f.run(&one_rule("unnecessary_getters_setters")).len(), 1);

    // An initializer-less field is `dynamic` — reports.
    let f = Fixture::parse(
        "class C {\n  var _x;\n  get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n",
    );
    assert_eq!(f.run(&one_rule("unnecessary_getters_setters")).len(), 1);

    // A `null` literal infers `dynamic` — reports.
    let f = Fixture::parse(
        "class C {\n  var _x = null;\n  get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n",
    );
    assert_eq!(f.run(&one_rule("unnecessary_getters_setters")).len(), 1);

    // The written-parameter spelling must match exactly — `String`
    // against an `int` literal stays silent, and a nullable `int?`
    // parameter is a different type from the non-nullable inference.
    let f = Fixture::parse(
        "class C {\n  var _x = 0;\n  get x => _x;\n  set x(String value) {\n    _x = value;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());

    let f = Fixture::parse(
        "class C {\n  var _x = 0;\n  get x => _x;\n  set x(int? value) {\n    _x = value;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
}

/// The bridge faces of the inference: the initializer's resolved
/// static type answers for the fold-resolvable shapes — a
/// dynamic-returning call is `dynamic` (agrees with the unwritten
/// parameter, disagrees with `int`), an `int`-returning call the
/// reverse, and a creation compares its constructed type.
#[test]
fn getter_setter_inferred_field_bridge_faces() {
    let run = |source: &str| {
        let f = Fixture::parse(source);
        f.run_semantic(&one_rule("unnecessary_getters_setters"))
    };
    // dynamic dynFn() — `var _x = dynFn();` + unwritten parameter:
    // dynamic == dynamic — reports.
    assert_eq!(
        run("dynamic dynFn() => 0;\nclass C {\n  var _x = dynFn();\n  get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n")
            .len(),
        1
    );
    // ... and disagrees with a written `int` parameter — silent.
    assert!(run("dynamic dynFn() => 0;\nclass C {\n  var _x = dynFn();\n  get x => _x;\n  set x(int value) {\n    _x = value;\n  }\n}\n")
        .is_empty());
    // int intFn() — `var _x = intFn();` + unwritten parameter:
    // int != dynamic — silent.
    assert!(run("int intFn() => 0;\nclass C {\n  var _x = intFn();\n  get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n")
        .is_empty());
    // ... and agrees with a written `int` parameter — reports.
    assert_eq!(
        run("int intFn() => 0;\nclass C {\n  var _x = intFn();\n  get x => _x;\n  set x(int value) {\n    _x = value;\n  }\n}\n")
            .len(),
        1
    );
    // A creation: `var _x = Helper();` against `Helper` and `dynamic`
    // parameters.
    assert_eq!(
        run("class Helper {}\nclass C {\n  var _x = Helper();\n  get x => _x;\n  set x(Helper value) {\n    _x = value;\n  }\n}\n")
            .len(),
        1
    );
    assert!(run("class Helper {}\nclass C {\n  var _x = Helper();\n  get x => _x;\n  set x(value) {\n    _x = value;\n  }\n}\n")
        .is_empty());
}

/// A getter not reading the field the setter writes passes
/// (`_checkForSimpleGetter`'s identity face).
#[test]
fn getter_returning_something_else_passes() {
    let source = "class C { int _x = 0; int get x => _other; }";
    // class0 C1 {2 int3 _x4 =5 06 ;7 int8 get9 x10 =>11 _other12 ;13 }14
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 3);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.begin(NodeKind::Method, 8);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "get", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 8, 9);
            identifier(b, 10, 11);
            b.begin(NodeKind::ExpressionBody, 11);
            b.patch(0, 11);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            identifier(b, 12, 13);
            b.end(NodeKind::ExpressionBody, 14);
            b.end(NodeKind::Method, 14);
            b.end(NodeKind::DeclarationBody, 14);
            b.end(NodeKind::ClassDeclaration, 15);
            b.end(NodeKind::TopLevelDeclaration, 15);
        }),
    );
    // No setter pair at all — the getter alone never reports.
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
}

// —— unnecessary_nullable_for_final_variable_declarations ——————————————

/// A `final` declaration with a nullable type and a non-null literal
/// initializer reports at the name.
#[test]
fn unnecessary_nullable_reports_literal_initializer() {
    let source = "final int? x = 1;";
    // final0 int1 ?2 x3 =4 15 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 1, 3);
            b.begin(NodeKind::InitializedIdentifier, 3);
            identifier(b, 3, 4);
            b.begin(NodeKind::VariableInitializer, 4);
            b.leaf(NodeKind::LiteralInt, 5);
            b.end(NodeKind::VariableInitializer, 6);
            b.end(NodeKind::InitializedIdentifier, 6);
            b.end(NodeKind::VariablesDeclaration, 7);
        }),
    );
    let lints = f.run(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Type could be non-nullable.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 3));
}

/// A null initializer and a non-literal initializer pass (the
/// documented literal-subset face).
#[test]
fn unnecessary_nullable_null_and_dynamic_initializers_pass() {
    let source = "final int? x = null;";
    // final0 int1 ?2 x3 =4 null5 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 1, 3);
            b.begin(NodeKind::InitializedIdentifier, 3);
            identifier(b, 3, 4);
            b.begin(NodeKind::VariableInitializer, 4);
            b.leaf(NodeKind::LiteralNull, 5);
            b.end(NodeKind::VariableInitializer, 6);
            b.end(NodeKind::InitializedIdentifier, 6);
            b.end(NodeKind::VariablesDeclaration, 7);
        }),
    );
    assert!(
        f.run(&one_rule(
            "unnecessary_nullable_for_final_variable_declarations"
        ))
        .is_empty()
    );

    let computed = "final int? x = y;";
    let f = Fixture::build(
        computed,
        unit_of(|b, _t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 1, 3);
            b.begin(NodeKind::InitializedIdentifier, 3);
            identifier(b, 3, 4);
            b.begin(NodeKind::VariableInitializer, 4);
            identifier(b, 5, 6);
            b.end(NodeKind::VariableInitializer, 6);
            b.end(NodeKind::InitializedIdentifier, 6);
            b.end(NodeKind::VariablesDeclaration, 7);
        }),
    );
    assert!(
        f.run(&one_rule(
            "unnecessary_nullable_for_final_variable_declarations"
        ))
        .is_empty()
    );
}

/// The keyword-anchored local face on a production parse — the written
/// `int?` answers from the token run after `final`
/// (`test_nullableType_variable`).
#[test]
fn unnecessary_nullable_reports_on_real_parse() {
    let source = "void f() {\n  final int? width = 3;\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule(
        "unnecessary_nullable_for_final_variable_declarations",
    ));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Type could be non-nullable.");
    let width = tok(&f.source, &f.tokens, "width", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, width));
}

/// A non-nullable written type and an unwritten type stay silent on the
/// same production face.
#[test]
fn unnecessary_nullable_non_nullable_written_type_passes_on_real_parse() {
    let f = Fixture::parse("void f() {\n  final int width = 3;\n  final rest = 4;\n}\n");
    assert!(
        f.run(&one_rule(
            "unnecessary_nullable_for_final_variable_declarations"
        ))
        .is_empty()
    );
}

// —— prefer_initializing_formals ————————————————————————————————————————

/// `A(int x) { this.x = x; }` — the exact body form with a same-class
/// field, matching written types, and a single reference reports at the
/// assignment (`test/rules/prefer_initializing_formals_test.dart`
/// `test_declareFinal`-family positive). Production-bridge fixture over
/// the landed `Fields` `MemberKind::NonStaticField` face.
#[test]
fn initializing_formals_reports_a_this_assignment() {
    let f = Fixture::parse("class A { int x = 0; A(int x) { this.x = x; } }");
    let lints = f.run(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use an initializing formal to assign a parameter to a field."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try using an initializing formal ('this.x') to initialize the field.")
    );
    let this = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this));
    assert_eq!(lints[0].range.end, lints[0].range.start + 10);
}

/// The initializer face reports over the whole entry — the bare `x = x`
/// sugar and the explicit `this.x = x` form alike (both are the
/// official `ConstructorFieldInitializer`). Production-bridge fixture.
#[test]
fn initializing_formals_reports_an_initializer_entry() {
    let f = Fixture::parse("class A { int x = 0; A(int x) : x = x; }");
    let lints = f.run(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1);
    let entry = tok(&f.source, &f.tokens, "x", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, entry));
    assert_eq!(lints[0].range.end, lints[0].range.start + 5);

    let f = Fixture::parse("class A { int x = 0; A(int x) : this.x = x; }");
    let lints = f.run(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1);
    let this = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this));
    assert_eq!(lints[0].range.end, lints[0].range.start + 10);
}

/// A written-type mismatch (`num` field, `int` parameter — the
/// initializing formal would not compile) and an unwritten parameter
/// type (no comparable spelling, :1589-1591 of the port) pass.
/// Production-bridge fixtures.
#[test]
fn initializing_formals_type_mismatch_passes() {
    let f = Fixture::parse("class A { num x = 0; A(int x) { this.x = x; } }");
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());

    let f = Fixture::parse("class A { int x = 0; A(x) { this.x = x; } }");
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
}

/// The resolved function-shape face: the field and the formal spell
/// the same function shape differently (the formal names its
/// parameter) — the interned shapes coincide and the resolved subtype
/// gate reports where the written-lexeme fallback stayed silent.
#[test]
fn initializing_formals_function_shape_reports_over_spelling() {
    let source =
        "class A { void Function(int) cb; A(void Function(int other) cb) { this.cb = cb; } }";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // The plain run keeps the lexeme approximation (spellings differ).
    let f = Fixture::parse(source);
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
}

/// A `T?` field with a `T` formal: nullability decides (Right Top) —
/// the parameter is a subtype of the field and reports, which the
/// lexeme fallback (different spellings) suppresses.
#[test]
fn initializing_formals_nullable_field_reports_through_types() {
    let f = Fixture::parse("class A { int? x; A(int x) { this.x = x; } }");
    let lints = f.run_semantic(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// A field initialized from a differently named parameter stays silent —
/// the upstream same-name requirement (`_checkInitializer`:
/// `field.name != parameter.name` returns, the conversion to `this.p`
/// would not preserve semantics).
#[test]
fn initializing_formals_different_names_pass() {
    let f = Fixture::parse("class A { int p = 0; A(int b) : p = b; }");
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
}

/// A `super.named(...)` redirection entry is not a field initializer and
/// its argument list does not reference the parameter for the
/// conversion count — the whole entry stays out (the upstream walk
/// visits `ConstructorFieldInitializer` entries only), the corpus
/// `avoid_unused_constructor_parameters.dart` face
/// (`D(this.q, int used) : super.named(c: used);`).
#[test]
fn initializing_formals_super_redirection_passes() {
    let source = concat!(
        "class C {\n",
        "  int p;\n",
        "  C(int a);\n",
        "  C.named({int c = 0});\n",
        "}\n",
        "class D extends C {\n",
        "  int q;\n",
        "  D(this.q, int used) : super.named(c: used);\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
}

/// A second reference to the parameter keeps the conversion potentially
/// semantic — no report (:208-222, count > 1). Production-bridge
/// fixture.
#[test]
fn initializing_formals_extra_reference_passes() {
    let f = Fixture::parse("class A { int x = 0; A(int x) { this.x = x; print(x); } }");
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
}

/// A reference captured by a nested binder of the same lexeme resolves
/// to the binder, not the parameter (`_ReferenceCounter`'s element
/// identity): a shadowing block local and a closure parameter do not
/// inflate the count, and the conversion reports.
#[test]
fn initializing_formals_captured_references_do_not_count() {
    let shadowed = concat!(
        "class A {\n",
        "  int x = 0;\n",
        "  A(int x) {\n",
        "    this.x = x;\n",
        "    {\n",
        "      var x = 1;\n",
        "      print(x);\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let f = Fixture::parse(shadowed);
    let lints = f.run(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    let closure_param = concat!(
        "class A {\n",
        "  int x = 0;\n",
        "  A(int x) {\n",
        "    this.x = x;\n",
        "    g((x) => x);\n",
        "  }\n",
        "}\n",
    );
    let f = Fixture::parse(closure_param);
    assert_eq!(f.run(&one_rule("prefer_initializing_formals")).len(), 1);
}

/// Production-bridge verification of the landed `Fields`
/// `MemberKind::NonStaticField` face: the exact body form reports at the
/// assignment on a real parse, and a written-type mismatch
/// (`String` parameter for an `int` field — the `isSubtypeOf`
/// approximation, :114-118) stays silent.
#[test]
fn initializing_formals_reports_on_real_parse() {
    let f = Fixture::parse("class A { int x = 0; A(int x) { this.x = x; } }");
    let lints = f.run(&one_rule("prefer_initializing_formals"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use an initializing formal to assign a parameter to a field."
    );
    let this = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this));
    // `this.x = x` — the whole assignment through the value parameter.
    assert_eq!(lints[0].range.end, lints[0].range.start + 10);

    let f = Fixture::parse("class A { int x = 0; A(String x) { this.x = x; } }");
    assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
}

// —— batch A2 divergence pins (#466) ———————————————————————————————————

/// An identifier with a declaration context (the local/parameter name
/// faces; the read gate keys on the expression context).
fn declared_identifier(
    b: &mut dartq_ast::AstBuilder,
    context: dartq_parser::IdentifierContext,
    first: u32,
    end: u32,
) {
    b.begin(NodeKind::Identifier, first);
    b.patch(0, context as u32);
    b.end(NodeKind::Identifier, end);
}

/// `int get x { var x = 1; return x; }` — the body local captures the
/// read, so it is not the getter reference (`isSelfReference`'s element
/// identity, recursive_getters.dart:57-60).
#[test]
fn recursive_getter_shadowed_local_passes() {
    let source = "int get x { var x = 1; return x; }";
    // int0 get1 x2 {3 var4 x5 =6 17 ;8 return9 x10 ;11 }12
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "get", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.begin(NodeKind::BlockBody, 3);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 3);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 4);
            declared_identifier(
                b,
                dartq_parser::IdentifierContext::LocalVariableDeclaration,
                5,
                6,
            );
            b.begin(NodeKind::VariableInitializer, 6);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 9);
            b.begin(NodeKind::ReturnStatement, 9);
            identifier(b, 10, 11);
            b.end(NodeKind::ReturnStatement, 12);
            b.end(NodeKind::Block, 13);
            b.end(NodeKind::BlockBody, 13);
            b.end(NodeKind::TopLevelMethod, 13);
        }),
    );
    assert!(f.run(&one_rule("recursive_getters")).is_empty());
}

/// `int get x { var h = (x) { return x; }; return 1; }` — the closure
/// parameter captures the read inside the closure body.
#[test]
fn recursive_getter_shadowed_parameter_passes() {
    let source = "int get x { var h = (x) { return x; }; return 1; }";
    // int0 get1 x2 {3 var4 h5 =6 (7 x8 )9 {10 return11 x12 ;13 }14 ;15 return16 117 ;18 }19
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, tok(source, t, "get", 0));
            b.leaf_span(NodeKind::Type, 0, 1);
            identifier(b, 2, 3);
            b.begin(NodeKind::BlockBody, 3);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 3);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 4);
            declared_identifier(
                b,
                dartq_parser::IdentifierContext::LocalVariableDeclaration,
                5,
                6,
            );
            b.begin(NodeKind::VariableInitializer, 6);
            b.begin(NodeKind::FunctionExpression, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::FormalParameter, 7);
            b.patch(0, NONE);
            b.patch(1, tok(source, t, "x", 1));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            declared_identifier(
                b,
                dartq_parser::IdentifierContext::FormalParameterDeclaration,
                8,
                9,
            );
            b.end(NodeKind::FormalParameter, 10);
            b.end(NodeKind::FormalParameters, 10);
            b.begin(NodeKind::BlockBody, 10);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 10);
            b.begin(NodeKind::ReturnStatement, 11);
            identifier(b, 12, 13);
            b.end(NodeKind::ReturnStatement, 14);
            b.end(NodeKind::Block, 15);
            b.end(NodeKind::BlockBody, 15);
            b.end(NodeKind::FunctionExpression, 15);
            b.end(NodeKind::VariableInitializer, 15);
            b.end(NodeKind::InitializedIdentifier, 15);
            b.end(NodeKind::VariablesDeclaration, 16);
            b.begin(NodeKind::ReturnStatement, 16);
            b.leaf(NodeKind::LiteralInt, 17);
            b.end(NodeKind::ReturnStatement, 19);
            b.end(NodeKind::Block, 20);
            b.end(NodeKind::BlockBody, 20);
            b.end(NodeKind::TopLevelMethod, 20);
        }),
    );
    assert!(f.run(&one_rule("recursive_getters")).is_empty());
}

/// `set x(int _x) { _x = _x; }` — the assignment target resolves to the
/// setter's parameter, not the field (`_checkForSimpleSetter`'s
/// `writeElement` face), so the pair passes.
#[test]
fn getter_setter_parameter_shadow_passes() {
    let source = "class C { int _x = 0; int get x => _x; set x(int _x) { _x = _x; } }";
    // class0 C1 {2 int3 _x4 =5 06 ;7 int8 get9 x10 =>11 _x12 ;13 set14 x15 (16 int17 _x18 )19 {20 _x21 =22 _x23 ;24 }25 }26
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 3);
            b.patch(0, MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.begin(NodeKind::Method, 8);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "get", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 8, 9);
            identifier(b, 10, 11);
            b.begin(NodeKind::ExpressionBody, 11);
            b.patch(0, 11);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            identifier(b, 12, 13);
            b.end(NodeKind::ExpressionBody, 14);
            b.end(NodeKind::Method, 14);
            b.begin(NodeKind::Method, 14);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "set", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::NoType, 14);
            identifier(b, 15, 16);
            b.begin(NodeKind::FormalParameters, 16);
            b.begin(NodeKind::FormalParameter, 16);
            b.patch(0, NONE);
            b.patch(1, tok(source, t, "_x", 1));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 17, 18);
            identifier(b, 18, 19);
            b.end(NodeKind::FormalParameter, 20);
            b.end(NodeKind::FormalParameters, 20);
            b.begin(NodeKind::BlockBody, 20);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 20);
            b.begin(NodeKind::ExpressionStatement, 20);
            b.begin(NodeKind::Assignment, 21);
            b.patch(0, tok(source, t, "=", 1));
            identifier(b, 21, 22);
            identifier(b, 23, 24);
            b.end(NodeKind::Assignment, 24);
            b.end(NodeKind::ExpressionStatement, 25);
            b.end(NodeKind::Block, 26);
            b.end(NodeKind::BlockBody, 26);
            b.end(NodeKind::Method, 26);
            b.end(NodeKind::DeclarationBody, 26);
            b.end(NodeKind::ClassDeclaration, 27);
            b.end(NodeKind::TopLevelDeclaration, 27);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
}

/// A getter whose body declares a local named exactly like the (absent)
/// field never wraps a field — no report.
#[test]
fn getter_setter_body_local_passes() {
    let source = "class C { int get x { var x = 1; return x; } set x(int v) { x = v; } }";
    // class0 C1 {2 int3 get4 x5 {6 var7 x8 =9 110 ;11 return12 x13 ;14 }15 set16 x17 (18 int19 v20 )21 {22 x23 =24 v25 ;26 }27 }28
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Method, 3);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "get", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            identifier(b, 5, 6);
            b.begin(NodeKind::BlockBody, 6);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 6);
            b.begin(NodeKind::VariablesDeclaration, 6);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 7);
            declared_identifier(
                b,
                dartq_parser::IdentifierContext::LocalVariableDeclaration,
                8,
                9,
            );
            b.begin(NodeKind::VariableInitializer, 9);
            b.leaf(NodeKind::LiteralInt, 10);
            b.end(NodeKind::VariableInitializer, 11);
            b.end(NodeKind::InitializedIdentifier, 12);
            b.end(NodeKind::VariablesDeclaration, 12);
            b.begin(NodeKind::ReturnStatement, 12);
            identifier(b, 13, 14);
            b.end(NodeKind::ReturnStatement, 15);
            b.end(NodeKind::Block, 16);
            b.end(NodeKind::BlockBody, 16);
            b.end(NodeKind::Method, 16);
            b.begin(NodeKind::Method, 16);
            b.patch(0, MemberKind::NonStaticMethod as u32);
            b.patch(1, tok(source, t, "set", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.leaf(NodeKind::NoType, 16);
            identifier(b, 17, 18);
            b.begin(NodeKind::FormalParameters, 18);
            b.begin(NodeKind::FormalParameter, 18);
            b.patch(0, NONE);
            b.patch(1, tok(source, t, "v", 0));
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, 0);
            b.leaf_span(NodeKind::Type, 19, 20);
            identifier(b, 20, 21);
            b.end(NodeKind::FormalParameter, 22);
            b.end(NodeKind::FormalParameters, 22);
            b.begin(NodeKind::BlockBody, 22);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 22);
            b.begin(NodeKind::ExpressionStatement, 22);
            b.begin(NodeKind::Assignment, 23);
            b.patch(0, tok(source, t, "=", 0));
            identifier(b, 23, 24);
            identifier(b, 25, 26);
            b.end(NodeKind::Assignment, 27);
            b.end(NodeKind::ExpressionStatement, 27);
            b.end(NodeKind::Block, 28);
            b.end(NodeKind::BlockBody, 28);
            b.end(NodeKind::Method, 28);
            b.end(NodeKind::DeclarationBody, 28);
            b.end(NodeKind::ClassDeclaration, 29);
            b.end(NodeKind::TopLevelDeclaration, 29);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
}

/// The correction spells the written tail verbatim — a comment inside
/// the tail rides along in the message text.
#[test]
fn generic_function_alias_correction_keeps_comments() {
    let source = "typedef void F(/* hi */ int);";
    // typedef0 void1 F2 (3 int4 )5 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::Typedef, 0);
            b.patch(0, tok(source, t, "typedef", 0));
            b.patch(1, NONE); // old form: no `=`
            b.leaf_span(NodeKind::Type, 1, 2);
            identifier(b, 2, 3);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::Typedef, 7);
        }),
    );
    let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].correction,
        Some(
            "Try using the generic function type syntax ('void Function(/* hi */ int)')."
                .to_owned()
        )
    );
}

/// The top-level `final` face on a production parse — the storage
/// keyword reads through the shared declaration-keyword walk (the
/// payload word stores only `late`), so `final handler = (...) {...};`
/// reports at the name (:42-49).
#[test]
fn function_variable_top_level_final_reports_on_real_parse() {
    let source = "final handler = (int input) {\n  print(input);\n};\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("prefer_function_declarations_over_variables"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use a function declaration rather than a variable assignment to bind a function to a name."
    );
    let handler = tok(&f.source, &f.tokens, "handler", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, handler));
}

/// `f = null;` after the declaration is a later write — the variable is
/// potentially mutated in scope and passes (:53).
#[test]
fn function_variable_mutated_later_passes() {
    let source = "void m() { var f = () {}; f = null; }";
    // void0 m1 (2 )3 {4 var5 f6 =7 (8 )9 {10 }11 ;12 f13 =14 null15 ;16 }17
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::BlockBody, 4);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::VariablesDeclaration, 4);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 6);
            identifier(b, 6, 7);
            b.begin(NodeKind::VariableInitializer, 7);
            b.begin(NodeKind::FunctionExpression, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.end(NodeKind::FormalParameters, 10);
            b.begin(NodeKind::Block, 10);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::FunctionExpression, 12);
            b.end(NodeKind::VariableInitializer, 12);
            b.end(NodeKind::InitializedIdentifier, 12);
            b.end(NodeKind::VariablesDeclaration, 13);
            b.begin(NodeKind::ExpressionStatement, 13);
            b.begin(NodeKind::Assignment, 13);
            b.patch(0, tok(source, t, "=", 1));
            identifier(b, 13, 14);
            b.leaf(NodeKind::LiteralNull, 15);
            b.end(NodeKind::Assignment, 16);
            b.end(NodeKind::ExpressionStatement, 17);
            b.end(NodeKind::Block, 18);
            b.end(NodeKind::BlockBody, 18);
            b.end(NodeKind::TopLevelMethod, 18);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );
}

/// A write inside a nested closure is a potential mutation too.
#[test]
fn function_variable_mutated_in_nested_closure_passes() {
    let source = "void m() { var f = () {}; g(() { f = 1; }); }";
    // void0 m1 (2 )3 {4 var5 f6 =7 (8 )9 {10 }11 ;12 g13 (14 (15 )16 {17 f18 =19 120 ;21 }22 )23 ;24 }25
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::BlockBody, 4);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::VariablesDeclaration, 4);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 6);
            identifier(b, 6, 7);
            b.begin(NodeKind::VariableInitializer, 7);
            b.begin(NodeKind::FunctionExpression, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.end(NodeKind::FormalParameters, 10);
            b.begin(NodeKind::Block, 10);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::FunctionExpression, 12);
            b.end(NodeKind::VariableInitializer, 12);
            b.end(NodeKind::InitializedIdentifier, 12);
            b.end(NodeKind::VariablesDeclaration, 13);
            b.begin(NodeKind::ExpressionStatement, 13);
            b.begin(NodeKind::Send, 13);
            identifier(b, 13, 14);
            b.begin(NodeKind::Arguments, 14);
            b.begin(NodeKind::FunctionExpression, 14);
            b.begin(NodeKind::FormalParameters, 14);
            b.end(NodeKind::FormalParameters, 17);
            b.begin(NodeKind::BlockBody, 17);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 17);
            b.begin(NodeKind::ExpressionStatement, 17);
            b.begin(NodeKind::Assignment, 18);
            b.patch(0, tok(source, t, "=", 1));
            identifier(b, 18, 19);
            b.leaf(NodeKind::LiteralInt, 20);
            b.end(NodeKind::Assignment, 21);
            b.end(NodeKind::ExpressionStatement, 22);
            b.end(NodeKind::Block, 23);
            b.end(NodeKind::BlockBody, 23);
            b.end(NodeKind::FunctionExpression, 23);
            b.end(NodeKind::Arguments, 24);
            b.end(NodeKind::Send, 24);
            b.end(NodeKind::ExpressionStatement, 25);
            b.end(NodeKind::Block, 26);
            b.end(NodeKind::BlockBody, 26);
            b.end(NodeKind::TopLevelMethod, 26);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );
}

/// The `++`/`--` write faces count as potential mutations
/// (`isPotentiallyMutatedInScope`'s unqualified-write family) — a
/// variable mutated only through them stays a variable and passes.
#[test]
fn function_variable_increment_writes_pass() {
    let f = Fixture::parse("void m() { var f = () {}; f++; }");
    assert!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );

    let f = Fixture::parse("void m() { var f = () {}; --f; }");
    assert!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );
}

/// A pattern-assignment write (`(f, _) = ...`) counts as a mutation —
/// the official's assigned-variable face — and passes.
#[test]
fn function_variable_pattern_assignment_write_passes() {
    let f = Fixture::parse("void m() { var f = () {}; (f, _) = g(); }");
    assert!(
        f.run(&one_rule("prefer_function_declarations_over_variables"))
            .is_empty()
    );
}

/// A write bound to a shadowing redeclaration resolves to the inner
/// binder — the outer variable is unmutated and reports (the element
/// identity of `isPotentiallyMutatedInScope`).
#[test]
fn function_variable_shadowed_write_reports() {
    let source = "void m() { var f = () {}; { var f = 1; f = 2; } }";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("prefer_function_declarations_over_variables"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use a function declaration rather than a variable assignment to bind a function to a name."
    );
    let name = tok(&f.source, &f.tokens, "f", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// Never reassigned — the local still reports (preserved face).
#[test]
fn function_variable_unmutated_local_reports() {
    let source = "void m() { var f = () {}; g(f); }";
    // void0 m1 (2 )3 {4 var5 f6 =7 (8 )9 {10 }11 ;12 g13 (14 f15 )16 ;17 }18
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::TopLevelMethod, 0);
            b.patch(0, NONE);
            b.leaf(NodeKind::NoType, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::FormalParameters, 1);
            b.end(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::BlockBody, 4);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 4);
            b.begin(NodeKind::VariablesDeclaration, 4);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 6);
            identifier(b, 6, 7);
            b.begin(NodeKind::VariableInitializer, 7);
            b.begin(NodeKind::FunctionExpression, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.end(NodeKind::FormalParameters, 10);
            b.begin(NodeKind::Block, 10);
            b.end(NodeKind::Block, 12);
            b.end(NodeKind::FunctionExpression, 12);
            b.end(NodeKind::VariableInitializer, 12);
            b.end(NodeKind::InitializedIdentifier, 12);
            b.end(NodeKind::VariablesDeclaration, 13);
            b.begin(NodeKind::ExpressionStatement, 13);
            b.begin(NodeKind::Send, 13);
            identifier(b, 13, 14);
            b.begin(NodeKind::Arguments, 14);
            identifier(b, 15, 16);
            b.end(NodeKind::Arguments, 17);
            b.end(NodeKind::Send, 17);
            b.end(NodeKind::ExpressionStatement, 18);
            b.end(NodeKind::Block, 19);
            b.end(NodeKind::BlockBody, 19);
            b.end(NodeKind::TopLevelMethod, 19);
        }),
    );
    let lints = f.run(&one_rule("prefer_function_declarations_over_variables"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 6));
}

// ---- #586 standard-set completions (production-bridge fixtures) ----

/// `annotate_overrides` (:58-64): a member overriding without
/// `@override` reports its name. The message is the current official
/// template (`diagnostic.g.dart` `annotateOverrides` problemMessage:
/// "The member '{0}' overrides an inherited member but isn't annotated
/// with '@override'.") with the member name as `{0}`.
#[test]
fn annotate_overrides_reports_unannotated_member() {
    let f = Fixture::parse(
        "class A { int x = 0; void foo() {} }\nclass B extends A {\n  int x = 1;\n  void foo() {}\n  void own() {}\n}",
    );
    let lints = f.run(&one_rule("annotate_overrides"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'x' overrides an inherited member but isn't annotated with '@override'."
    );
    assert_eq!(
        lints[1].message,
        "The member 'foo' overrides an inherited member but isn't annotated with '@override'."
    );
}

/// `@override`-annotated members and members with no inherited
/// namesake stay silent (:42, :96-99).
#[test]
fn annotate_overrides_annotation_and_non_overrides_pass() {
    let f = Fixture::parse(
        "class A { int x = 0; void foo() {} }\nclass B extends A {\n  @override\n  int x = 1;\n  @override\n  void foo() {}\n}",
    );
    assert!(f.run(&one_rule("annotate_overrides")).is_empty());
}

/// The accessor faces: an unannotated overriding getter reports with
/// the plain name, an unannotated overriding setter reports with the
/// official `x=` argument (the overridden element's `name` — analyzer
/// setter elements carry the `=` suffix), and the operator keeps the
/// method face; the annotated member stays silent. `A`'s own
/// unannotated `==` rides the `Object` root (no written supertype).
#[test]
fn annotate_overrides_accessor_faces() {
    let f = Fixture::parse(
        "class A {\n  int get x => 0;\n  set x(int v) {}\n  bool operator ==(Object other) => true;\n}\nclass B extends A {\n  int get x => 1;\n  @override\n  set x(int v) {}\n  bool operator ==(Object other) => true;\n}",
    );
    let lints = f.run(&one_rule("annotate_overrides"));
    let messages: Vec<&str> = lints.iter().map(|lint| lint.message.as_ref()).collect();
    assert_eq!(
        messages,
        vec![
            "The member '==' overrides an inherited member but isn't annotated with '@override'.",
            "The member 'x' overrides an inherited member but isn't annotated with '@override'.",
            "The member '==' overrides an inherited member but isn't annotated with '@override'.",
        ],
        "{messages:?}"
    );
}

/// The `Object` root face: an unannotated `operator ==` in a class
/// with no written supertype overrides `Object.==` — the official
/// `getInheritedMember` resolves the implicit root (the lint-gate
/// `avoid_null_checks_in_equality_operators.dart` 12:12/17:12 faces,
/// reported at the `==` token). The `hashCode` getter answers the
/// getter namespace; a name outside the `Object` member table (`+`)
/// stays silent.
#[test]
fn annotate_overrides_object_root_reports() {
    let f = Fixture::parse(
        "class C {\n  operator ==(Object? other) => other != null;\n}\n\nclass D {\n  operator ==(Object other) => other != this;\n}\n",
    );
    let lints = f.run(&one_rule("annotate_overrides"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    let eq = tok(&f.source, &f.tokens, "==", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, eq));
    assert_eq!(lints[0].range.end, utf16_at(&f, eq) + 2);
    assert_eq!(
        lints[0].message,
        "The member '==' overrides an inherited member but isn't annotated with '@override'."
    );

    let f = Fixture::parse("class C {\n  int get hashCode => 0;\n}\n");
    let lints = f.run(&one_rule("annotate_overrides"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'hashCode' overrides an inherited member but isn't annotated with '@override'."
    );

    let f = Fixture::parse("class B {\n  B operator +(B other) => this;\n}\n");
    assert!(f.run(&one_rule("annotate_overrides")).is_empty());
}

/// The annotated operator face: `@override` on an `operator ==`
/// declaration (with or without a written return type) stays silent —
/// the members_family.dart rc2 divergence (the metadata lookup must
/// reach the operator's `MetadataStar` like any other member).
#[test]
fn annotate_overrides_annotated_operator_stays_silent() {
    let f = Fixture::parse(
        "class PairKey {\n  @override\n  bool operator ==(Object other) => other is PairKey;\n}\n",
    );
    assert!(f.run(&one_rule("annotate_overrides")).is_empty());

    let f = Fixture::parse("class C {\n  @override\n  operator ==(Object other) => true;\n}\n");
    assert!(f.run(&one_rule("annotate_overrides")).is_empty());
}

/// The unannotated setter reports with the `x=` argument (:47
/// `arguments: [member.name!]` — the setter element's name).
#[test]
fn annotate_overrides_setter_reports_equals_suffix() {
    let f = Fixture::parse(
        "class A { set x(int v) {} }\nclass B extends A {\n  set x(int v) {}\n}",
    );
    let lints = f.run(&one_rule("annotate_overrides"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'x=' overrides an inherited member but isn't annotated with '@override'."
    );
}

/// The primary-constructor parameter face (:74-93): a `var`/`final`
/// formal of a class primary constructor overriding an inherited
/// getter reports at its name with no `=` suffix; the gates hold —
/// `@override` metadata, a keywordless formal, and a non-overriding
/// name stay silent.
#[test]
fn annotate_overrides_primary_constructor_faces() {
    let f = Fixture::parse(
        "class A { int x = 0; }\nclass B(var int x) extends A;\nclass C(final int x) extends A;\nclass D(int x) extends A;\nclass E(var int z) extends A;\nclass F(@override var int x) extends A;",
    );
    let lints = f.run(&one_rule("annotate_overrides"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'x' overrides an inherited member but isn't annotated with '@override'."
    );
    assert_eq!(
        lints[1].message,
        "The member 'x' overrides an inherited member but isn't annotated with '@override'."
    );
}

/// The `lib/` gate (`rule_context.dart:15-21`) plus the wildcard and
/// doc-comment skips of `avoid_renaming_method_parameters`.
#[test]
fn avoid_renaming_reports_renamed_parameters_in_lib() {
    let mut f = Fixture::parse(
        "class A { void foo(int aa, String bb) {} }\nclass B extends A {\n  void foo(int cc, String dd) {}\n}",
    );
    f.path = "lib/a.dart".into();
    let lints = f.run(&one_rule("avoid_renaming_method_parameters"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter name 'cc' doesn't match the name 'aa' in the overridden method."
    );
    assert_eq!(
        lints[1].message,
        "The parameter name 'dd' doesn't match the name 'bb' in the overridden method."
    );
}

#[test]
fn avoid_renaming_gates() {
    let source =
        "class A { void foo(int aa) {} }\nclass B extends A {\n  void foo(int cc) {}\n}";
    // Outside `lib/` the official rule never registers its visitors
    // (`avoid_renaming_method_parameters.dart:33` —
    // `if (!context.isInLibDir) return;`).
    let mut f = Fixture::parse(source);
    f.path = "tool/gen.dart".into();
    assert!(
        f.run(&one_rule("avoid_renaming_method_parameters"))
            .is_empty()
    );

    // Matching names, wildcards, and a documented rename stay silent.
    let mut f = Fixture::parse(
        "class A { void foo(int aa, int bb, int cc) {} }\nclass B extends A {\n  /// Keep `cc` renamed for docs.\n  void foo(int aa, int _, int cc) {}\n}",
    );
    f.path = "lib/a.dart".into();
    assert!(
        f.run(&one_rule("avoid_renaming_method_parameters"))
            .is_empty()
    );
}

/// An abstract inherited method has no concrete parameter names to
/// match (`getInheritedConcreteMember` face) — no report.
#[test]
fn avoid_renaming_abstract_inherited_passes() {
    let mut f = Fixture::parse(
        "abstract class A { void foo(int aa); }\nclass B extends A {\n  void foo(int cc) {}\n}",
    );
    f.path = "lib/a.dart".into();
    assert!(
        f.run(&one_rule("avoid_renaming_method_parameters"))
            .is_empty()
    );
}

/// The `Object` root face: `operator ==` is a checked method whose
/// inherited concrete member is `Object.operator ==(Object other)` — a
/// renamed parameter reports at the parameter token (the lint-gate
/// `annotate_overrides_family.dart` 26:27 face). A subclass parameter
/// matching its superclass's name resolves through the written
/// supertype's concrete walk (the bridge, semantics-mounted — the
/// plain run has no bridge and would mis-fall to the root) and stays
/// silent.
#[test]
fn avoid_renaming_object_root_reports() {
    let mut f =
        Fixture::parse("class A {\n  @override\n  bool operator ==(Object o) => true;\n}\n");
    f.path = "lib/a.dart".into();
    let lints = f.run(&one_rule("avoid_renaming_method_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let o = tok(&f.source, &f.tokens, "o", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, o));
    assert_eq!(lints[0].range.end, utf16_at(&f, o) + 1);
    assert_eq!(
        lints[0].message,
        "The parameter name 'o' doesn't match the name 'other' in the overridden method."
    );

    let mut f = Fixture::parse(
        "class A {\n  @override\n  bool operator ==(Object o) => true;\n}\n\nclass F extends A {\n  bool operator ==(Object o) => false;\n}\n",
    );
    f.path = "lib/a.dart".into();
    let lints = f.run_semantic(&one_rule("avoid_renaming_method_parameters"));
    // `A`'s rename stays (the `Object` root); `F`'s matching parameter
    // resolves through the written supertype's concrete walk and stays
    // silent.
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.start, utf16_at(&f, o));
}

/// `overridden_fields` (:41-47): a field redeclaring an inherited
/// instance field reports.
#[test]
fn overridden_fields_reports_redeclared_field() {
    let f = Fixture::parse("class A { int x = 0; }\nclass B extends A { int x = 1; }");
    let lints = f.run(&one_rule("overridden_fields"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Field overrides a field inherited from 'A'."
    );
    // The report anchors at the redeclaring field's name token
    // (`reportAtToken(nameToken)`, :79) — name start, name length.
    let x = tok(&f.source, &f.tokens, "x", 1);
    let start = utf16_at(&f, x);
    assert_eq!(lints[0].range, start..start + 1);
}

/// Declared getters/setters are not variable-backed fields
/// (`isOriginVariable` face) and static members are not instance
/// members — neither reports.
#[test]
fn overridden_fields_setter_and_static_passes() {
    let f = Fixture::parse(
        "class A { set x(int v) {} static int s = 0; }\nclass B extends A { int x = 1; static int s = 1; }",
    );
    assert!(f.run(&one_rule("overridden_fields")).is_empty());
}

/// `unnecessary_overrides` (:52-113): a pure `super` forward reports;
/// abstract inherited methods have nothing to forward to.
#[test]
fn unnecessary_overrides_reports_bare_super_forward() {
    let f = Fixture::parse(
        "class A { void foo(int aa) {} }\nclass B extends A {\n  void foo(int aa) { super.foo(aa); }\n}",
    );
    let lints = f.run(&one_rule("unnecessary_overrides"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary override.");
}

#[test]
fn unnecessary_overrides_abstract_and_extra_work_passes() {
    // Abstract inherited: no concrete member to forward to.
    let f = Fixture::parse(
        "abstract class A { void foo(int aa); }\nclass B extends A {\n  void foo(int aa) { super.foo(aa); }\n}",
    );
    assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());

    // Two statements: the override does more than forward (:52-56).
    let f = Fixture::parse(
        "class A { void foo(int aa) {} }\nclass B extends A {\n  void foo(int aa) { super.foo(aa); aa++; }\n}",
    );
    assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());
}

/// Getter/setter forwards through every body form.
#[test]
fn unnecessary_overrides_accessors() {
    let f = Fixture::parse(
        "class A {\n  int get g => 1;\n  set s(int v) {}\n}\nclass B extends A {\n  int get g => super.g;\n  set s(int v) { super.s = v; }\n}",
    );
    let lints = f.run(&one_rule("unnecessary_overrides"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary override.");
    assert_eq!(lints[1].message, "Unnecessary override.");
}

// —— always_declare_return_types ————————————————————————————————————

/// `test/rules/always_declare_return_types_test.dart` — the untyped
/// top-level function reports with the function variants.
#[test]
fn declare_return_types_reports_an_untyped_function() {
    let f = Fixture::parse("f() => 1;");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The function 'f' should have a return type but doesn't."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding a return type to the function.")
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    assert_eq!(lints[0].range.end, lints[0].range.start + 1);
}

/// A method uses the method variants of the message.
#[test]
fn declare_return_types_reports_an_untyped_method() {
    let f = Fixture::parse("class C { m() {} }");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The method 'm' should have a return type but doesn't."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding a return type to the method.")
    );
}

/// Getters count as functions (`get p => 1;`).
#[test]
fn declare_return_types_reports_an_untyped_getter() {
    let f = Fixture::parse("get p => 1;");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "The function 'p' should have a return type but doesn't."
    );
}

/// Local functions report with the function variants; a written `void`
/// return type skips like any other type.
#[test]
fn declare_return_types_reports_a_local_function() {
    let f = Fixture::parse("void f() { g() {} }");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The function 'g' should have a return type but doesn't."
    );
}

/// Setters never report (:69-70), top-level or in-class.
#[test]
fn declare_return_types_setters_pass() {
    let f = Fixture::parse("set s(int v) {}\nclass C { set s(int v) {} }");
    assert!(f.run(&one_rule("always_declare_return_types")).is_empty());
}

/// Written return types pass, on every declaration family.
#[test]
fn declare_return_types_typed_members_pass() {
    let f = Fixture::parse("String g() => '';\nclass D { void m() {} int n() => 2; }");
    assert!(f.run(&one_rule("always_declare_return_types")).is_empty());
}

/// A typed declaration whose arrow body exposes identifier rows names
/// the declaration, never the body expression (`int get count =>
/// _count;` stayed silent at the name context) — and the old-form
/// typedef's leading return type is not its name.
#[test]
fn declare_return_types_typed_arrow_bodies_pass() {
    let f = Fixture::parse(
        "int get count => other;\nclass C { int get size => _size; }\n\
             class D { _V open() => _V(); }\ntypedef int F();\n",
    );
    assert!(f.run(&one_rule("always_declare_return_types")).is_empty());
}

/// The untyped arrow-body shape reports at the declaration name — the
/// body's leaked identifier rows must not take the report slot.
#[test]
fn declare_return_types_untyped_arrow_body_reports_at_the_name() {
    let f = Fixture::parse("class C { m() => x; }");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The method 'm' should have a return type but doesn't."
    );
    let m = tok(&f.source, &f.tokens, "m", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, m));
}

/// The old `typedef F();` form reports; the `=` alias form carries a
/// function type and skips (:57-63).
#[test]
fn declare_return_types_old_typedef_reports() {
    let f = Fixture::parse("typedef F(); typedef G = void Function();");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The function 'F' should have a return type but doesn't."
    );
}

/// Untyped `operator []=` skips (:70; the official
/// `TokenType.INDEX_EQ` guard) and a written return type passes.
#[test]
fn declare_return_types_operators_pass() {
    let f =
        Fixture::parse("class C { operator []=(int i, int v) {} int operator +(int o) => 1; }");
    assert!(f.run(&one_rule("always_declare_return_types")).is_empty());
}

/// An untyped user-definable operator reports with the method face and
/// the operator's lexeme as the name (`operator +(o) => this;` is
/// valid Dart with a `dynamic` return). Upstream Dart SDK source,
/// `pkg/linter/lib/src/rules/always_declare_return_types.dart:66-84`.
#[test]
fn declare_return_types_untyped_operator_reports() {
    let f = Fixture::parse("class C { operator +(o) => this; }");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The method '+' should have a return type but doesn't."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding a return type to the method.")
    );
    let plus = tok(&f.source, &f.tokens, "+", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, plus));
    assert_eq!(lints[0].range.end, lints[0].range.start + 1);
}

/// An untyped `operator []` reports with the method face at the
/// operator token (only setters and the `TokenType.INDEX_EQ` name
/// skip, :70), and a named method written after it is not an operator
/// itself (the immediate-adjacency probe shared with
/// `operator_name`). Upstream Dart SDK source,
/// `pkg/linter/lib/src/rules/always_declare_return_types.dart:66-84`.
#[test]
fn declare_return_types_method_after_operator_reports() {
    let f = Fixture::parse("class C { operator [](i) => this; m() {} }");
    let lints = f.run(&one_rule("always_declare_return_types"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The method '[]' should have a return type but doesn't."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding a return type to the method.")
    );
    let index = tok(&f.source, &f.tokens, "[]", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, index));
    assert_eq!(lints[0].range.end, lints[0].range.start + 2);
    assert_eq!(
        lints[1].message,
        "The method 'm' should have a return type but doesn't."
    );
    let m = tok(&f.source, &f.tokens, "m", 0);
    assert_eq!(lints[1].range.start, utf16_at(&f, m));
}

/// The `test_*` / `solo_test_*` exemption applies only inside a test
/// directory (:76-81, `isInTestDirectory`).
#[test]
fn declare_return_types_test_directory_exemption() {
    let source = "class C { test_body() {} }\nclass D { other() {} }";
    let mut f = Fixture::parse(source);
    let facts = crate::engine::WorkspaceFacts {
        own_package: "own".to_owned(),
        dependencies: ["own".to_owned()].into_iter().collect(),
        dev_dependencies: rustc_hash::FxHashSet::default(),
        root: std::path::PathBuf::from("/pkg"),
        lib_dir: std::path::PathBuf::from("/pkg/lib"),
    };
    // Outside the test directories both methods report.
    f.path = std::path::PathBuf::from("/pkg/lib/a.dart");
    let lints = f.run_workspace(&one_rule("always_declare_return_types"), &facts);
    assert_eq!(lints.len(), 2, "{lints:?}");
    // Under `test/` the `test_body` method is exempt.
    f.path = std::path::PathBuf::from("/pkg/test/a_test.dart");
    let lints = f.run_workspace(&one_rule("always_declare_return_types"), &facts);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The method 'other' should have a return type but doesn't."
    );
}

// —— always_put_required_named_parameters_first —————————————————————

/// A required named parameter after an optional one reports at the
/// parameter's name.
#[test]
fn required_named_first_reports_a_misplaced_parameter() {
    let source = "void f({int? a, required int b}) {}";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("always_put_required_named_parameters_first"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Required named parameters should be before optional named parameters."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(
            "Try moving the required named parameter to be before any optional named \
                 parameters."
        )
    );
    let b = tok(&f.source, &f.tokens, "b", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, b));
}

/// Every required parameter after the first optional reports.
#[test]
fn required_named_first_reports_each_misplaced_parameter() {
    let f = Fixture::parse("void g({int? a, required int b, required int c}) {}");
    let lints = f.run(&one_rule("always_put_required_named_parameters_first"));
    assert_eq!(lints.len(), 2, "{lints:?}");
}

/// In-order lists pass, with or without positional parameters.
#[test]
fn required_named_first_ordered_lists_pass() {
    let f = Fixture::parse(
        "void h({required int a, int? b}) {}\nvoid i(int x, {required int a, int? b}) {}",
    );
    assert!(
        f.run(&one_rule("always_put_required_named_parameters_first"))
            .is_empty()
    );
}

/// Positional parameters never arm the flag.
#[test]
fn required_named_first_positional_only_passes() {
    let f = Fixture::parse("void j(int a, [int b = 1]) {}");
    assert!(
        f.run(&one_rule("always_put_required_named_parameters_first"))
            .is_empty()
    );
}

// —— avoid_final_parameters ————————————————————————————————————————

/// `final` on a positional formal reports at the keyword. The runs opt
/// out of the shipped-default primary-constructors face, which is the
/// rule's registration gate (:56-59 — the declaring-parameter form
/// supersedes it, so at the default features the oracle emits the
/// parser's `extraneous_modifier` and no lint).
#[test]
fn final_parameters_reports_a_final_formal() {
    let source = "void f(final int x) {}";
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("avoid_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Parameters should not be marked as 'final'."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the keyword 'final'.")
    );
    let kw = tok(&f.source, &f.tokens, "final", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// Named and optional formals report too.
#[test]
fn final_parameters_reports_named_and_optional() {
    let mut f = Fixture::parse("void g({final int y = 1}) {}\nvoid h([final int z = 2]) {}");
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("avoid_final_parameters"));
    assert_eq!(lints.len(), 2, "{lints:?}");
}

/// Untyped `final` formals report as well.
#[test]
fn final_parameters_reports_untyped_formals() {
    let mut f = Fixture::parse("void l(final x) {}");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("avoid_final_parameters")).len(), 1);
}

/// Plain formals, local `final` variables, and generic function type
/// signatures pass (:63).
#[test]
fn final_parameters_valid_faces_pass() {
    let mut f = Fixture::parse(
        "void i(int x) {}\nvoid j(void Function(final int) cb) {}\n\
             void k() { final int local = 1; }",
    );
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("avoid_final_parameters")).is_empty());
}

/// At the shipped-default features the rule stays registered off — the
/// oracle at the same posture rejects `final` formals as syntax errors
/// and lints nothing.
#[test]
fn final_parameters_default_features_stay_silent() {
    let f = Fixture::parse("void f(final int x) {}\nvoid g({final int y = 1}) {}");
    assert!(f.run(&one_rule("avoid_final_parameters")).is_empty());
}

// —— avoid_classes_with_only_static_members —————————————————————————

/// A static-only class with a method reports at the class name.
#[test]
fn only_static_members_reports_a_static_class() {
    let source = "class A { static void m() {} }";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("avoid_classes_with_only_static_members"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Classes should define instance members.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding instance behavior or moving the members out of the class.")
    );
    let a = tok(&f.source, &f.tokens, "A", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
}

/// A non-const static field alone reports (:66-67); all-const members
/// and instance members pass.
#[test]
fn only_static_members_field_and_const_faces() {
    let f = Fixture::parse(
        "class B { static int two = 2; }\nclass C { static const three = 3; }\n\
             class D { int four = 4; }",
    );
    let lints = f.run(&one_rule("avoid_classes_with_only_static_members"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "B", 0))
    );
}

/// A same-file supertype carrying instance members seals a subclass
/// (:43-53).
#[test]
fn only_static_members_inherited_instance_members_seal() {
    let f = Fixture::parse(
        "class Base { int five = 5; }\nclass E extends Base { static void m() {} }",
    );
    assert!(
        f.run(&one_rule("avoid_classes_with_only_static_members"))
            .is_empty()
    );
}

/// A named constructor exempts (:57-61); the default-form constructor
/// does not; `sealed` classes skip (:40-41).
#[test]
fn only_static_members_constructor_and_sealed_faces() {
    let f = Fixture::parse(
        "class F { static void m() {} F.named(); }\n\
             class G { static void m() {} G(); }\n\
             sealed class H { static void m() {} }",
    );
    let lints = f.run(&one_rule("avoid_classes_with_only_static_members"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "G", 0))
    );
}

// —— avoid_multiple_declarations_per_line ———————————————————————————

/// A multi-variable list reports once, at the second variable's name.
#[test]
fn multiple_declarations_reports_the_second_variable() {
    let source = "var a = 1, b = 2;";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("avoid_multiple_declarations_per_line"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Multiple variables declared on a single line."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try splitting the variable declarations into multiple lines.")
    );
    let b = tok(&f.source, &f.tokens, "b", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, b));
}

/// Typed lists and field declarations report the same way.
#[test]
fn multiple_declarations_typed_and_field_faces() {
    let f = Fixture::parse("int c = 3, d = 4;\nclass A { int x = 1, y = 2; }");
    let lints = f.run(&one_rule("avoid_multiple_declarations_per_line"));
    assert_eq!(lints.len(), 2, "{lints:?}");
}

/// Single-variable lists pass; the `for` initializer list skips
/// (:38-40, the production parent is the `ForStatement` row).
#[test]
fn multiple_declarations_single_and_for_faces_pass() {
    let f = Fixture::parse(
        "var e = 1;\nvoid f() {\n  for (var i = 0, j = 1; i < 1; i++) {\n    print(i + j);\n  }\n}",
    );
    assert!(
        f.run(&one_rule("avoid_multiple_declarations_per_line"))
            .is_empty()
    );
}

/// A local multi-variable list reports.
#[test]
fn multiple_declarations_local_face() {
    let f = Fixture::parse("void f() { var a = 1, b = 2; }");
    assert_eq!(
        f.run(&one_rule("avoid_multiple_declarations_per_line"))
            .len(),
        1
    );
}

// —— avoid_private_typedef_functions (W3 #992) ————————————————————

/// The library type-use mount the driver folds
/// ([`crate::engine::type_use_counts`] per unit): one lexeme → `Type`-row count
/// over the given units.
fn type_uses<'a>(fixtures: &[&'a Fixture]) -> rustc_hash::FxHashMap<&'a str, u32> {
    let mut uses = rustc_hash::FxHashMap::default();
    for f in fixtures {
        for (lexeme, count) in crate::engine::type_use_counts(&f.source, &f.ast, &f.tokens) {
            *uses.entry(lexeme).or_insert(0) += count;
        }
    }
    uses
}

/// Runs `rules` with the library type-use face mounted — the driver's
/// `Unit::library_type_uses` seam (#1640).
fn run_with_type_uses(
    f: &Fixture,
    rules: &EnableSet,
    uses: &rustc_hash::FxHashMap<&str, u32>,
) -> Vec<crate::Lint> {
    let unit = crate::Unit {
        source: &f.source,
        tokens: &f.tokens,
        ast: &f.ast,
        lines: &f.lines,
        path: Some(&f.path),
        features: f.features,
        version: f.version,
        workspace: None,
        reachability: None,
        async_context_uses: None,
        library_type_uses: Some(uses),
    };
    crate::run(&unit, rules, &rustc_hash::FxHashSet::default())
}

/// The old-form private typedef used once reports at its name
/// (`avoid_private_typedef_functions.dart` doc example).
#[test]
fn private_typedef_old_form_used_once_reports() {
    let f = Fixture::parse("typedef int _Td();\n_Td? f;\n");
    let lints = run_with_type_uses(
        &f,
        &one_rule("avoid_private_typedef_functions"),
        &type_uses(&[&f]),
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The typedef is unnecessary because it is only used in one place."
    );
    let name = tok(&f.source, &f.tokens, "_Td", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// The new-form function alias reports; a named-type alias, the
/// generic-parameter form and the `void` alias skip (:61-63 — `void`
/// is a `NamedType` upstream).
#[test]
fn private_typedef_new_form_and_exemptions() {
    let f = Fixture::parse("typedef _P = void Function();\nvoid g(_P p) {}\n");
    assert_eq!(
        run_with_type_uses(
            &f,
            &one_rule("avoid_private_typedef_functions"),
            &type_uses(&[&f])
        )
        .len(),
        1
    );

    // The aliased type is not a function type (:62-63).
    let f = Fixture::parse("typedef _L = List<String>;\nfinal _L x = [];\n");
    assert!(
        run_with_type_uses(
            &f,
            &one_rule("avoid_private_typedef_functions"),
            &type_uses(&[&f])
        )
        .is_empty()
    );

    // Generic parameters skip (:61).
    let f = Fixture::parse("typedef _G<T> = void Function(T);\nvoid h(_G<int> g) {}\n");
    assert!(
        run_with_type_uses(
            &f,
            &one_rule("avoid_private_typedef_functions"),
            &type_uses(&[&f])
        )
        .is_empty()
    );

    // The `void` alias is a NamedType upstream (:62-63).
    let f = Fixture::parse("typedef _V = void;\n_V? n;\n");
    assert!(
        run_with_type_uses(
            &f,
            &one_rule("avoid_private_typedef_functions"),
            &type_uses(&[&f])
        )
        .is_empty()
    );
}

/// A public name and a multiply-used private name pass.
#[test]
fn private_typedef_public_and_multi_use_pass() {
    let f = Fixture::parse("typedef int Td();\nTd? f;\n");
    assert!(
        run_with_type_uses(
            &f,
            &one_rule("avoid_private_typedef_functions"),
            &type_uses(&[&f])
        )
        .is_empty()
    );

    let f = Fixture::parse("typedef int _Td();\n_Td? a;\n_Td b() => 0;\n");
    assert!(
        run_with_type_uses(
            &f,
            &one_rule("avoid_private_typedef_functions"),
            &type_uses(&[&f])
        )
        .is_empty()
    );
}

/// #1640 pin ①: the typedef declares in a part file, the defining unit
/// uses it twice — the library-wide count (2) silences the declaring
/// part's run, which the former same-file face over-reported (its own
/// rows carry no use).
#[test]
fn private_typedef_part_declared_library_wide_uses_silence() {
    let defining = Fixture::parse("_Td? a;\n_Td b() => 0;\n");
    let part = Fixture::parse("part of 'lib.dart';\ntypedef int _Td();\n");
    let uses = type_uses(&[&defining, &part]);
    let rules = one_rule("avoid_private_typedef_functions");
    assert!(run_with_type_uses(&part, &rules, &uses).is_empty());
    assert!(run_with_type_uses(&defining, &rules, &uses).is_empty());
}

/// #1640 pin ②: a private typedef used at most once across the whole
/// library fires in the run of the unit that declares it — here the
/// part: the mounted face answers the library-wide count (the one use
/// sits in the defining unit), and an absent face stays silent rather
/// than fabricating a count.
#[test]
fn private_typedef_single_library_wide_use_fires_in_part_run() {
    let defining = Fixture::parse("_Td? once;\n");
    let part = Fixture::parse("part of 'lib.dart';\ntypedef int _Td();\n");
    let rules = one_rule("avoid_private_typedef_functions");

    // One use library-wide (in the defining unit): the part run fires
    // at the declaration name.
    let lints = run_with_type_uses(&part, &rules, &type_uses(&[&defining, &part]));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&part.source, &part.tokens, "_Td", 0);
    assert_eq!(lints[0].range.start, utf16_at(&part, name));

    // Zero uses anywhere: the name misses the map and answers 0 — same
    // verdict.
    let defining = Fixture::parse("int top = 0;\n");
    let part = Fixture::parse("part of 'lib.dart';\ntypedef int _Td();\n");
    let lints = run_with_type_uses(&part, &rules, &type_uses(&[&defining, &part]));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // The unmounted face stays silent — no count is invented.
    let part = Fixture::parse("part of 'lib.dart';\ntypedef int _Td();\n");
    assert!(
        part.run(&one_rule("avoid_private_typedef_functions"))
            .is_empty()
    );
}

// —— avoid_returning_this (W3 #992) ————————————————————————————————

/// The block-body fluent method reports at its `this`
/// (`avoid_returning_this.dart` doc example).
#[test]
fn returning_this_block_body_reports() {
    let f = Fixture::parse("class A {\n  A add(int x) {\n    return this;\n  }\n}\n");
    let lints = f.run(&one_rule("avoid_returning_this"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Don't return 'this' from a method.");
    let this_tok = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this_tok));
}

/// The expression body reports at the method name (:100-104); a
/// non-`this` return silences the method (:58-62).
#[test]
fn returning_this_expression_and_silenced_faces() {
    let f = Fixture::parse("class A {\n  A m() => this;\n}\n");
    let lints = f.run(&one_rule("avoid_returning_this"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "m", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));

    let f = Fixture::parse("class A {\n  A m() {\n    return A();\n  }\n}\n");
    assert!(f.run(&one_rule("avoid_returning_this")).is_empty());
}

/// `@override`, operators, extension members and foreign return types
/// bail (:71, :77, :81-85, :91).
#[test]
fn returning_this_exemptions_pass() {
    // @override (:77).
    let f = Fixture::parse("class A {\n  @override\n  A m() => this;\n}\n");
    assert!(f.run(&one_rule("avoid_returning_this")).is_empty());
    // Operator (:71).
    let f = Fixture::parse("class A {\n  A operator +(A other) => this;\n}\n");
    assert!(f.run(&one_rule("avoid_returning_this")).is_empty());
    // Extension member (:91).
    let f = Fixture::parse("extension E on A {\n  E m() => this;\n}\n");
    assert!(f.run(&one_rule("avoid_returning_this")).is_empty());
    // The return type does not name the enclosing declaration (:81-85).
    let f = Fixture::parse("class A {\n  B m() => this;\n}\nclass B {}\n");
    assert!(f.run(&one_rule("avoid_returning_this")).is_empty());
}

// —— avoid_unused_constructor_parameters (W3 #992) —————————————————

/// The unused plain parameter reports with the name argument; the
/// used one stays silent.
#[test]
fn unused_constructor_parameter_reports() {
    let f = Fixture::parse("class A {\n  A(int a, int b) : b = 0;\n}\n");
    let lints = f.run(&one_rule("avoid_unused_constructor_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter 'a' is not used in the constructor."
    );
    // The report anchors at the parameter row (`int a`).
    let int_tok = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, int_tok));
}

/// Field/super formals, wildcards, `@Deprecated`, `external` and body
/// uses all keep the constructor silent (:50-54, :70, :100-107).
#[test]
fn unused_constructor_parameter_exemptions_pass() {
    // The field formal skips; the plain unused `y` still reports
    // (`test_fieldFormalParameter` — one exempt face in a constructor
    // never silences the rest).
    let f = Fixture::parse("class A {\n  int x;\n  A(this.x, int y);\n}\n");
    let lints = f.run(&one_rule("avoid_unused_constructor_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter 'y' is not used in the constructor."
    );

    let f = Fixture::parse("class A {\n  A(int _);\n}\n");
    assert!(
        f.run(&one_rule("avoid_unused_constructor_parameters"))
            .is_empty()
    );

    // `@Deprecated` exempts on the PARAMETER only (`test_deprecated` —
    // `C(@Deprecated('') int p)`); the constructor's own metadata never
    // silences a parameter.
    let f = Fixture::parse("class A {\n  A(@Deprecated('x') int y);\n}\n");
    assert!(
        f.run(&one_rule("avoid_unused_constructor_parameters"))
            .is_empty()
    );

    let f = Fixture::parse("class A {\n  @Deprecated('x')\n  A(int y);\n}\n");
    assert_eq!(
        f.run(&one_rule("avoid_unused_constructor_parameters"))
            .len(),
        1
    );

    let f = Fixture::parse("class A {\n  external A(int y);\n}\n");
    assert!(
        f.run(&one_rule("avoid_unused_constructor_parameters"))
            .is_empty()
    );

    let f = Fixture::parse("class A {\n  int y;\n  A(int y) {\n    this.y = y;\n  }\n}\n");
    assert!(
        f.run(&one_rule("avoid_unused_constructor_parameters"))
            .is_empty()
    );
}

/// The primary-constructor face scans the primary body and the field
/// initializers only (:81-93, :109-112); a declaring (`final`)
/// parameter declares a field and skips.
#[test]
fn unused_primary_constructor_parameter_faces() {
    let f = Fixture::parse("class A(int x, int y) {\n  this : y = 0;\n  int x;\n  int y;\n}\n");
    let lints = f.run(&one_rule("avoid_unused_constructor_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter 'x' is not used in the constructor."
    );

    let f = Fixture::parse("class A(final int x) {}\n");
    assert!(
        f.run(&one_rule("avoid_unused_constructor_parameters"))
            .is_empty()
    );
}

/// A redirecting factory skips — only the redirect target's own
/// unused parameters report (`test_redirectingConstructor1`: the
/// `factory C(int p) = C.named;` redirect itself stays silent while
/// `C.named(int p)` reports).
#[test]
fn unused_constructor_parameter_redirecting_factory_skips() {
    let f = Fixture::parse("class C {\n  C.named(int p);\n  factory C(int p) = C.named;\n}\n");
    let lints = f.run(&one_rule("avoid_unused_constructor_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter 'p' is not used in the constructor."
    );
    // The report anchors at C.named's parameter, not the factory's.
    let named_int = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, named_int));
}

// —— avoid_types_on_closure_parameters (W3 #992) ———————————————————

/// A written `FunctionType` initializer context reports each typed
/// parameter at its type; `dynamic` skips (:60-62).
#[test]
fn types_on_closure_parameters_initializer_face() {
    let f = Fixture::parse("void Function(int) f = (int x, dynamic y) {};\n");
    let lints = f.run(&one_rule("avoid_types_on_closure_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary type annotation on a function expression parameter."
    );
    let int_tok = tok(&f.source, &f.tokens, "int", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, int_tok));
}

/// The function-typed suffix form reports at the parameter (:54-57).
#[test]
fn types_on_closure_parameters_suffix_form() {
    let f = Fixture::parse("void Function(int) f = (int x, void g(int y)) => 1;\n");
    let lints = f.run(&one_rule("avoid_types_on_closure_parameters"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    let void_tok = tok(&f.source, &f.tokens, "void", 1);
    assert_eq!(lints[1].range.start, utf16_at(&f, void_tok));
}

/// An assignment to a written-`FunctionType` target reports; untyped
/// or non-function contexts never do (:494-502).
#[test]
fn types_on_closure_parameters_assignment_and_misses() {
    let f = Fixture::parse(
        "void Function(int) f = (x) => 1;\nvoid g() {\n  f = (int x) => 2;\n}\n",
    );
    let lints = f.run(&one_rule("avoid_types_on_closure_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let int_tok = tok(&f.source, &f.tokens, "int", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, int_tok));

    let f = Fixture::parse("var f = (int x) => 1;\n");
    assert!(
        f.run(&one_rule("avoid_types_on_closure_parameters"))
            .is_empty()
    );

    let f = Fixture::parse("int f = (int x) => 1;\n");
    assert!(
        f.run(&one_rule("avoid_types_on_closure_parameters"))
            .is_empty()
    );
}

/// A generic function type annotation never qualifies — the official
/// `type is NamedType` gate (:60-62) leaves `void Function() cb`
/// silent while a `NamedType` parameter reports.
#[test]
fn types_on_closure_parameters_function_type_annotation_passes() {
    let f = Fixture::parse("void Function() mk = (void Function() cb, int n) => cb;\n");
    let lints = f.run(&one_rule("avoid_types_on_closure_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let int_tok = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, int_tok));
}

/// The resolved SDK-callback face (`list.map((int e) => …)`): the
/// context type is the callee's corresponding formal — a captured
/// `Ty::Function` — so the written `int` reports. The plain run (no
/// semantics) keeps the documented under-report posture.
#[test]
fn types_on_closure_parameters_signature_face() {
    let source = "class C { void each(void Function(int) f) {} void run(C c) { c.each((int e) => e); } }\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("avoid_types_on_closure_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let int_tok = tok(&f.source, &f.tokens, "int", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, int_tok));

    let f = Fixture::parse(source);
    assert!(
        f.run(&one_rule("avoid_types_on_closure_parameters"))
            .is_empty()
    );
}

/// The resolved member-target face (`c.onValue = (int e) => e;`): the
/// target's static type is the field's captured function type.
#[test]
fn types_on_closure_parameters_member_target_face() {
    let source =
        "class C { void Function(int) onValue; void run(C c) { c.onValue = (int e) => e; } }\n";
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("avoid_types_on_closure_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let int_tok = tok(&f.source, &f.tokens, "int", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, int_tok));
}

// —— initialize_in_field_declaration (W3 #992) —————————————————————

/// A primary body initializer referencing a parameter reports at the
/// field name; a non-reference and a late field stay silent (:66-82).
#[test]
fn initialize_in_field_declaration_faces() {
    let f = Fixture::parse("class C(int x) {\n  this : y = x;\n  int y;\n}\n");
    let lints = f.run(&one_rule("initialize_in_field_declaration"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Field should be initialized in the field declaration."
    );
    let y = tok(&f.source, &f.tokens, "y", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, y));

    let f = Fixture::parse(
        "class C(int x) {\n  this : v = 0, l2 = x;\n  late int l2;\n  int v;\n}\n",
    );
    assert!(
        f.run(&one_rule("initialize_in_field_declaration"))
            .is_empty()
    );
}

/// The field must be declared in the same class (:70-75).
#[test]
fn initialize_in_field_declaration_foreign_field_passes() {
    let f = Fixture::parse("class C(int x) {\n  this : z = x;\n}\n");
    assert!(
        f.run(&one_rule("initialize_in_field_declaration"))
            .is_empty()
    );
}

/// The registration gate (:35): without primary constructors the rule
/// never registers (the active face is covered above — the shipped
/// default has the bit on).
#[test]
fn initialize_in_field_declaration_feature_gate() {
    let mut f = Fixture::parse("class C(int x) {\n  this : y = x;\n  int y;\n}\n");
    f.features.primary_constructors = false;
    assert!(
        f.run(&one_rule("initialize_in_field_declaration"))
            .is_empty()
    );
}

/// The inverted registration gate (:56-59): with primary constructors
/// (the shipped default) `avoid_final_parameters` never registers.
#[test]
fn avoid_final_parameters_feature_gate() {
    let mut f = Fixture::parse("void f(final x) {}");
    assert!(f.features.primary_constructors);
    assert!(f.run(&one_rule("avoid_final_parameters")).is_empty());
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("avoid_final_parameters")).len(), 1);
}

// —— empty_container_bodies ———————————————————————————————————————————

/// `test/rules/empty_container_bodies_test.dart` `test_class_onDifferentLines`:
/// an empty class body reports its brace pair with the kind word.
#[test]
fn empty_class_body_reports() {
    let f = Fixture::parse("class C {\n}\n");
    let lints = f.run(&one_rule("empty_container_bodies"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Empty class bodies should be written using a ';' rather than '{}'."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try replacing the class body with ';'.")
    );
    let open = tok(&f.source, &f.tokens, "{", 0);
    let close = tok(&f.source, &f.tokens, "}", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, open));
    assert_eq!(lints[0].range.end, utf16_at(&f, close) + 1);
}

/// `test_class_notEmpty`: a member row is not an empty body.
#[test]
fn non_empty_body_passes() {
    let f = Fixture::parse("class C {\n  C();\n}\n");
    assert!(f.run(&one_rule("empty_container_bodies")).is_empty());
}

/// `test_class_withComment` / `test_class_withDocComment`: any comment
/// between the braces (`precedingComments != null`) silences the rule.
#[test]
fn commented_body_passes() {
    let f = Fixture::parse("class C {\n  // eol\n}\n");
    assert!(f.run(&one_rule("empty_container_bodies")).is_empty());
    let f = Fixture::parse("class C {\n  /// doc\n}\n");
    assert!(f.run(&one_rule("empty_container_bodies")).is_empty());
}

/// `test_mixin` / `test_extension` / `test_extensionType`: the kind
/// word follows the container row.
#[test]
fn container_kind_words() {
    for (source, kind) in [
        ("mixin M {}", "mixin"),
        ("extension on String {}", "extension"),
        ("extension type E(String self) {}", "extension type"),
    ] {
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("empty_container_bodies"));
        assert_eq!(lints.len(), 1, "{source}: {lints:?}");
        assert_eq!(
            lints[0].message,
            format!("Empty {kind} bodies should be written using a ';' rather than '{{}}'.")
        );
    }
}

/// An enum body is a different face upstream (`BlockEnumBody` is not
/// registered) — the dartq `EnumBody` row kind never reaches the rule.
#[test]
fn enum_body_never_reports() {
    let f = Fixture::parse("enum E {\n  a\n}\n");
    assert!(f.run(&one_rule("empty_container_bodies")).is_empty());
}

/// The registration bail (:35): without primary constructors the rule
/// never registers.
#[test]
fn empty_container_bodies_feature_gate() {
    let mut f = Fixture::parse("class C {\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("empty_container_bodies")).is_empty());
}

// —— unnecessary_type_name_in_constructor —————————————————————————————

/// `test_generative_unnamed`: the unnamed constructor reports at the
/// type-name echo.
#[test]
fn unnamed_constructor_reports_at_type_name() {
    let f = Fixture::parse("class C {\n  C();\n}\n");
    let lints = f.run(&one_rule("unnecessary_type_name_in_constructor"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary type name in a constructor.");
    let c = tok(&f.source, &f.tokens, "C", 1); // the echo after `{`
    assert_eq!(lints[0].range.start, utf16_at(&f, c));
}

/// `test_generative_named`: the type name reports, never the name part.
#[test]
fn named_constructor_reports_at_type_name() {
    let f = Fixture::parse("class C {\n  C.name();\n}\n");
    let lints = f.run(&one_rule("unnecessary_type_name_in_constructor"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let c = tok(&f.source, &f.tokens, "C", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, c));
}

/// `test_generative_unnamed_explicitNew`: `.new` reports at the type
/// name (upstream's `typeName` branch precedes `newKeyword`).
#[test]
fn explicit_new_reports_at_type_name() {
    let f = Fixture::parse("class C {\n  C.new();\n}\n");
    let lints = f.run(&one_rule("unnecessary_type_name_in_constructor"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let c = tok(&f.source, &f.tokens, "C", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, c));
}

/// `test_factory_named`: factories report at their type name (the
/// `C._()` in the body is an instance creation — outside the face).
#[test]
fn factory_constructor_reports_at_type_name() {
    let f = Fixture::parse("class C {\n  factory C.name() => C._();\n}\n");
    let lints = f.run(&one_rule("unnecessary_type_name_in_constructor"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let c = tok(&f.source, &f.tokens, "C", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, c));
}

/// The registration bail (:36-38).
#[test]
fn unnecessary_type_name_feature_gate() {
    let mut f = Fixture::parse("class C {\n  C();\n}\n");
    f.features.primary_constructors = false;
    assert!(
        f.run(&one_rule("unnecessary_type_name_in_constructor"))
            .is_empty()
    );
}

// —— unnecessary_const_in_enum_constructor ————————————————————————————

/// The constructor face (:31-38): an explicit `const` on an enum
/// constructor reports at the keyword; a class constructor's `const`
/// is outside the face.
#[test]
fn enum_constructor_const_reports() {
    let f = Fixture::parse("enum E {\n  a;\n  const E();\n}\n");
    let lints = f.run(&one_rule("unnecessary_const_in_enum_constructor"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary 'const' keyword in an enum constructor."
    );
    let kw = tok(&f.source, &f.tokens, "const", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));

    let f = Fixture::parse("class C {\n  const C();\n}\n");
    assert!(
        f.run(&one_rule("unnecessary_const_in_enum_constructor"))
            .is_empty()
    );
}

/// An enum constructor without `const` passes.
#[test]
fn enum_constructor_without_const_passes() {
    let f = Fixture::parse("enum E {\n  a;\n  E();\n}\n");
    assert!(
        f.run(&one_rule("unnecessary_const_in_enum_constructor"))
            .is_empty()
    );
}

/// The primary-constructor face (:39-46): `const` on an enum primary
/// constructor reports at the keyword (payload word 1, bridge
/// `end_primary_constructor`).
#[test]
fn enum_primary_const_reports() {
    let f = Fixture::parse("enum const E(int x) {\n  a;\n}\n");
    let lints = f.run(&one_rule("unnecessary_const_in_enum_constructor"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let kw = tok(&f.source, &f.tokens, "const", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// The primary-constructor parent face (:39-46): a class primary
/// constructor's `const` is outside the face (hand-built — the
/// production class head recovery path is outside the lint's faces).
#[test]
fn class_primary_const_passes() {
    let source = "class const C(int x) {}";
    // class0 const1 C2 (3 int4 x5 )6 {7 }8
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ClassDeclaration, 0);
            // The class clause words (`extends`/`with`/`implements`)
            // are all absent for `class const C(int x) {}`.
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::PrimaryConstructor, 3);
            b.patch(0, dartq_parser::DeclarationKind::Class as u32);
            b.patch(1, tok(source, t, "const", 0));
            // `has_constructor_name` — `C(int x)` is unnamed.
            b.patch(2, 0);
            b.begin(NodeKind::FormalParameters, 3);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::PrimaryConstructor, 6);
            b.end(NodeKind::ClassDeclaration, 9);
        }),
    );
    assert!(
        f.run(&one_rule("unnecessary_const_in_enum_constructor"))
            .is_empty()
    );
}

/// The registration bail (:36-40).
#[test]
fn unnecessary_const_in_enum_feature_gate() {
    let mut f = Fixture::parse("enum E {\n  a;\n  const E();\n}\n");
    f.features.primary_constructors = false;
    assert!(
        f.run(&one_rule("unnecessary_const_in_enum_constructor"))
            .is_empty()
    );
}

// —— use_primary_constructors —————————————————————————————————————————

/// `test_class_withDefaultConstructor` / the empty-block shape: no
/// constructor at all reports at the container name.
#[test]
fn class_without_constructor_reports() {
    for source in ["class C;\n", "class C {\n}\n"] {
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("use_primary_constructors"));
        assert_eq!(lints.len(), 1, "{source}: {lints:?}");
        assert_eq!(lints[0].message, "Use a primary constructor.");
        let c = tok(&f.source, &f.tokens, "C", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, c));
    }
}

/// `test_class_withExternalConstructor`: an external constructor skips
/// the container (:65-68).
#[test]
fn external_constructor_passes() {
    let f = Fixture::parse("class C {\n  external C();\n}\n");
    assert!(f.run(&one_rule("use_primary_constructors")).is_empty());
}

/// `test_class_withFactory`: the root (the single non-redirecting
/// generative constructor) reports at its name part; the factory and
/// the redirect are not roots.
#[test]
fn factory_shape_reports_at_root_name() {
    let f = Fixture::parse("class C {\n  C.a();\n  factory C.b() => C.a();\n}\n");
    let lints = f.run(&one_rule("use_primary_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
}

/// `test_class_withMultipleLevelsOfRedirect`: the redirect chain's root
/// reports at its name.
#[test]
fn redirect_chain_reports_at_root_name() {
    let f =
        Fixture::parse("class C {\n  C.a();\n  C.b() : this.a();\n  C.c() : this.b();\n}\n");
    let lints = f.run(&one_rule("use_primary_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, a));
}

/// `test_class_withMultipleRoots`: two non-redirecting generative
/// constructors skip (none can be the root, :69-73).
#[test]
fn multiple_roots_pass() {
    let f = Fixture::parse("class C {\n  C.a();\n  C.b();\n}\n");
    assert!(f.run(&one_rule("use_primary_constructors")).is_empty());
}

/// `test_class_withPrimaryConstructor`: a written primary constructor
/// is already the primary form.
#[test]
fn primary_constructor_passes() {
    let f = Fixture::parse("class C();\n");
    assert!(f.run(&one_rule("use_primary_constructors")).is_empty());
}

/// `test_class_withSingleGenerativeConstructor`: an unnamed root
/// reports at the type-name echo (the `name == null` → `typeName`
/// order, :94-100).
#[test]
fn unnamed_root_reports_at_type_name() {
    let f = Fixture::parse("class C {\n  C();\n}\n");
    let lints = f.run(&one_rule("use_primary_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let c = tok(&f.source, &f.tokens, "C", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, c));
}

/// `test_enum_withDefaultConstructor`: a constructor-less enum reports
/// at its name.
#[test]
fn enum_without_constructor_reports() {
    let f = Fixture::parse("enum E {\n  a, b, c\n}\n");
    let lints = f.run(&one_rule("use_primary_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let e = tok(&f.source, &f.tokens, "E", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, e));
}

/// `test_enum_withFactory`: the unnamed generative root reports at the
/// type-name echo.
#[test]
fn enum_with_root_reports() {
    let f = Fixture::parse("enum E {\n  a, b, c;\n  E();\n  factory E.f() => b;\n}\n");
    let lints = f.run(&one_rule("use_primary_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    // Upstream `test_enum_withFactory` reports the root `E()`'s
    // type-name echo (`constructor.typeName`), the second `E`.
    let e = tok(&f.source, &f.tokens, "E", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, e));
}

/// `test_enum_withMultipleLevelsOfRedirect`: the enum redirect chain's
/// root reports at its name.
#[test]
fn enum_redirect_chain_reports_at_root_name() {
    let f = Fixture::parse(
        "enum E {\n  a.e(), b.f(), c.g();\n  E.e();\n  E.f() : this.e();\n  E.g() : this.f();\n}\n",
    );
    let lints = f.run(&one_rule("use_primary_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    // The second lowercase `e`: the first sits in the `a.e()` value.
    let e = tok(&f.source, &f.tokens, "e", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, e));
}

/// `test_enum_withPrimaryConstructor` / `test_enums_withExternalConstructor`.
#[test]
fn enum_primary_and_external_pass() {
    let f = Fixture::parse("enum E() {\n  a, b, c;\n}\n");
    assert!(f.run(&one_rule("use_primary_constructors")).is_empty());
    let f = Fixture::parse("enum E {\n  a, b, c;\n  external E();\n}\n");
    assert!(f.run(&one_rule("use_primary_constructors")).is_empty());
}

/// The registration bail (:35).
#[test]
fn use_primary_constructors_feature_gate() {
    let mut f = Fixture::parse("class C;\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("use_primary_constructors")).is_empty());
}

// —— var_with_no_type_annotation ——————————————————————————————————————

/// A `var` parameter without a type reports at the keyword (:37-66);
/// the rule registers only WITHOUT primary constructors (:34-36), so
/// the shipped default is silent.
#[test]
fn var_parameter_reports_without_primary_constructors() {
    let source = "void f(var x) {}";
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("var_with_no_type_annotation"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Avoid declaring parameters with `var` and no type annotation."
    );
    let kw = tok(&f.source, &f.tokens, "var", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// Typed, unmarked, and `final` parameters pass; function-typed
/// parameters skip (:50).
#[test]
fn var_with_no_type_valid_faces_pass() {
    let mut f = Fixture::parse("void f(int x, y, final z) {}");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("var_with_no_type_annotation")).is_empty());
    let mut f = Fixture::parse("void f(void g(int x)) {}");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("var_with_no_type_annotation")).is_empty());
}

/// Named and optional `var` formals report (:37-66 — the
/// `FormalParameterList` face covers every nesting).
#[test]
fn named_and_optional_var_report() {
    let mut f = Fixture::parse("void f({var x = 1, var y}) {}");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("var_with_no_type_annotation")).len(), 2);
    let mut f = Fixture::parse("void g([var z]) {}");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("var_with_no_type_annotation")).len(), 1);
}

/// The inverted registration bail (:34-36): with primary constructors
/// (the shipped default) the rule never registers.
#[test]
fn var_with_no_type_feature_gate() {
    let f = Fixture::parse("void f(var x) {}");
    assert!(f.features.primary_constructors);
    assert!(f.run(&one_rule("var_with_no_type_annotation")).is_empty());
}

// —— prefer_final_parameters ——————————————————————————————————————————

/// `test/rules/prefer_final_parameters_test.dart` `test_method`: an
/// unassigned method parameter reports at the whole parameter.
#[test]
fn method_param_reports() {
    let source = "class C {\n  void m(String p) {\n    print(p);\n  }\n}\n";
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The parameter 'p' should be final.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try making the parameter final.")
    );
    let ty = tok(&f.source, &f.tokens, "String", 0);
    let name = tok(&f.source, &f.tokens, "p", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, ty));
    assert_eq!(lints[0].range.end, utf16_at(&f, name) + 1);
}

/// `test_method_final`: a marked parameter passes.
#[test]
fn final_param_passes() {
    let mut f = Fixture::parse("class C {\n  void m(final String p) {}\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

/// `test_blockBody_reassigned` / `test_expressionBody_reassigned`: any
/// unqualified write in scope silences the parameter.
#[test]
fn reassigned_param_passes() {
    let mut f = Fixture::parse("class C {\n  void m(String p) {\n    p = 'x';\n  }\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
    let mut f = Fixture::parse("class C {\n  void m(int p) => p = 3;\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
    let mut f = Fixture::parse("class C {\n  void m(int p) {\n    p++;\n  }\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

/// `test_closure`: a function-expression parameter reports — and so do
/// a top-level function's: the analyzer's `FunctionDeclaration` wraps a
/// registered `FunctionExpression`, so the face covers top-level and
/// local functions (our `TopLevelMethod`/`LocalFunctionDeclaration`
/// rows; oracle 3.13.1 fires on `void f(dynamic o)`).
#[test]
fn closure_top_level_and_local_params_report() {
    let source = "var f = (Object p) {\n  print(p);\n};\n";
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let ty = tok(&f.source, &f.tokens, "Object", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, ty));

    let mut f = Fixture::parse("void g(int p) {\n  print(p);\n}\n");
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The parameter 'p' should be final.");
    let ty = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, ty));
}

/// The lint-gate wave1 under-report faces: the version-pinned legacy
/// fixture's `dynamic o` parameter (`invalid_case_patterns.dart`
/// 16:8), the `require_trailing_commas.dart` group
/// (`g(int a, String s)`, `h(bool a)` — 19:8 ×3), and the
/// `no_wildcard_variable_uses.dart` `__` parameter (22:16 — only the
/// lone `_` is the wildcard).
#[test]
fn top_level_function_faces_report() {
    let mut f = Fixture::parse("void f(dynamic o) {\n  print(o);\n}\n");
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The parameter 'o' should be final.");
    let dynamic = tok(&f.source, &f.tokens, "dynamic", 0);
    let o = tok(&f.source, &f.tokens, "o", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, dynamic));
    assert_eq!(lints[0].range.end, utf16_at(&f, o) + 1);

    let mut f = Fixture::parse(
        "void g(int a, String s) {}\nvoid h(bool a) {\n  assert(a, 'msg');\n}\n",
    );
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    let messages: Vec<&str> = lints.iter().map(|lint| lint.message.as_ref()).collect();
    assert_eq!(
        messages,
        vec![
            "The parameter 'a' should be final.",
            "The parameter 's' should be final.",
            "The parameter 'a' should be final.",
        ],
        "{messages:?}"
    );

    let mut f = Fixture::parse("void paramUser(int __) {\n  print(__);\n}\n");
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The parameter '__' should be final.");
}

/// A local function's parameters report like a top-level function's
/// (the same `FunctionDeclaration` face upstream).
#[test]
fn local_function_param_reports() {
    let mut f = Fixture::parse(
        "void outer() {\n  void inner(int p) {\n    print(p);\n  }\n  inner(1);\n}\n",
    );
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The parameter 'p' should be final.");
}

/// The `var_with_no_type_annotation.dart` faces (18:8 ×5): `var`
/// parameters and the unmarked declarator of a typed group report;
/// `final` skips.
#[test]
fn var_params_report() {
    let mut f = Fixture::parse(
        "void f(var x) {}\nvoid g({var x = 1}) {}\nvoid h([var z]) {}\nvoid ok(int a, b, final c) {}\n",
    );
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    let messages: Vec<&str> = lints.iter().map(|lint| lint.message.as_ref()).collect();
    assert_eq!(
        messages,
        vec![
            "The parameter 'x' should be final.",
            "The parameter 'x' should be final.",
            "The parameter 'z' should be final.",
            "The parameter 'a' should be final.",
            "The parameter 'b' should be final.",
        ],
        "{messages:?}"
    );
}

/// `test_constructor_usedInInitializer`: an initializer read is not a
/// write — the parameter reports.
#[test]
fn constructor_used_in_initializer_reports() {
    let source = "class C {\n  String x = '';\n  C(String x) : this.x = x;\n}\n";
    let mut f = Fixture::parse(source);
    f.features.primary_constructors = false;
    let lints = f.run(&one_rule("prefer_final_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let ty = tok(&f.source, &f.tokens, "String", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, ty));
}

/// `test_initializingFormal_positional` / `_named`: field/super
/// formals are final by definition (:67-70).
#[test]
fn initializing_formals_pass() {
    let mut f = Fixture::parse("class C {\n  String f;\n  C(this.f);\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
    let mut f = Fixture::parse("class C {\n  String f;\n  C({required this.f});\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

/// `test_method_wildcard` / `test_constructor_unused_wildcard`: a lone
/// `_` skips under the wildcard bit.
#[test]
fn wildcard_params_pass() {
    let mut f = Fixture::parse("class C {\n  void m(String _) {}\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
    let mut f = Fixture::parse("class C {\n  C(String _);\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

/// Optional and named groups report through the container face (the
/// analyzer's parameter list includes both nests).
#[test]
fn optional_and_named_params_report() {
    let mut f = Fixture::parse("class C {\n  void m({int a, int b}) {}\n}\n");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("prefer_final_parameters")).len(), 2);
    let mut f = Fixture::parse("class C {\n  void m([int a, int b]) {}\n}\n");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("prefer_final_parameters")).len(), 2);
}

/// `test_listPattern_destructured` / `test_recordPattern_destructured`:
/// an assigned variable pattern is a write (:72-73).
#[test]
fn pattern_assignment_mutates() {
    let mut f =
        Fixture::parse("class C {\n  void m(int p) {\n    [_, p, _] = [1, 2, 3];\n  }\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
    let mut f =
        Fixture::parse("class C {\n  void m(int a, int b) {\n    (a, b) = (1, 2);\n  }\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

/// A nested closure's own parameter captures the write: the write
/// resolves to the inner binder and the outer parameter stays
/// unmutated — it reports; without the rebinding the write hits the
/// outer parameter and silences it (`isPotentiallyMutatedInScope` is
/// element-faced, `scope_analyzer.dart:1094-1103`).
#[test]
fn closure_shadow_gates_the_write() {
    let mut f = Fixture::parse("class C {\n  void m(int p) {\n    (p) { p = 1; };\n  }\n}\n");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("prefer_final_parameters")).len(), 1);
    let mut f = Fixture::parse("class C {\n  void m(int p) {\n    () { p = 1; };\n  }\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

/// A `var` parameter is neither `final` nor `const` — it reports.
#[test]
fn var_keyword_still_reports() {
    let mut f = Fixture::parse("class C {\n  void m(var p) {}\n}\n");
    f.features.primary_constructors = false;
    assert_eq!(f.run(&one_rule("prefer_final_parameters")).len(), 1);
}

/// The inverted registration gate (:48-52): with primary constructors
/// (the shipped default) the rule never registers.
#[test]
fn prefer_final_parameters_feature_gate() {
    let f = Fixture::parse("class C {\n  void m(String p) {}\n}\n");
    assert!(f.features.primary_constructors);
    assert!(f.run(&one_rule("prefer_final_parameters")).is_empty());
}

// —— avoid_void_async ——————————————————————————————————————————————————

/// `test_function_async_voidReturnType_block`: a `void` `async`
/// function reports at its name.
#[test]
fn avoid_void_async_reports_a_void_async_function() {
    let f = Fixture::parse("void f() async {}\n");
    let lints = f.run(&one_rule("avoid_void_async"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "An 'async' function should have a 'Future' return type when it doesn't return a value."
    );
    let name = tok(&f.source, &f.tokens, "f", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_getter_async`: a `void` async getter reports at its name.
#[test]
fn avoid_void_async_getter_reports() {
    let f = Fixture::parse("void get l async => null;\n");
    let lints = f.run(&one_rule("avoid_void_async"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "l", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_function_async_futureVoidReturnType_block`: a `Future<void>`
/// return type passes.
#[test]
fn avoid_void_async_future_return_type_passes() {
    let f = Fixture::parse("Future<void> f() async {}\n");
    assert!(f.run(&one_rule("avoid_void_async")).is_empty());
}

/// `test_function_asyncStar`: a generator passes.
#[test]
fn avoid_void_async_generator_passes() {
    let f = Fixture::parse("Stream<void> f() async* {}\n");
    assert!(f.run(&one_rule("avoid_void_async")).is_empty());
}

/// `test_function_notAsync_voidReturnType`: a sync `void` function
/// passes.
#[test]
fn avoid_void_async_not_async_passes() {
    let f = Fixture::parse("void f() {}\n");
    assert!(f.run(&one_rule("avoid_void_async")).is_empty());
}

/// `test_main_async`: a top-level `main` is exempt; a local `main`
/// still reports (`test_functionLocal_main_async_arrow`).
#[test]
fn avoid_void_async_main_faces() {
    let f = Fixture::parse("void main() async {}\n");
    assert!(f.run(&one_rule("avoid_void_async")).is_empty());
    let f = Fixture::parse("void f() {\n  void main() async {}\n}\n");
    assert_eq!(f.run(&one_rule("avoid_void_async")).len(), 1);
}

// —— async_return_with_no_await ————————————————————————————————————————

/// `test_function`: a `Future` production returned from an `async`
/// function reports at the `return`.
#[test]
fn async_return_reports_a_future_return() {
    let f = Fixture::parse("Future<int> foo() async {\n  return Future.value(42);\n}\n");
    let lints = f.run(&one_rule("async_return_with_no_await"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Returning a 'Future' without 'await'.");
    let kw = tok(&f.source, &f.tokens, "return", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// `test_arrowFunction`: the arrow face reports at the `=>`.
#[test]
fn async_return_arrow_body_reports() {
    let f = Fixture::parse("Future<int> foo() async => Future.value(42);\n");
    let lints = f.run(&one_rule("async_return_with_no_await"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let arrow = tok(&f.source, &f.tokens, "=>", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, arrow));
}

/// `test_dynamic_returnType`: no written return type is the dynamic
/// face — a `Future` production still reports.
#[test]
fn async_return_dynamic_return_type_reports() {
    let f = Fixture::parse("foo() async {\n  return Future.value(42);\n}\n");
    assert_eq!(f.run(&one_rule("async_return_with_no_await")).len(), 1);
}

/// The await face: an awaited production is not itself a future.
#[test]
fn async_return_awaited_passes() {
    let f = Fixture::parse("Future<int> foo() async {\n  return await Future.value(42);\n}\n");
    assert!(f.run(&one_rule("async_return_with_no_await")).is_empty());
}

/// `test_notAsync` / `test_sync` / `test_wrongType`: a sync body, a
/// non-future expression, and a non-future written return type pass.
#[test]
fn async_return_non_future_faces_pass() {
    let f = Fixture::parse("Future<int> foo() {\n  return Future.value(42);\n}\n");
    assert!(f.run(&one_rule("async_return_with_no_await")).is_empty());
    let f = Fixture::parse("Future<int> foo() async {\n  return 0;\n}\n");
    assert!(f.run(&one_rule("async_return_with_no_await")).is_empty());
    let f = Fixture::parse("int foo() async {\n  return Future.value(1);\n}\n");
    assert!(f.run(&one_rule("async_return_with_no_await")).is_empty());
}

/// `test_withinTryCatch`: a return inside the try body defers to the
/// analyzer's try-block diagnostic; the catch-body return reports.
#[test]
fn async_return_try_faces() {
    let f = Fixture::parse(
        "Future<int> foo() async {\n  try {\n    return Future.value(42);\n  } catch (_) {}\n}\n",
    );
    assert!(f.run(&one_rule("async_return_with_no_await")).is_empty());
    let f = Fixture::parse(
        "Future<int> foo() async {\n  try {} catch (_) {\n    return Future.value(42);\n  }\n  return -1;\n}\n",
    );
    let lints = f.run(&one_rule("async_return_with_no_await"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let kw = tok(&f.source, &f.tokens, "return", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// A nested closure's own body answers first — an async closure with
/// a future production reports, a sync closure stays silent.
#[test]
fn async_return_closure_faces() {
    let f = Fixture::parse(
        "void f() {\n  Future<int> g() async {\n    return Future.value(1);\n  }\n}\n",
    );
    assert_eq!(f.run(&one_rule("async_return_with_no_await")).len(), 1);
    let f = Fixture::parse(
        "Future<int> f() async {\n  g() {\n    return Future.value(1);\n  }\n  return Future.value(2);\n}\n",
    );
    let lints = f.run(&one_rule("async_return_with_no_await"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

// —— prefer_asserts_in_initializer_lists ———————————————————————————————

/// `test_assertContainsFieldFormalParameter`: a leading assert over
/// the constructor's parameter reports at the `assert` keyword.
#[test]
fn prefer_asserts_reports_a_leading_assert() {
    let f = Fixture::parse(
        "class C {\n  int? f;\n  C({this.f}) {\n    assert(f != null);\n  }\n}\n",
    );
    let lints = f.run(&one_rule("prefer_asserts_in_initializer_lists"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Assert should be in the initializer list."
    );
    let kw = tok(&f.source, &f.tokens, "assert", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

/// `test_assertContainsInstanceMethod` / the `this` face: instance
/// access silences the assert.
#[test]
fn prefer_asserts_instance_access_passes() {
    let f =
        Fixture::parse("class C {\n  C() {\n    assert(m());\n  }\n  bool m() => true;\n}\n");
    assert!(
        f.run(&one_rule("prefer_asserts_in_initializer_lists"))
            .is_empty()
    );
    let f = Fixture::parse(
        "class C {\n  int? m() => null;\n  C() {\n    assert(this.m() != null);\n  }\n}\n",
    );
    assert!(
        f.run(&one_rule("prefer_asserts_in_initializer_lists"))
            .is_empty()
    );
}

/// `test_afterFirstStatement`: only leading asserts report.
#[test]
fn prefer_asserts_after_first_statement_passes() {
    let f = Fixture::parse(
        "class C {\n  C.named(a) {\n    print('');\n    assert(a != null);\n  }\n}\n",
    );
    assert!(
        f.run(&one_rule("prefer_asserts_in_initializer_lists"))
            .is_empty()
    );
}

/// A factory constructor never reports (:149).
#[test]
fn prefer_asserts_factory_passes() {
    let f = Fixture::parse(
        "class C {\n  factory C(a) {\n    assert(a != null);\n    return C._();\n  }\n  C._();\n}\n",
    );
    assert!(
        f.run(&one_rule("prefer_asserts_in_initializer_lists"))
            .is_empty()
    );
}

/// The primary-constructor-body face (:144-146): the paired primary's
/// parameters answer the fold.
#[test]
fn prefer_asserts_primary_body_reports() {
    let f = Fixture::parse("class C(int x) {\n  this {\n    assert(x != null);\n  }\n}\n");
    let lints = f.run(&one_rule("prefer_asserts_in_initializer_lists"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let kw = tok(&f.source, &f.tokens, "assert", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, kw));
}

// —— test_types_in_equals ——————————————————————————————————————————————

/// `test_doesNotUseIs`: a cast of the `==` parameter reports at the
/// cast with the owner's name.
#[test]
fn test_types_in_equals_reports_a_cast() {
    let f = Fixture::parse(
        "class C {\n  bool operator ==(Object other) {\n    C otherC = other as C;\n    return true;\n  }\n}\n",
    );
    let lints = f.run(&one_rule("test_types_in_equals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Missing type test for 'C' in '=='.");
    assert_eq!(
        lints[0].correction,
        Some("Try testing the type of 'C'.".to_owned())
    );
    let cast = tok(&f.source, &f.tokens, "other", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, cast));
}

/// A cast of anything but the parameter passes; a non-`==` operator
/// and a top-level function are out of face.
#[test]
fn test_types_in_equals_negative_faces() {
    let f = Fixture::parse(
        "class C {\n  bool operator ==(Object other) {\n    C mine = this as C;\n    return true;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("test_types_in_equals")).is_empty());
    let f = Fixture::parse(
        "class C {\n  bool same(Object other) {\n    C mine = other as C;\n    return true;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("test_types_in_equals")).is_empty());
    let f = Fixture::parse("void f(Object other) {\n  var b = other as bool;\n}\n");
    assert!(f.run(&one_rule("test_types_in_equals")).is_empty());
}

// —— avoid_positional_boolean_parameters ———————————————————————————————

/// A second positional `bool` parameter reports at the parameter.
#[test]
fn avoid_positional_bool_reports_a_function() {
    let f = Fixture::parse("void f(bool p1, bool p2) {}\n");
    let lints = f.run(&one_rule("avoid_positional_boolean_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'bool' parameters should be named parameters."
    );
    let param = tok(&f.source, &f.tokens, "bool", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, param));
}

/// A lone positional `bool`, named booleans, and a third bool never
/// report twice (`reportAtNode(second)`).
#[test]
fn avoid_positional_bool_counts_pass() {
    let f = Fixture::parse("void f(bool b) {}\n");
    assert!(
        f.run(&one_rule("avoid_positional_boolean_parameters"))
            .is_empty()
    );
    let f = Fixture::parse("void f({bool p1, bool p2}) {}\n");
    assert!(
        f.run(&one_rule("avoid_positional_boolean_parameters"))
            .is_empty()
    );
    let f = Fixture::parse("void f(bool p1, bool p2, bool p3) {}\n");
    assert_eq!(
        f.run(&one_rule("avoid_positional_boolean_parameters"))
            .len(),
        1
    );
}

/// Private declarations skip (:51/:62/:93) — the function name and
/// the constructor's name part.
#[test]
fn avoid_positional_bool_private_passes() {
    let f = Fixture::parse("void _f(bool p1, bool p2) {}\n");
    assert!(
        f.run(&one_rule("avoid_positional_boolean_parameters"))
            .is_empty()
    );
    let f = Fixture::parse("class C {\n  C._named(bool p1, bool p2);\n}\n");
    assert!(
        f.run(&one_rule("avoid_positional_boolean_parameters"))
            .is_empty()
    );
}

/// The fixed-signature face: a same-file override skips (:79 + the
/// extensions) while the superclass declaration reports.
#[test]
fn avoid_positional_bool_override_passes() {
    let f = Fixture::parse(
        "class A {\n  void m(bool p1, bool p2) {}\n}\n\nclass B extends A {\n  void m(bool p1, bool p2) {}\n}\n",
    );
    let lints = f.run(&one_rule("avoid_positional_boolean_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let first = tok(&f.source, &f.tokens, "bool", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, first));
}

/// The primary-constructor face (:88-96): the second positional bool
/// reports (the storage keyword does not matter).
#[test]
fn avoid_positional_bool_primary_constructor_reports() {
    let f = Fixture::parse("class C(bool p1, final bool p2) {}\n");
    let lints = f.run(&one_rule("avoid_positional_boolean_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let param = tok(&f.source, &f.tokens, "final", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, param));
}

/// The generic-function-type face (:68-72): a `void Function(...)`
/// parameter's second positional bool reports; the closure parameter
/// lists never reach the rule (`test_anonymousFunction`).
#[test]
fn avoid_positional_bool_generic_function_type_faces() {
    let f =
        Fixture::parse("class C {\n  void m(void Function(bool p1, bool p2) callback) {}\n}\n");
    let lints = f.run(&one_rule("avoid_positional_boolean_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let p2 = tok(&f.source, &f.tokens, "bool", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, p2));
    let f = Fixture::parse("void f(List<bool> list) {\n  list.where((bool e) => e);\n}\n");
    assert!(
        f.run(&one_rule("avoid_positional_boolean_parameters"))
            .is_empty()
    );
}

/// `test_constructor_fieldFormalParameter_positional`: a field formal
/// reads the same-class field's written `bool`.
#[test]
fn avoid_positional_bool_field_formal_reports() {
    let f = Fixture::parse("class C {\n  bool p2;\n  C.named(bool p1, this.p2);\n}\n");
    let lints = f.run(&one_rule("avoid_positional_boolean_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let param = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, param));
}

// —— avoid_field_initializers_in_const_classes —————————————————————————

/// `test_constClass_fieldInitiailizer`: an initialized final field in
/// a const class reports at the variable.
#[test]
fn avoid_field_initializers_const_class_field_reports() {
    let f = Fixture::parse("class C {\n  final a = 1;\n  const C();\n}\n");
    let lints = f.run(&one_rule("avoid_field_initializers_in_const_classes"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Fields in 'const' classes should not have initializers."
    );
    let var = tok(&f.source, &f.tokens, "a", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, var));
}

/// A non-const class (:91-99) and a mixin (:72/:93) never report.
#[test]
fn avoid_field_initializers_non_const_owners_pass() {
    let f = Fixture::parse("class C {\n  final a = 1;\n  C();\n}\n");
    assert!(
        f.run(&one_rule("avoid_field_initializers_in_const_classes"))
            .is_empty()
    );
    let f = Fixture::parse("mixin M {\n  final a = 1;\n}\n");
    assert!(
        f.run(&one_rule("avoid_field_initializers_in_const_classes"))
            .is_empty()
    );
}

/// `test_constClass_constructorInitializer`: a const constructor's
/// field initializer reports at the initializer.
#[test]
fn avoid_field_initializers_constructor_initializer_reports() {
    let f = Fixture::parse("class C {\n  final a;\n  const C() : a = 1;\n}\n");
    let lints = f.run(&one_rule("avoid_field_initializers_in_const_classes"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let init = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, init));
}

/// `test_constClass_constructorInitializer_usingParameter`: a
/// parameter reference in the initializer expression skips.
#[test]
fn avoid_field_initializers_parameter_reference_passes() {
    let f = Fixture::parse("class C {\n  final a;\n  const C(b) : a = b;\n}\n");
    assert!(
        f.run(&one_rule("avoid_field_initializers_in_const_classes"))
            .is_empty()
    );
}

/// `test_constClass_multipleConstructors`: several constructors skip
/// the initializer face (:75).
#[test]
fn avoid_field_initializers_multiple_constructors_pass() {
    let f = Fixture::parse(
        "class C {\n  final a;\n  const C.c1() : a = 1;\n  const C.c2() : a = 2;\n}\n",
    );
    assert!(
        f.run(&one_rule("avoid_field_initializers_in_const_classes"))
            .is_empty()
    );
}

/// `test_constClass_fieldInitializer_primary`: a primary parameter is
/// in scope in a field initializer (:100-115) — the referencing
/// variable passes, a non-referencing one reports.
#[test]
fn avoid_field_initializers_primary_param_faces() {
    let f = Fixture::parse("class const C(int x) {\n  final int y = x + 2;\n}\n");
    assert!(
        f.run(&one_rule("avoid_field_initializers_in_const_classes"))
            .is_empty()
    );
    let f = Fixture::parse("class const C(int x) {\n  final y = x + 2, z = 0;\n}\n");
    let lints = f.run(&one_rule("avoid_field_initializers_in_const_classes"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let z = tok(&f.source, &f.tokens, "z", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, z));
}

/// `test_constClass_constructorInitializer_primary`: the primary's
/// const keyword (payload word 1) drives the initializer face.
#[test]
fn avoid_field_initializers_primary_body_initializer_reports() {
    let f = Fixture::parse("class const C(int a) {\n  final a;\n  this : this.a = 0;\n}\n");
    let lints = f.run(&one_rule("avoid_field_initializers_in_const_classes"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    // `reportAtNode(node)` — the field initializer's `this`, the
    // second `this` in the source (the first is the body keyword).
    let init = tok(&f.source, &f.tokens, "this", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, init));
}

// —— unnecessary_primary_constructor_body ——————————————————————————————

/// `test_emptyBody_semicolon` / `test_emptyBody_block`: an empty
/// primary constructor body reports at the `this`.
#[test]
fn unnecessary_primary_body_empty_reports() {
    let f = Fixture::parse("class C() {\n  this;\n}\n");
    let lints = f.run(&one_rule("unnecessary_primary_constructor_body"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary primary constructor body.");
    let this = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this));

    let f = Fixture::parse("class C() {\n  this {}\n}\n");
    let lints = f.run(&one_rule("unnecessary_primary_constructor_body"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let this = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this));
}

/// `test_hasDocComment` / `test_hasMetadata` / `test_hasInitializer` /
/// `test_hasNonEmptyBody`: every exemption keeps the rule silent.
#[test]
fn unnecessary_primary_body_exemptions_pass() {
    let f = Fixture::parse("class C() {\n  /// comment\n  this;\n}\n");
    assert!(
        f.run(&one_rule("unnecessary_primary_constructor_body"))
            .is_empty()
    );
    let f = Fixture::parse("class C() {\n  @deprecated\n  this;\n}\n");
    assert!(
        f.run(&one_rule("unnecessary_primary_constructor_body"))
            .is_empty()
    );
    let f = Fixture::parse("class C(int i) {\n  this : assert(i >= 0);\n}\n");
    assert!(
        f.run(&one_rule("unnecessary_primary_constructor_body"))
            .is_empty()
    );
    let f = Fixture::parse("class C(int i) {\n  this {\n    print(i);\n  }\n}\n");
    assert!(
        f.run(&one_rule("unnecessary_primary_constructor_body"))
            .is_empty()
    );
}

// —— use_declaring_parameters ——————————————————————————————————————————

/// `test_fieldFormalParameter_noType`: an untyped field formal
/// reports at its name.
#[test]
fn use_declaring_parameters_field_formal_reports() {
    let f = Fixture::parse("class C(this.i) {\n  int i;\n}\n");
    let lints = f.run(&one_rule("use_declaring_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Use a declaring parameter.");
    let name = tok(&f.source, &f.tokens, "i", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_fieldFormalParameter_sameType` /
/// `test_fieldFormalParameter_differentType`: the written spellings
/// decide.
#[test]
fn use_declaring_parameters_field_formal_type_faces() {
    let f = Fixture::parse("class C(int this.i) {\n  int i;\n}\n");
    assert_eq!(f.run(&one_rule("use_declaring_parameters")).len(), 1);
    let f = Fixture::parse("class C(int this.i) {\n  num i;\n}\n");
    assert!(f.run(&one_rule("use_declaring_parameters")).is_empty());
}

/// `test_finalPrivateField_requiredPositional`: a body initializer
/// assigning the parameter to a same-named (modulo underscore) field
/// reports at the parameter name.
#[test]
fn use_declaring_parameters_initializer_assigns_reports() {
    let f = Fixture::parse("class C(int i) {\n  final int _i;\n  this : _i = i;\n}\n");
    let lints = f.run(&one_rule("use_declaring_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "i", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_assignedInBody_requiredPositional`: a body-block assignment
/// reports at the parameter name.
#[test]
fn use_declaring_parameters_body_assigns_reports() {
    let f =
        Fixture::parse("class C(int i) {\n  int i = 0;\n  this {\n    this.i = i;\n  }\n}\n");
    let lints = f.run(&one_rule("use_declaring_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "i", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_differentName_requiredPositional` /
/// `test_differentType_requiredPositional`: a foreign field name and a
/// non-bare assigning expression skip.
#[test]
fn use_declaring_parameters_non_declaring_passes() {
    let f = Fixture::parse("class C(int i) {\n  int x;\n  this : x = i;\n}\n");
    assert!(f.run(&one_rule("use_declaring_parameters")).is_empty());
    let f = Fixture::parse("class C(int? i) {\n  int x;\n  this : x = i ?? 0;\n}\n");
    assert!(f.run(&one_rule("use_declaring_parameters")).is_empty());
}

/// `test_notAssigned_optionalNamed`: a body-less primary has no
/// declaring candidates; storage-keyword parameters skip (:49).
#[test]
fn use_declaring_parameters_no_body_and_storage_pass() {
    let f = Fixture::parse("class C({int i = 0});\n");
    assert!(f.run(&one_rule("use_declaring_parameters")).is_empty());
    let f = Fixture::parse("class C(final int i) {\n  final int i2;\n  this : i2 = i;\n}\n");
    assert!(f.run(&one_rule("use_declaring_parameters")).is_empty());
}

/// The registration gate (:35): without primary constructors the rule
/// never registers.
#[test]
fn use_declaring_parameters_feature_gate() {
    let mut f = Fixture::parse("class C(this.i) {\n  int i;\n}\n");
    f.features.primary_constructors = false;
    assert!(f.run(&one_rule("use_declaring_parameters")).is_empty());
}

/// The un-annotated class's `==` reports at the member's first token
/// (`test_classEqaHashCode` face); an `@immutable` class and a mixin
/// member (not a `ClassDeclaration` ancestor) pass.
#[test]
fn avoid_equals_and_hash_code_on_mutable_classes_gate() {
    let f = Fixture::parse("class C {\n  bool operator ==(Object other) => true;\n}\n");
    let lints = f.run(&one_rule("avoid_equals_and_hash_code_on_mutable_classes"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let first = tok(&f.source, &f.tokens, "bool", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, first));
    assert_eq!(
        lints[0].message,
        "The method '==' should not be overridden in classes not annotated with '@immutable'."
    );
    let f = Fixture::parse("@immutable\nclass C {\n  int get hashCode => 0;\n}\n");
    assert!(
        f.run(&one_rule("avoid_equals_and_hash_code_on_mutable_classes"))
            .is_empty()
    );
    let f = Fixture::parse("mixin M {\n  int get hashCode => 0;\n}\n");
    assert!(
        f.run(&one_rule("avoid_equals_and_hash_code_on_mutable_classes"))
            .is_empty()
    );
}

/// `test_class_setter_noGetter` reports at the setter name; the
/// declared-getter pair passes (`test_class_setter_andGetter`).
#[test]
fn avoid_setters_without_getters_gate() {
    let f = Fixture::parse("class A {\n  set x(int value) {}\n}\n");
    let lints = f.run(&one_rule("avoid_setters_without_getters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
    // Message: upstream Dart SDK source,
    // pkg/linter/lib/src/rules/avoid_setters_without_getters.dart.
    assert_eq!(lints[0].message, "Setter has no corresponding getter.");
    let f = Fixture::parse("class A {\n  int get x => 0;\n\n  set x(int value) {}\n}\n");
    assert!(f.run(&one_rule("avoid_setters_without_getters")).is_empty());
}

/// `test_assignToField`-shape: a `void` one-parameter method whose body
/// assigns the parameter to a same-class field reports at the name;
/// a compound assignment and a shadowed target pass.
#[test]
fn use_setters_to_change_properties_gate() {
    let f = Fixture::parse(
        "class A {\n  int x = 0;\n\n  void setX(int value) {\n    x = value;\n  }\n}\n",
    );
    let lints = f.run(&one_rule("use_setters_to_change_properties"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "setX", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
    // Message: upstream Dart SDK source,
    // pkg/linter/lib/src/rules/use_setters_to_change_properties.dart.
    assert_eq!(lints[0].message, "The method is used to change a property.");
    let f = Fixture::parse(
        "class A {\n  int x = 0;\n\n  void bump(int value) {\n    x += value;\n  }\n}\n",
    );
    assert!(
        f.run(&one_rule("use_setters_to_change_properties"))
            .is_empty()
    );
    let f = Fixture::parse(
        "class A {\n  int x = 0;\n\n  void setLocal(int x) {\n    x = 1;\n  }\n}\n",
    );
    assert!(
        f.run(&one_rule("use_setters_to_change_properties"))
            .is_empty()
    );
}

/// `test_returnThis`-shape: a zero-parameter non-void method returning
/// `Foo(this)` reports at the name; the `to`-prefixed and `void`
/// shapes pass.
#[test]
fn use_to_and_as_if_applicable_gate() {
    let f = Fixture::parse("class A {\n  A wrap() => A(this);\n}\n");
    let lints = f.run(&one_rule("use_to_and_as_if_applicable"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "wrap", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
    // Message: upstream Dart SDK source,
    // pkg/linter/lib/src/rules/use_to_and_as_if_applicable.dart.
    assert_eq!(
        lints[0].message,
        "Start the name of the method with 'to' or 'as'."
    );
    let f = Fixture::parse("class A {\n  A toA() => A(this);\n\n  void none() {}\n}\n");
    assert!(f.run(&one_rule("use_to_and_as_if_applicable")).is_empty());
}

// —— member-name hazard pins (#1791) ——————————————————————————————————
//
// The production tiling places a written return type's `Identifier`
// (a `TypeReference` leaf) INSIDE the member window BEFORE the name
// row, so every name read must go through the context-stamped
// `Identifier` (`member_name` / `member_declaration_name`). These tests
// pin the report anchor or message argument at the member NAME, never
// the return type — a regression to a first-`Identifier` read shifts
// the assertion onto the type token.

/// Named return types: `int`/`String`/`Future<int>`/`void` methods
/// report at the name token, not the type. `avoid_void_async` reports
/// at the name only for `void`; the `use_to_and_as_if_applicable` face
/// reports at the name for a non-`void` typed method.
#[test]
fn member_name_named_return_types_anchor_at_the_name() {
    for source in [
        "class A {\n  int wrap() => A(this);\n}\n",
        "class A {\n  String wrap() => A(this);\n}\n",
        "class A {\n  Future<int> wrap() => A(this);\n}\n",
    ] {
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("use_to_and_as_if_applicable"));
        assert_eq!(lints.len(), 1, "{source}: {lints:?}");
        let name = tok(&f.source, &f.tokens, "wrap", 0);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, name),
            "{source}: report must anchor at the member name"
        );
    }
    // `void` anchors the same way through `avoid_void_async`.
    let f = Fixture::parse("void f() async {}\n");
    let lints = f.run(&one_rule("avoid_void_async"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "f", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// A bare-name (untyped) method answers the name — `foo()` with no
/// return type still reports at `foo`.
#[test]
fn member_name_untyped_method() {
    let f = Fixture::parse("class A {\n  wrap() => A(this);\n}\n");
    let lints = f.run(&one_rule("use_to_and_as_if_applicable"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "wrap", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// An operator's `OperatorName` rides the member's preceding sibling on
/// the production tiling (not a direct child) — `hash_and_equals`
/// still finds `==` through the adjacent-sibling arm.
#[test]
fn member_name_operator_adjacent_sibling() {
    let f = Fixture::parse("class A {\n  bool operator ==(o) => true;\n}\n");
    let lints = f.run(&one_rule("hash_and_equals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Missing a corresponding override of 'hashCode'."
    );
    let op = tok(&f.source, &f.tokens, "==", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, op));
}

/// A local function's name nests under `FunctionName` —
/// `member_declaration_name` reaches it (`avoid_void_async` reports at
/// the local function's name).
#[test]
fn member_declaration_name_local_function() {
    let f = Fixture::parse("void f() {\n  void g() async {}\n}\n");
    let lints = f.run(&one_rule("avoid_void_async"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "g", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// A function-typed formal parameter's name is the row's own span
/// head on the production tiling (no name row exists) — the `{0}`
/// argument and the report span both answer `g`, never the return
/// type.
#[test]
fn function_typed_formal_parameter_name() {
    let f = Fixture::parse("void f(int g()) {}\n");
    let lints = f.run(&one_rule("use_function_type_syntax_for_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use the generic function type syntax to declare the parameter 'g'."
    );
    let name = tok(&f.source, &f.tokens, "g", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// The constructor face (`test_classLevelAnnotation`-shape): a
/// constructor of a `@deprecated` class reports at the name echo; a
/// deprecated constructor passes. The parameter face: a `this.x`
/// formal of a deprecated field reports at the parameter, the inverse
/// at the field name.
#[test]
fn deprecated_consistency_gate() {
    let f = Fixture::parse("@deprecated\nclass C {\n  C();\n}\n");
    let lints = f.run(&one_rule("deprecated_consistency"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let name = tok(&f.source, &f.tokens, "C", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
    assert_eq!(
        lints[0].message,
        "Constructors in a deprecated class should be deprecated."
    );
    let f = Fixture::parse("@deprecated\nclass C {\n  @Deprecated('x')\n  C();\n}\n");
    assert!(f.run(&one_rule("deprecated_consistency")).is_empty());
    let f = Fixture::parse("class C {\n  @deprecated\n  int x = 0;\n\n  C(this.x);\n}\n");
    let lints = f.run(&one_rule("deprecated_consistency"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let receiver = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, receiver));
    let f = Fixture::parse("class C {\n  int x = 0;\n\n  C(@deprecated this.x);\n}\n");
    let lints = f.run(&one_rule("deprecated_consistency"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let field = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, field));
}

/// The `void` and `Future` faces: an awaitless `async void` reports at
/// the keyword; an `await` use and a non-`Future` return pass.
#[test]
fn unnecessary_async_gate() {
    let f = Fixture::parse("void f() async {}\n");
    let lints = f.run(&one_rule("unnecessary_async"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let keyword = tok(&f.source, &f.tokens, "async", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, keyword));
    // Message: upstream Dart SDK source,
    // pkg/linter/lib/src/rules/unnecessary_async.dart.
    assert_eq!(
        lints[0].message,
        "Don't make a function 'async' if it doesn't use 'await'."
    );
    let f = Fixture::parse("Future<int> f() async {\n  return 1;\n}\n");
    assert!(f.run(&one_rule("unnecessary_async")).is_empty());
    let f = Fixture::parse("Future<int> f() async {\n  return Future.value(1);\n}\n");
    assert_eq!(f.run(&one_rule("unnecessary_async")).len(), 1);
    let f = Fixture::parse("void f() async {\n  await g();\n}\n\nvoid g() {}\n");
    assert!(f.run(&one_rule("unnecessary_async")).is_empty());
}

/// `int x = 1;` reports at the type (`test_int`-shape); the
/// uninitialized, coerced-int, nullable and inferred-var shapes pass.
#[test]
fn omit_local_variable_types_gate() {
    let f = Fixture::parse("void f() {\n  int x = 1;\n}\n");
    let lints = f.run(&one_rule("omit_local_variable_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let type_kw = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, type_kw));
    // Message: upstream Dart SDK source,
    // pkg/linter/lib/src/rules/omit_local_variable_types.dart.
    assert_eq!(
        lints[0].message,
        "Unnecessary type annotation on a local variable."
    );
    let f = Fixture::parse(
        "void f() {\n  int x;\n  var y = 1;\n  double d = 1;\n  int? z = 1;\n  String s = g();\n}\n\nString g() => '';\n",
    );
    assert!(f.run(&one_rule("omit_local_variable_types")).is_empty());
}

/// The collection-literal spelling faces over the production tiling:
/// the childless `ListLiteral`/`SetOrMapLiteral`/`MapEntry` rows leave
/// their element pieces as owner-sibling rows inside the literal's
/// span, so `collection_spelling` reads them off the sibling run —
/// the element segment between two uncovered commas, with a `MapEntry`
/// marker closing an entry segment and the colon (payload word 0)
/// splitting its key/value sides.
#[test]
fn collection_spelling_production_faces() {
    // `List<int>`/`Set<int>` spellings revive: the written type equals
    // the derived element spelling.
    let f = Fixture::parse("void f() {\n  List<int> l = [1, 2];\n}\n");
    let lints = f.run(&one_rule("omit_obvious_local_variable_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse("void f() {\n  Set<int> s = {1, 2};\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    let f = Fixture::parse("void f() {\n  List<String> l = ['a', 'b'];\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );

    // The map face: `var m = {1: 2}` spells `Map<int, int>` — the
    // entry is obvious, so `specify_nonobvious` stays silent (a dead
    // element enumeration answered `None` and reported the `var`).
    let f = Fixture::parse("void f() {\n  var m = {1: 2};\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    // Multi-entry faces spell (`{'a': 1, 'b': 2}` is
    // `Map<String, int>`); a key/value pair of different types still
    // spells (`Map<int, String>`); the mixed-element list bails.
    let f = Fixture::parse("void f() {\n  var m = {'a': 1, 'b': 2};\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    let f = Fixture::parse("void f() {\n  var m = {1: 'a'};\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    let f = Fixture::parse("void f() {\n  var l = [1, 'a'];\n}\n");
    assert_eq!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .len(),
        1
    );
}

/// Nested collection elements spell through recursion (`[[1]]` is
/// `List<List<int>>`), and mixed element spellings bail.
#[test]
fn collection_spelling_nested_and_mixed() {
    let f = Fixture::parse("void f() {\n  var x = [[1], [2]];\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    // `[[1], {2: 3}]` mixes `List<int>` with `Map<int, int>` — the
    // spellings differ, so the literal is not obvious and `var`
    // reports (the official's `_sameOrNull` fails the same way).
    let f = Fixture::parse("void f() {\n  var x = [[1], {2: 3}];\n}\n");
    assert_eq!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .len(),
        1
    );
    // A nested map value spells through the entry's colon split.
    let f = Fixture::parse("void f() {\n  var m = {1: {2: 3}};\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
}

/// The non-collection faces around the fix: an explicitly typed
/// literal keeps its `<int>` arm, an empty literal and a call
/// initializer stay on their documented faces.
#[test]
fn collection_spelling_boundaries() {
    // `<int>[1]` spells `List<int>` off the sibling `TypeArguments`
    // row — the written `var` stays silent under `specify_nonobvious`
    // and a written `List<int>` reports under `omit_obvious`.
    let f = Fixture::parse("void f() {\n  var y = <int>[1];\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    let f = Fixture::parse("void f() {\n  List<int> y = <int>[1];\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    // An empty literal carries no element spelling (upstream bails on
    // the `null` derived type) — `var` reports as non-obvious.
    let f = Fixture::parse("void f() {\n  var e = [];\n  var g = {};\n}\n");
    assert_eq!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .len(),
        2
    );
    // A call initializer is not a literal face — `var z = foo()`
    // keeps its documented under/over-report shape (unchanged).
    let f = Fixture::parse("void f() {\n  var z = bar('x');\n}\nString bar(String s) => s;\n");
    assert_eq!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .len(),
        1
    );
}

/// The multi-argument generic written run (`Map<int, int>`), the
/// `>>`-closer written run (`List<List<int>>`), and the explicit
/// type-arguments arm over the production tiling: the written type
/// walks back over `,` and the spelling compare splits `>>`, while the
/// literal's `<` lives on the sibling `TypeArguments` row.
#[test]
fn collection_spelling_generic_written_and_typed_faces() {
    // `Map<int,int> m = {1:2}` — the written run reads whole (the `,`
    // is inside the type's own argument list) and the entry-derived
    // spelling `Map<int,int>` matches.
    let f = Fixture::parse("void f() {\n  Map<int,int> m = {1: 2};\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    // `List<List<int>> l = [[1]]` — the written `>>` is one token; the
    // derived spelling's `>` pieces split-match it.
    let f = Fixture::parse("void f() {\n  List<List<int>> l = [[1]];\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    // `<List<int>>[[1]]` — the literal's explicit arguments close on a
    // `>>` token; the spare `>` belongs to the inner `List<int>`.
    let f = Fixture::parse("void f() {\n  var l = <List<int>>[[1]];\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    // `Map<int,int> m = <int,int>{1:2}` — the `<` arm fires off the
    // sibling `TypeArguments` row even with heterogeneous elements.
    let f = Fixture::parse("void f() {\n  Map<int,int> m = <int,int>{1: 2};\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    let f = Fixture::parse("void f() {\n  var l = <int>[1, 'a'];\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
}

/// Spread and if elements participate in the fold with `null` legs
/// (upstream `_sameOrNull`, `obvious_types.dart` :14-15) rather than
/// bailing: `{...o, 1: 2}` derives `Map<int, int>` from the entry leg,
/// `[if (c) 1, 2]` derives `List<int>` from the then element's
/// `elementType`. A `for` element is never obvious (:56-57).
#[test]
fn collection_spelling_spread_and_if_participate() {
    // The map face through a spread: the entry side alone derives.
    let f = Fixture::parse("void f() {\n  var o = {1: 2};\n  var m = {...o, 1: 2};\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
    let f =
        Fixture::parse("void f() {\n  var o = {1: 2};\n  Map<int,int> m = {...o, 1: 2};\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    // The if element contributes its then element's `elementType`:
    // `[if (c) 1, 2]` derives `List<int>`; `{if (c) 1: 2}` contributes
    // only null legs — not obvious upstream (:152-153).
    let f = Fixture::parse("void f() {\n  List<int> l = [if (c) 1, 2];\n}\n");
    assert_eq!(
        f.run(&one_rule("omit_obvious_local_variable_types")).len(),
        1
    );
    let f = Fixture::parse("void f() {\n  var m = {if (c) 1: 2};\n}\n");
    assert_eq!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .len(),
        1
    );
    let f = Fixture::parse("void f() {\n  Map<int,int> m = {if (c) 1: 2};\n}\n");
    assert!(
        f.run(&one_rule("omit_obvious_local_variable_types"))
            .is_empty()
    );
    // A `for` element is never obvious: `var l = [for (var i in x) i]`
    // reports under `specify_nonobvious`.
    let f = Fixture::parse("void f() {\n  var l = [for (var i in x) i];\n}\n");
    assert_eq!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .len(),
        1
    );
}

/// The `omit_local_variable_types` collection and creation faces over
/// the production tiling: the `<` arm reads the sibling `TypeArguments`
/// row, and nested written generics compare through the `>>` split.
#[test]
fn omit_local_variable_types_collection_faces() {
    let f = Fixture::parse("void f() {\n  List<int> l = <int>[1];\n}\n");
    assert_eq!(f.run(&one_rule("omit_local_variable_types")).len(), 1);
    let f = Fixture::parse("void f() {\n  Map<int,int> m = <int,int>{1: 2};\n}\n");
    assert_eq!(f.run(&one_rule("omit_local_variable_types")).len(), 1);
    // The written `>>` run and a nested literal argument face.
    let f = Fixture::parse("void f() {\n  List<List<int>> l = <List<int>>[[1]];\n}\n");
    assert_eq!(f.run(&one_rule("omit_local_variable_types")).len(), 1);
    // A written type that differs from the literal's arguments stays
    // silent.
    let f = Fixture::parse("void f() {\n  List<String> l = <int>[1];\n}\n");
    assert!(f.run(&one_rule("omit_local_variable_types")).is_empty());
    // `Foo<List<int>> f = Foo<List<int>>()` — the creation face's
    // nested argument compare.
    let f = Fixture::parse(
        "class Foo<T> {\n  Foo();\n}\nvoid f() {\n  Foo<List<int>> x = Foo<List<int>>();\n}\n",
    );
    assert_eq!(f.run(&one_rule("omit_local_variable_types")).len(), 1);
}

/// The interface face and the `Object` face report at the member name;
/// the `@redeclare` annotation and a fresh member pass.
#[test]
fn annotate_redeclares_gate() {
    let f = Fixture::parse(
        "class A {\n  void m() {}\n}\nextension type E(A a) implements A {\n  void m() {}\n}\n",
    );
    let lints = f.run(&one_rule("annotate_redeclares"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'm' is redeclaring but isn't annotated with '@redeclare'."
    );
    let name = tok(&f.source, &f.tokens, "m", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));

    let f = Fixture::parse(
        "class A {\n  void m() {}\n}\nextension type E(A a) implements A {\n  @redeclare\n  void m() {}\n}\n",
    );
    assert!(f.run(&one_rule("annotate_redeclares")).is_empty());

    let f = Fixture::parse("extension type E(int i) {\n  String toString() => '';\n}\n");
    assert_eq!(f.run(&one_rule("annotate_redeclares")).len(), 1);
}

/// An `implements` name whose class (or a concrete same-file ancestor)
/// declares `==` reports at the interface name; a plain class passes.
#[test]
fn avoid_implementing_value_types_gate() {
    let f = Fixture::parse(
        "class A {\n  bool operator ==(Object o) => false;\n}\nclass C implements A {}\n",
    );
    let lints = f.run(&one_rule("avoid_implementing_value_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Classes that override '==' should not be implemented."
    );
    let name = tok(&f.source, &f.tokens, "A", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));

    let f = Fixture::parse("class A {}\nclass C implements A {}\nclass D extends A {}\n");
    assert!(
        f.run(&one_rule("avoid_implementing_value_types"))
            .is_empty()
    );
}

/// `int i = 1;` reports at the type; a call initializer passes.
#[test]
fn omit_obvious_local_variable_types_gate() {
    let f = Fixture::parse("void f() {\n  int i = 1;\n}\n");
    let lints = f.run(&one_rule("omit_obvious_local_variable_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Omit the type annotation on a local variable when the type is obvious."
    );
    let type_kw = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, type_kw));

    let f = Fixture::parse("void f() {\n  int i = compute();\n}\nint compute() => 1;\n");
    assert!(
        f.run(&one_rule("omit_obvious_local_variable_types"))
            .is_empty()
    );
}

/// A static field with an obvious initializer reports at the type; a
/// call initializer passes.
#[test]
fn omit_obvious_property_types_gate() {
    let f = Fixture::parse("class C {\n  static int i = 1;\n}\n");
    let lints = f.run(&one_rule("omit_obvious_property_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The type annotation isn't needed because it is obvious."
    );
    let type_kw = tok(&f.source, &f.tokens, "int", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, type_kw));

    let f = Fixture::parse("class C {\n  static int i = compute();\n}\nint compute() => 1;\n");
    assert!(f.run(&one_rule("omit_obvious_property_types")).is_empty());
}

/// A non-const empty-bodied constructor on an `@immutable` class
/// reports at the name; a `const` constructor and a non-immutable
/// class pass.
#[test]
fn prefer_const_constructors_in_immutables_gate() {
    let f = Fixture::parse("@immutable\nclass A {\n  A();\n}\n");
    let lints = f.run(&one_rule("prefer_const_constructors_in_immutables"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Constructors in '@immutable' classes should be declared as 'const'."
    );
    let name = tok(&f.source, &f.tokens, "A", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));

    let f = Fixture::parse("@immutable\nclass A {\n  const A();\n}\nclass B {\n  B();\n}\n");
    assert!(
        f.run(&one_rule("prefer_const_constructors_in_immutables"))
            .is_empty()
    );
}

/// A never-reassigned for-each loop variable reports at the name; an
/// incremented one passes.
#[test]
fn prefer_final_in_for_each_gate() {
    let f = Fixture::parse("void f() {\n  for (var i in [1, 2, 3]) {\n    print(i);\n  }\n}\n");
    let lints = f.run(&one_rule("prefer_final_in_for_each"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The variable 'i' should be final.");
    let name = tok(&f.source, &f.tokens, "i", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));

    let f = Fixture::parse("void f() {\n  for (var i in [1, 2, 3]) {\n    i++;\n  }\n}\n");
    assert!(f.run(&one_rule("prefer_final_in_for_each")).is_empty());
}

/// A never-reassigned local reports at the keyword; a reassigned one
/// passes.
#[test]
fn prefer_final_locals_gate() {
    let f = Fixture::parse("void f() {\n  var a = '';\n  print(a);\n}\n");
    let lints = f.run(&one_rule("prefer_final_locals"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Local variables should be final.");
    let keyword = tok(&f.source, &f.tokens, "var", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, keyword));

    let f = Fixture::parse("void f() {\n  var a = '';\n  a = 'b';\n}\n");
    assert!(f.run(&one_rule("prefer_final_locals")).is_empty());
}

/// A call-initialized untyped local reports at the declaration; a
/// typed one and a literal-initialized one pass.
#[test]
fn specify_nonobvious_local_variable_types_gate() {
    let f = Fixture::parse("void f() {\n  var h = bar('x');\n}\nString bar(String s) => '';\n");
    let lints = f.run(&one_rule("specify_nonobvious_local_variable_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Specify the type of a local variable when the type is non-obvious."
    );
    let keyword = tok(&f.source, &f.tokens, "var", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, keyword));

    let f = Fixture::parse("void f() {\n  int i = 1;\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );

    let f = Fixture::parse("void f() {\n  var h = bar();\n}\nString bar() => '';\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_local_variable_types"))
            .is_empty()
    );
}

/// A static field initialized from a call reports at the declaration;
/// a literal-initialized one passes.
#[test]
fn specify_nonobvious_property_types_gate() {
    let f = Fixture::parse(
        "class C {\n  static var h = bar('x');\n}\nString bar(String s) => '';\n",
    );
    let lints = f.run(&one_rule("specify_nonobvious_property_types"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "A type annotation is needed because it isn't obvious."
    );
    let keyword = tok(&f.source, &f.tokens, "var", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, keyword));

    let f = Fixture::parse("class C {\n  static var h = 1;\n}\n");
    assert!(
        f.run(&one_rule("specify_nonobvious_property_types"))
            .is_empty()
    );
}

/// An uninitialized private nullable field reports at the name; a
/// const-constructored class and a field read anywhere pass.
#[test]
fn use_late_for_private_fields_and_variables_gate() {
    let f = Fixture::parse("class C {\n  int? _i;\n}\n");
    let lints = f.run(&one_rule("use_late_for_private_fields_and_variables"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use 'late' for private members with a non-nullable type."
    );
    let name = tok(&f.source, &f.tokens, "_i", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));

    let f = Fixture::parse("class C {\n  const C();\n  int? _i;\n}\n");
    assert!(
        f.run(&one_rule("use_late_for_private_fields_and_variables"))
            .is_empty()
    );

    let f = Fixture::parse("class C {\n  int? _i;\n  void f() {\n    print(_i);\n  }\n}\n");
    assert!(
        f.run(&one_rule("use_late_for_private_fields_and_variables"))
            .is_empty()
    );
}

// —— prefer_const_declarations ——————————————————————————————————————————

/// `test_localVariable_final_intLiteral` — the list span runs from the
/// keyword through the initializer (upstream `reportAtNode` of the
/// `VariableDeclarationList`).
#[test]
fn prefer_const_declarations_local_reports() {
    let f = Fixture::parse("void f() { final x = 1; }\n");
    let lints = f.run(&one_rule("prefer_const_declarations"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use 'const' for final variables initialized to a constant value."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try replacing 'final' with 'const'.")
    );
    let keyword = tok(&f.source, &f.tokens, "final", 0);
    let literal = tok(&f.source, &f.tokens, "1", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, keyword));
    assert_eq!(lints[0].range.end, utf16_at(&f, literal) + 1);
}

/// `test_localVariable_final_listLiteral` and the typed/untyped
/// type-argument faces — a bare collection literal is a runtime
/// literal, the `TypedLiteral` arm (:58).
#[test]
fn prefer_const_declarations_bare_collection_stays_silent() {
    for source in [
        "void f() { final x = []; }\n",
        "void f() { final x = <int>[]; }\n",
        "void f() { final List<int> x = []; }\n",
        "void f() { final x = {}; }\n",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("prefer_const_declarations")).is_empty(),
            "{source}"
        );
    }
}

/// `test_localVariable_final_listLiteral_const` and the map face — the
/// written `const` keyword qualifies.
#[test]
fn prefer_const_declarations_const_collection_reports() {
    for source in [
        "final x = const [];\n",
        "final x = const <int>[];\n",
        "final x = const {};\n",
    ] {
        let f = Fixture::parse(source);
        assert_eq!(
            f.run(&one_rule("prefer_const_declarations")).len(),
            1,
            "{source}"
        );
    }
}

/// The tear-off faces (upstream `test_*Tearoff*`): a `final` variable
/// initialized to a constructor tear-off (`C.new`, `C.new` on a
/// generic class with or without written type arguments) or a
/// `static` method tear-off (`C.m`, `C.m<int>`) holds a constant
/// expression — the tear-off is constant regardless of the
/// constructor's own `const`-ness — so the list reports.
#[test]
fn prefer_const_declarations_tearoffs_report() {
    for source in [
        // Constructor tear-off, declared-type inference.
        "class C<T> {\n  C();\n}\nvoid f() {\n  final C<int> Function() c = C.new;\n}\n",
        // Constructor tear-off, instantiated.
        "class C<T> {\n  C();\n}\nvoid f() {\n  final c = C<int>.new;\n}\n",
        // Constructor tear-off, uninstantiated.
        "class C {\n  C();\n}\nvoid f() {\n  final x = C.new;\n}\n",
        // Static-method tear-off, declared-type inference.
        "class C {\n  static void m<T>(T t) {}\n}\nvoid f() {\n  final C<int> Function() c = C.m;\n}\n",
        // Static-method tear-off, instantiated.
        "class C {\n  static void m<T>(T t) {}\n}\nvoid f() {\n  final c = C.m<int>;\n}\n",
        // Static-method tear-off, uninstantiated.
        "class C {\n  static void m() {}\n}\nvoid f() {\n  final x = C.m;\n}\n",
    ] {
        let f = Fixture::parse(source);
        assert_eq!(
            f.run(&one_rule("prefer_const_declarations")).len(),
            1,
            "{source}"
        );
    }
}

/// The corpus mirror of the under-fire blocks (gate
/// lints-new/oracle-2026-10-18): static-method tear-offs inside the
/// corpus's own context — generic classes whose static members return
/// the (undefined-here) `C<X>`, functions before their classes. The
/// constructor-tear-off blocks (`X.new` over a constructor-less class)
/// ride the implicit-constructor tear-off face owned by the
/// const-ability module; the `m<T>` type-variable instantiation negative
/// (upstream test_staticFunctionTearoff_instantiatedWithTypeVariable)
/// is gated by that module's type-argument read too.
#[test]
fn prefer_const_declarations_corpus_tearoff_blocks_report() {
    let f = Fixture::parse(
        "// POSITIVE upstream test_staticFunctionTearoff_inference (1 row at `c`).\nvoid fStaticTearoffInference() {\n  final CStaticTear<int> Function() c = CStaticTear.m;\n}\n\nclass CStaticTear<T> {\n  static C<X> m<X>() => C<X>();\n}\n\n// POSITIVE upstream test_staticFunctionTearoff_instantiatedWithType (1 row\n// at `c`).\nvoid fStaticTearoffInstantiated() {\n  final c = CStaticTearInst.m<int>;\n}\n\nclass CStaticTearInst<T> {\n  static C<X> m<X>() => C<X>();\n}\n\n// POSITIVE upstream test_staticFunctionTearoff_uninstantiated (1 row at\n// `x`).\nvoid fStaticTearoffUninstantiated() {\n  final x = CStaticTearUninst.m;\n}\n\nclass CStaticTearUninst<T> {\n  static C<X> m<X>() => C<X>();\n}\n",
    );
    let lints = f.run(&one_rule("prefer_const_declarations"));
    assert_eq!(lints.len(), 3, "static tear-off blocks: {lints:?}");
}

/// upstream `test_staticFunctionTearoff_instantiatedWithTypeVariable`:
/// a tear-off instantiated with an in-scope type variable is never
/// constant, while the concrete instantiation reports.
#[test]
fn prefer_const_declarations_type_variable_instantiation_stays_silent() {
    let f = Fixture::parse(
        "void f<T>() {\n  final c = C.m<T>;\n}\nclass C<X> {\n  static C<X> m<X>() => C<X>();\n}\n",
    );
    assert!(f.run(&one_rule("prefer_const_declarations")).is_empty());
    let f = Fixture::parse(
        "void f() {\n  final c = C.m<int>;\n}\nclass C<X> {\n  static C<X> m<X>() => C<X>();\n}\n",
    );
    assert_eq!(f.run(&one_rule("prefer_const_declarations")).len(), 1);
}

/// `test_localVariable_final_constructorInvocation_new` vs `_const` —
/// a creation is a constant only when `const` is written.
#[test]
fn prefer_const_declarations_creation_needs_the_const_keyword() {
    let f = Fixture::parse("void f() { final x = C(); }\nclass C { const C(); }\n");
    assert!(f.run(&one_rule("prefer_const_declarations")).is_empty());
    let f = Fixture::parse("void f() { final x = const C(); }\nclass C { const C(); }\n");
    assert_eq!(f.run(&one_rule("prefer_const_declarations")).len(), 1);
}

/// `test_localVariable_final_intLiteral_multiple` — one initializer
/// missing skips the report (:56-57).
#[test]
fn prefer_const_declarations_missing_initializer_stays_silent() {
    let f = Fixture::parse("void f() { final x, y = 1; }\n");
    assert!(f.run(&one_rule("prefer_const_declarations")).is_empty());
}

/// `test_staticField_final_listLiteral` and the instance-field face —
/// only the static `FieldDeclaration` is visited (:31-34).
#[test]
fn prefer_const_declarations_only_static_fields_report() {
    let f = Fixture::parse("class A { static final x = const []; }\n");
    assert_eq!(f.run(&one_rule("prefer_const_declarations")).len(), 1);
    let f = Fixture::parse("class A { final x = const []; }\n");
    assert!(f.run(&one_rule("prefer_const_declarations")).is_empty());
}

/// `test_topLevelVariable_final_topLevelVariable_const` — the
/// same-file const-bound reference face.
#[test]
fn prefer_const_declarations_const_reference_reports() {
    let f = Fixture::parse("const x = 3;\nfinal y = x;\n");
    assert_eq!(f.run(&one_rule("prefer_const_declarations")).len(), 1);
}

/// `test_localVariable_final_staticProperty_const` — a static const
/// member read qualifies; the runtime reads (:144-166) do not.
#[test]
fn prefer_const_declarations_member_faces() {
    let f = Fixture::parse("class K { static const z = 7; }\nfinal w = K.z;\n");
    assert_eq!(f.run(&one_rule("prefer_const_declarations")).len(), 1);
    let f = Fixture::parse("void f() { final a = 7.hashCode; final b = 7.toString(); }\n");
    assert!(f.run(&one_rule("prefer_const_declarations")).is_empty());
}

/// `test_recordLiteral` — records are no `TypedLiteral`s: a
/// const-able record fires, a closure-bearing one stays silent.
#[test]
fn prefer_const_declarations_record_faces() {
    let f = Fixture::parse("final t = (1, 2);\n");
    assert_eq!(f.run(&one_rule("prefer_const_declarations")).len(), 1);
    let f = Fixture::parse("final t = (1, () {});\n");
    assert!(f.run(&one_rule("prefer_const_declarations")).is_empty());
}

// —— prefer_mixin ———————————————————————————————————————————————————————

/// `PreferMixinTestLanguage219.test_mixedInClass` — a plain class in a
/// with clause reports at the clause type.
#[test]
fn prefer_mixin_plain_class_reports() {
    let f = Fixture::parse("class A {}\nclass B extends Object with A {}\n");
    let lints = f.run(&one_rule("prefer_mixin"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Only mixins should be mixed in.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try converting 'A' to a mixin.")
    );
    let with_type = tok(&f.source, &f.tokens, "A", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, with_type));
    assert_eq!(lints[0].range.end, utf16_at(&f, with_type) + 1);
}

/// `test_mixedInMixin_ok` / `test_mixinClass` — `mixin` and
/// `mixin class` targets pass.
#[test]
fn prefer_mixin_mixin_targets_stay_silent() {
    let f = Fixture::parse("mixin M {}\nclass C with M {}\n");
    assert!(f.run(&one_rule("prefer_mixin")).is_empty());
    let f = Fixture::parse("mixin class M {}\nclass Z with M {}\n");
    assert!(f.run(&one_rule("prefer_mixin")).is_empty());
}

/// `test_mixedInClass_typAlias` — the generic-type-alias walk.
#[test]
fn prefer_mixin_typedef_alias_reports() {
    let f = Fixture::parse("class A {}\ntypedef AA = A;\nabstract class CC with AA {}\n");
    let lints = f.run(&one_rule("prefer_mixin"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try converting 'AA' to a mixin.")
    );
    // Upstream marks the `with` clause `AA` (the second `AA` token —
    // `prefer_mixin.dart:44` reports the clause type, not the typedef
    // name).
    let with_type = tok(&f.source, &f.tokens, "AA", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, with_type));
}

/// The named mixin application carries a with clause too
/// (`class B = Object with A;`).
#[test]
fn prefer_mixin_mixin_application_reports() {
    let f = Fixture::parse("class A {}\nclass B = Object with A;\n");
    assert_eq!(f.run(&one_rule("prefer_mixin")).len(), 1);
}

/// Two clause types report independently; an enum target is neither
/// element kind and stays silent.
#[test]
fn prefer_mixin_clause_type_faces() {
    let f = Fixture::parse("class A {}\nclass C {}\nclass B with A, C {}\n");
    assert_eq!(f.run(&one_rule("prefer_mixin")).len(), 2);
    let f = Fixture::parse("enum E { a }\nclass B with E {}\n");
    assert!(f.run(&one_rule("prefer_mixin")).is_empty());
}

// —— use_enums ——————————————————————————————————————————————————————————

/// `test_simple_private` — two candidate constants and a const
/// constructor report at the class name.
#[test]
fn use_enums_simple_reports() {
    let f = Fixture::parse(
        "class _A {\n  static const _A a = _A();\n  static const _A b = _A();\n  const _A();\n}\n",
    );
    let lints = f.run(&one_rule("use_enums"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Class should be an enum.");
    let name = tok(&f.source, &f.tokens, "_A", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_constructor_private` — the public class needs a private named
/// constructor; `test_no_lint_constructor_public` bails on the unnamed
/// one, `test_no_lint_nonConstConstructor` on the missing `const`.
#[test]
fn use_enums_constructor_gates() {
    let f = Fixture::parse(
        "class A { static const A a = A._(); static const A b = A._(); const A._(); }\n",
    );
    assert_eq!(f.run(&one_rule("use_enums")).len(), 1);
    let f = Fixture::parse(
        "class A { static const A a = A(1); static const A b = A(2); final int i; const A(this.i); }\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
    let f = Fixture::parse(
        "class _A { static const _A a = _A(); static const _A b = _A(); _A(); }\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
}

/// `test_no_lint_abstract` / `test_no_lint_extends_notObject` /
/// `test_extendsObject` — the abstract and supertype gates.
#[test]
fn use_enums_header_gates() {
    let f = Fixture::parse(
        "abstract class A { static const A a = A._(); static const A b = A._(); const A._(); }\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
    let f = Fixture::parse(
        "class O { const O(); }\nclass A extends O { static const A a = A._(); static const A b = A._(); const A._(); }\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
    let f = Fixture::parse(
        "class A extends Object { static const A a = A._(); static const A b = A._(); const A._(); }\n",
    );
    assert_eq!(f.run(&one_rule("use_enums")).len(), 1);
}

/// `test_no_lint_nonInstanceCreationInitialization` — one candidate.
#[test]
fn use_enums_needs_two_candidates() {
    let f = Fixture::parse(
        "class _A { static const _A a = _A(); static const _A b = a; const _A(); }\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
}

/// `test_no_lint_overrides_hashCode` / `_equals` /
/// `test_no_lint_implements_values_field` — the member disqualifiers.
#[test]
fn use_enums_member_disqualifiers() {
    for tail in [
        "  int get hashCode => 0;\n",
        "  @override\n  bool operator ==(Object other) => false;\n",
        "  static final List<_A> values = [a, b];\n",
        "  int get index => 0;\n",
    ] {
        let source = String::from(
            "class _A {\n  static const _A a = _A();\n  static const _A b = _A();\n  const _A();\n",
        ) + tail
            + "}\n";
        let f = Fixture::parse(&source);
        assert!(f.run(&one_rule("use_enums")).is_empty(), "{source}");
    }
}

/// `test_no_lint_extended` / `test_no_lint_implemented` — the
/// subclass and interface bails.
#[test]
fn use_enums_outside_subclass_bails() {
    let f = Fixture::parse(
        "class _E {\n  static const _E a = _E();\n  static const _E b = _E();\n  const _E();\n}\nclass F extends _E { const F() : super._(); }\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
    let f = Fixture::parse(
        "class _E {\n  static const _E a = _E();\n  static const _E b = _E();\n  const _E();\n}\nclass F implements _E {}\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
}

/// `test_no_lint_constructorUsedOutsideClass` — a top-level generative
/// invocation bails; `test_referencedFactoryConstructor` — a factory
/// use does not.
#[test]
fn use_enums_outside_creation_faces() {
    let f = Fixture::parse(
        "class _E {\n  static const _E a = _E();\n  static const _E b = _E();\n  const _E();\n}\n_E get e => _E();\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
    let f = Fixture::parse(
        "class _E {\n  static const _E c = _E();\n  static const _E d = _E();\n  const _E();\n  factory _E.withValue(int x) => c;\n}\n_E e = _E.withValue(0);\n",
    );
    assert_eq!(f.run(&one_rule("use_enums")).len(), 1);
}

/// `test_no_lint_constructorUsedInConstructor` — a generative
/// invocation inside the class outside the candidates bails.
#[test]
fn use_enums_internal_invocation_bails() {
    let f = Fixture::parse(
        "class _E {\n  static const _E a = _E();\n  static const _E b = _E();\n  const _E({_E e = const _E()});\n}\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
}

/// `test_primaryConstructor_private` — the primary constructor is the
/// class's generative constructor; `test_primaryConstructor_hashCode`
/// bails on the parameter name.
#[test]
fn use_enums_primary_constructor_faces() {
    let f = Fixture::parse(
        "class const A._(final int value) {\n  static const A a = A._(1);\n  static const A b = A._(2);\n}\n",
    );
    assert_eq!(f.run(&one_rule("use_enums")).len(), 1);
    let f = Fixture::parse(
        "class const A._(final int hashCode) {\n  static const A a = A._(1);\n  static const A b = A._(2);\n}\n",
    );
    assert!(f.run(&one_rule("use_enums")).is_empty());
}

/// The written-`const` wrapper candidates (`static const A a =
/// const A._();`) resolve to the same generative constructor.
#[test]
fn use_enums_const_written_candidates_report() {
    let f = Fixture::parse(
        "class _A {\n  static const _A a = const _A();\n  static const _A b = const _A();\n  const _A();\n}\n",
    );
    assert_eq!(f.run(&one_rule("use_enums")).len(), 1);
}

/// The enhanced-enums gate — upstream registers only with the feature
/// (`enabledIn: 2.17`); a pre-2.17 settled version stays silent.
#[test]
fn use_enums_version_gate() {
    let mut f = Fixture::parse(
        "class _A { static const _A a = _A(); static const _A b = _A(); const _A(); }\n",
    );
    f.version = Some(dartq_language::LanguageVersion::new(2, 16));
    assert!(f.run(&one_rule("use_enums")).is_empty());
    f.version = Some(dartq_language::LanguageVersion::new(2, 17));
    assert_eq!(f.run(&one_rule("use_enums")).len(), 1);
}
