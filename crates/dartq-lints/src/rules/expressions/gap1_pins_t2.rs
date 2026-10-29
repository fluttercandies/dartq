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

/// The wave posture: the four rules are silent under the plain run
/// (the face absent) and report with it mounted.
#[test]
fn gap1_t2_rules_require_the_semantic_face() {
    let sources = [
        (
            "discarded_futures",
            "Future<void> f() async {}\nvoid g() {\n  f();\n}\n",
        ),
        (
            "unawaited_futures",
            "Future<void> f() async {}\nvoid g() async {\n  f();\n}\n",
        ),
        (
            "unnecessary_null_aware_operator_on_extension_on_nullable",
            "extension E on int? {\n  int get foo => 1;\n}\nvoid f(int? i) {\n  i?.foo;\n}\n",
        ),
        (
            "use_truncating_division",
            "void f(int a, int b) {\n  print((a / b).toInt());\n}\n",
        ),
    ];
    for (rule, source) in sources {
        let plain = Fixture::parse(source);
        assert!(plain.run(&one_rule(rule)).is_empty(), "{rule}");
        assert_eq!(run_semantic(source, rule).len(), 1, "{rule}");
    }
}

/// The sync-context faces: a `Future`-typed drop reports at the
/// selector in a sync body — bare and member invocations — and the
/// `FutureOr` shape reports its paired texts; an `async` body, an
/// `await`, a `var` binding, a `return`, and the wrapper exemption
/// stay silent.
#[test]
fn discarded_futures_sync_faces() {
    let lints = run_semantic(
        "Future<void> f() async {}\nvoid g() {\n  f();\n}\n",
        "discarded_futures",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'Future'-returning calls in a non-'async' function."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(
            "Try either making the enclosing function 'async' and awaiting the expression or wrapping the expression in 'unawaited'."
        )
    );

    // The member-invocation face reports at the method name.
    assert_eq!(
        run_semantic(
            "class C {\n  Future<void> m() async {}\n}\nvoid g(C c) {\n  c.m();\n}\n",
            "discarded_futures"
        )
        .len(),
        1
    );

    let lints = run_semantic(
        "FutureOr<int> h() => 1;\nvoid g() {\n  h();\n}\n",
        "discarded_futures",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "'FutureOr'-returning calls in a non-'async' function."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try making the enclosing function 'async' and awaiting the expression.")
    );

    for source in [
        // The async-body gate (:38-40): `async` bodies belong to
        // `unawaited_futures`.
        "Future<void> f() async {}\nvoid g() async {\n  f();\n}\n",
        // A stored value is not discarded (the assignment escape).
        "Future<void> f() async {}\nvoid g() {\n  var x = f();\n}\n",
        // Neither is a returned one (the return-position dispatch).
        "Future<void> f() async {}\nFuture<void> g() {\n  return f();\n}\n",
    ] {
        assert!(
            run_semantic(source, "discarded_futures").is_empty(),
            "{source}"
        );
    }
}

/// The async-context faces: a `Future`-typed drop reports in an `async`
/// body — the `FutureOr` shape with its paired texts — and a sync body,
/// an `await`, and the nearest-body gate (a sync closure inside an
/// `async` function) stay silent.
#[test]
fn unawaited_futures_async_faces() {
    let lints = run_semantic(
        "Future<void> f() async {}\nvoid g() async {\n  f();\n}\n",
        "unawaited_futures",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Missing an 'await' for the 'Future' computed by this expression."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(
            "Try adding an 'await' or wrapping the expression with 'unawaited' (if it is a 'Future')."
        )
    );

    let lints = run_semantic(
        "FutureOr<int> h() => 1;\nvoid g() async {\n  h();\n}\n",
        "unawaited_futures",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Missing an 'await' for the 'FutureOr' computed by this expression."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding an 'await'.")
    );

    for source in [
        // The sync-body gate (:38-40).
        "Future<void> f() async {}\nvoid g() {\n  f();\n}\n",
        // The awaited shape is not unawaited (the await escape).
        "Future<void> f() async {}\nvoid g() async {\n  await f();\n}\n",
        // The nearest body decides: the statement sits in a sync
        // closure inside the `async` function.
        "Future<void> f() async {}\nvoid g() async {\n  [1].forEach((_) {\n    f();\n  });\n}\n",
    ] {
        assert!(
            run_semantic(source, "unawaited_futures").is_empty(),
            "{source}"
        );
    }
}

/// The extension faces: a null-aware access over a nullable receiver
/// whose selector resolves to a same-file extension member declared
/// `on <T>?` reports at the `?.` — the property and invocation shapes;
/// a non-nullable receiver, a non-nullable `on` type, an unresolvable
/// receiver, an unlisted member, and ordinary class members stay
/// silent.
#[test]
fn unnecessary_null_aware_extension_on_nullable_faces() {
    let source = "extension E on int? {\n  int get foo => 1;\n}\nvoid f(int? i) {\n  i?.foo;\n}\n";
    let lints = run_semantic(
        source,
        "unnecessary_null_aware_operator_on_extension_on_nullable",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary use of a null-aware operator to invoke an extension method on a nullable type."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the '?'.")
    );

    // The invocation face reports too.
    assert_eq!(
        run_semantic(
            "extension E on int? {\n  int foo() => 1;\n}\nvoid f(int? i) {\n  i?.foo();\n}\n",
            "unnecessary_null_aware_operator_on_extension_on_nullable"
        )
        .len(),
        1
    );

    for source in [
        // A non-nullable target keeps the null-aware meaningful (the
        // `isNullable` gate).
        "extension E on int? {\n  int get foo => 1;\n}\nvoid f(int i) {\n  i?.foo;\n}\n",
        // The `on` type must be nullable-marked.
        "extension E on int {\n  int get foo => 1;\n}\nvoid f(int? i) {\n  i?.foo;\n}\n",
        // An unresolvable receiver stays silent (the under-report
        // posture).
        "extension E on int? {\n  int get foo => 1;\n}\nvoid f(dynamic d) {\n  d?.foo;\n}\n",
        // The member must belong to the extension.
        "extension E on int? {\n  int get foo => 1;\n}\nvoid f(int? i) {\n  i?.bar;\n}\n",
        // An ordinary nullable class member access never reports.
        "class C {\n  int get foo => 1;\n}\nvoid f(C? c) {\n  c?.foo;\n}\n",
    ] {
        assert!(
            run_semantic(
                source,
                "unnecessary_null_aware_operator_on_extension_on_nullable"
            )
            .is_empty(),
            "{source}"
        );
    }
}

/// The truncating-division faces: `(a / b).toInt()` over two `int`
/// operands reports at the invocation; an unparenthesized division, a
/// non-`int` operand, another selector, a bare parenthesized
/// division, and `~/` itself stay silent.
#[test]
fn use_truncating_division_faces() {
    let lints = run_semantic(
        "void f(int a, int b) {\n  print((a / b).toInt());\n}\n",
        "use_truncating_division",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Use truncating division.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(
            "Try using truncating division, '~/', instead of regular division ('/') followed by 'toInt()'."
        )
    );

    for source in [
        // The parenthesized-parent requirement (:54-55).
        "void f(int a, int b) {\n  var x = a / b;\n}\n",
        // A non-`int` operand keeps the division (:40-44).
        "void f(double a, double b) {\n  print((a / b).toInt());\n}\n",
        // Another selector is not the shape (:63-65).
        "void f(int a, int b) {\n  print((a / b).round());\n}\n",
        // A bare parenthesized division carries no invocation tail.
        "void f(int a, int b) {\n  print((a / b));\n}\n",
        // Truncating division itself never matches (:37).
        "void f(int a, int b) {\n  print((a ~/ b).toInt());\n}\n",
    ] {
        assert!(
            run_semantic(source, "use_truncating_division").is_empty(),
            "{source}"
        );
    }
}
