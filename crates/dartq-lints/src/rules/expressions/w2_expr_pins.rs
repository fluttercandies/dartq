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

/// The wave posture: both rules are silent under the plain run (the
/// face absent) and report with it mounted.
#[test]
fn w2_expr_rules_require_the_semantic_face() {
    let sources = [
        ("avoid_print", "void f() {\n  print('hi');\n}\n"),
        (
            "no_runtimetype_tostring",
            "class C {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
        ),
    ];
    for (rule, source) in sources {
        let plain = Fixture::parse(source);
        assert!(plain.run(&one_rule(rule)).is_empty(), "{rule}");
        assert_eq!(run_semantic(source, rule).len(), 1, "{rule}");
    }
}

/// The bare `print` face: a top-level and a class-method call report
/// at the method name; a same-file shadow, a member callee, the
/// `kDebugMode` then branch (braced and not), and another condition
/// stay silent. The closure bail keeps a call under the branch
/// reporting.
#[test]
fn avoid_print_faces() {
    let lints = run_semantic("void f() {\n  print('hi');\n}\n", "avoid_print");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Don't invoke 'print' in production code.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try using a logging framework.")
    );
    // The report anchors at the method name (:43).
    assert_eq!(lints[0].range, 13..18);

    assert_eq!(
        run_semantic(
            "class C {\n  void f() {\n    print('x');\n  }\n}\n",
            "avoid_print"
        )
        .len(),
        1
    );
    // The closure bail (:62-66): a call inside a closure under the
    // branch is not debug-only.
    assert_eq!(
        run_semantic(
            "void f(bool kDebugMode) {\n  if (kDebugMode) foo(() {\n    print('x');\n  });\n}\n",
            "avoid_print"
        )
        .len(),
        1
    );

    for source in [
        // A same-file `print` shadows the core function (the binding
        // gate).
        "void print(Object? o) {}\nvoid f() {\n  print('x');\n}\n",
        // A member callee is another face (the prefixed core spelling
        // under-reports).
        "class C {\n  void print() {}\n  void f() {\n    C.print();\n  }\n}\n",
        // The `kDebugMode` then branch exempts, braced or not
        // (:52-68).
        "void f(bool kDebugMode) {\n  if (kDebugMode) print('x');\n}\n",
        "void f(bool kDebugMode) {\n  if (kDebugMode) {\n    print('x');\n  }\n}\n",
    ] {
        assert!(run_semantic(source, "avoid_print").is_empty(), "{source}");
    }
    // Another condition does not exempt — upstream `_isDebugOnly`
    // requires the condition to be the `kDebugMode` element
    // (avoid_print.dart :52-68), so a `debug` flag keeps the lint.
    assert_eq!(
        run_semantic(
            "void f(bool debug) {\n  if (debug) print('x');\n}\n",
            "avoid_print"
        )
        .len(),
        1
    );
}

/// The multibyte-trivia face: three em-dashes ahead of the call put
/// six bytes of UTF-16/byte drift on the `print` selector — the shape
/// that used to land the binding probe on the argument identifier's
/// reference row (a phantom local binding) and silence the report.
/// The shadow join reads the matched row's own name, so a
/// non-`print` binding never shadows.
#[test]
fn avoid_print_multibyte_trivia_drift_reports() {
    let source = "// ———\nvoid f() {\n  var bad = 1;\n  print(bad);\n}\n";
    let lints = run_semantic(source, "avoid_print");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Don't invoke 'print' in production code.");
}

/// The tear-off argument face (:70-80): a bare `print` identifier
/// passed as an argument reports at the identifier —
/// `list.forEach(print)` and `f(print)` — and the gates hold: a
/// same-file `print` shadow stays silent (the binding gate), a member
/// tear-off `C.print` is not a `SimpleIdentifier` (the access-marker
/// gate), other bare uses never visit (`print;`, `var x = print;`),
/// and the debug-only walk does not apply on this face (the
/// then-branch call still reports). The invocation face stacks with
/// it: `print(print)` is two rows.
#[test]
fn avoid_print_tear_off_argument_faces() {
    let member_call = "void f(List<int> xs) {\n  xs.forEach(print);\n}\n";
    let lints = run_semantic(member_call, "avoid_print");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let off = u32::try_from(member_call.find("print").expect("anchor")).expect("u32");
    assert_eq!(lints[0].range, off..off + 5);

    let plain_argument = "void g(int x) {}\nvoid f() {\n  g(print);\n}\n";
    let lints = run_semantic(plain_argument, "avoid_print");
    assert_eq!(lints.len(), 1, "{lints:?}");
    let off = u32::try_from(plain_argument.rfind("print").expect("anchor")).expect("u32");
    assert_eq!(lints[0].range, off..off + 5);

    // The binding gate: a same-file `print` shadows the core function.
    let shadow = "void print(Object? o) {}\nvoid g(int x) {}\nvoid f() {\n  g(print);\n}\n";
    assert!(run_semantic(shadow, "avoid_print").is_empty(), "{shadow}");

    // The access-marker gate: a member tear-off is not a
    // `SimpleIdentifier` (the unresolved receiver keeps the binding
    // gate out of the verdict).
    let member_tear_off = "void g(void h()) {}\nvoid f() {\n  g(Unknown.print);\n}\n";
    assert!(
        run_semantic(member_tear_off, "avoid_print").is_empty(),
        "{member_tear_off}"
    );

    // Other bare uses are never arguments — silent.
    for source in [
        "void f() {\n  print;\n}\n",
        "void f() {\n  var x = print;\n}\n",
    ] {
        assert!(run_semantic(source, "avoid_print").is_empty(), "{source}");
    }

    // The debug-only walk is invocation-face-only (:70-80).
    let debug_argument =
        "void g(int x) {}\nvoid f(bool kDebugMode) {\n  if (kDebugMode) g(print);\n}\n";
    assert_eq!(run_semantic(debug_argument, "avoid_print").len(), 1);

    // The invocation face stacks: `print(print)` reports at both the
    // method name and the argument.
    let stacked = "void f() {\n  print(print);\n}\n";
    let mut offsets: Vec<u32> = run_semantic(stacked, "avoid_print")
        .into_iter()
        .map(|lint| lint.range.start)
        .collect();
    offsets.sort_unstable();
    assert_eq!(offsets.len(), 2, "{offsets:?}");
    let base = u32::try_from(stacked.find("print").expect("anchor")).expect("u32");
    assert_eq!(offsets, vec![base, base + 6]);
}

/// The `runtimeType`-access faces: the bare implicit-`this` read,
/// the explicit `this`/`super` receivers, and an extension on a
/// same-file concrete class report at the method name; a plain
/// receiver, a bare `toString()`, another selector, an unresolved
/// name, and the skip posture (assert, throw, catch, mixin, abstract
/// class, and non-concrete-class extensions) stay silent.
#[test]
fn no_runtimetype_tostring_faces() {
    let lints = run_semantic(
        "class C {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
        "no_runtimetype_tostring",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Using 'toString' on a 'Type' is not safe in production code."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the usage of 'toString' or restructuring the code.")
    );
    // The report anchors at the method name (:67-74).
    assert_eq!(lints[0].range, 39..47);

    // The explicit receivers are syntactic faces (:108-112).
    assert_eq!(
        run_semantic(
            "class C {\n  void f() {\n    this.runtimeType.toString();\n  }\n}\n",
            "no_runtimetype_tostring"
        )
        .len(),
        1
    );
    assert_eq!(
        run_semantic(
            "class C {\n  void f() {\n    super.runtimeType.toString();\n  }\n}\n",
            "no_runtimetype_tostring"
        )
        .len(),
        1
    );
    // An extension on a same-file concrete class keeps the verdict
    // (the `!(is ClassElement && !isAbstract)` arm, :93-105).
    assert_eq!(
        run_semantic(
            "class D {}\nextension E on D {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
            "no_runtimetype_tostring"
        )
        .len(),
        1
    );

    for source in [
        // A plain receiver is not a `runtimeType` access (:113-120).
        "class C {\n  void f(C c) {\n    c.runtimeType.toString();\n  }\n}\n",
        // A bare `toString()` carries no real target (:68-69).
        "class C {\n  void f() {\n    toString();\n  }\n}\n",
        // Only `toString` is the selector (:67).
        "class C {\n  void f() {\n    runtimeType.toStringDeep();\n  }\n}\n",
        // An unresolved bare `runtimeType` (top level, static
        // context) stays silent.
        "void f() {\n  runtimeType.toString();\n}\n",
        "class C {\n  static void f() {\n    runtimeType.toString();\n  }\n}\n",
        // The skip posture (:76-112): assert, throw, catch clause,
        // mixin, and abstract class.
        "class C {\n  void f() {\n    assert(runtimeType.toString() != null);\n  }\n}\n",
        "class C {\n  void f() {\n    throw runtimeType.toString();\n  }\n}\n",
        "class C {\n  void f() {\n    try {} catch (e) {\n      runtimeType.toString();\n    }\n  }\n}\n",
        "mixin M {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
        "abstract class C {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
        // An extension whose `on` type is abstract, or not a
        // same-file class, skips (:93-105).
        "abstract class A {}\nextension E on A {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
        "extension E on int {\n  void f() {\n    runtimeType.toString();\n  }\n}\n",
    ] {
        assert!(
            run_semantic(source, "no_runtimetype_tostring").is_empty(),
            "{source}"
        );
    }
}

// —— the incoming sextet (#1147/#1148 batch) ————————————————————————

/// `prefer_const_constructors` production-bridge pins: a const
/// constructor's unnamed, named, and written-type-argument creations
/// report spanning the whole creation; a non-const constructor, a
/// written `const`, and a generic creation without written type
/// arguments (the conservative bail) stay silent; the `@literal` and
/// type-variable faces decide per the gates below.
#[test]
fn prefer_const_constructors_faces() {
    let lints = Fixture::parse("class C {\n  const C();\n}\nvoid f() {\n  C();\n}\n")
        .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use 'const' with the constructor to improve performance."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try adding the 'const' keyword to the constructor invocation.")
    );
    // The whole creation (`C()` — 38..41).
    assert_eq!(lints[0].range, 38..41);

    // The named-selector face (`C.n()` — 40..45).
    let lints = Fixture::parse("class C {\n  const C.n();\n}\nvoid f() {\n  C.n();\n}\n")
        .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 40..45);

    // Written type arguments pass the generic gate (`G<int>()`).
    let lints = Fixture::parse("class G<T> {\n  const G();\n}\nvoid f() {\n  G<int>();\n}\n")
        .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // The downward-inference face (:80-97 — a generic class created
    // without written type arguments still reports when the context
    // type's written arguments are concrete, `G<int> f() => G()`).
    let lints = Fixture::parse("class G<T> {\n  const G();\n}\nG<int> f() => G();\n")
        .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // The named primary-constructor face (`CPrimaryNamed.named(1)` —
    // the whole `C.named(1)` send, 66..88); the `const`-declaring
    // primary ctor resolves through the `.named` selector's
    // declaration-name head.
    let lints = Fixture::parse(
        "class const CPrimaryNamed.named(final int x);\nvar cPrimaryNamed = CPrimaryNamed.named(1);\n",
    )
    .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 66..88);

    // The unnamed and extension-type primary-constructor faces (the
    // corpus's `CPrimaryConst(1)` / `EConstPrimary(1)` rows): the
    // header `const` decides through the primary-constructor row.
    for source in [
        "class const CPrimaryConst(final int x);\nvar cPrimaryConst = CPrimaryConst(1);\n",
        "extension type const EConstPrimary(int i) {}\nvar eConstPrimary = EConstPrimary(1);\n",
    ] {
        let lints = Fixture::parse(source).run(&one_rule("prefer_const_constructors"));
        assert_eq!(lints.len(), 1, "{source}");
    }

    // The `const new()` declaring-constructor face (`ANewSyntax()` —
    // 60..72): the `new` name sits one token before the `(` head, the
    // ctor still reads as declaring-`const`.
    let lints = Fixture::parse(
        "class ANewSyntax {\n  const new();\n}\nANewSyntax aNewSyntax = ANewSyntax();\n",
    )
    .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 60..72);

    for source in [
        // A non-const constructor never reports.
        "class C {\n  C();\n}\nvoid f() {\n  C();\n}\n",
        // A written `const` is the dispatch bail.
        "class C {\n  const C();\n}\nvoid f() {\n  const C();\n}\n",
        // A generic class created without written type arguments
        // bails only when the context type names a type parameter
        // (linter#4531): `AImplicit<T, int> fImplicit<T>() => AImplicit()`.
        "class AImplicit<T, U> {\n  const AImplicit();\n}\nAImplicit<T, int> fImplicit<T>() => AImplicit();\n",
    ] {
        let fixture = Fixture::parse(source);
        assert!(
            fixture
                .run(&one_rule("prefer_const_constructors"))
                .is_empty(),
            "{source}"
        );
    }

    // The `@literal` gate rides the analyzer's declaring-library read
    // (`_isPackageMetaGetter`, `library.name == 'meta'`): an unnamed
    // meta never recognizes the annotation and the creation reports —
    // the live oracle's own face over a differential corpus's vendored
    // meta — while a `library meta;` unit resolves it and bails.
    let lints = Fixture::parse("class C {\n  @literal\n  const C();\n}\nvoid f() {\n  C();\n}\n")
        .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let fixture = Fixture::parse(
        "library meta;\nclass C {\n  @literal\n  const C();\n}\nvoid f() {\n  C();\n}\n",
    );
    assert!(
        fixture
            .run(&one_rule("prefer_const_constructors"))
            .is_empty()
    );

    // A written type argument naming an in-scope type variable is not
    // constable — `const C<U>()` is a constant-creation error (the
    // upstream test_cannotBeConst_explicitTypeArgument_typeVariable_*
    // faces) — on the creation's header and nested inside its
    // arguments. A concrete written argument keeps the fire.
    for source in [
        "class ATypeVarFn<T> {\n  const ATypeVarFn();\n}\nvoid fTypeVarFn<U>() => ATypeVarFn<U>();\n",
        "class ATypeVarCtor<T> {\n  const ATypeVarCtor({required List<T> p});\n  factory ATypeVarCtor.named() => ATypeVarCtor(p: <T>[]);\n}\n",
    ] {
        let fixture = Fixture::parse(source);
        assert!(
            fixture
                .run(&one_rule("prefer_const_constructors"))
                .is_empty(),
            "{source}"
        );
    }
    let lints = Fixture::parse(
        "class ADynArg<T> {\n  const ADynArg();\n}\nvar aDynArg = ADynArg<dynamic>();\n",
    )
    .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // The canBeConst tail is per creation: a creation inside a
    // collection-for element reports (its own const-ability is
    // independent of the enclosing literal's), and a map argument
    // holding a const entry is constable.
    let lints = Fixture::parse(
        "class K {\n  const K();\n}\nfinal k = K([for (var i = 0; i < 5; ++i) K()]);\n",
    )
    .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let lints = Fixture::parse(
        "class C {\n  const C(Map<int, C?> p);\n}\nvar x = C({1: const C(null)});\n",
    )
    .run(&one_rule("prefer_const_constructors"));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// `prefer_const_constructors` const-context suppression pins: a
/// creation inside a `static const` field initializer, a `const` local
/// declaration, an annotation argument, an enum element's arguments,
/// or a `const` collection literal is already const — the upstream
/// `canBeConst`/`isConst` gate stays silent and no row reports. The
/// corpus-shaped member faces ride the same walk: typed declarations,
/// named private constructors, multi-declarator lists, and self-class
/// references.
#[test]
fn prefer_const_constructors_const_context_suppressed() {
    for source in [
        // A `static const` field's own initializer is in a const
        // context — `B()` inside `static const b = B()` stays silent.
        "class B {\n  const B();\n}\nclass A {\n  static const b = B();\n}\n",
        // A `const` local's initializer likewise (`const b = B()`).
        "class B {\n  const B();\n}\nvoid f() {\n  const b = B();\n}\n",
        // An enum element's argument list is a constant context.
        "class B {\n  const B();\n}\nenum E {\n  a(B()),\n  b(B());\n  const E(B b);\n}\n",
        // An annotation's argument list is a constant context.
        "class B {\n  const B();\n}\n@B()\nvoid f() {}\n",
        // Inside a written `const [...]` every element is const.
        "class B {\n  const B();\n}\nvoid f() {\n  const l = [B()];\n}\n",
        // The use_enums corpus face: typed declarators, named private
        // constructors, multi-declarator lists, self-class references —
        // every creation inside the const declarator is already const.
        "class A {\n  const A._(this.v);\n  final int v;\n}\nclass U {\n  static const A a = A._(1), b = A._(2);\n}\n",
        // The const-ability tail stays per creation: `F()` resolves to
        // the factory — the class's const NAMED constructor must not
        // make the unnamed face const-able (the upstream
        // test_no_lint_factorySome face).
        "class F {\n  factory F() => g();\n  const F._();\n}\nvoid g() {\n  F();\n}\n",
    ] {
        let fixture = Fixture::parse(source);
        assert!(
            fixture
                .run(&one_rule("prefer_const_constructors"))
                .is_empty(),
            "{source}"
        );
    }
}

/// `prefer_const_constructors` dot-shorthand pins: a `.new()` /
/// `.named(…)` shorthand under a written context type reports spanning
/// the `.` through the argument list; a written `const` (the dispatch
/// bail), a non-const constructor, and a non-const argument stay
/// silent; the `@literal` face rides the declaring-library gate. A
/// chained method call after the creation (`.tightFor(…).copyWith(…)`)
/// reports once, at the creation.
#[test]
fn prefer_const_constructors_dot_shorthand_faces() {
    for (source, want) in [
        // The `.new()` shorthand under a declared type.
        ("class A {\n  const A();\n}\nA a = .new();\n", 1),
        // The named shorthand with a trailing method call — the
        // creation reports; `copyWith` is an ordinary method send.
        (
            "class A {\n  const A.t({int? h});\n  A cw({int? w}) => this;\n}\n\
                 void f() {\n  A a = .t(h: 4).cw(w: 4);\n}\n",
            1,
        ),
        // The getter-return context type (`C get f => .new(1)`).
        ("class const C(final int x);\nC get f => .new(1);\n", 1),
        // A written `const` is the dispatch bail.
        (
            "class A {\n  const A.t({int? h});\n}\n\
                 void f() {\n  A a = const .t(h: 4);\n}\n",
            0,
        ),
        // The `@literal` gate rides the declaring-library read: an
        // unnamed meta never recognizes the annotation, so the
        // shorthand creation reports like any other.
        ("class K {\n  @literal\n  const K();\n}\nK k = .new();\n", 1),
        // A non-const constructor stays silent.
        ("class A {\n  A();\n}\nA a = .new();\n", 0),
        // A non-const argument stays silent (`ANotConst()` cannot be
        // const, so the shorthand creation cannot).
        (
            "class A {\n  int c;\n  A(): c = 0;\n}\n\
                 extension type const B(A a) {}\nB get g => .new(A());\n",
            0,
        ),
    ] {
        let lints = Fixture::parse(source).run(&one_rule("prefer_const_constructors"));
        assert_eq!(lints.len(), want, "{source}");
    }
}

/// `use_named_constants` remaining resolved faces: the constant
/// pattern's written-`const` operand (`if (a case const A(0))`,
/// `case const A(1):`) reports at the creation — not the pattern —
/// and the extension-type const declaration resolves through payload
/// word 2. A private `static const` still matches (upstream judges
/// accessibility against the same library).
#[test]
fn use_named_constants_pattern_and_extension_type_faces() {
    for (source, range) in [
        (
            "class A {\n  const A(this.v);\n  final int v;\n  static const zero = A(0);\n}\nvoid f(A a) {\n  if (a case const A(0)) {}\n}\n",
            107..111,
        ),
        (
            "class A {\n  const A(this.v);\n  final int v;\n  static const one = A(1);\n}\nvoid f(A a) {\n  switch (a) {\n    case const A(1):\n  }\n}\n",
            117..121,
        ),
        (
            "extension type const A(int i) {\n  static const zero = A(0);\n}\nvoid f() {\n  const a = A(0);\n}\n",
            85..89,
        ),
    ] {
        let lints = Fixture::parse(source).run(&one_rule("use_named_constants"));
        assert_eq!(lints.len(), 1, "{source}");
        assert_eq!(lints[0].range, range, "{source}");
    }
}

/// `avoid_redundant_argument_values` scope-resolved callee faces: the
/// enum-constant's `EnumBody`-sibling `Arguments` (issue 3617), the
/// block-local function, the function-typed formal of an enclosing
/// executable (the implicit-`null` default reports on a written `null`),
/// and the `var a = A.new` tear-off variable.
#[test]
fn avoid_redundant_argument_values_scope_faces() {
    for (source, range) in [
        // The enum element's arguments tile under the `EnumBody`.
        (
            "enum E {\n  a(test: false);\n  const E({bool test = false});\n}\n",
            19..24,
        ),
        // The block-local function's own list.
        (
            "void f() {\n  void g({bool p = true}) {}\n  g(p: true);\n}\n",
            47..51,
        ),
        // The function-typed formal's implicit `null` default.
        (
            "void f(void Function({int? p}) g) {\n  g(p: null);\n}\n",
            43..47,
        ),
        (
            "void f(void Function([int? p]) g) {\n  g(null);\n}\n",
            40..44,
        ),
        // The tear-off variable resolves to `A`'s constructor list.
        (
            "class A {\n  A({bool p = true});\n}\nvoid f() {\n  var a = A.new;\n  a(p: true);\n}\n",
            69..73,
        ),
        // The primary-constructor face (`class C({bool p = true})`).
        (
            "class C({bool p = true});\nvoid f() {\n  C(p: true);\n}\n",
            44..48,
        ),
    ] {
        let lints = Fixture::parse(source).run(&one_rule("avoid_redundant_argument_values"));
        assert_eq!(lints.len(), 1, "{source}");
        assert_eq!(lints[0].range, range, "{source}");
    }
    // A local function declared AFTER the call does not bind.
    assert!(
        Fixture::parse("void f() {\n  g(p: true);\n  void g({bool p = true}) {}\n}\n")
            .run(&one_rule("avoid_redundant_argument_values"))
            .is_empty()
    );
}

/// creation equal to the class's `static const` initializer reports
/// with `Type.field`; an unmatched creation stays silent.
#[test]
fn use_named_constants_faces() {
    let lints = Fixture::parse(
        "class C {\n  const C();\n  static const zero = C();\n}\nvoid f() {\n  const C();\n}\n",
    )
    .run(&one_rule("use_named_constants"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use the constant 'C.zero' rather than a constructor returning the same object."
    );
    assert_eq!(lints[0].correction.as_deref(), Some("Try using 'C.zero'."));
    // The whole written-const wrapper (`const C()` — 65..74).
    assert_eq!(lints[0].range, 65..74);

    // No matching static constant — silent.
    let fixture = Fixture::parse("class C {\n  const C();\n}\nvoid f() {\n  const C();\n}\n");
    assert!(fixture.run(&one_rule("use_named_constants")).is_empty());
}

/// `use_named_constants` shorthand + contextual-const faces: the
/// written-`const` shorthand (`const .new(0)`), the contextually-const
/// shorthand (`const A a = .new(0)`), and the contextually-const bare
/// creation (`const a = A(0)`) report; a stateless class's `A(1)`
/// matches `zero = A(0)` (upstream's computed-value equality under a
/// discarded-parameter ctor), and the `A.zero` read is never a
/// reconstruction.
#[test]
fn use_named_constants_shorthand_and_context_faces() {
    // `const .new(0)` — the explicit-const shorthand.
    let lints = Fixture::parse(
        "void f() {\n  A a = const .new(0);\n}\nclass A {\n  const A(int value);\n  static const zero = A(0);\n}\n",
    )
    .run(&one_rule("use_named_constants"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use the constant 'A.zero' rather than a constructor returning the same object."
    );

    // `const A a = .new(0)` — the contextually-const shorthand.
    let lints = Fixture::parse(
        "void f() {\n  const A a = .new(0);\n}\nclass A {\n  const A(int value);\n  static const zero = A(0);\n}\n",
    )
    .run(&one_rule("use_named_constants"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // `const a = A(0)` — the implicit-const declarator face.
    let lints = Fixture::parse(
        "void f() {\n  const a = A(0);\n}\nclass A {\n  const A(int value);\n  static const zero = A(0);\n}\n",
    )
    .run(&one_rule("use_named_constants"));
    assert_eq!(lints.len(), 1, "{lints:?}");

    // The stateless loose match: `A(1)` equals `zero = A(0)` when the
    // ctor discards the parameter (empty constant state).
    let lints = Fixture::parse(
        "void f() {\n  const A(1);\n}\nclass A {\n  const A(int value);\n  // ignore: unused_field\n  static const _zero = A(0);\n}\n",
    )
    .run(&one_rule("use_named_constants"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use the constant 'A._zero' rather than a constructor returning the same object."
    );

    // A bare `A.zero` read is not a reconstruction — silent.
    let fixture = Fixture::parse(
        "void f() {\n  A.zero;\n}\nclass A {\n  const A(int value);\n  static const zero = A(0);\n}\n",
    );
    assert!(fixture.run(&one_rule("use_named_constants")).is_empty());
}

#[test]
fn avoid_redundant_argument_values_meta_annotation_faces() {
    let report = |source: &str| {
        Fixture::parse(source)
            .run(&one_rule("avoid_redundant_argument_values"))
            .len()
    };
    // The vendored-meta face: the pinned `''` default reports.
    assert_eq!(report("@Immutable('')\nclass C {}\n"), 1);
    assert_eq!(report("@Literal('')\nclass C {}\n"), 1);
    // A different value stays silent.
    assert_eq!(report("@Immutable('x')\nclass C {}\n"), 0);
    // A named argument cannot match the positional-only parameter.
    assert_eq!(report("@Immutable(reason: '')\nclass C {}\n"), 0);
    // A same-file `Immutable` class shadows the meta face; its
    // constructor has no defaults, so `''` stays silent.
    assert_eq!(
        report(
            "class Immutable {\n  const Immutable(String reason);\n}\n@Immutable('')\nclass C {}\n",
        ),
        0
    );
}

/// `avoid_redundant_argument_values` production-bridge pins: named
/// and optional-positional defaults equal to the written argument
/// report at the argument expression; a different value, a
/// `@required` parameter, the implicit-`null` face, and the
/// optional-positional break decide the counts. The factory redirect
/// resolves the defaults against the target's parameters.
#[test]
fn avoid_redundant_argument_values_faces() {
    let source = "void g({int x = 1}) {}\nvoid f() {\n  g(x: 1);\n}\n";
    let lints = Fixture::parse(source).run(&one_rule("avoid_redundant_argument_values"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The value of the argument is redundant because it matches the default value."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the argument.")
    );
    // The argument expression `1` (41..42).
    assert_eq!(lints[0].range, 41..42);

    // The optional-positional face and the break after it: only the
    // last argument reports (`g(1, 2)` — `2` at 52..53).
    let lints = Fixture::parse("void g([int a = 1, int b = 2]) {}\nvoid f() {\n  g(1, 2);\n}\n")
        .run(&one_rule("avoid_redundant_argument_values"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 52..53);

    // The implicit-`null` face: a written `null` against a default-less
    // optional (`g(null)` — 35..39).
    let lints = Fixture::parse("void g([int? x]) {}\nvoid f() {\n  g(null);\n}\n")
        .run(&one_rule("avoid_redundant_argument_values"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 35..39);

    // The factory redirect: `A(x: 1)` resolves through `= B` to `B`'s
    // constructor defaults (`1` at 115..116).
    let lints = Fixture::parse(
        "class A {\n  const factory A({int x = 1}) = B;\n}\nclass B implements A {\n  const B({int x = 1});\n}\nvoid f() {\n  A(x: 1);\n}\n",
    )
    .run(&one_rule("avoid_redundant_argument_values"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 115..116);

    for source in [
        // A different value never reports.
        "void g({int x = 1}) {}\nvoid f() {\n  g(x: 2);\n}\n",
        // A `@required` parameter keeps its argument.
        "void g({@required int x = 1}) {}\nvoid f() {\n  g(x: 1);\n}\n",
        // A required positional cannot be written.
        "void g(int x) {}\nvoid f() {\n  g(1);\n}\n",
    ] {
        let fixture = Fixture::parse(source);
        assert!(
            fixture
                .run(&one_rule("avoid_redundant_argument_values"))
                .is_empty(),
            "{source}"
        );
    }
}

/// The evaluated-value equality (`checkArgument`'s `computeConstantValue`
/// comparison, :71-80): values spelled differently but equal report
/// where upstream does, unequal values stay silent, num kinds never
/// cross (`DartObjectImpl` equality is type-equal), a throwing constant
/// never matches — not even at the identical spelling (`hasKnownValue`
/// false) — and an escape-decoded string equals its cooked spelling.
#[test]
fn avoid_redundant_argument_values_evaluated_equality() {
    for source in [
        // The documented textual under-report: `1 + 1` equals the `2`
        // default.
        "void g({int x = 2}) {}\nvoid f() {\n  g(x: 1 + 1);\n}\n",
        // Identical multi-token spellings still report.
        "void g({int x = 1 + 1}) {}\nvoid f() {\n  g(x: 1 + 1);\n}\n",
        // `'\x41'` cooks to `'A'` on both faces.
        "void g({String s = 'A'}) {}\nvoid f() {\n  g(s: '\\x41');\n}\n",
    ] {
        let lints = Fixture::parse(source).run(&one_rule("avoid_redundant_argument_values"));
        assert_eq!(lints.len(), 1, "{source}");
    }
    for source in [
        // A different evaluated value never reports.
        "void g({int x = 2}) {}\nvoid f() {\n  g(x: 1 + 2);\n}\n",
        // `2` and `2.0` are distinct `DartObjectImpl`s (int vs double).
        "void g({num x = 2}) {}\nvoid f() {\n  g(x: 2.0);\n}\n",
        // A throwing default has no known value — the same spelling of
        // the argument stays silent with it.
        "void g({int x = 1 ~/ 0}) {}\nvoid f() {\n  g(x: 1 ~/ 0);\n}\n",
    ] {
        let fixture = Fixture::parse(source);
        assert!(
            fixture
                .run(&one_rule("avoid_redundant_argument_values"))
                .is_empty(),
            "{source}"
        );
    }
}

/// `avoid_slow_async_io` pins: the wave posture keeps the plain run
/// silent (the production-bridge guardrail); the semantic face
/// reports the whole invocation for the instance `Directory.exists`
/// and the `FileSystemEntity.type` static, and the sync twin stays
/// silent.
#[test]
fn avoid_slow_async_io_faces() {
    let instance = "import 'dart:io';\nvoid f(Directory dir) async {\n  await dir.exists();\n}\n";
    let fixture = Fixture::parse(instance);
    assert!(fixture.run(&one_rule("avoid_slow_async_io")).is_empty());
    let lints = run_semantic(instance, "avoid_slow_async_io");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Use of an async 'dart:io' method.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try using the synchronous version of the method.")
    );
    // The whole invocation (`dir.exists()` — 56..68).
    assert_eq!(lints[0].range, 56..68);

    // The static face (`FileSystemEntity.type(p)` — 39..63).
    let lints = run_semantic(
        "import 'dart:io';\nvoid f(String p) {\n  FileSystemEntity.type(p);\n}\n",
        "avoid_slow_async_io",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 39..63);

    // `File.lastModified` reports; the sync twin stays silent.
    let lints = run_semantic(
        "import 'dart:io';\nvoid f(File file) {\n  file.lastModified();\n}\n",
        "avoid_slow_async_io",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        run_semantic(
            "import 'dart:io';\nvoid f(Directory dir) {\n  dir.existsSync();\n}\n",
            "avoid_slow_async_io",
        )
        .is_empty()
    );

    // The extension-type face (upstream
    // `test_extensionType_implementsDirectory` /
    // `test_extensionType_redeclares_exists`): a receiver declared as an
    // extension type `implements Directory` reports the invocation
    // (`ext.exists()` — 126..138); an extension type that redeclares the
    // member stays silent (the redeclared member is the extension
    // type's own, not `dart:io`'s).
    let lints = run_semantic(
        "import 'dart:io';\nextension type ExtDirExists(Directory dir) implements Directory {}\nvoid f(ExtDirExists dir) async {\n  await dir.exists();\n}\n",
        "avoid_slow_async_io",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 126..138);
    assert!(
        run_semantic(
            "import 'dart:io';\nextension type ExtFileRedeclares(File file) {\n  void exists() {}\n}\nvoid f(ExtFileRedeclares file) async {\n  file.exists();\n}\n",
            "avoid_slow_async_io",
        )
        .is_empty()
    );
}

/// `future_sync_value` pins: a non-future first argument reports at
/// the `value` token; `dynamic`, `Future`, and `FutureOr` arguments
/// stay silent; the `new`-keyword face reports; the 3.9 gate keeps
/// the rule off and 3.10 turns it on.
#[test]
fn future_sync_value_faces() {
    // The wave posture: the plain production-bridge run stays silent
    // (the argument-type face is absent).
    let fixture = Fixture::parse("Future<int> f() => Future.value(1);");
    assert!(fixture.run(&one_rule("future_sync_value")).is_empty());
    let lints = run_semantic("Future<int> f() => Future.value(1);", "future_sync_value");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "For synchronous values, `Future.syncValue` is more performant."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try replacing `Future.value` with `Future.syncValue`.")
    );
    // The constructor-name token `value` (26..31).
    assert_eq!(lints[0].range, 26..31);

    // The `new`-keyword face (`new Future.value(1)` — `value` at 30..35).
    let lints = run_semantic(
        "Future<int> f() => new Future.value(1);",
        "future_sync_value",
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range, 30..35);

    for source in [
        // A dynamic argument is exempt (:69-72).
        "Future<int> f(p) => Future.value(p);",
        // Future-typed and FutureOr-typed arguments are exempt.
        "Future<int> f(Future<int> f) => Future.value(f);",
        "Future<int> f(FutureOr<int> f) => Future.value(f);",
        // No argument at all — silent.
        "Future<int> f() => Future.value();",
    ] {
        assert!(
            run_semantic(source, "future_sync_value").is_empty(),
            "{source}"
        );
    }

    // The 3.9 gate keeps the rule off; 3.10 turns it on.
    let source = "Future<int> f() => Future.value(1);";
    assert!(
        run_semantic_version(source, "future_sync_value", LanguageVersion::new(3, 9)).is_empty()
    );
    assert_eq!(
        run_semantic_version(source, "future_sync_value", LanguageVersion::new(3, 10)).len(),
        1
    );
}

/// `unnecessary_this_alias` pins: the wave posture (the
/// production-bridge guardrail), the `is`/`as`/`== null`/`!`/if-case
/// promotion faces, the type-equality/late/mutation/no-usage
/// exemptions, and the 3.13/3.15 gate.
#[test]
fn unnecessary_this_alias_faces() {
    let is_face = "class C {\n  void m() {\n    var self = this;\n    if (self is C) {}\n  }\n}\n";
    let fixture = Fixture::parse(is_face);
    assert!(fixture.run(&one_rule("unnecessary_this_alias")).is_empty());
    let lints = run_semantic(is_face, "unnecessary_this_alias");
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary 'this' alias.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the variable and using 'this' directly.")
    );
    // The variable's name `self` (31..35).
    assert_eq!(lints[0].range, 31..35);

    for (source, count) in [
        // The `as` face.
        (
            "class C {\n  void m() {\n    var self = this;\n    (self as C).m();\n  }\n}\n",
            1,
        ),
        // The `== null` and `!= null` faces.
        (
            "class C {\n  void m() {\n    var self = this;\n    if (self == null) {}\n  }\n}\n",
            1,
        ),
        (
            "class C {\n  void m() {\n    var self = this;\n    if (null != self) {}\n  }\n}\n",
            1,
        ),
        // The `!` postfix and the null-aware `?.` marker.
        (
            "class C {\n  void m() {\n    var self = this;\n    self!.m();\n  }\n}\n",
            1,
        ),
        (
            "class C {\n  void m() {\n    var self = this;\n    self?.m();\n  }\n}\n",
            1,
        ),
        // The if-case face.
        (
            "class C {\n  void m() {\n    var self = this;\n    if (self case C _) {}\n  }\n}\n",
            1,
        ),
        // The parenthesized initializer and parenthesized usage.
        (
            "class C {\n  void m() {\n    var self = (this);\n    if ((self) == null) {}\n  }\n}\n",
            1,
        ),
    ] {
        assert_eq!(
            run_semantic(source, "unnecessary_this_alias").len(),
            count,
            "{source}"
        );
    }

    for source in [
        // A written different type never equals the `this` type.
        "class C {\n  void m() {\n    Object self = this;\n    if (self is C) {}\n  }\n}\n",
        // `late` aliases are reassignable by design.
        "class C {\n  void m() {\n    late var self = this;\n    if (self is C) {}\n  }\n}\n",
        // A mutated alias promotes unreliably.
        "class C {\n  void m() {\n    var self = this;\n    if (self is C) {}\n    self = C();\n  }\n}\n",
        // No promotion usage.
        "class C {\n  void m() {\n    var self = this;\n    print(self);\n  }\n}\n",
        // Not a `this` initializer.
        "class C {\n  void m(C other) {\n    var self = other;\n    if (self is C) {}\n  }\n}\n",
        // The non-null-aware faces stay silent.
        "class C {\n  void m() {\n    var self = this;\n    self.m();\n  }\n}\n",
    ] {
        assert!(
            run_semantic(source, "unnecessary_this_alias").is_empty(),
            "{source}"
        );
    }

    // The 3.13 gate keeps the rule off; 3.15 turns it on.
    assert!(
        run_semantic_version(
            is_face,
            "unnecessary_this_alias",
            LanguageVersion::new(3, 13)
        )
        .is_empty()
    );
    assert_eq!(
        run_semantic_version(
            is_face,
            "unnecessary_this_alias",
            LanguageVersion::new(3, 15)
        )
        .len(),
        1
    );
}
