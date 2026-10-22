//! Scope and inference faces the analyzer corpus exposed: the enum's
//! implicit `Enum` interface, the if-case pattern scope, a for-element's
//! loop variable inside an initializer, and explicit type arguments as
//! final closure-parameter solutions.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, code_names, report_ops, report_ops_imports, rows, RigImport};

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

/// A generic callee's own parameter never escapes as the caller's static
/// type: `var n = min(a, b)` is unknown here (the oracle infers `int`),
/// so `off + n` is never refined to `double` and the `int` sinks stay
/// silent; an explicit `min<int>(...)` instantiates `int`; the receiver's
/// written arguments instantiate an own member's return (`Lst<int>`'s
/// `firstOf()` answers `int`); inside the generic class its own `E` is a
/// legitimate result type.
#[test]
fn a_callee_parameter_never_leaks_as_the_caller_type() {
    let report = report_ops(
        "T min<T extends num>(T a, T b) => a;\n\
         class Bytes { void setRange(int a, int b, List<int> v) {} }\n\
         class Lst<E> {\n\
           E firstOf() => throw 0;\n\
           E self() { var e = firstOf(); return e; }\n\
         }\n\
         void f(Bytes bytes, int off, int size, List<int> v, Lst<int> xs) {\n\
           var n = min(v.length, size - off);\n\
           bytes.setRange(off, off + n, v);\n\
           var m = min<int>(v.length, size);\n\
           bytes.setRange(off, off + m, v);\n\
           int first = xs.firstOf();\n\
           first;\n\
         }\n",
    );
    assert_silent(&report);
}

/// The instantiated own-member return still reports a real mismatch:
/// `Lst<String>`'s `firstOf()` into an `int` slot.
#[test]
fn an_instantiated_member_return_reports_a_real_mismatch() {
    let report = report_ops(
        "class Lst<E> { E firstOf() => throw 0; }\n\
         void f(Lst<String> xs) { int first = xs.firstOf(); first; }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// An inherited generic member instantiates through the receiver viewed
/// as an instance of the declaring container (`asInstanceOf`): `Lst<Item>`
/// calling `Itr<E>.map` binds the closure's `item` to `Item`, and
/// `Itr<E>.firstOf()` returns `Item` — never the `Object?` recovery or
/// the raw `E`.
#[test]
fn inherited_generic_members_instantiate_through_the_hierarchy() {
    let report = report_ops(
        "class Itr<E> {\n\
           Iterable<R> map<R>(R Function(E) f) => throw 0;\n\
           Iterable<E> where(bool Function(E) t) => throw 0;\n\
           E firstOf() => throw 0;\n\
         }\n\
         class Lst<E> extends Itr<E> {}\n\
         class Item { int get id => 0; }\n\
         void f(Lst<Item> items) {\n\
           var ids = items.map((item) => item.id);\n\
           ids;\n\
           items.where((i) => i.id > 0);\n\
           Item first = items.firstOf();\n\
           first;\n\
         }\n",
    );
    assert_silent(&report);
    let mismatch = report_ops(
        "class Itr<E> { E firstOf() => throw 0; }\n\
         class Lst<E> extends Itr<E> {}\n\
         class Item {}\n\
         void f(Lst<Item> items) { int first = items.firstOf(); first; }\n",
    );
    assert_eq!(
        code_names(&mismatch),
        vec!["invalid_assignment"],
        "{:?}",
        rows(&mismatch)
    );
}

/// The core primitives' closed lattice: a declared class is never an
/// `int` and `int` never a declared class (both directions report), while
/// an extension type over `int` stays undecided (it may implement `int`).
#[test]
fn a_declared_class_never_relates_to_a_core_primitive() {
    let report = report_ops(
        "class Item {}\n\
         void f(Item i, int n) { int x = i; Item y = n; x; y; }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["invalid_assignment", "invalid_assignment"],
        "{:?}",
        rows(&report)
    );
    let extension_type = report_ops(
        "extension type Id(int i) implements int {}\n\
         void f(Id id) { int x = id; x; }\n",
    );
    assert_silent(&extension_type);
}


/// A class-head send resolves its named constructor before any member: the
/// factory `MFT.read` IS the creation (silent, the created `MFT` flows),
/// while the same selector through a subclass is an `undefined_method` —
/// static members are not inherited (dart analyze 3.13.1 verdicts).
#[test]
fn a_named_constructor_outranks_members_and_statics_do_not_inherit() {
    let report = report_ops(
        "class MT { static MT read(int r) => MT(); }\n\
         class MFT extends MT { factory MFT.read(int r) => MFT._(); MFT._(); }\n\
         class Item { Item({required MFT functionType}); }\n\
         Item f(int r) => Item(functionType: MFT.read(r));\n\
         class Sub extends MT {}\n\
         MT g(int r) => Sub.read(r);\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_method"],
        "{:?}",
        rows(&report)
    );
}

/// A prefix-resolved creation (`const fb.Reader().vTableGet(...)`, the
/// unprefixed `fb.Reader()`): the created value rows stack over the whole
/// written span, so a following member never pops the enclosing
/// assignment's receiver — `b ??= fb.R().get(0)` reads `get` on the
/// created `R`, never on `b` (dart analyze 3.13.1: silent).
#[test]
fn a_prefixed_creation_folds_a_value_row_under_its_members() {
    let source = |call: &str| {
        format!(
            "class B {{ final int? v; B(this.v); }}\n\
             void f(B? b) {{\n\
               b ??= {call};\n\
             }}\n"
        )
    };
    for call in ["const fb.R().get(0)", "fb.R().get(0)"] {
        let report = report_ops_imports(
            &source(call),
            &[RigImport {
                uri: "package:probe/reader.dart",
                prefix: Some("fb"),
                source: "class R { int get(int i) => i; }\n",
            }],
        );
        assert_silent(&report);
    }
}

/// An extension-getter argument to a FOREIGN generic helper types the
/// closure formals through upward inference (the analyzer corpus's
/// `forCorrespondingPairs(fragmentList.withOriginDeclaration, infoList,
/// (fragment, info) …)`): the flush face leaves the extension read
/// `Unknown` (the merge-time field face never types one), so the schema
/// solver re-types the argument's row on demand — the element flows into
/// `T1` and the formal binds `Fragment`, not the `Object?` recovery whose
/// every member read reported `unchecked_use_of_nullable_value`.
#[test]
fn an_extension_getter_argument_types_the_foreign_helper_closure_formals() {
    let report = report_ops_imports(
        "class Fragment { void setCodeRange(int a) {} }\n\
         class Info { int codeOffset = 0; }\n\
         extension on Bag<Fragment> {\n\
           Bag<Fragment> get withOriginDeclaration => this;\n\
         }\n\
         void apply(Bag<Fragment> fragmentList, Bag<Info> infoList) {\n\
           forCorrespondingPairs(fragmentList.withOriginDeclaration, infoList, (\n\
             fragment,\n\
             info,\n\
           ) {\n\
             fragment.setCodeRange(info.codeOffset);\n\
           });\n\
         }\n",
        &[RigImport {
            uri: "package:probe/coll.dart",
            prefix: None,
            source: "class Bag<T> {}\n\
                     void forCorrespondingPairs<T1, T2>(\n\
                       Bag<T1> list1,\n\
                       Bag<T2> list2,\n\
                       void Function(T1, T2) f,\n\
                     ) {} \n",
        }],
    );
    assert_silent(&report);
}
