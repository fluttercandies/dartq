use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The W7 rig: the diagnosed source sits in `package:t` with an
/// unprefixed import of the vendored meta stand-in (a
/// `package:meta/…` graph node declaring the `literal` face), the same
/// resolution shape the corpus's hand-configured `package_config`
/// provides. `with_meta = false` drops the import edge — a bare
/// `@literal` spelling then resolves only against in-file
/// declarations, the oracle's silent face.
fn literal_report(source: &str, with_meta: bool) -> FileReport {
    // The dart:core primitive stub (the W5 rig's convention): `int` is
    // not a builtin for the type-use rows, so field types would mint
    // `undefined_class` without it. Appended after the case, keeping
    // every in-case offset stable for the span assertions.
    let source: &str = &(String::from(source) + "\nclass int {}\n");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let meta_file = files.intern(Path::new("meta.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let meta_source = "library meta;\n\
                           const Object literal = _Literal();\n\
                           class _Literal { const _Literal(); }";
    let scanned_meta = dartq_scanner::scan(
        meta_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        meta_file,
    );
    assert!(scanned_meta.diagnostics.is_empty());
    let elements = collect(source, &scanned.tokens, features, &strings);
    let meta_elements = collect(meta_source, &scanned_meta.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let meta_lib = builder.intern("package:meta/meta.dart");
    builder.library_name(meta_lib, "meta");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in meta_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(meta_lib, name, meta_file);
    }
    if with_meta {
        builder.import(
            lib,
            ImportEdge {
                target: Some(meta_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(meta_file, meta_elements);
    diagnose_with(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        // The same derivation the engine's fold call site makes from
        // the settled set (the `inference-update-2` release gate).
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The corpus fixture's firing face: a non-const creation of a
/// `@literal` constructor with all-constant arguments reports at the
/// whole invocation (`Point(1, 2)`, length 11 — the probed oracle row
/// is `15|11|11`) with the verbatim problem and correction messages.
#[test]
fn literal_creation_reports_at_the_invocation() {
    let src = "import 'package:meta/meta.dart';\n\
                   class Point {\n  final int x, y;\n\n  @literal\n  const Point(this.x, this.y);\n}\n\
                   final p = Point(1, 2);\n";
    let rep = literal_report(src, true);
    assert_eq!(
        code_names(&rep),
        ["non_const_call_to_literal_constructor"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(
        d.message,
        "This instance creation must be 'const', because the Point constructor is marked as '@literal'."
    );
    assert_eq!(
        d.correction.as_deref(),
        Some("Try adding a 'const' keyword.")
    );
    let at = src.find("Point(1, 2)").expect("invocation");
    assert_eq!(d.range.offset, u32::try_from(at).unwrap());
    assert_eq!(d.range.length, 11);
}

/// The named form reports with the WRITTEN constructor-reference text
/// (`{0}` = `P5.bar` — the written reference, not the element display)
/// and the whole-invocation anchor.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn named_literal_creation_reports_with_written_text() {
    let src = "import 'package:meta/meta.dart';\n\
                   class P5 {\n  final int v;\n  @literal\n  const P5.bar(this.v);\n}\n\
                   final t = P5.bar(1);\n";
    let rep = literal_report(src, true);
    assert_eq!(
        code_names(&rep),
        ["non_const_call_to_literal_constructor"],
        "{:?}",
        code_names(&rep)
    );
    let d = &rep.diagnostics[0];
    assert_eq!(
        d.message,
        "This instance creation must be 'const', because the P5.bar constructor is marked as '@literal'."
    );
    let at = src.find("P5.bar(1)").expect("invocation");
    assert_eq!(d.range.offset, u32::try_from(at).unwrap());
    assert_eq!(d.range.length, "P5.bar(1)".len() as u32);
}

/// A const-evaluatable argument of unknown value keeps `canBeConst`
/// true (a const variable's folded value — the probe's const-var row).
#[test]
fn const_variable_argument_stays_feasible() {
    let src = "import 'package:meta/meta.dart';\n\
                   class Point {\n  final int x, y;\n  @literal\n  const Point(this.x, this.y);\n}\n\
                   const c = 1;\n\
                   final u = Point(c, 2);\n";
    let rep = literal_report(src, true);
    assert_eq!(
        code_names(&rep),
        ["non_const_call_to_literal_constructor"],
        "{:?}",
        code_names(&rep)
    );
}

/// The silent faces (each probed silent on 3.13.1): the already-`const`
/// invocation (the fixture's `q`), the non-const-evaluatable argument
/// (the fixture's `r` — a non-const top-level variable), a constructor
/// without the annotation (the import alone reports nothing), the
/// annotation on the CLASS header (the official reads constructor
/// metadata), an in-file `literal` shadowing the meta face (resolution
/// leaves meta), and a named-only class whose unnamed creation cannot
/// resolve (decision step 1).
#[test]
fn literal_creation_silent_faces() {
    let point = "import 'package:meta/meta.dart';\n\
                     class Point {\n  final int x, y;\n  @literal\n  const Point(this.x, this.y);\n}\n";
    // Already const.
    let rep = literal_report(&format!("{point}final q = const Point(3, 4);\n"), true);
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
    // Non-const argument.
    let rep = literal_report(
        &format!("{point}final notConst = 0;\nfinal r = Point(notConst, 6);\n"),
        true,
    );
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
    // No annotation on the constructor.
    let rep = literal_report(
        "import 'package:meta/meta.dart';\n\
             class Q4 {\n  final int v;\n  const Q4(this.v);\n}\n\
             final z = Q4(1);\n",
        true,
    );
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
    // The annotation sits on the class header, not the constructor.
    let rep = literal_report(
        "import 'package:meta/meta.dart';\n\
             @literal\n\
             class Q6 {\n  final int v;\n  const Q6(this.v);\n}\n\
             final w = Q6(1);\n",
        true,
    );
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
    // An in-file `literal` shadows the meta face: the annotation
    // resolves locally, never to meta.
    let rep = literal_report(
        "class literal { const literal(); }\n\
             class Q3 {\n  final int v;\n  @literal\n  const Q3(this.v);\n}\n\
             final s = Q3(1);\n",
        false,
    );
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
    // The unnamed creation cannot resolve (named-only class).
    let rep = literal_report(
        "import 'package:meta/meta.dart';\n\
             class P2 {\n  final int v;\n  @literal\n  const P2.named(this.v);\n}\n\
             final t = P2(1);\n",
        true,
    );
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
}

/// `new`-prefixed shapes stay silent: the oracle's `..._using_new`
/// twin is not in the frozen 43-code universe, so a plain-code row
/// there would be dartq-only gate damage (3.13.1 emits the plain code
/// here — the deliberate forward-compat divergence is fixture-neutral
/// and 3.15-ready).
#[test]
fn new_prefixed_creation_stays_silent() {
    let src = "import 'package:meta/meta.dart';\n\
                   class Point {\n  final int x, y;\n  @literal\n  const Point(this.x, this.y);\n}\n\
                   final n = new Point(1, 2);\n";
    let rep = literal_report(src, true);
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
}

/// A non-const constructor annotated `@literal` stays silent: the
/// oracle's `canBeConst` opens with `element == null ||
/// !element.isConst → false` (ast.dart:13743-13744), so the annotation
/// alone never reports. The gate is constructor-level — the const
/// sibling in the same class still reports (the container-level
/// const-ctor fact would wrongly pass both, #279's imprecision).
#[test]
fn non_const_literal_constructor_stays_silent() {
    let src = "import 'package:meta/meta.dart';\n\
                   class C {\n  final int v;\n  @literal\n  C(this.v);\n\n  @literal\n  const C.named(this.v);\n}\n\
                   final a = C(1);\n\
                   final b = C.named(2);\n";
    let rep = literal_report(src, true);
    assert_eq!(
        code_names(&rep),
        ["non_const_call_to_literal_constructor"],
        "{:?}",
        code_names(&rep)
    );
    let at = src.find("C.named(2)").expect("named invocation");
    let d = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_const_call_to_literal_constructor")
        .expect("named reports");
    assert_eq!(d.range.offset, u32::try_from(at).unwrap());
}

/// A non-candidate nested argument list must not steal the candidate's
/// pairing: the plain call `foo(2)` closes its own (empty) per-list
/// slot, the candidate stays armed until the creation's own `)`, and
/// the nested call's node makes `canBeConst` false — the oracle is
/// silent (a runtime call inside a would-be-const creation).
#[test]
fn nested_plain_call_keeps_candidate_pairing() {
    let src = "import 'package:meta/meta.dart';\n\
                   class Point {\n  final int x, y;\n  @literal\n  const Point(this.x, this.y);\n}\n\
                   final r = Point(1, foo(2));\n";
    let rep = literal_report(src, true);
    assert!(
        !code_names(&rep).contains(&"non_const_call_to_literal_constructor"),
        "{:?}",
        code_names(&rep)
    );
}

/// Candidate-in-candidate pairing: each list's slot closes its own
/// window — the inner creation reports at its own invocation, the
/// outer's close consumes its own arm and adjudicates on the nested
/// placeholder (the outer stays silent there; the oracle would report
/// it too — the under-report is the standing posture,
/// `close_literal_ctor`).
#[test]
fn nested_candidate_reports_inner_once() {
    let src = "import 'package:meta/meta.dart';\n\
                   class Point {\n  final int x, y;\n  @literal\n  const Point(this.x, this.y);\n}\n\
                   final r = Point(Point(1, 2), 3);\n";
    let rep = literal_report(src, true);
    let at = src.find("Point(1, 2)").expect("inner invocation");
    let d = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_const_call_to_literal_constructor")
        .expect("inner reports");
    assert_eq!(d.range.offset, u32::try_from(at).unwrap());
    assert_eq!(d.range.length, 11);
    assert_eq!(
        code_names(&rep)
            .iter()
            .filter(|name| **name == "non_const_call_to_literal_constructor")
            .count(),
        1,
        "{:?}",
        code_names(&rep)
    );
}
