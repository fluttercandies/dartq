//! The lint pack-C wave's production-bridge pins: the verdict shapes
//! the conformance sweep flagged red — the send-group piece starvation
//! (`no_dynamic_casts` condition/collection/forEach faces), the void
//! acceptability truth table (`void_checks`), the explicit-`.call` and
//! tear-off-instantiation faces (`implicit_call_tearoffs`), and the
//! `typesAreUnrelated` matrix (`unrelated_type_equality_checks`).

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
use dartq_source::{FileTable, StringTable};

use crate::engine::Lint;
use crate::testsupport::{Fixture, one_rule};

/// Runs `rule` over `source` with the semantic face mounted — the same
/// single-file `package:t` build shape as `gap1_pins`.
fn run_semantic(source: &str, rule: &str) -> Vec<Lint> {
    let mut fixture = Fixture::parse(source);
    let version = LanguageVersion::new(3, 15);
    fixture.version = Some(version);
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("lint_semantic.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(version));
    let scanned = dartq_scanner::scan(source, crate::testsupport::CONFIG, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/lint_semantic.dart");
    for (name, declaration) in elements.declarations() {
        if declaration.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
    let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
    let index: &'static ElementIndex = Box::leak(Box::new(index));
    let analysis = analyze_unit(
        source,
        &scanned.tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
    );
    let unit = crate::engine::Unit {
        source: &fixture.source,
        tokens: &fixture.tokens,
        ast: &fixture.ast,
        lines: &fixture.lines,
        path: Some(&fixture.path),
        features: fixture.features,
        version: fixture.version,
        workspace: None,
        reachability: None,
        async_context_uses: None,
        library_type_uses: None,
    };
    unit.run_with_semantics(
        &one_rule(rule),
        &rustc_hash::FxHashSet::default(),
        Some(&analysis),
    )
}

/// `no_dynamic_casts`: the send-group pieces of a condition operand
/// tile as the container's siblings — the faces assemble the window.
#[test]
fn no_dynamic_casts_condition_and_collection_faces() {
    for source in [
        "void f(dynamic a) {\n  if (a) {}\n}\n",
        "void f(dynamic a) {\n  while (a) {}\n}\n",
        "void f(dynamic a) {\n  do {} while (a);\n}\n",
        "void f(dynamic a) {\n  for (; a;) {}\n}\n",
        "void f(dynamic a) {\n  a ? 1 : 2;\n}\n",
        "void f(dynamic a) {\n  [if (a) 7];\n}\n",
        "void f(dynamic a) {\n  var list = <int>[a];\n}\n",
        "void f(dynamic a) {\n  var list = <int>[...a];\n}\n",
        "void f(dynamic a) {\n  var set = <int>{a};\n}\n",
        "void f(dynamic a) {\n  var map = <int, String>{a: 'x'};\n}\n",
        "void f(dynamic a) {\n  var map = <String, int>{'x': a};\n}\n",
        "void f(dynamic a) {\n  var map = <String, int>{...a};\n}\n",
        "void f(dynamic a) {\n  for (var x in a) {}\n}\n",
        "int f(dynamic a) => a;\n",
    ] {
        assert_eq!(
            run_semantic(source, "no_dynamic_casts").len(),
            1,
            "{source}"
        );
    }
}

/// `no_dynamic_casts`: the `a && b` right operand reports at `b` alone —
/// the container's covered span anchors at the operator, so the report
/// opens at the run's earliest head token.
#[test]
fn no_dynamic_casts_logical_right_span() {
    let lints = run_semantic(
        "void f(bool a, dynamic b) {\n  a && b;\n}\n",
        "no_dynamic_casts",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!((lints[0].range.end - lints[0].range.start), 1, "{lints:?}");
}

/// `void_checks`: `dynamic` is not void-acceptable upstream
/// (:55-65 has no `DynamicType` arm) — a `void` formal rejects a
/// `dynamic` argument, and a plain `void` return reports the dynamic
/// value only.
#[test]
fn void_checks_dynamic_argument_and_return() {
    for source in [
        "void m(void arg) {}\nvoid f(dynamic p) {\n  m(p);\n}\n",
        "void m([void v]) {}\nvoid f(dynamic p) {\n  m(p);\n}\n",
        "void m({required void p}) {}\nvoid f(dynamic p) {\n  m(p: p);\n}\n",
        "void f(dynamic p) {\n  return p;\n}\n",
    ] {
        assert_eq!(run_semantic(source, "void_checks").len(), 1, "{source}");
    }
    // A non-dynamic value returned under `void` stays silent
    // (:142-146).
    assert!(
        run_semantic("void f() {\n  return 1;\n}\n", "void_checks").is_empty()
    );
}

/// `void_checks`: a member write (`a.x = 1` over `FutureOr<void>`)
/// reports the whole assignment; a `[p] = …` pattern reports the
/// void-typed variable itself.
#[test]
fn void_checks_member_write_and_pattern() {
    assert_eq!(
        run_semantic(
            "import 'dart:async';\nvoid f(A a) {\n  a.x = 1;\n}\nclass A {\n  FutureOr<void> x;\n  A(this.x);\n}\n",
            "void_checks",
        )
        .len(),
        1
    );
    let lints = run_semantic(
        "void f() {\n  void p;\n  [p] = <int>[7];\n}\n",
        "void_checks",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!((lints[0].range.end - lints[0].range.start), 1, "{lints:?}");
}

/// `implicit_call_tearoffs`: the explicit `.call` selector is the
/// spelled tear-off — silent; the `c<T>`/`C()<T>` instantiation is the
/// implicit `.call` instantiation and reports context-free.
#[test]
fn implicit_call_tearoffs_explicit_and_instantiation() {
    for source in [
        "void Function() f() => C().call;\nclass C {\n  void call() {}\n}\n",
        "class Callable {\n  void call() {}\n}\nvoid callIt(void Function() f) {\n  f();\n}\nvoid f() {\n  callIt(Callable().call);\n}\n",
    ] {
        assert!(
            run_semantic(source, "implicit_call_tearoffs").is_empty(),
            "{source}"
        );
    }
    for source in [
        "void f(C c) {\n  c<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
        "void f() {\n  C()<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
        "void f(C c) {\n  Function fn = c<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
        "Function f = C()<int>;\nclass C {\n  void call<T>(T arg) {}\n}\n",
    ] {
        assert_eq!(
            run_semantic(source, "implicit_call_tearoffs").len(),
            1,
            "{source}"
        );
    }
}

/// `implicit_call_tearoffs`: the collection-element context supplies
/// the function-type target (`<void Function()>[c]`,
/// `List<void Function()> f = [c]`).
#[test]
fn implicit_call_tearoffs_collection_element_context() {
    for source in [
        "void f(C c) {\n  <void Function()>[c];\n}\nclass C {\n  void call() {}\n}\n",
        "List<void Function()> f(C c) {\n  return [c];\n}\nclass C {\n  void call() {}\n}\n",
    ] {
        assert_eq!(
            run_semantic(source, "implicit_call_tearoffs").len(),
            1,
            "{source}"
        );
    }
}

/// `unrelated_type_equality_checks`: the upstream matrix — type
/// variables need proved-unrelated bounds, enums and shared-super
/// classes only report through the direct-supertype matrix, records
/// compare by shape.
#[test]
fn unrelated_type_equality_matrix() {
    for source in [
        // Unbounded variables and shared bounds stay silent (:200-203).
        "void f<A, B>(A a, B b) {\n  if (a == b) {}\n}\n",
        "void f<A extends int, B extends int>(A a, B b) {\n  if (a == b) {}\n}\n",
        "void f<A, B extends A>(A a, B b) {\n  if (a == b) {}\n}\n",
        // Same-shaped records and shared direct supers stay silent.
        "void f(D1 x, E1 y) {\n  if (x == y) {}\n}\nclass C {}\nclass D1 extends C {}\nclass E1 extends C {}\n",
        "bool f((int, int) a, (int, int) b) => a == b;\n",
        // Same-representation extension types erase to `String` — silent.
        "void m(A a, B b) {\n  if (a == b) {}\n}\nextension type A(String value) {}\nextension type B(String value) {}\n",
    ] {
        assert!(
            run_semantic(source, "unrelated_type_equality_checks").is_empty(),
            "{source}"
        );
    }
    for source in [
        // Distinct enums, records of a different shape, record vs
        // interface, unrelated-bounded `List<A>`/`List<B>`, and the
        // shared-distant-supertype pair all report.
        "void f(E x, F y) {\n  if (x == y) {}\n}\nenum E { one, two }\nenum F { three, four }\n",
        "bool f((int, int) a, (String, String) b) => a == b;\n",
        "bool f((int, int) a, String b) => a == b;\n",
        "void f<A extends int, B extends bool>(List<A> a, List<B> b) {\n  if (a == b) {}\n}\n",
        "void f(D2 x, E2 y) {\n  if (x == y) {}\n}\nclass C {}\nclass D1 extends C {}\nclass E1 extends C {}\nclass D2 extends D1 {}\nclass E2 extends E1 {}\n",
        "void m(A a, B b) {\n  if (a == b) {}\n}\nextension type A(int value) {}\nextension type B(String value) {}\n",
    ] {
        assert_eq!(
            run_semantic(source, "unrelated_type_equality_checks").len(),
            1,
            "{source}"
        );
    }
}
