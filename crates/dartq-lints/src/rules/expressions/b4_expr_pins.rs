use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder,
    Namespaces,
};
use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
use dartq_source::{FileTable, StringTable};

use crate::engine::Lint;
use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// Runs `rule` over `source` with the semantic face mounted: the
/// source folded as the sole analyzed file of `package:t` — the
/// driver's per-file build shape. The context tables leak
/// (`'static`); the tests are one-shot processes.
fn run_semantic(source: &str, rule: &str) -> Vec<Lint> {
    run_semantic_dep(source, None, rule)
}

/// [`run_semantic`] with a dependency file declared and imported
/// unprefixed into the library — the cross-file closure faces.
fn run_semantic_dep(source: &str, dependency: Option<&str>, rule: &str) -> Vec<Lint> {
    let version = LanguageVersion::new(3, 15);
    let mut fixture = Fixture::parse(source);
    fixture.version = Some(version);
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("lint_b4.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(version));
    let scanned = dartq_scanner::scan(source, crate::testsupport::CONFIG, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/lint_b4.dart");
    for (name, declaration) in elements.declarations() {
        if declaration.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    if let Some(dependency) = dependency {
        let dep_file = files.intern(std::path::Path::new("kit.dart"));
        let scanned_dep = dartq_scanner::scan(dependency, crate::testsupport::CONFIG, dep_file);
        assert!(
            scanned_dep.diagnostics.is_empty(),
            "scanner diagnostics: {dependency:?}"
        );
        let dep_elements = collect(dependency, &scanned_dep.tokens, features, strings);
        let dep_lib = builder.intern("package:t/kit.dart");
        for (name, declaration) in dep_elements.declarations() {
            if declaration.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(dep_lib, name, dep_file);
        }
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        index.insert(dep_file, dep_elements);
    }
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
fn b4_rules_require_the_semantic_face() {
    let sources = [
        (
            "literal_only_boolean_expressions",
            "void f() {\n  if (1 == 1) {}\n}\n",
        ),
        (
            "noop_primitive_operations",
            "void f(String x) {\n  x.toString();\n}\n",
        ),
        ("unnecessary_statements", "void f() {\n  1 + 2;\n}\n"),
        ("prefer_int_literals", "double x = 1.0;\n"),
    ];
    for (rule, source) in sources {
        let plain = Fixture::parse(source);
        assert!(plain.run(&one_rule(rule)).is_empty(), "{rule}");
        assert_eq!(run_semantic(source, rule).len(), 1, "{rule}");
    }
}

/// `literal_only_boolean_expressions`: the constant-condition faces —
/// `while (true)` passes, a non-constant condition passes, the `do`,
/// `for`, and `when` faces report at their anchors.
#[test]
fn literal_only_condition_faces() {
    let source = "void f() {\n  if (1 == 1) {}\n}\n";
    let lints = run_semantic(source, "literal_only_boolean_expressions");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The Boolean expression has a constant value."
    );
    // The report anchors at the whole `if` statement.
    let f = Fixture::parse(source);
    let start = tok(source, &f.tokens, "if", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    assert_eq!(
        run_semantic(
            "void f() {\n  do {} while (!true);\n}\n",
            "literal_only_boolean_expressions"
        )
        .len(),
        1
    );
    // The `for` face reports at the whole statement: the body's closing
    // token seals in (oracle span length 15 — the `}` included).
    let source = "void f() {\n  for (;true;) {}\n}\n";
    let lints = run_semantic(source, "literal_only_boolean_expressions");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(source, &f.tokens, "for", 0))
    );
    assert_eq!(
        lints[0].range.end,
        utf16_at(&f, tok(source, &f.tokens, "}", 0)) + 1
    );
    // A non-block body ends at its own terminator (oracle span length 12).
    let source = "void f() {\n  for (;1;) 2;\n}\n";
    let lints = run_semantic(source, "literal_only_boolean_expressions");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    assert_eq!(
        lints[0].range.end,
        utf16_at(&f, tok(source, &f.tokens, ";", 2)) + 1
    );
    // The `when` guard face.
    assert_eq!(
        run_semantic(
            "void f(int x) {\n  switch (x) {\n    case 1 when true:\n      break;\n  }\n}\n",
            "literal_only_boolean_expressions"
        )
        .len(),
        1
    );

    for source in [
        // `while (true)` passes (dart-lang/linter#453).
        "void f() {\n  while (true) {}\n}\n",
        // A non-constant condition passes.
        "void f() {\n  bool b = true;\n  if (b) {}\n}\n",
        // The `??` face reads the left operand alone — non-literal.
        "void f(bool? b) {\n  if (b ?? true) {}\n}\n",
    ] {
        assert!(
            run_semantic(source, "literal_only_boolean_expressions").is_empty(),
            "{source}"
        );
    }
}

/// The string constant-value gate (:23): a plain string counts, an
/// interpolated one counts only when every interpolation is itself
/// literal-only.
#[test]
fn literal_only_string_gate_faces() {
    // A plain string literal is a valued constant (`'' == ''` reports).
    assert_eq!(
        run_semantic(
            "void f() {\n  if ('a' == 'b') {}\n}\n",
            "literal_only_boolean_expressions"
        )
        .len(),
        1
    );
    // A non-constant interpolation is not — the condition stays silent.
    assert!(
        run_semantic(
            "void f(String x) {\n  if ('${x}' == 'b') {}\n}\n",
            "literal_only_boolean_expressions"
        )
        .is_empty()
    );
    // A literal-only interpolation keeps the constant-valued string.
    assert_eq!(
        run_semantic(
            "void f() {\n  if ('${1}' == 'b') {}\n}\n",
            "literal_only_boolean_expressions"
        )
        .len(),
        1
    );
}

/// The `is` and case-clause faces: a resolved type recurses into the
/// operand, a type-parameter type keeps the condition silent, and an
/// `if` carrying a case clause never reports.
#[test]
fn literal_only_is_and_case_clause_faces() {
    // A resolved core type recurses: the literal operand reports.
    assert_eq!(
        run_semantic(
            "void f() {\n  if (1 is int) {}\n}\n",
            "literal_only_boolean_expressions"
        )
        .len(),
        1
    );
    // A type-parameter type bails (:36).
    assert!(
        run_semantic(
            "void f<T>(int x) {\n  if (x is T) {}\n}\n",
            "literal_only_boolean_expressions"
        )
        .is_empty()
    );
    // The case-clause skip (:86) — the condition never walks.
    assert!(
        run_semantic(
            "void f(int x) {\n  if (x case 1) {}\n}\n",
            "literal_only_boolean_expressions"
        )
        .is_empty()
    );
}

/// `noop_primitive_operations`: the typed receiver families report at
/// the member name — the `String` face non-nullable, the `int`
/// invariants and `double.toDouble()` over nullable receivers too
/// (upstream reads no null gate there) — and the resolved chain
/// receiver reports.
#[test]
fn noop_primitive_typed_receiver_faces() {
    let source = "void f(String x) {\n  x.toString();\n}\n";
    let lints = run_semantic(source, "noop_primitive_operations");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The expression has no effect and can be removed."
    );
    let f = Fixture::parse(source);
    let start = tok(source, &f.tokens, "toString", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    assert_eq!(
        run_semantic(
            "void f(int x) {\n  x.toInt();\n  x.round();\n  x.ceil();\n  x.floor();\n  x.truncate();\n}\n",
            "noop_primitive_operations"
        )
        .len(),
        5
    );
    assert_eq!(
        run_semantic(
            "void f(double x) {\n  x.toDouble();\n}\n",
            "noop_primitive_operations"
        )
        .len(),
        1
    );
    // The chain receiver resolves through the member walk.
    assert_eq!(
        run_semantic(
            "class C {\n  String get s => '';\n}\nvoid f(C c) {\n  c.s.toString();\n}\n",
            "noop_primitive_operations"
        )
        .len(),
        1
    );
    // A literal receiver decides closed.
    assert_eq!(
        run_semantic(
            "void f() {\n  'a'.toString();\n}\n",
            "noop_primitive_operations"
        )
        .len(),
        1
    );

    for source in [
        // A nullable receiver is meaningful for `toString`.
        "void f(String? x) {\n  x.toString();\n}\n",
        // `num` is not `int`.
        "void f(num x) {\n  x.toInt();\n}\n",
        // An unresolved receiver stays silent.
        "void f(dynamic x) {\n  x.toString();\n}\n",
    ] {
        assert!(
            run_semantic(source, "noop_primitive_operations").is_empty(),
            "{source}"
        );
    }
}

/// The `print` descent and the interpolation face report at the inner
/// `toString`; a shadowing declaration, a plain argument, and `super`
/// stay silent.
#[test]
fn noop_primitive_descent_faces() {
    let source = "void f(Object x) {\n  print(x.toString());\n}\n";
    let lints = run_semantic(source, "noop_primitive_operations");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    let start = tok(source, &f.tokens, "toString", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    assert_eq!(
        run_semantic(
            "void f() {\n  print(null.toString());\n}\n",
            "noop_primitive_operations"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic(
            "void f() {\n  var s = '${1.toString()}';\n}\n",
            "noop_primitive_operations"
        )
        .len(),
        1
    );
    // A same-file `print` shadows the core function (the binding gate).
    assert!(
        run_semantic(
            "void print(Object? o) {}\nvoid f(Object x) {\n  print(x.toString());\n}\n",
            "noop_primitive_operations"
        )
        .is_empty()
    );

    for source in [
        "void f() {\n  print(7);\n}\n",
        "class C {\n  String m() => super.toString();\n}\n",
    ] {
        assert!(
            run_semantic(source, "noop_primitive_operations").is_empty(),
            "{source}"
        );
    }
}

/// The middle empty literal of an adjacent-strings group reports at
/// itself; the ends pass, and a non-empty middle passes.
#[test]
fn noop_primitive_adjacent_empty_middle_faces() {
    let source = "void f(String x) {\n  x = 'a' '' 'b';\n}\n";
    let lints = run_semantic(source, "noop_primitive_operations");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    let start = tok(source, &f.tokens, "''", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    for source in [
        "void f(String x) {\n  x = '' 'a' 'b';\n}\n",
        "void f(String x) {\n  x = 'a' 'b' '';\n}\n",
        "void f(String x) {\n  x = 'a' 'nonempty' 'b';\n}\n",
    ] {
        assert!(
            run_semantic(source, "noop_primitive_operations").is_empty(),
            "{source}"
        );
    }
}

/// The ratchet pin: the fixture-style source reports the full
/// nine-row shape set in one function — the five `int` invariants,
/// the non-nullable `String` `toString`, the `double` `toDouble`, the
/// adjacent-strings middle `''`, and the `print` descent (the `Object`
/// receiver keeps the typed face silent, so only the descent fires).
#[test]
fn noop_primitive_fixture_shape_set() {
    let source = concat!(
        "void f(int x, String s, double d, Object o) {\n",
        "  var a = x.toInt();\n",
        "  var b = x.round();\n",
        "  var c = x.ceil();\n",
        "  var e = x.floor();\n",
        "  var g = x.truncate();\n",
        "  var h = s.toString();\n",
        "  var i = d.toDouble();\n",
        "  var j = 'a' '' 'b';\n",
        "  print(o.toString());\n",
        "}\n",
    );
    let lints = run_semantic(source, "noop_primitive_operations");
    assert_eq!(lints.len(), 9, "{lints:?}");
    for lint in &lints {
        assert_eq!(
            lint.message,
            "The expression has no effect and can be removed."
        );
    }
    // The reported spans are the member names, the middle empty
    // literal, and the descent's inner `toString` — never a statement
    // head.
    let f = Fixture::parse(source);
    let mut expected: Vec<u32> = [
        ("toInt", 0),
        ("round", 0),
        ("ceil", 0),
        ("floor", 0),
        ("truncate", 0),
        ("toString", 0),
        ("toDouble", 0),
        ("''", 0),
        ("toString", 1),
    ]
    .into_iter()
    .map(|(text, nth)| {
        let start = tok(source, &f.tokens, text, nth);
        utf16_at(&f, start)
    })
    .collect();
    let mut starts: Vec<u32> = lints.iter().map(|lint| lint.range.start).collect();
    expected.sort_unstable();
    starts.sort_unstable();
    assert_eq!(starts, expected);
}

/// `unnecessary_statements`: the effect-free faces report at the
/// expression; the getter exemptions and invocations stay silent; a
/// type-parameter name is not a getter and reports.
#[test]
fn unnecessary_statements_faces() {
    let source = "void f() {\n  1 + 2;\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary statement.");
    let f = Fixture::parse(source);
    let start = tok(source, &f.tokens, "1", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    // A field read through a typed receiver reports.
    assert_eq!(
        run_semantic(
            "class C {\n  int x = 1;\n}\nvoid f(C c) {\n  c.x;\n}\n",
            "unnecessary_statements"
        )
        .len(),
        1
    );
    // A cross-file class field read reports (the semantic member walk).
    assert_eq!(
        run_semantic_dep(
            "void f(K c) {\n  c.x;\n}\n",
            Some("class K {\n  int x = 1;\n}\n"),
            "unnecessary_statements"
        )
        .len(),
        1
    );
    // A type-parameter name is not a getter — upstream's `visitNode`
    // verdict.
    assert_eq!(
        run_semantic("void f<T>(T x) {\n  T;\n}\n", "unnecessary_statements").len(),
        1
    );

    // A statement-level binary reports the whole expression opening at
    // the operand head — `a.b + y` opens at `a`, never at the `+` tile
    // anchor the container row opens on.
    let source = "class C {\n  int b = 1;\n}\nvoid f(C a, int y) {\n  a.b + y;\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    let a = tok(source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a), "start at `a`");

    for source in [
        "void f() {\n  print(1);\n}\n",
        // An origin getter read keeps the getter face.
        "int get g => 1;\nvoid f() {\n  g;\n}\n",
        // A cross-file getter stays silent (the conservative family).
        "void f(K k) {\n  k.g;\n}\n",
        // An assignment has a clear effect.
        "void f(int x) {\n  x = 1;\n}\n",
    ] {
        let lints = match source {
            "void f(K k) {\n  k.g;\n}\n" => run_semantic_dep(
                source,
                Some("class K {\n  int get g => 1;\n}\n"),
                "unnecessary_statements",
            ),
            _ => run_semantic(source, "unnecessary_statements"),
        };
        assert!(lints.is_empty(), "{source}: {lints:?}");
    }
}

/// The classic-`for` header faces (:209-217): the initialization and the
/// updaters walk separately off the statement window (the parts leaf seals
/// childless after the whole header), the header condition's wrapper row
/// stays silent (upstream reads the initialization and the updaters only),
/// and a non-block body statement still reports.
#[test]
fn unnecessary_statements_for_header_faces() {
    // Both header expressions report, each at its own operand head.
    let source = "void f(int i, bool b) {\n  for (1 + 2; b; 1 + 2) {\n    i = 1;\n  }\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 2, "{lints:?}");
    let f = Fixture::parse(source);
    for (lint, nth) in lints.iter().zip([0, 1]) {
        let one = tok(source, &f.tokens, "1", nth);
        assert_eq!(lint.range.start, utf16_at(&f, one), "operand {nth}");
    }

    // An updater write has a clear effect; the condition stays silent.
    assert_eq!(
        run_semantic(
            "void f(int i) {\n  for (;; i = i + 1) {\n    i = 2;\n  }\n}\n",
            "unnecessary_statements"
        )
        .len(),
        0
    );
    assert_eq!(
        run_semantic(
            "void f(int i, bool b) {\n  for (; b; i = 1) {\n    i = 4;\n  }\n}\n",
            "unnecessary_statements"
        )
        .len(),
        0
    );

    // A non-effect updater binary reports at its operand head.
    let source = "void f(int i, bool b) {\n  for (; b; i == 1) {\n    i = 3;\n  }\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    let i = tok(source, &f.tokens, "i", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, i));

    // The non-block body statement keeps its face.
    let source = "void f(int i) {\n  for (;;) 1 + 2;\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    let one = tok(source, &f.tokens, "1", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, one));
}

/// The oracle span faces: a bare-send-tailed binary container seals past
/// the enclosing close (`;`, `,`, `)`), so the report ends at the
/// children's tiled tail operand — never a byte past it — and a
/// classic-`for` initialization whose wide seal crosses the left
/// separator still joins the initialization walk.
#[test]
fn unnecessary_statements_span_faces() {
    // The block binary ends at `s` (oracle length 6 — the `;` stays out).
    let source = "void f(int a, String s) {\n  a == s;\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    assert_eq!(lints[0].range.start, utf16_at(&f, tok(source, &f.tokens, "a", 1)));
    assert_eq!(lints[0].range.end, utf16_at(&f, tok(source, &f.tokens, "s", 1)) + 1);

    // A member-access tail bounds the same way (oracle length 13).
    let source = "void f(int a, String s) {\n  a == s.length;\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    assert_eq!(
        lints[0].range.end,
        utf16_at(&f, tok(source, &f.tokens, "length", 0)) + "length".len() as u32
    );

    // The header faces report separately, each ending at its own `s`
    // (oracle lengths 6 and 6).
    let source =
        "void f(int a, String s, bool b) {\n  for (a == s; b; a == s) {\n    a = 1;\n  }\n}";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 2, "{lints:?}");
    let f = Fixture::parse(source);
    assert_eq!(lints[0].range.start, utf16_at(&f, tok(source, &f.tokens, "a", 1)));
    assert_eq!(lints[0].range.end, utf16_at(&f, tok(source, &f.tokens, "s", 1)) + 1);
    assert_eq!(lints[1].range.start, utf16_at(&f, tok(source, &f.tokens, "a", 2)));
    assert_eq!(lints[1].range.end, utf16_at(&f, tok(source, &f.tokens, "s", 2)) + 1);

    // The non-block body statement ends at its own tail too (oracle
    // length 6).
    let source = "void f(int a, String s) {\n  for (;;) a == s;\n}\n";
    let lints = run_semantic(source, "unnecessary_statements");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(source);
    assert_eq!(lints[0].range.start, utf16_at(&f, tok(source, &f.tokens, "a", 1)));
    assert_eq!(lints[0].range.end, utf16_at(&f, tok(source, &f.tokens, "s", 1)) + 1);
}

/// The cascade face: a bare method section is a tear-off and reports;
/// an invocation section and a getter section stay silent.
#[test]
fn unnecessary_statements_cascade_faces() {
    assert_eq!(
        run_semantic(
            "class C {\n  void m() {}\n}\nvoid f(C c) {\n  c..m;\n}\n",
            "unnecessary_statements"
        )
        .len(),
        1
    );
    for source in [
        // An invocation section has a clear effect.
        "class C {\n  void m() {}\n}\nvoid f(C c) {\n  c..m();\n}\n",
        // A getter section is a value read.
        "class C {\n  int get g => 1;\n}\nvoid f(C c) {\n  c..g;\n}\n",
    ] {
        assert!(
            run_semantic(source, "unnecessary_statements").is_empty(),
            "{source}"
        );
    }
}

/// `prefer_int_literals`: the parameter faces — a positional and a
/// named argument against a `double` formal report, a member callee
/// through the semantic walk, an `int` formal and a generic callee
/// stay silent.
#[test]
fn prefer_int_literals_argument_faces() {
    assert_eq!(
        run_semantic(
            "void g(double d) {}\nvoid f() {\n  g(1.0);\n}\n",
            "prefer_int_literals"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic(
            "void g({double d = 0}) {}\nvoid f() {\n  g(d: 1.0);\n}\n",
            "prefer_int_literals"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic(
            "class C {\n  void m(double d) {}\n}\nvoid f(C c) {\n  c.m(1.0);\n}\n",
            "prefer_int_literals"
        )
        .len(),
        1
    );
    // The cross-file member face.
    assert_eq!(
        run_semantic_dep(
            "void f(K k) {\n  k.m(1.0);\n}\n",
            Some("class K {\n  void m(double d) {}\n}\n"),
            "prefer_int_literals"
        )
        .len(),
        1
    );
    // The negated form rides the same formal face.
    assert_eq!(
        run_semantic(
            "void g(double d) {}\nvoid f() {\n  g(-1.0);\n}\n",
            "prefer_int_literals"
        )
        .len(),
        1
    );

    for source in [
        // An `int` formal is not a double context.
        "void g(int d) {}\nvoid f() {\n  g(1.0);\n}\n",
        // A generic callee's parameter is not written `double`.
        "void g<T>(T x) {}\nvoid f() {\n  g(1.0);\n}\n",
        // An unresolved callee stays silent.
        "void f() {\n  unknown(1.0);\n}\n",
        // A non-integral value never reports.
        "void g(double d) {}\nvoid f() {\n  g(1.5);\n}\n",
    ] {
        assert!(
            run_semantic(source, "prefer_int_literals").is_empty(),
            "{source}"
        );
    }
}

/// The lexical faces kept from the inherited wave: the written
/// variable type, the return type, the arrow body, and the typed list.
#[test]
fn prefer_int_literals_lexical_faces() {
    assert_eq!(
        run_semantic("double x = 1.0;\n", "prefer_int_literals").len(),
        1
    );
    assert_eq!(
        run_semantic("double f() {\n  return 1.0;\n}\n", "prefer_int_literals").len(),
        1
    );
    assert_eq!(
        run_semantic("double f() => 1.0;\n", "prefer_int_literals").len(),
        1
    );
    assert_eq!(
        run_semantic(
            "void f() {\n  var x = <double>[1.0];\n}\n",
            "prefer_int_literals"
        )
        .len(),
        1
    );
    assert!(run_semantic("double x = 1.5;\n", "prefer_int_literals").is_empty());
    // An anonymous boundary has no written return type.
    assert!(run_semantic("void f() {\n  () => 1.0;\n}\n", "prefer_int_literals").is_empty());
}

/// `avoid_bool_literals_in_conditional_expressions`: the both-arms
/// exact non-nullable `bool` gate (:46-47) over the semantic faces —
/// the literals answer by their own face, a bare identifier at its
/// flow read (promotions included), a bare call through the cross-file
/// callee — and one report at the whole conditional when an arm is a
/// `bool` literal (the oracle observable: the upstream per-arm pair
/// (:48-49) collapses to one identical diagnostic at one span).
/// Dynamic, unpromoted `bool?`, and resolved non-`bool` arms keep
/// the gate silent, the plain run stays silent (the wave posture),
/// and an if-statement is not a target (:34). An initializer-inferred
/// local answers at its read like a declared arm (`var t = c` adopts
/// the staged inferred type — the oracle reports on that fixture,
/// `INFO|LINT|AVOID_BOOL_LITERALS_IN_CONDITIONAL_EXPRESSIONS` at the
/// conditional span); only an arm with no decided face stays silent.
#[test]
fn bool_literal_conditional_faces() {
    // Both literal arms collapse to ONE lint at the whole conditional;
    // the plain run (the face absent) stays silent.
    let source = "void f(bool b) {\n  var x = b ? true : false;\n}\n";
    let plain = Fixture::parse(source);
    assert!(
        plain
            .run(&one_rule("avoid_bool_literals_in_conditional_expressions"))
            .is_empty()
    );
    let lints = run_semantic(source, "avoid_bool_literals_in_conditional_expressions");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Conditional expressions with a 'bool' literal can be simplified."
    );
    let f = Fixture::parse(source);
    // The report opens at the condition's first token — the operand
    // head `b` (the parameter reference, nth=1), never the row's `?`
    // delta-tile anchor.
    let start = tok(source, &f.tokens, "b", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    // The written `bool` parameter answers at the flow read (the
    // written-type face's green cases).
    assert_eq!(
        run_semantic(
            "void f(bool c, bool d) {\n  var x = c ? d : true;\n}\n",
            "avoid_bool_literals_in_conditional_expressions"
        )
        .len(),
        1
    );

    // The initializer-inferred local answers at its flow read too:
    // `var t = c` adopts the staged inferred type, so the arm is
    // exactly `bool` (the oracle reports on this fixture at the
    // conditional span). The report anchors the condition's `c` —
    // the third spelling occurrence.
    let inferred = "void f(bool c) {\n  var t = c;\n  var x = c ? t : true;\n}\n";
    let lints = run_semantic(inferred, "avoid_bool_literals_in_conditional_expressions");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(inferred);
    let c = tok(&f.source, &f.tokens, "c", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, c));

    for source in [
        // A resolved non-`bool` arm fails the gate.
        "void f(bool c, int i) {\n  var x = c ? i : true;\n}\n",
        // `dynamic` is not `boolType` (:39-40).
        "void f(bool c, dynamic d) {\n  var x = c ? d : true;\n}\n",
        // An unpromoted `bool?` arm is not `bool`.
        "void f(bool c, bool? b) {\n  var x = c ? b : true;\n}\n",
        // An if-statement is not a conditional expression (:30).
        "void f(bool c) {\n  if (c) {\n    var x = true;\n  }\n}\n",
    ] {
        assert!(
            run_semantic(source, "avoid_bool_literals_in_conditional_expressions").is_empty(),
            "{source}"
        );
    }

    // The promotion face: a `bool?` promoted inside `if` reads exact
    // `bool` at the analyzer's `staticType`, which the arm would report
    // beside the literal. The substrate's truthy-`if` does not promote
    // the bare `bool?` operand (dartq-flow promotes `!=`/`is`/pattern
    // tests, not the bare condition read), so the read stays `bool?`
    // and the gate stays silent — the documented under-report.
    assert!(
        run_semantic(
            "void f(bool? b) {\n  if (b) {\n    var x = b ? b : false;\n  }\n}\n",
            "avoid_bool_literals_in_conditional_expressions"
        )
        .is_empty()
    );

    // The cross-file callee face: a dep-declared `bool` function
    // answers an invocation arm — the gate passes with one literal
    // arm reporting, and with no literal arms at all it stays silent.
    assert_eq!(
        run_semantic_dep(
            "void f(bool c) {\n  var x = c ? g() : false;\n}\n",
            Some("bool g() => true;\n"),
            "avoid_bool_literals_in_conditional_expressions"
        )
        .len(),
        1
    );
    assert!(
        run_semantic_dep(
            "void f(bool c) {\n  var x = c ? g() : h();\n}\n",
            Some("bool g() => true;\nbool h() => false;\n"),
            "avoid_bool_literals_in_conditional_expressions"
        )
        .is_empty()
    );
}

/// `unnecessary_lambdas`, the signature gate's report faces
/// (`isAssignableTo(invocationType, nodeType)`, :208/:246, read as
/// [`dartq_semantic::FnSig::assignable_from`]'s `Some(true)`): a
/// zero-parameter forwarding lambda over a bare top-level callee (the
/// message and the whole-lambda anchor pinned), a typed
/// parameter-forwarding lambda, an expression-statement block body,
/// a cross-file top-level callee (the dependency's declaration
/// composes), an implicit-`this` method, a `final` typed receiver's
/// method, a named-parameter forward, the zero-argument bare
/// creation (`test_constructorCall_noArgs`), and a `new` creation
/// whose constructor formals compose (:208-223). The plain run stays
/// silent (the wave posture).
#[test]
fn unnecessary_lambdas_signature_gate_faces() {
    let source = "void g() {}\nvoid main() {\n  var cb = () => g();\n}\n";
    assert!(
        Fixture::parse(source)
            .run(&one_rule("unnecessary_lambdas"))
            .is_empty(),
        "the plain run stays silent"
    );
    let lints = run_semantic(source, "unnecessary_lambdas");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Closure should be a tearoff.");
    let f = Fixture::parse(source);
    let start = tok(source, &f.tokens, "(", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    for source in [
        // A typed parameter forwards to a matching formal.
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) => g(a));\n}\n",
        // The expression-statement block body (:160-167).
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) {\n    g(a);\n  });\n}\n",
        // The `return` block body (:168-175).
        "int g(int a) => a;\nvoid f(List<int> xs) {\n  xs.map((int a) {\n    return g(a);\n  });\n}\n",
        // An implicit-`this` method composes through the member face.
        "class C {\n  void m(int e) {}\n  void f(List<int> xs) {\n    xs.forEach((int e) => m(e));\n  }\n}\n",
        // A `final` typed receiver's method (:283-284).
        "class K {\n  bool has(int e) => true;\n}\nvoid f(List<int> xs) {\n  final K k = K();\n  xs.where((int e) => k.has(e));\n}\n",
        // A named parameter forwards by name; the nullable target
        // formal is Right Top for the non-nullable lambda formal.
        "void g({required int? a}) {}\nvoid f() {\n  var cb = ({required int a}) => g(a: a);\n}\n",
        // The zero-argument bare creation is provable by construction.
        "class C {}\nvoid f() {\n  var mk = () => C();\n}\n",
        // A `new` creation composes the constructor's formals.
        "class C {\n  C(int a);\n}\nvoid f() {\n  var mk = (int a) => new C(a);\n}\n",
        // An undeclared unnamed constructor composes the implicit
        // default's empty table.
        "class C {}\nvoid f() {\n  var mk = () => new C();\n}\n",
    ] {
        let lints = run_semantic(source, "unnecessary_lambdas");
        assert_eq!(lints.len(), 1, "{source}\n{lints:?}");
    }

    // The cross-file callee: the dependency's declaration composes.
    assert_eq!(
        run_semantic_dep(
            "void f(List<int> xs) {\n  xs.forEach((int e) => g(e));\n}\n",
            Some("void g(int e) {}\n"),
            "unnecessary_lambdas"
        )
        .len(),
        1
    );
}

/// `unnecessary_lambdas`, the `parametersMatch` fallback's report
/// faces (:107-151, consulted at :246-249). The non-`FunctionType`
/// arm (:108-111, "We must assume is valid here"): a callee the
/// model resolves to no function type — a local function, an
/// unresolved spelling, the untyped-formal forward onto one. The
/// FunctionType path (:112-150) over the undecided `None` verdict —
/// the inferred channel: an untyped, `dynamic`, or parameterized
/// formal the context types upstream, a generic callee's
/// instantiation, distinct named types — rides the required-positional
/// counts, oracle-verified (the #2095 lane's live probes on 3.13.1;
/// #2109's resolved-callee shape).
#[test]
fn unnecessary_lambdas_fallback_faces() {
    for source in [
        // A local function composes no model signature.
        "void f(List<int> xs) {\n  void local(int a) {}\n  xs.forEach((int a) => local(a));\n}\n",
        // An unresolved callee.
        "void f(List<int> xs) {\n  xs.forEach((int a) => missing(a));\n}\n",
        // The gate row itself: an untyped formal forwarding to an
        // unresolved core spelling.
        "void f(List<int> xs) {\n  xs.forEach((e) => print(e));\n}\n",
        // The #2109 shape: the callee resolves (`void Function(Object?)`)
        // but the untyped formal's inferred channel returns `None`; the
        // required-positional counts (1 == 1) carry the face. The
        // oracle infers `e` from the context formal's
        // `void Function(Object)` and fires.
        "void print(Object? e) {}\nvoid handler(void Function(Object) f) {}\nvoid main() {\n  handler((e) => print(e));\n}\n",
        // `None`: the untyped formal is inferred upstream.
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((a) => g(a));\n}\n",
        // `None`: the generic callee's instantiation (the model does
        // not name it); upstream fires on the instantiated invoke
        // type.
        "void g<T>(T a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) => g(a));\n}\n",
        // `None`: distinct named types are undecided at M1; upstream's
        // subtype check proves the tear-off stands (`int <: num` on
        // the contravariant leaf).
        "void g(num a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) => g(a));\n}\n",
        // `None`: a written `dynamic` formal (upstream: `dynamic` is
        // top on the contravariant leaf).
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((dynamic a) => g(a));\n}\n",
        // `None`: a parameterized written formal (the argument slots
        // are pool-bound); upstream's types are identical.
        "void g(List<int> a) {}\nvoid f(List<List<int>> xs) {\n  xs.forEach((List<int> a) => g(a));\n}\n",
    ] {
        let lints = run_semantic(source, "unnecessary_lambdas");
        assert_eq!(lints.len(), 1, "{source}\n{lints:?}");
        assert_eq!(lints[0].message, "Closure should be a tearoff.");
    }
}

/// `unnecessary_lambdas`, the silences. The structural bails (an
/// `async` body :156, several statements :160, an argument that is
/// not the lambda's own parameter :232-239 — arithmetic, a capturing
/// closure, a foreign name — explicit type arguments :285, a
/// null-aware chain :282, a non-`final` or parameter receiver
/// :283/:70-76); the `Some(false)` faces (a nullable lambda formal
/// against a non-nullable target formal, a required target formal
/// the lambda lacks, a constructor with a required formal the lambda
/// lacks); the `None` faces the `parametersMatch` gates fail (an
/// optional target formal against the untyped forward's required
/// count, :112-120); and the faces that never reach the gate (the
/// bare creation with arguments). The undecided `None` faces that
/// used to sit here report through the fallback now
/// (`unnecessary_lambdas_fallback_faces`).
#[test]
fn unnecessary_lambdas_silent_faces() {
    for source in [
        // The async body keyword bails.
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) async => g(a));\n}\n",
        // Two statements bail.
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) {\n    g(a);\n    g(a);\n  });\n}\n",
        // Parameter arithmetic is not a forward.
        "void g(int a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) => g(a + 1));\n}\n",
        // A capturing closure argument is not a forward.
        "bool g(bool Function(int) t) => true;\nvoid f(List<int> xs) {\n  xs.where((int a) => g((int b) => b == a));\n}\n",
        // A foreign name is not the lambda's own parameter.
        "void g(int a) {}\nvoid f(List<int> xs, int a) {\n  xs.forEach((int e) => g(a));\n}\n",
        // Explicit type arguments bail.
        "void g<T>(T a) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) => g<int>(a));\n}\n",
        // A null-aware chain bails.
        "class K {\n  void touch(int e) {}\n}\nvoid f(List<int> xs) {\n  final K? k = null;\n  xs.forEach((int e) => k?.touch(e));\n}\n",
        // A mutable typed receiver is not final.
        "class K {\n  bool has(int e) => true;\n}\nvoid f(List<int> xs) {\n  K k = K();\n  xs.where((int e) => k.has(e));\n}\n",
        // The lambda's own parameter as receiver.
        "class K {\n  bool has(K e) => true;\n}\nvoid f(List<K> xs) {\n  xs.where((K e) => e.has(e));\n}\n",
        // `late final` demotes the verdict.
        "class K {\n  bool has(int e) => true;\n}\nvoid f(List<int> xs) {\n  late final K k;\n  xs.where((int e) => k.has(e));\n}\n",
        // `Some(false)`: a nullable lambda formal cannot stand at a
        // non-nullable target formal.
        "void g(int a) {}\nvoid f(List<int?> xs) {\n  xs.forEach((int? a) => g(a));\n}\n",
        // `Some(false)`: a required target formal the lambda lacks.
        "void g(int a, int b) {}\nvoid f(List<int> xs) {\n  xs.forEach((int a) => g(a));\n}\n",
        // `Some(false)`: a constructor formal the lambda lacks.
        "class C {\n  C(int a);\n}\nvoid f() {\n  var mk = () => new C();\n}\n",
        // `None` the fallback fails: the optional target formal
        // cannot face the untyped forward's required count
        // (:112-120).
        "void g([int? a]) {}\nvoid f(List<int> xs) {\n  xs.forEach((e) => g(e));\n}\n",
        // `None`: a bare creation with arguments carries no type-use
        // row (only the zero-argument shape is provable).
        "class C {\n  C(int a);\n}\nvoid f() {\n  var mk = (int a) => C(a);\n}\n",
    ] {
        assert!(
            run_semantic(source, "unnecessary_lambdas").is_empty(),
            "{source}"
        );
    }
}
