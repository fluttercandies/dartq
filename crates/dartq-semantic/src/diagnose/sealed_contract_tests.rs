use super::Receiver;
use crate::collector::collect;
use crate::diagnose::{Engine, FileReport, StrictModes, Walk, diagnose_with};
use crate::library::ElementIndex;
use crate::model::{ContainerKind, MicroKind, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The two-library W6 rig: `source` is the diagnosed file of the
/// analyzed package `package:t`; `dependency` is analyzed too but
/// belongs to a foreign package per `dependency_uri`, carries the
/// `@sealed` surface, and is imported unprefixed; the meta stand-in
/// node backs the annotation's resolution face (`is_meta_getter` keys
/// the `package:meta/` uri — the vendored corpus stand-in's shape).
fn sealed_report(source: &str, dependency: &str, dependency_uri: &str) -> FileReport {
    sealed_report_prefixed(source, dependency, dependency_uri, None)
}

/// [`sealed_report`] through a prefixed import (`import ... as p;`) —
/// the prefix's merged table is the lookup surface the prefixed-callee
/// arm of `member_formals_full` resolves against.
fn prefixed_unit_report(main_source: &str, dependency: &str) -> FileReport {
    sealed_report_prefixed(main_source, dependency, "package:t/other.dart", Some("p"))
}

fn sealed_report_prefixed(
    source: &str,
    dependency: &str,
    dependency_uri: &str,
    prefix: Option<&str>,
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
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
    let scanned_dep = dartq_scanner::scan(
        dependency,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(
        scanned_dep.diagnostics.is_empty(),
        "scanner diagnostics: {dependency:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern(dependency_uri);
    let meta = builder.intern("package:meta/meta.dart");
    builder.library_name(meta, "meta");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(dep_lib, name, dep_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(dep_lib),
            prefix: prefix.map(|p| strings.intern(p)),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        dep_lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The canonical violation: an extends face over a cross-package
/// `@sealed` class reports `subtype_of_sealed_class` once with the
/// verbatim problem and correction messages.
#[test]
fn cross_package_sealed_extends_reports_verbatim() {
    let report = sealed_report(
        "import 'package:s/kit.dart'; class Sub extends Guard {}",
        "import 'package:meta/meta.dart'; @sealed class Guard {}",
        "package:s/kit.dart",
    );
    assert_eq!(code_names(&report), ["subtype_of_sealed_class"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The class 'Guard' shouldn't be extended, mixed in, or implemented because it's sealed."
    );
    assert_eq!(
        report.diagnostics[0].correction.as_deref(),
        Some(
            "Try composing instead of inheriting, or refer to the documentation of 'Guard' for more information."
        )
    );
}

// —— #2153: the library-graph name world and its consuming faces ————

/// The rig's code list without the missing-core `undefined_class` rows
/// (the two-library rig carries no dart:core face; `String`/`int`
/// return types report there).
fn codes_sans_core(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
        .into_iter()
        .filter(|code| *code != "undefined_class")
        .collect()
}

/// A switch expression over a sealed class declared in an imported
/// library resolves the subclass set through the library graph (the
/// torture `errprobe/e12` shape, oracle 3.13.1:
/// `non_exhaustive_switch_expression` at the `switch` keyword, witness
/// in the root's declaration order).
#[test]
fn cross_library_sealed_scrutinee_reports_the_missing_subclass() {
    let main = "import 'other.dart'; String twelve(Shape s) => switch (s) { Circle() => 'c' };";
    let report = foreign_unit_report(
        main,
        "sealed class Shape { const Shape(); } \
             final class Circle extends Shape { const Circle(); } \
             final class Square extends Shape { const Square(); } \
             final class Rect extends Shape { const Rect(); }",
    );
    assert_eq!(
        codes_sans_core(&report),
        ["non_exhaustive_switch_expression"],
        "{:?}",
        code_names(&report)
    );
    // The witness is the first uncovered subclass in the root
    // library's declaration order. (The oracle renders object
    // witnesses as `Square()` — a pre-existing message-text
    // divergence of the witness printer, shared with the in-file face.)
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "non_exhaustive_switch_expression")
        .expect("the covering row");
    let message = &row.message;
    assert!(
        message.starts_with("The type 'Shape' isn't exhaustively matched")
            && message.contains("'Square"),
        "{message}"
    );
    assert_eq!(row.range.offset as usize, main.find("switch").unwrap());
    // Every subclass covered: exhaustive, silent.
    let covered = foreign_unit_report(
        "import 'other.dart'; int f(S s) => switch (s) { A() => 1, B() => 2 };",
        "sealed class S {} class A extends S {} class B extends S {}",
    );
    assert!(
        codes_sans_core(&covered).is_empty(),
        "{:?}",
        code_names(&covered)
    );
}

/// A foreign enum with a missing constant reports the public row; a
/// foreign private-constant witness is the `_private` family the face
/// does not model, so it stays silent (constant_verifier.dart:1110).
#[test]
fn cross_library_enum_scrutinee_reports_and_private_witness_suppresses() {
    let report = foreign_unit_report(
        "import 'other.dart'; int f(E e) => switch (e) { E.a => 1 };",
        "enum E { a, b }",
    );
    assert_eq!(
        codes_sans_core(&report),
        ["non_exhaustive_switch_expression"],
        "{:?}",
        code_names(&report)
    );
    let private = foreign_unit_report(
        "import 'other.dart'; int f(E e) => switch (e) { E.a => 1 };",
        "enum E { a, _b }",
    );
    assert!(
        codes_sans_core(&private).is_empty(),
        "{:?}",
        code_names(&private)
    );
}

/// An object-pattern field naming no getter of a foreign head reports
/// `undefined_getter` at the shorthand's variable (the torture
/// `errprobe/e10` shape, oracle 3.13.1); declared fields, getters, and
/// `Object` members resolve silently.
#[test]
fn cross_library_object_pattern_unknown_field_reports_undefined_getter() {
    let main = "import 'other.dart'; \
                    String ten(Object o) => switch (o) { Point(:var nope) => 'x', _ => 'y' };";
    let report = foreign_unit_report(
        main,
        "class int {} class Point { final int x; const Point(this.x); int get m => x; }",
    );
    let mut codes = codes_sans_core(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        ["undefined_getter", "unused_local_variable"],
        "{:?}",
        code_names(&report)
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_getter")
        .expect("the getter row");
    assert_eq!(
        row.message,
        "The getter 'nope' isn't defined for the type 'Point'."
    );
    assert_eq!(row.range.offset as usize, main.find("nope").unwrap());
    let named = foreign_unit_report(
        "import 'other.dart'; \
             String f(Object o) => switch (o) { Point(gone: var q, :var x, :var m, :var hashCode) => '$q$x$m$hashCode', _ => 'y' };",
        "class int {} class Point { final int x; const Point(this.x); int get m => x; }",
    );
    assert_eq!(
        codes_sans_core(&named),
        ["undefined_getter"],
        "{:?}",
        code_names(&named)
    );
    let row = named
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_getter")
        .expect("the getter row");
    assert_eq!(
        row.message,
        "The getter 'gone' isn't defined for the type 'Point'."
    );
}

/// A const creation of a class whose const constructor lives in an
/// imported library is a valid constant (the torture `summary/chain`
/// shape — 93 rows of `const_initialized_with_non_constant_value` +
/// `const_with_non_const` before the resolved-container fact); a
/// foreign non-const constructor still reports both.
#[test]
fn cross_library_const_constructor_is_a_valid_const_creation() {
    let ok = foreign_unit_report(
        "import 'other.dart'; const b = A(9); const c = const A(1);",
        "class int {} class A { final int s; const A(this.s); }",
    );
    assert!(ok.diagnostics.is_empty(), "{:?}", code_names(&ok));
    let bad = foreign_unit_report(
        "import 'other.dart'; const b = N(9);",
        "class int {} class N { final int s; N(this.s); }",
    );
    assert_eq!(
        code_names(&bad),
        [
            "const_initialized_with_non_constant_value",
            "const_with_non_const"
        ],
        "{:?}",
        code_names(&bad)
    );
}

/// Same-package `@sealed` stays silent
/// (`_isLibraryInWorkspacePackage`): the dependency's uri names the
/// analyzed package `t`, so the sealed contract does not reach across.
#[test]
fn same_package_sealed_supertype_stays_silent() {
    let report = sealed_report(
        "import 'package:t/kit.dart'; class Same extends Local {}",
        "import 'package:meta/meta.dart'; @sealed class Local {}",
        "package:t/kit.dart",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// The two faces split on the DIRECT `on` constraint (the official
/// `superclassConstraints.contains` arm): a mixin whose `on` list
/// carries the sealed class reports the constraint variant, a mixin
/// merely implementing it the regular one.
#[test]
fn mixin_constraint_and_implements_faces_split() {
    let report = sealed_report(
        "import 'package:s/kit.dart'; \
             mixin NeedsGuard on Guard {} \
             mixin Adopts implements Guard {}",
        "import 'package:meta/meta.dart'; @sealed class Guard {}",
        "package:s/kit.dart",
    );
    assert_eq!(
        code_names(&report),
        ["mixin_on_sealed_class", "subtype_of_sealed_class"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The class 'Guard' shouldn't be used as a mixin constraint because it is sealed, and any class mixing in this mixin must have 'Guard' as a superclass."
    );
}

/// Per-sealed-supertype multiplicity and transitivity: two distinct
/// sealed supertypes report at one declaration each once (no break),
/// and a sealed supertype reachable only through a same-package
/// unsealed middle reports at the transitive subtype too.
#[test]
fn closure_reports_each_sealed_supertype_transitively() {
    let report = sealed_report(
        "import 'package:s/kit.dart'; \
             class Breach extends Guard implements Vault {} \
             class Mid extends Guard {} \
             class Deep extends Mid {}",
        "import 'package:meta/meta.dart'; @sealed class Guard {} @sealed class Vault {}",
        "package:s/kit.dart",
    );
    assert_eq!(
        code_names(&report),
        [
            "subtype_of_sealed_class", // Breach <- Guard
            "subtype_of_sealed_class", // Breach <- Vault
            "subtype_of_sealed_class", // Mid <- Guard
            "subtype_of_sealed_class", // Deep <- Guard, transitively
        ]
    );
}

/// The closure walk is cycle-safe: a cyclic hierarchy terminates and
/// reports once per declaring node, never per path.
#[test]
fn cyclic_hierarchy_reports_each_node_once() {
    let report = sealed_report(
        "import 'package:s/kit.dart'; \
             class A extends B {} \
             class B extends A implements C {}",
        "import 'package:meta/meta.dart'; @sealed class C {}",
        "package:s/kit.dart",
    );
    // A's closure holds B and the sealed C; B's revisits A (the cycle)
    // before reaching C again — one row per node, no loop.
    assert_eq!(
        code_names(&report),
        ["subtype_of_sealed_class", "subtype_of_sealed_class"]
    );
}

// —— #769: the discarded-value machine (unused_result) ——————————————

/// The meta stand-in the rig analyzes like any dependency: the
/// `UseResult` class the annotation resolves through (`check_annotation`'s
/// namespace face, the import-use credit, the exported probe). The
/// package_meta stand-in it mirrors (formerly vendored, now a pub-cache
/// package) carries the full class; the rig needs the declaration face only —
/// the meta-face lanes key the uri, and the `unless` value rides the
/// source walk, never the element shapes.
const META_STANDIN: &str = "class UseResult {\n\
                                \x20 const UseResult();\n\
                                \x20 const UseResult.unless({required String parameterDefined});\n\
                                }";

/// The meta rig: `source` is the sole file of a library with one
/// import edge to the analyzed `package:meta/meta.dart` node — the
/// `imports_meta` face `@UseResult` resolves through.
fn use_result_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let meta_file = files.intern(Path::new("meta.dart"));
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
    let meta_scanned = dartq_scanner::scan(
        META_STANDIN,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        meta_file,
    );
    assert!(
        meta_scanned.diagnostics.is_empty(),
        "scanner diagnostics: {META_STANDIN:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let meta_elements = collect(META_STANDIN, &meta_scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let meta = builder.intern("package:meta/meta.dart");
    builder.library_name(meta, "meta");
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
        builder.declare(meta, name, meta_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
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

/// A bare discarded `f()` fires at the callee's name token; the same
/// result consumed as an argument, an assignment RHS, or a variable
/// initializer stays silent (`_isUsed`'s parent sets; the
/// `ArgumentList` seal covers only the list's own expressions — the
/// callee's candidate predates the watermark).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn unused_result_bare_call_fires_used_shapes_silent() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   @UseResult()\nint f() => 1;\n\
                   int use(int x) => x;\n\
                   int? sink;\n\
                   void main() {\n  f();\n  use(f());\n  sink = f();\n  var y = f();\n  sink = y;\n}";
    let report = use_result_report(src);
    assert_eq!(code_names(&report), ["unused_result"]);
    let d = &report.diagnostics[0];
    assert_eq!(d.message, "The value of 'f' should be used.");
    let head = src.find("void main() {\n  f();").unwrap() + "void main() {\n  ".len();
    assert_eq!(d.range.offset, head as u32);
    assert_eq!(d.range.length, 1);
}

/// The constructor face anchors the WHOLE `A()` (`nodeToAnnotate`)
/// with the class name as the display (`A.new` would be the main-branch
/// drift).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn unused_result_constructor_anchors_whole_invocation() {
    let src = "import 'package:meta/meta.dart';\n\
                   class A {\n  @UseResult()\n  A();\n}\n\
                   void main() {\n  A();\n}";
    let report = use_result_report(src);
    assert_eq!(code_names(&report), ["unused_result"]);
    let d = &report.diagnostics[0];
    assert_eq!(d.message, "The value of 'A' should be used.");
    let head = src.find("void main() {\n  A();").unwrap() + "void main() {\n  ".len();
    assert_eq!(d.range.offset, head as u32);
    assert_eq!(d.range.length, 3);
}

/// `await`, parentheses, and `!` recurse to the inner call (`_isUsed`'s
/// transparent arms — all three fire); the null assert co-fires its own
/// diagnostic.
#[test]
fn unused_result_transparent_wrappers_still_fire() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   @UseResult()\nint f() => 1;\n\
                   void main() async {\n  await f();\n  (f());\n  f()!;\n}";
    let report = use_result_report(src);
    let mut codes = code_names(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "unnecessary_non_null_assertion",
            "unused_result",
            "unused_result",
            "unused_result",
        ]
    );
}

/// The `unless` faces: `g(x: 1)` satisfies `parameterDefined: 'x'` and
/// stays silent; `f()` (no argument) fires; f's annotation also
/// co-reports `undefined_referenced_parameter` at the written `'x'`
/// value expression — quotes included (`_findUndefinedUseResultParameter`
/// anchors at the expression) — while g's parameter exists and never
/// co-reports.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn unless_bypass_and_undefined_parameter_corow() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   @UseResult.unless(parameterDefined: 'x')\nvoid g({required int x}) {}\n\
                   @UseResult.unless(parameterDefined: 'x')\nvoid f({required int y}) {}\n\
                   void main() {\n  g(x: 1);\n  f();\n}";
    let report = use_result_report(src);
    // `f()` misses `f`'s required named `y` — the oracle's
    // `missing_required_argument` co-row (probed 3.13.1:
    // `error - missing_required_argument` at the `f()` entity).
    assert_eq!(
        code_names(&report),
        [
            "undefined_referenced_parameter",
            "missing_required_argument",
            "unused_result"
        ]
    );
    let corow = &report.diagnostics[0];
    assert_eq!(corow.message, "The parameter 'x' isn't defined by 'f'.");
    let at = src.find("'x')\nvoid f").unwrap();
    assert_eq!(corow.range.offset, at as u32);
    assert_eq!(corow.range.length, 3);
}

/// A reference argument gates on the const value but messages the
/// written text (`toStringValue` gates, `toString` messages — the
/// official arms at `annotation_verifier.dart:531-537`): `s` folds to
/// 'undef', `f` names no such formal — the co-row anchors at the
/// written identifier and names it, while the value gates the bypass.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn unless_reference_reports_the_written_text() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\nclass String {}\n\
                   const String s = 'undef';\n\
                   @UseResult.unless(parameterDefined: s)\nvoid f([int? value]) {}\n\
                   void main() {\n  f();\n}";
    let report = use_result_report(src);
    assert_eq!(
        code_names(&report),
        ["undefined_referenced_parameter", "unused_result"]
    );
    let corow = &report.diagnostics[0];
    assert_eq!(corow.message, "The parameter 's' isn't defined by 'f'.");
    let at = src.find("s)\nvoid f").unwrap();
    assert_eq!(corow.range.offset, at as u32);
    assert_eq!(corow.range.length, 1);
}

/// The gate compares the const value, not the written name: `name`
/// folds to 'value', the formal `value` satisfies it — silent, and the
/// supplied `value:` argument engages the bypass. Naming the written
/// identifier instead would false-positive here.
#[test]
fn unless_reference_gate_reads_the_const_value() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\nclass String {}\n\
                   const String name = 'value';\n\
                   @UseResult.unless(parameterDefined: name)\nvoid g({int? value}) {}\n\
                   void main() {\n  g(value: 1);\n}";
    let report = use_result_report(src);
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// A non-const reference never gates (`computeConstantValue` null):
/// no co-row — the discarded call still fires its own row.
#[test]
fn unless_non_const_reference_stays_silent() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   final String t = 'undef';\n\
                   @UseResult.unless(parameterDefined: t)\nvoid h([int? value]) {}\n\
                   void main() {\n  h();\n}";
    let report = use_result_report(src);
    let codes = code_names(&report);
    assert!(
        !codes.contains(&"undefined_referenced_parameter"),
        "{codes:?}"
    );
}

/// The for-parts split: the condition alone is a use (the official
/// `ForParts` arm, `use_result_verifier.dart:168-172` — `return
/// parent.condition2 == node` intercepts before `ForLoopParts`); the
/// initializer and the updater both discard, and the int-typed
/// condition additionally reports `non_bool_condition` (the
/// `test_for` fixture face, upstream Dart SDK source,
/// pkg/analyzer/test/src/diagnostics/non_bool_condition_test.dart).
#[test]
fn for_initializer_and_condition_silent_updater_fires() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   @UseResult()\nint f() => 1;\n\
                   void main() {\n  for (f(); f(); f()) {}\n}";
    let report = use_result_report(src);
    assert_eq!(
        code_names(&report),
        ["unused_result", "non_bool_condition", "unused_result"]
    );
    let d = &report.diagnostics[0];
    let head = src.find("for (").unwrap();
    assert_eq!(d.range.offset as usize, head + "for (".len());
    let last = &report.diagnostics[2];
    assert_eq!(last.range.offset as usize, head + "for (f(); f(); ".len());
}

/// The cascade split: the target is a use, the section is not. The
/// section's callee resolves on the cascade target's static type
/// (`int`, the declared return of `f`) — `g` is no member of it, so
/// the section reports the oracle's member miss (probed 3.13.1:
/// "The method 'g' isn't defined for the type 'int'.") and carries no
/// `unused_result` (an undefined callee joins no `@UseResult` credit).
#[test]
fn cascade_target_silent_section_fires() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   @UseResult()\nint f() => 1;\n\
                   @UseResult()\nint g() => 2;\n\
                   void main() {\n  f()..g();\n}";
    let report = use_result_report(src);
    assert_eq!(code_names(&report), ["undefined_method"]);
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'g' isn't defined for the type 'int'."
    );
    let head = src.find("f()..g()").unwrap();
    assert_eq!(
        report.diagnostics[0].range.offset as usize,
        head + "f()..".len()
    );
}

/// The member faces fire at the selector token: instance method,
/// getter, and field-read rows (the field's implicit getter unwraps to
/// the annotated variable). The receiver carries a declared type — the
/// model deliberately under-reports `var`-inferred receivers (the M1
/// posture), so the inferred shape can never reach the member walk.
#[test]
fn unused_result_member_faces_fire() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   class C {\n  @UseResult()\n  int get g => 1;\n\
                   @UseResult()\n  int m() => 2;\n\
                   @UseResult()\n  int field = 3;\n}\n\
                   void main() {\n  C c = C();\n  c.g;\n  c.m;\n  c.field;\n}";
    let report = use_result_report(src);
    assert_eq!(
        code_names(&report),
        ["unused_result", "unused_result", "unused_result"]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The value of 'g' should be used."
    );
}

/// An `unless` face on a getter never co-reports (the official walk's
/// null `parameterList` arm) while the discarded read itself fires.
/// The receiver carries a declared type (the model under-reports
/// `var`-inferred receivers).
#[test]
fn unless_on_getter_never_reports_parameter_corow() {
    let src = "import 'package:meta/meta.dart';\n\
                   class int {}\n\
                   class C {\n  @UseResult.unless(parameterDefined: 'x')\n  int get g => 1;\n}\n\
                   void main() {\n  C c = C();\n  c.g;\n}";
    let report = use_result_report(src);
    assert_eq!(code_names(&report), ["unused_result"]);
}

// —— G4 phase-2 T-B: the cross-pool projection boundary —————————————

/// [`sealed_report`] under the boundary tests' name: the dependency is
/// a real second unit, indexed under its own file id, so its declared
/// types carry genuinely foreign [`crate::model::ArgListId`] slots.
fn foreign_unit_report(main_source: &str, dependency: &str) -> FileReport {
    sealed_report(main_source, dependency, "package:t/other.dart")
}

/// The boundary's entry memo keeps one projected copy per foreign
/// member: the same inherited member surfaced through two direct faces
/// (the superclass and an interface extending it) stays pointer-equal,
/// so the face-chain dedup still emits a single verdict, and the
/// message renders the inherited signature with its arguments through
/// the diagnosing pool, pool-valid for message rendering. The
/// dependency declares its own `int`/`List` spellings so `S.m`'s
/// formal carries a real declared type — an undeclared spelling is a
/// genuine `InvalidType` there, and the #2192 leniency (the oracle's
/// own InvalidType posture) leaves that member mismatch silent.
#[test]
fn foreign_member_surfaces_once_across_two_faces() {
    let report = foreign_unit_report(
        "import 'other.dart'; class int {} class List<T> {} \
             class B extends S implements I { void m(List<int> x) {} }",
        "class int {} class List<T> {} \
             class S { void m(List<int>? x) {} } class I extends S {}",
    );
    assert_eq!(
        code_names(&report),
        ["invalid_override"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "'B.m' ('void Function(List<int>)') isn't a valid override of 'S.m' ('void Function(List<int>?)')."
    );
}

/// A foreign-origin property column (F4) renders through the boundary
/// pool: `x.a = null` decides on the projected `List<int>` and the
/// message names it exactly — main's own pool never held the slot.
#[test]
fn foreign_property_target_renders_in_assignment_verdict() {
    let report = foreign_unit_report(
        "import 'other.dart'; void f(A x) { x.a = null; }",
        "class A { List<int> a = []; }",
    );
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'Null' can't be assigned to a variable of type 'List<int>'."
    );
}

/// A static call column on an imported container resolves through
/// `member_formals_full`'s direct `Receiver::Container` arm: the
/// named-argument membership face — the column that reaches a static
/// callee from a full parse — answers off the projected table without
/// touching a foreign slot. The positional face rides the same arm via
/// the unmerged placeholder (`check_arguments` classifies the receiver
/// row below the callee before the dot-access merge), and decides the
/// definite `C`-against-`D` miss through the subtype walk. The target
/// must resolve in the asking scope: an unresolvable formal type (the
/// SDK spellings this bare harness cannot see) is the oracle's
/// InvalidType target — the faces stay silent (CLI-probed identical to
/// the oracle on the `A.m(c)`-into-`List<int>` shape).
#[test]
fn foreign_static_call_column_resolves_through_the_boundary() {
    let report = foreign_unit_report(
        "import 'other.dart'; class C {} class D {} void f(C c) { A.m(c); A.m(bad: 1); }",
        "class A { static void m(D x) {} }",
    );
    // The arity check co-fires beside the named-argument miss
    // (`type_analyzer.dart:5843` — the named loop and the deficit check
    // are independent; capture-proven on the `f(b: 1)` shapes).
    let mut codes = code_names(&report);
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "argument_type_not_assignable",
            "not_enough_positional_arguments",
            "undefined_named_parameter"
        ],
        "{:?}",
        code_names(&report)
    );
    let arity = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "not_enough_positional_arguments")
        .expect("arity row present");
    assert_eq!(
        arity.message,
        "1 positional argument expected by 'm', but 0 found."
    );
}

/// The same static call column checks its positional arguments end to
/// end: the foreign formal's `List<int>` renders through the boundary
/// pool — the exact projection the direct-engine differential below
/// pins for the table itself, now live in the verdict message.
#[test]
fn foreign_static_call_positional_argument_renders_through_the_boundary() {
    let report = foreign_unit_report(
        "import 'other.dart'; void f() { A.m(null); }",
        "class A { static void m(List<int> x) {} }",
    );
    assert_eq!(
        code_names(&report),
        ["argument_type_not_assignable"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The argument type 'Null' can't be assigned to the parameter type 'List<int>'. "
    );
}

/// The projection differential for that arm: the resolved container
/// table's `List<int>` formal renders exactly through the boundary pool.
/// Main's own pool is empty, so the one interned slot is the
/// projection's — an unprojected foreign slot has no local slot to read
/// and the render cannot decide at all.
#[test]
fn foreign_static_call_formals_render_through_the_boundary() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("other.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let main_source = "import 'other.dart'; class C {} void f(C c) { A.m(c); }";
    let dep_source = "class A { static void m(List<int> x) {} }";
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let scanned_dep = dartq_scanner::scan(
        dep_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(scanned_dep.diagnostics.is_empty());
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dep_source, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern("package:t/other.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib, name, file);
        }
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(dep_lib, name, dep_file);
        }
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
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    let tokens: &'static TokenTable = Box::leak(Box::new(scanned.tokens));
    let posture = LibraryClassModifiers::default();
    let engine = Engine::new(
        &graph,
        &namespaces,
        &index,
        &strings,
        main_source,
        tokens,
        file,
        lib,
        false,
        false,
        false,
        true,
        StrictModes::default(),
        &posture,
        None,
    );
    let recv = Receiver::Container {
        name: strings.intern("A"),
        kind: ContainerKind::Class,
    };
    let formals = engine
        .member_formals_full(&recv, strings.intern("m"))
        .expect("the imported static method resolves");
    let render = |ty: Ty| ty.render(&strings, &engine.boundary.args.borrow());
    assert_eq!(render(formals.positional[0].ty), "List<int>");
    assert_eq!(
        engine.boundary.args.borrow().len(),
        1,
        "the projected formal is the boundary pool's one slot"
    );
}

/// An imported top-level variable (F5) rides its projected type into
/// the same verdict face — the bare read's row is boundary-local.
#[test]
fn foreign_top_level_variable_renders_in_assignment_verdict() {
    let report = foreign_unit_report(
        "import 'other.dart'; void f() { top = null; }",
        "List<int> top = [];",
    );
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "A value of type 'Null' can't be assigned to a variable of type 'List<int>'."
    );
}

/// A prefixed top-level callee (`p.f(null)` through `import as p`)
/// checks its positional arguments against the target's projected
/// formals — the same verdict the unqualified `f(null)` shape gives,
/// with the foreign `List<int>` slot rendered through the boundary
/// pool (`member_formals_full`'s `Receiver::Prefix` arm).
#[test]
fn prefixed_top_level_call_checks_positional_arguments() {
    let report = prefixed_unit_report(
        "import 'other.dart' as p; void f() { p.g(null); }",
        "void g(List<int> x) {}",
    );
    assert_eq!(
        code_names(&report),
        ["argument_type_not_assignable"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The argument type 'Null' can't be assigned to the parameter type 'List<int>'. "
    );
}

/// The same arm is strictly a projection, not a new report source: an
/// assignable argument through a resolved prefixed callee stays
/// silent, and a callee the prefix table does not name carries the
/// invocation face's own row — `undefined_function`, the oracle's
/// prefixed invocation-miss face (upstream Dart SDK source,
/// pkg/analyzer/lib/src/error/lookup_failure_reporter.dart,
/// `PrefixedLookupDomain._readFailure`: a prefixed lookup miss with
/// `ReadSyntax.invocation` reports `undefinedFunction`) — while the
/// argument verdict stays suppressed (the argument-bearing prefixed
/// miss never mints one).
#[test]
fn prefixed_top_level_call_argument_misses_stay_quiet() {
    let pass = prefixed_unit_report(
        "import 'other.dart' as p; void f() { p.g(1); }",
        "void g(int x) {}",
    );
    assert!(pass.diagnostics.is_empty(), "{:?}", code_names(&pass));

    let miss = prefixed_unit_report(
        "import 'other.dart' as p; void f() { p.gone(null); }",
        "void g(int x) {}",
    );
    assert_eq!(code_names(&miss), ["undefined_function"]);
}

/// An unregistered prefix never reaches the formals arm: the lookup
/// misses before any projection, so no argument verdict can be minted
/// off an unresolved prefix.
#[test]
fn unregistered_prefix_callee_suppresses_the_argument_check() {
    let report = prefixed_unit_report(
        "import 'other.dart' as p; void f() { nosuch.g(null); }",
        "void g(int x) {}",
    );
    assert!(!code_names(&report).contains(&"argument_type_not_assignable"));
}

/// An imported generic member's row type and a local annotation land in
/// one pool (the boundary), so a genuinely different cast target never
/// compares equal to the foreign slot — the cross-pool numeric-slot
/// collision cannot mint a false `unnecessary_cast`.
#[test]
fn foreign_row_type_never_false_positives_unnecessary_cast() {
    let report = foreign_unit_report(
        "import 'other.dart'; class int {} class List<T> {} class Map<K, V> {} \
             void f(A x) { x.a as List<String>; }",
        "class A { List<int> a = []; }",
    );
    assert!(
        !code_names(&report).contains(&"unnecessary_cast"),
        "{:?}",
        code_names(&report)
    );
}

/// The boundary unit face: a foreign member walk re-interns the
/// declared type into the diagnosing pool, the slot memo dedups the
/// shared list across members and repeat probes, and a foreign
/// top-level declaration projects once per diagnosis with a stable
/// projected identity.
#[test]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn boundary_reinterns_foreign_types_and_memoizes_slots() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("other.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let main_source = "import 'other.dart'; void f(A x) { x.a; x.b; }";
    let dep_source = "class A { List<int> a = []; List<int> b = []; } List<int> top = [];";
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty());
    let scanned_dep = dartq_scanner::scan(
        dep_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(scanned_dep.diagnostics.is_empty());
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dep_source, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern("package:t/other.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(lib, name, file);
        }
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind != MicroKind::TypeParam {
            builder.declare(dep_lib, name, dep_file);
        }
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
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    let tokens: &'static TokenTable = Box::leak(Box::new(scanned.tokens));
    let posture = LibraryClassModifiers::default();
    let engine = Engine::new(
        &graph,
        &namespaces,
        &index,
        &strings,
        main_source,
        tokens,
        file,
        lib,
        false,
        false,
        false,
        true,
        StrictModes::default(),
        &posture,
        None,
    );
    // Main's own pool is empty; the foreign `List<int>` field type is
    // the boundary pool's one non-empty slot.
    let container = strings.intern("A");
    let first = match engine.walk_members(container, strings.intern("a")) {
        Walk::Found(entry) => entry,
        walk => panic!("expected a found walk, got {walk:?}"),
    };
    let render = |ty: Ty| ty.render(&strings, &engine.boundary.args.borrow());
    assert_eq!(render(first.var_ty), "List<int>");
    assert_eq!(engine.boundary.args.borrow().len(), 1);
    // A second member sharing the same argument list dedups into the
    // same slot — structural equality across entries, one copy.
    let second = match engine.walk_members(container, strings.intern("b")) {
        Walk::Found(entry) => entry,
        walk => panic!("expected a found walk, got {walk:?}"),
    };
    assert_eq!(second.var_ty, first.var_ty);
    assert_eq!(
        engine.boundary.args.borrow().len(),
        1,
        "the slot memo dedups the shared list across members"
    );
    // A repeat probe serves the memoized member — the pool stays put.
    match engine.walk_members(container, strings.intern("a")) {
        Walk::Found(again) => assert_eq!(render(again.var_ty), "List<int>"),
        walk => panic!("expected a found walk, got {walk:?}"),
    }
    assert_eq!(engine.boundary.args.borrow().len(), 1);
    // A foreign top-level declaration projects once per diagnosis, and
    // its projected identity is stable across lookups.
    let top = strings.intern("top");
    let decl = engine
        .top_level_decl(top)
        .expect("the imported declaration resolves");
    assert_eq!(render(decl.var_ty), "List<int>");
    let again = engine
        .top_level_decl(top)
        .expect("the imported declaration resolves");
    assert_eq!(decl.ptr(), again.ptr(), "the decl memo serves one copy");
}
