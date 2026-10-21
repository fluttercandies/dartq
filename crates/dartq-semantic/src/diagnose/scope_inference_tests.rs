//! Scope and inference faces the analyzer corpus exposed: the enum's
//! implicit `Enum` interface, the if-case pattern scope, a for-element's
//! loop variable inside an initializer, and explicit type arguments as
//! final closure-parameter solutions.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `priority.index` on an enum-typed receiver resolves through the
/// enum's implicit `Enum` interface — in an operator operand (the
/// operator-merged member path) as much as in a plain read.
#[test]
fn enum_index_resolves_through_the_implicit_enum_interface() {
    let report = report_ops(
        "enum Pri { nothing, high }\n\
         class Dr { Pri get workPriority => Pri.nothing; }\n\
         bool f(Dr driver, Pri best) {\n\
           Pri priority = driver.workPriority;\n\
           if (priority.index > best.index) { return true; }\n\
           if (priority.name.length == 0) { return true; }\n\
           return priority.index == 0;\n\
         }\n",
    );
    assert_silent(&report);
}

/// `if (parent case PA parent) ... else if (parent is CN) parent.name`:
/// the case clause's `parent` is in scope in the then statement only —
/// the else branch reads the OUTER `parent`, promoted to `CN`.
#[test]
fn if_case_pattern_variable_is_scoped_to_the_then_statement() {
    let report = report_ops(
        "abstract class Node { Node? get parent; }\n\
         class PA extends Node { Node? get parent => null; Node get propertyName => this; }\n\
         class CN extends Node { Node? get parent => null; Node get name => this; }\n\
         class Id extends Node {\n\
           Node? get parent => null;\n\
           bool get inDeclarationContext {\n\
             var parent = this.parent!;\n\
             if (parent case PA parent) {\n\
               return identical(parent.propertyName, this);\n\
             } else if (parent is CN) {\n\
               return identical(parent.name, this);\n\
             }\n\
             return false;\n\
           }\n\
         }\n\
         bool identical(Object? a, Object? b) => true;\n",
    );
    assert_silent(&report);
}

/// The then statement itself still reads the pattern variable: a member
/// only the pattern's type declares resolves there, and a member it lacks
/// reports against it.
#[test]
fn if_case_pattern_variable_binds_in_the_then_statement() {
    let report = report_ops(
        "abstract class Node {}\n\
         class PA extends Node { int get propertyName => 0; }\n\
         int f(Node parent) {\n\
           if (parent case PA parent) {\n\
             return parent.propertyName + parent.missing;\n\
           }\n\
           return 0;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_getter"],
        "{:?}",
        rows(&report)
    );
}

/// A for-element's loop variable inside a creation argument (`Ver(1,
/// byFragment: { for (var fa in xs) fa.k: fa.v })`) types from its own
/// iterable — never from the enclosing `var v = Ver(...)` declarator,
/// whose initializer window stays its own.
#[test]
fn for_element_loop_variable_inside_an_initializer_types_from_its_iterable() {
    let report = report_ops(
        "class FA { int get fragment => 0; String get reporter => ''; }\n\
         class Mp<K, V> { Iterable<V> get values => throw 0; }\n\
         class Ver { Ver(int a, {required Map<int, String> byFragment}); }\n\
         class C {\n\
           final Mp<String, FA> _files = Mp();\n\
           final List<FA> _list = [];\n\
           void compute() {\n\
             var v = Ver(1, byFragment: { for (var fa in _files.values) fa.fragment: fa.reporter });\n\
             v;\n\
             var w = Ver(1, byFragment: { for (var fc in _list) fc.fragment: fc.reporter });\n\
             w;\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

/// `folders.fold<Fo?>(null, (result, folder) ...)`: the explicit type
/// argument fixes `T := Fo?` — the `null` argument never re-solves or
/// poisons it — and the receiver's `Lst<Fo?>` instantiates `E := Fo?` for
/// the second formal (a nullable element binds as itself).
#[test]
fn explicit_type_arguments_fix_closure_parameter_schemas() {
    let report = report_ops(
        "class Fo { String get path => ''; bool contains(String p) => true; }\n\
         class Lst<E> { T fold<T>(T initial, T combine(T previous, E element)) => initial; R mapIt<R>(R Function(E) f) => throw 0; }\n\
         Fo? lowest(Lst<Fo?> folders) {\n\
           return folders.fold<Fo?>(null, (result, folder) {\n\
             if (result == null) { return folder; }\n\
             else if (folder != null && result.contains(folder.path)) { return result; }\n\
             return null;\n\
           });\n\
         }\n\
         int lens(Lst<Fo> fs) => fs.mapIt((f) => f.path.length);\n",
    );
    assert_silent(&report);
}
