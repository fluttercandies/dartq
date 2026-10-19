use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
use dartq_source::{FileTable, StringTable};

use crate::engine::Lint;
use crate::testsupport::{Fixture, one_rule};

/// Runs `rule` over `source` with the semantic face mounted: the
/// source folded as the sole analyzed file of `package:t` — the
/// driver's per-file build shape. The context tables leak
/// (`'static`); the tests are one-shot processes.
fn run_semantic(source: &str, rule: &str) -> Vec<Lint> {
    run_semantic_version(source, rule, LanguageVersion::new(3, 15))
}

/// [`run_semantic`] at an explicit effective library version — the
/// version-gated rules' faces (`future_sync_value`,
/// `unnecessary_this_alias`).
fn run_semantic_version(source: &str, rule: &str, version: LanguageVersion) -> Vec<Lint> {
    let mut fixture = Fixture::parse(source);
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

/// The wave posture: every static-type-consumer rule is silent under
/// the plain run (the face absent) and reports with it mounted.
#[test]
fn gap1_rules_require_the_semantic_face() {
    let source = "void f(dynamic d) {\n  d.foo();\n}\n";
    let plain = Fixture::parse(source);
    for rule in [
        "avoid_dynamic_calls",
        "no_dynamic_casts",
        "avoid_type_to_string",
        "cast_nullable_to_non_nullable",
        "unnecessary_null_checks",
        "unnecessary_unawaited",
    ] {
        assert!(plain.run(&one_rule(rule)).is_empty(), "{rule}");
    }
    assert_eq!(run_semantic(source, "avoid_dynamic_calls").len(), 1);
}

/// The member faces report at the receiver: a `dynamic`-typed read
/// under a method invocation, a property access (statement and write
/// target), and the bare-invocation tear-off face (`dynamic f()`
/// called bare). The exemptions and non-`dynamic` reads stay silent.
#[test]
fn avoid_dynamic_calls_member_and_tear_off_faces() {
    let lints = run_semantic(
        "void f(dynamic d) {\n  d.foo();\n}\n",
        "avoid_dynamic_calls",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Method invocation or property access on a 'dynamic' target."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try giving the target a type.")
    );

    assert_eq!(
        run_semantic("void f(dynamic d) {\n  d.foo;\n}\n", "avoid_dynamic_calls").len(),
        1
    );
    assert_eq!(
        run_semantic(
            "void f(dynamic d) {\n  d.foo = 1;\n}\n",
            "avoid_dynamic_calls"
        )
        .len(),
        1
    );
    // The bare tear-off face: a `dynamic`-typed local variable read as
    // a call reports; a declared function's tear-off is a typed
    // function type and never answers (:174-181).
    assert_eq!(
        run_semantic(
            "void f() {\n  dynamic x = () {};\n  x();\n}\n",
            "avoid_dynamic_calls"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic("void f(dynamic d) {\n  d += 1;\n}\n", "avoid_dynamic_calls").len(),
        1
    );
    assert_eq!(
        run_semantic("void f(dynamic d) {\n  d + 1;\n}\n", "avoid_dynamic_calls").len(),
        1
    );
    assert_eq!(
        run_semantic("void f(dynamic d) {\n  -d;\n}\n", "avoid_dynamic_calls").len(),
        1
    );

    for source in [
        // The every-object exemptions (:103-114, :144-150).
        "void f(dynamic d) {\n  d.toString();\n}\n",
        "void f(dynamic d) {\n  d.hashCode;\n}\n",
        "void f(dynamic d) {\n  d.noSuchMethod(f);\n}\n",
        // `??=` and plain `=` are not dynamic calls (:62-65, the
        // read-type gate).
        "void f(dynamic d) {\n  d ??= 1;\n  d = 1;\n}\n",
        // `==`/`!=` exist on every type (:76-82).
        "void f(dynamic d) {\n  d == 1;\n}\n",
        // The explicit-cast bail (:205-207).
        "void f(dynamic d) {\n  (d as dynamic).foo();\n}\n",
        // A typed receiver never reports.
        "void f(int i) {\n  i.foo();\n}\n",
        // The postfix `!` is not a dynamic call (:136-139).
        "void f(dynamic d) {\n  d!;\n}\n",
    ] {
        assert!(
            run_semantic(source, "avoid_dynamic_calls").is_empty(),
            "{source}"
        );
    }
}

/// The implicit-cast faces: a `dynamic` source into a typed target —
/// a written declaration, an assignment target, a return, and a
/// resolved formal — reports at the source; untyped targets, `Object?`,
/// explicit casts, and bare-call arguments stay silent.
#[test]
fn no_dynamic_casts_typed_context_faces() {
    let lints = run_semantic("void f(dynamic d) {\n  int i = d;\n}\n", "no_dynamic_casts");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Implicit cast from 'dynamic'.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding an explicit cast or changing the target type.")
    );

    assert_eq!(
        run_semantic(
            "void f(dynamic d) {\n  int i;\n  i = d;\n}\n",
            "no_dynamic_casts"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic(
            "int g() {\n  dynamic d;\n  return d;\n}\n",
            "no_dynamic_casts"
        )
        .len(),
        1
    );
    // The member-formal face: `c.m(d)` against `void m(int x)`.
    assert_eq!(
        run_semantic(
            "class C {\n  void m(int x) {}\n}\nvoid f(C c, dynamic d) {\n  c.m(d);\n}\n",
            "no_dynamic_casts"
        )
        .len(),
        1
    );

    for source in [
        // An untyped target carries no context type (:219-221).
        "void f(dynamic d) {\n  var v = d;\n}\n",
        // `Object?` and `dynamic` targets exempt (:257-258).
        "void f(dynamic d) {\n  Object? o = d;\n  dynamic e = d;\n}\n",
        // The explicit-cast bail (:261-263).
        "void f(dynamic d) {\n  int i = d as int;\n}\n",
        // A bare `return;` carries no expression.
        "void g(dynamic d) {\n  return;\n}\n",
        // A `dynamic`-typed formal target is exempt.
        "void f(dynamic d, dynamic e) {\n  e = d;\n}\n",
    ] {
        assert!(
            run_semantic(source, "no_dynamic_casts").is_empty(),
            "{source}"
        );
    }
}

/// The `Type` receiver face: a `Type`-typed receiver's `toString`
/// reports at the method name; other receivers and nullable `Type?`
/// stay silent.
#[test]
fn avoid_type_to_string_type_receiver_face() {
    let lints = run_semantic(
        "void f(Type t) {\n  t.toString();\n}\n",
        "avoid_type_to_string",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Using 'toString' on a 'Type' is not safe in production code."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try a normal type check or compare the 'runtimeType' directly.")
    );

    for source in [
        // A non-`Type` receiver never matches (:98-105).
        "void f(String s) {\n  s.toString();\n}\n",
        // A different member is not the shape.
        "void f(Type t) {\n  t.hashCode;\n}\n",
    ] {
        assert!(
            run_semantic(source, "avoid_type_to_string").is_empty(),
            "{source}"
        );
    }
}

/// The cast face: a nullable source into a non-nullable written target
/// reports; nullable targets, `dynamic` sources, and non-nullable
/// sources stay silent.
#[test]
fn cast_nullable_to_non_nullable_faces() {
    let lints = run_semantic(
        "void f(int? n) {\n  n as int;\n}\n",
        "cast_nullable_to_non_nullable",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Don't cast a nullable value to a non-nullable type."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding a not-null assertion ('!') to make the type non-nullable.")
    );

    for source in [
        // A nullable target needs no assertion (:45-46).
        "void f(int? n) {\n  n as int?;\n}\n",
        // `dynamic` sources are excluded (:41-42).
        "void f(dynamic d) {\n  d as int;\n}\n",
        // A non-nullable source never reports (:44).
        "void f(int n) {\n  n as int;\n}\n",
    ] {
        assert!(
            run_semantic(source, "cast_nullable_to_non_nullable").is_empty(),
            "{source}"
        );
    }
}

/// The context faces: a `!` under a nullable written type (declaration
/// initializer or return) reports at the operator; a non-nullable
/// context keeps the check.
#[test]
fn unnecessary_null_checks_nullable_context_faces() {
    let lints = run_semantic(
        "void f(int? n) {\n  int? y = n!;\n}\n",
        "unnecessary_null_checks",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary use of a null check ('!').");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the null check.")
    );

    assert_eq!(
        run_semantic(
            "int? g(int? n) {\n  return n!;\n}\n",
            "unnecessary_null_checks"
        )
        .len(),
        1
    );

    // The arrow body and the transparent `(...)` wrapper (`n!`'s
    // nearest non-paren ancestor chain) report the same way.
    assert_eq!(
        run_semantic("int? f(int? n) => n!;\n", "unnecessary_null_checks").len(),
        1
    );
    assert_eq!(
        run_semantic(
            "void f(int? n) {\n  int? y = (n!);\n}\n",
            "unnecessary_null_checks"
        )
        .len(),
        1
    );
    // A field initializer is the same VariableDeclaration face.
    assert_eq!(
        run_semantic(
            "class C {\n  int? y = first()!;\n}\n",
            "unnecessary_null_checks"
        )
        .len(),
        1
    );
    // The async flatten unwraps the `Future` wrapper through its first
    // type argument; without the marker the wrapper itself decides.
    assert_eq!(
        run_semantic(
            "Future<int?> f(int? n) async => n!;\n",
            "unnecessary_null_checks"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic("Future<int?> f(int? n) => n!;\n", "unnecessary_null_checks").len(),
        0
    );

    // The official lookup is `thisOrAncestorOfType<FunctionExpression>`
    // (:41): class methods never satisfy it, so their return and arrow
    // faces stay silent exactly like the oracle.
    for source in [
        "class C {\n  int? m(int? n) => n!;\n}\n",
        "class C {\n  int? m(int? n) {\n    return n!;\n  }\n}\n",
    ] {
        assert!(
            run_semantic(source, "unnecessary_null_checks").is_empty(),
            "{source}"
        );
    }

    for source in [
        // The check is required under a non-nullable context.
        "void f(int? n) {\n  int y = n!;\n}\n",
        "int g(int? n) {\n  return n!;\n}\n",
        // An untyped binding carries no context face.
        "void f(int? n) {\n  var y = n!;\n}\n",
        // The `!` is not in a direct context position.
        "void f(int? n) {\n  int? y = n!.foo();\n}\n",
        // An argument position carries the callee's parameter type,
        // not the declaration's (:132-159).
        "void f(int? n, int g(int? x)) {\n  int? y = g(n!);\n}\n",
    ] {
        assert!(
            run_semantic(source, "unnecessary_null_checks").is_empty(),
            "{source}"
        );
    }
}

/// The `unawaited` face: a bare core-`unawaited` wrapper over a
/// non-`Future` expression reports at the name; a `Future`-typed
/// argument and a same-file shadow of the name stay silent.
#[test]
fn unnecessary_unawaited_not_future_faces() {
    let lints = run_semantic(
        "int g() => 1;\nvoid f() {\n  unawaited(g());\n}\n",
        "unnecessary_unawaited",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary use of 'unawaited'.");

    for source in [
        // A `Future`-typed argument keeps the wrapper.
        "Future<void> g() async {}\nvoid f() {\n  unawaited(g());\n}\n",
        // A same-file declaration of the name is not dart:async's.
        "void unawaited(int x) {}\nint g() => 1;\nvoid f() {\n  unawaited(g());\n}\n",
        // The prefixed shape skips (:38).
        "int g() => 1;\nvoid f(p p) {\n  p.unawaited(g());\n}\n",
    ] {
        assert!(
            run_semantic(source, "unnecessary_unawaited").is_empty(),
            "{source}"
        );
    }
}
