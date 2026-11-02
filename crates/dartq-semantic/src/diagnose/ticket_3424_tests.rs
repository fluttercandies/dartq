//! #3424 pins — the untyped collection literal's own static type under a
//! declared context its head cannot match.
//!
//! # The defect
//!
//! An unannotated `[...]`/`{...}` literal pushes an opaque row (`Unknown`),
//! so a declaration initializer or an assignment whose target type the
//! literal cannot adopt asked no assignability question at all:
//! `C<int> c = {3, 1, 4};` over `C<T> implements LinkedHashSet<T>` stayed
//! silent where the oracle reports "A value of type 'Set<int>' can't be
//! assigned to a variable of type 'C<int>'".
//!
//! # The contract (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/typed_literal_resolver.dart)
//!
//! The downward constraint `L<E> <: K` (the literal's own head against the
//! context's `futureOrBase`) matches only when `K` is a top face or one of
//! `L`'s supertypes (`List`/`Set` reach `Iterable`; everything reaches
//! `Object`) — then the literal is assignable by construction. Otherwise
//! the leaves alone infer the arguments (their least upper bound, `dynamic`
//! with no leaves), an empty `{}` is a set exactly when `K` implements
//! `Iterable` and not `Map`, and the resulting type is checked against the
//! declared one. A leaf whose exact static type this layer cannot spell
//! refuses the face (the message prints the type, so a guess would
//! misprint it).

use super::test_rig::{CORE_PRELUDE, report_ops};

/// Every `invalid_assignment` row of `source` as `(anchored text,
/// message)`, in source order. The fixtures are ASCII, so the UTF-16
/// ranges index the prelude-prefixed text directly.
fn assignment_rows(source: &str) -> Vec<(String, String)> {
    let report = report_ops(source);
    let text = format!("{CORE_PRELUDE}{source}");
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| {
            let start = d.range.offset as usize;
            let end = start + d.range.length as usize;
            (text[start..end].to_string(), d.message.to_string())
        })
        .collect()
}

fn row(anchor: &str, from: &str, to: &str) -> (String, String) {
    (
        anchor.to_string(),
        format!("A value of type '{from}' can't be assigned to a variable of type '{to}'."),
    )
}

/// The `LinkedHashSet` subclass of the co19 `exact_types_of_literals`
/// family: a proper `Set` subtype no set literal can ever be.
const SET_SUBCLASS: &str = "abstract class LinkedHashSet<E> implements Set<E> {}\n\
     abstract class C<T> implements LinkedHashSet<T> {}\n";

/// A non-const set literal against a `Set` subclass reads the leaves'
/// upward type — a trailing comma closes the last element, a raw context
/// displays instantiated to bounds, and a `dynamic` leaf absorbs the join.
#[test]
fn set_literal_against_a_set_subclass_reads_the_leaves_type() {
    let source = format!(
        "{SET_SUBCLASS}void main() {{\n  C<int> c2 = {{3, 1, 4,}};\n  C c3 = {{3, 1, 4,}};\n  \
         dynamic d;\n  C c4 = {{3, 1, 4, null, d}};\n}}\n"
    );
    assert_eq!(
        assignment_rows(&source),
        [
            row("{3, 1, 4,}", "Set<int>", "C<int>"),
            row("{3, 1, 4,}", "Set<int>", "C<dynamic>"),
            row("{3, 1, 4, null, d}", "Set<dynamic>", "C<dynamic>"),
        ]
    );
}

/// The const twins anchor over the `const` keyword; a `null` leaf re-arms
/// `?` on the join, and a type literal beside `int` joins at `Object`.
#[test]
fn const_set_leaves_join_nullable_and_object() {
    let source = format!(
        "{SET_SUBCLASS}void main() {{\n  C c4 = const {{3, 1, 4,null, }};\n  \
         LinkedHashSet c8 = const {{3, 1, 4, null, bool}};\n}}\n"
    );
    assert_eq!(
        assignment_rows(&source),
        [
            row("const {3, 1, 4,null, }", "Set<int?>", "C<dynamic>"),
            row(
                "const {3, 1, 4, null, bool}",
                "Set<Object?>",
                "LinkedHashSet<dynamic>"
            ),
        ]
    );
}

/// An empty `{}` under a context implementing `Iterable` (and not `Map`) is
/// a set literal: the failed downward match leaves `Set<dynamic>`, whatever
/// the context's own argument.
#[test]
fn empty_braces_take_the_set_side_of_an_iterable_context() {
    let source = "abstract class SplayTreeSet<E> implements Set<E> {}\n\
                  void main() {\n  SplayTreeSet s1 = {};\n  SplayTreeSet<int> s2 = {};\n}\n";
    assert_eq!(
        assignment_rows(source),
        [
            row("{}", "Set<dynamic>", "SplayTreeSet<dynamic>"),
            row("{}", "Set<dynamic>", "SplayTreeSet<int>"),
        ]
    );
}

/// A type-parameter context decides the side through its bound and never
/// matches the literal's head (a set is no subtype of `T`).
#[test]
fn empty_braces_read_a_type_parameter_through_its_bound() {
    let source = "class D<T extends Iterable<Object>> {\n  T t = {};\n}\n";
    assert_eq!(
        assignment_rows(source),
        [row("{}", "Set<dynamic>", "T")]
    );
}

/// The assignment face anchors the whole literal — its first row is the
/// first leaf, never the opening brace.
#[test]
fn assignment_face_anchors_the_whole_literal() {
    let source = format!("{SET_SUBCLASS}void f(C<int> c) {{\n  c = {{1, 2}};\n}}\n");
    assert_eq!(
        assignment_rows(&source),
        [row("{1, 2}", "Set<int>", "C<int>")]
    );
}

/// The list and map families mirror the set face, an empty `{}` under a
/// `Map` implementer (or a non-collection context) is a map literal, and a
/// non-collection context reports the literal's own type — the empty `[]`
/// anchors over both brackets (its row is a zero-width `[`), `const`
/// included.
#[test]
fn list_and_map_families_and_non_collection_contexts() {
    let source = "abstract class L<E> implements List<E> {}\n\
                  abstract class M<K, V> implements Map<K, V> {}\n\
                  class A {}\n\
                  void main() {\n  L<int> l = [1, 2];\n  M<String, int> m = {'a': 1};\n  \
                  M e = {};\n  A a = {};\n  String s = [1];\n  String t = [];\n  \
                  String u = const [];\n}\n";
    assert_eq!(
        assignment_rows(source),
        [
            row("[1, 2]", "List<int>", "L<int>"),
            row("{'a': 1}", "Map<String, int>", "M<String, int>"),
            row("{}", "Map<dynamic, dynamic>", "M<dynamic, dynamic>"),
            row("{}", "Map<dynamic, dynamic>", "A"),
            row("[1]", "List<int>", "String"),
            row("[]", "List<dynamic>", "String"),
            row("const []", "List<dynamic>", "String"),
        ]
    );
}

/// Contexts the literal's head matches — the top faces, `Object`, the
/// literal's own head, `Iterable` for lists and sets, a `FutureOr` around
/// one — type the literal downward and ask nothing (`Object? y; y = [];`
/// keeps its write promotion untouched).
#[test]
fn adopting_contexts_stay_silent() {
    let source = "void main() {\n  Object? x = [];\n  Object? y;\n  y = [];\n  \
                  Object o = {};\n  dynamic dd = {1};\n  Iterable<int> i = {};\n  \
                  Iterable<num> n = [1];\n  Set<int> s = {1, 2,};\n  List<int> l = [1, 2,];\n  \
                  Map<int, int> m = {};\n  FutureOr<List<int>> f = [];\n}\n";
    let rows = assignment_rows(source);
    assert!(rows.is_empty(), "{rows:#?}");
}

/// Leaves without an exact face refuse the whole question — user classes
/// whose shared supertype needs the ranked walk keep their under-report
/// (a guessed element type never misprints the message). A spread element
/// DOES type: the oracle reports `Set<int>` for `{...xs}` over
/// `List<int>` (probed: dart analyze 3.13.7 `INVALID_ASSIGNMENT`), so
/// the literal's own `Set<int>` answers the assignability question the
/// declared `C<int>` fails.
#[test]
fn leaves_without_an_exact_face_refuse() {
    let source = format!(
        "{SET_SUBCLASS}abstract class B {{}}\nclass B1 extends B {{}}\nclass B2 extends B {{}}\n\
         void main(List<int> xs) {{\n  C<int> spread = {{...xs}};\n  \
         C<B> mixed = {{B1(), B2()}};\n}}\n"
    );
    let rows = assignment_rows(&source);
    assert_eq!(
        rows,
        [row("{...xs}", "Set<int>", "C<int>")],
        "{rows:#?}"
    );
}
