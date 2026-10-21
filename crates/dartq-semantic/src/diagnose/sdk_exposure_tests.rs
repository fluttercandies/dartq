//! The engine faces the indexed SDK sources exposed (#2515): shapes the
//! names-only SDK face used to leave under-reported now resolve through
//! real declarations, and each must answer exactly as the oracle does.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `RegExp('x').allMatches(text)` inside a field initializer's closure:
/// the creation of an abstract class through its factory constructor is
/// a `Created` receiver, and the member chain resolves on it — never an
/// implicit-this read of `allMatches`.
#[test]
fn abstract_class_factory_creation_is_a_receiver() {
    let report = report_ops(
        "abstract class Pattern { Iterable<Match> allMatches(String s); }\n\
         class Match {}\n\
         abstract interface class RegExp implements Pattern {\n\
           factory RegExp(String source) => _Re(source);\n\
         }\n\
         class _Re implements RegExp {\n\
           _Re(String s);\n\
           Iterable<Match> allMatches(String s) => throw s;\n\
         }\n\
         class Options {\n\
           void Function(String) debugPrint = (text) =>\n\
               RegExp('.{1,300}').allMatches(text).length;\n\
         }\n",
    );
    assert_silent(&report);
}

/// `comparison(element, other)` on a function-typed final field: the
/// bare send invokes the field's function type, so the result is the
/// signature's return (`int`), never the function type itself.
#[test]
fn function_typed_field_invocation_yields_the_return() {
    let report = report_ops(
        "class Q<E> {\n\
           final int Function(E, E) comparison;\n\
           Q(this.comparison);\n\
           int cmp(E a, E b) {\n\
             int comp;\n\
             comp = comparison(a, b);\n\
             return comp;\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A superclass that mixes in its concrete members (`abstract mixin class
/// SetBase<E> with SetMixin<E>`'s shape): a concrete subclass inherits
/// them through the mixin application and declares nothing abstract.
#[test]
fn mixin_provided_members_are_concrete_through_the_superclass() {
    let report = report_ops(
        "abstract class Sink<E> { bool any(bool Function(E) test); int get length; }\n\
         mixin SinkMixin<E> implements Sink<E> {\n\
           bool any(bool Function(E) test) => false;\n\
         }\n\
         abstract class SinkBase<E> with SinkMixin<E> {}\n\
         class PathSink extends SinkBase<String?> {\n\
           int get length => 0;\n\
         }\n",
    );
    assert_silent(&report);
}

/// An override whose superinterface is reached through a generic
/// superclass hop (`PathSet extends SetBase<String?>` overriding
/// `Set.removeWhere(bool Function(E))`): the inherited signature
/// substitutes `E := String?` through the hop before the comparison.
#[test]
fn override_substitutes_through_the_generic_superclass_hop() {
    let report = report_ops(
        "abstract class Coll<E> { void removeWhere(bool Function(E) test); }\n\
         abstract class CollBase<E> implements Coll<E> {\n\
           void removeWhere(bool Function(E) test) {}\n\
         }\n\
         class PathColl extends CollBase<String?> {\n\
           @override\n\
           void removeWhere(bool Function(String?) test) {}\n\
         }\n",
    );
    assert_silent(&report);
}

/// The implicit-this face runs from the UNQUALIFIED nodes only
/// (`visitUnqualifiedNameExpression`, `UnqualifiedFunctionInvocation`,
/// the unqualified assignment targets call
/// `_checkForInvalidInstanceMemberAccess2`; a `PropertyAccess` /
/// `MethodInvocation` with a real target never does): a member reached
/// through a receiver inside an instance field initializer — a top-level
/// read, a creation, a chained send, a closure body — is silent, while
/// the bare instance read still reports.
#[test]
fn qualified_member_access_in_a_field_initializer_is_not_implicit_this() {
    let report = report_ops(
        "class A { int get n => 0; Iterable<int> all(String s) => []; }\n\
         A top = A();\n\
         class B {\n\
           int x = top.n;\n\
           int y = A().n;\n\
           int z = A().all('x').length;\n\
           void Function(String) f = (t) => A().all(t).length;\n\
         }\n",
    );
    assert_silent(&report);
    let bare = report_ops("class C {\n  int get n => 0;\n  int y = n;\n}\n");
    assert_eq!(
        code_names(&bare),
        vec!["implicit_this_reference_in_initializer"],
        "{:?}",
        rows(&bare)
    );
}

/// An explicit `this` in an instance field initializer is the keyword's
/// own face (`invalid_reference_to_this`, `visitThisExpression`); the
/// member it qualifies never reports the implicit-this face on top.
#[test]
fn explicit_this_in_a_field_initializer_reports_the_keyword_only() {
    let report = report_ops("class C {\n  int x = 0;\n  int y = this.x;\n}\n");
    assert_eq!(
        code_names(&report),
        vec!["invalid_reference_to_this"],
        "{:?}",
        rows(&report)
    );
}
