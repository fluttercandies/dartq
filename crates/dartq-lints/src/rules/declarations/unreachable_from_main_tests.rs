use crate::engine::{Lint, ReachabilityFace};
use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The `(nth-occurrence UTF-16 offset, reachable)` face flag of `name`.
fn flag(f: &Fixture, name: &str, nth: usize, reachable: bool) -> (u32, bool) {
    (utf16_at(f, tok(&f.source, &f.tokens, name, nth)), reachable)
}

/// Runs the rule with the given face mounted (flags sorted ascending,
/// the seam contract).
fn run(f: &Fixture, has_entry: bool, entries: &[(u32, bool)]) -> Vec<Lint> {
    let mut flags = entries.to_vec();
    flags.sort_unstable_by_key(|&(offset, _)| offset);
    let face = ReachabilityFace {
        has_entry,
        flags: &flags,
    };
    f.run_with(&one_rule("unreachable_from_main"), None, Some(&face), None)
}

/// `test_topLevelFunction_unreachable` under an entry-less library —
/// `if (entryPoints.isEmpty) return`: nothing reports at all. The entry
/// read is LOCAL (the official walk is per-library): no `main`, no
/// exempting annotation — the face's `has_entry` cannot revive it.
#[test]
fn library_without_entry_point_reports_nothing() {
    let f = Fixture::parse("void stray() {}\n");
    assert!(run(&f, false, &[flag(&f, "stray", 0, false)]).is_empty());
    // The imported-file shape: the workspace graph marked the row
    // reachable (`has_entry` true), but this library declares no entry.
    assert!(run(&f, true, &[flag(&f, "stray", 0, true)]).is_empty());
}

/// `test_topLevelFunction_unreachable` — an unreferenced top-level
/// function reports at its name; the message template is verbatim.
#[test]
fn unreachable_top_level_function_reports() {
    let f = Fixture::parse("void main() {}\nvoid stray() {}\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "stray", 0, false)],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'stray' in an executable library."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try referencing the member or removing it.")
    );
    assert_eq!(lints[0].range.start, flag(&f, "stray", 0, false).0);
}

/// `test_topLevelFunction_reachable`: a locally referenced declaration
/// stays silent however the face flags it, and a flag row absent from
/// the face is the conservative no-report. A `Some(true)` face row with
/// no local reference reports — the engine graph is workspace-global
/// (a type annotation, another library's edge); the closure is
/// authoritative for a single-file library. Oracle-verified
/// (`dart analyze`): the closure answers both directions — `used`
/// reachable from `main` while `k` reports, then `used` reporting too
/// once `main` drops the call.
#[test]
fn reachable_and_unrowed_declarations_stay_silent() {
    let f = Fixture::parse("void main() {\n  used();\n}\nvoid used() {}\nint k = 0;\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "used", 0, false)],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'k' in an executable library."
    );
    let f = Fixture::parse("void main() {}\nvoid used() {}\nint k = 0;\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "used", 0, true)],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'used' in an executable library."
    );
    assert_eq!(
        lints[1].message,
        "Unreachable member 'k' in an executable library."
    );
}

/// `test_topLevelFunction_reachable_private` — private declarations
/// carry flags but never report.
#[test]
fn private_declarations_never_report() {
    let f = Fixture::parse("void main() {}\nvoid _dead() {}\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "_dead", 0, false)],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_topLevelFunction_unreachable_visibleForTesting` — the
/// annotation exempts; without it the same declaration reports.
#[test]
fn visible_for_testing_annotation_exempts() {
    let f = Fixture::parse("void main() {}\n\n@visibleForTesting\nvoid probed() {}\n");
    assert!(
        run(
            &f,
            true,
            &[flag(&f, "main", 0, true), flag(&f, "probed", 0, false)]
        )
        .is_empty()
    );
    let f = Fixture::parse("void main() {}\nvoid probed() {}\n");
    assert_eq!(
        run(
            &f,
            true,
            &[flag(&f, "main", 0, true), flag(&f, "probed", 0, false)]
        )
        .len(),
        1
    );
}

/// `test_topLevelVariable_unreachable` — a multi-variable top-level
/// declaration splits per variable, each judged at its own name.
#[test]
fn top_level_variables_split_per_variable() {
    let f = Fixture::parse("void main() {\n  print(used);\n}\nint used = 0, dead = 0;\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "used", 0, false),
            flag(&f, "dead", 0, false),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'dead' in an executable library."
    );
}

/// `test_class_instanceMethod_unreachable` +
/// `test_class_unreachable` — a dead member of a reachable container
/// reports; a dead container reports alone (outermost container wins).
#[test]
fn dead_member_of_reachable_class_reports_dead_class_wins() {
    let f = Fixture::parse(
        "void main() {\n  C();\n}\n\nclass C {\n  static void stray() {}\n}\n\nclass D {\n  static void dead() {}\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 1, true),
            flag(&f, "stray", 0, false),
            flag(&f, "D", 0, false),
            flag(&f, "dead", 0, false),
        ],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'stray' in an executable library."
    );
    assert_eq!(
        lints[1].message,
        "Unreachable member 'D' in an executable library."
    );
}

/// `test_constructor_named_new` / unreachable named constructor — a
/// named constructor displays `Type.name`; the unnamed face displays
/// `Type.new`, anchored at its own class-name echo. `main`'s `C.x`
/// keeps the class reachable without counting either constructor.
#[test]
fn constructors_display_qualified_names() {
    let f = Fixture::parse("void main() {\n  C.x;\n}\n\nclass C {\n  C.named();\n  C();\n}\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 1, true),
            flag(&f, "named", 0, false),
            flag(&f, "C", 3, false),
        ],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C.named' in an executable library."
    );
    assert_eq!(
        lints[1].message,
        "Unreachable member 'C.new' in an executable library."
    );
}

/// `test_constructor_named_onEnum` (the exclusion face) and
/// `test_enum_reachableViaValue`'s dead twin: an enum's constructors
/// and constants never gather; its dead static members report.
#[test]
fn enum_constructors_and_constants_never_gather() {
    let f = Fixture::parse(
        "void main() {\n  E.a;\n}\n\nenum E {\n  a, b;\n\n  const E._();\n\n  static void stray() {}\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "E", 1, true),
            flag(&f, "stray", 0, false),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'stray' in an executable library."
    );
}

/// The test-harness names never gather (`isTestMethod`).
#[test]
fn test_harness_method_names_never_report() {
    let f = Fixture::parse(
        "void main() {\n  T();\n}\n\nclass T {\n  void test_one() {}\n  void solo_test_two() {}\n  void setUp() {}\n  void tearDown() {}\n  void setUpClass() {}\n  void tearDownClass() {}\n}\n",
    );
    // `main` is the entry and `T` is reachable; the six unreachable
    // test-harness methods never gather, so nothing reports.
    let entries = [
        flag(&f, "main", 0, true),
        flag(&f, "T", 1, true),
        flag(&f, "test_one", 0, false),
        flag(&f, "solo_test_two", 0, false),
        flag(&f, "setUp", 0, false),
        flag(&f, "tearDown", 0, false),
        flag(&f, "setUpClass", 0, false),
        flag(&f, "tearDownClass", 0, false),
    ];
    let lints = run(&f, true, &entries);
    assert!(lints.is_empty(), "{lints:?}");
}

/// The widget-preview suppression resolves the annotation INTO the
/// flutter widget-previews library (upstream `hasWidgetPreview`): a
/// bare `@Preview` spelling with no flutter import does not suppress
/// (the unit declares no resolvable `Preview`), the
/// `package:flutter/widget_previews.dart` import surface does.
#[test]
fn preview_suppression_needs_the_flutter_import_surface() {
    let bare = "void main() {\n  C.x;\n}\n\nclass C {\n  @Preview\n  static void previewed() {}\n\n  static void stray() {}\n}\n";
    let f = Fixture::parse(bare);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 1, true),
            flag(&f, "previewed", 0, false),
            flag(&f, "stray", 0, false),
        ],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'previewed' in an executable library."
    );
    let imported = "import 'package:flutter/widget_previews.dart';\n\nvoid main() {\n  C.x;\n}\n\nclass C {\n  @Preview\n  static void previewed() {}\n\n  static void stray() {}\n}\n";
    let f = Fixture::parse(imported);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 1, true),
            flag(&f, "previewed", 0, false),
            flag(&f, "stray", 0, false),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'stray' in an executable library."
    );
}

/// `test_class_instanceField_unreachable` — field variables report at
/// their own names under the field row's metadata face.
#[test]
fn field_variables_report_at_their_names() {
    let f =
        Fixture::parse("void main() {\n  C.live;\n}\n\nclass C {\n  int live = 0, gone = 0;\n}\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 1, true),
            flag(&f, "live", 1, false),
            flag(&f, "gone", 0, false),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'gone' in an executable library."
    );
}

/// Typedef and extension candidates display their bare names; a dead
/// extension's members stay suppressed behind it.
#[test]
fn typedef_and_extension_display_their_names() {
    let f = Fixture::parse(
        "void main() {}\n\ntypedef F = void Function();\n\nextension Ext on int {\n  void m() {}\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "F", 0, false),
            flag(&f, "Ext", 0, false),
            flag(&f, "m", 0, false),
        ],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'F' in an executable library."
    );
    assert_eq!(
        lints[1].message,
        "Unreachable member 'Ext' in an executable library."
    );
}

/// `test_class_reachable_referencedInTypeAnnotation_external*` /
/// `…_referencedDeepInTypeAnnotation_externalMethodDeclaration`: the
/// written type of an external variable and the return type of an
/// external function mark their class reachable (the external
/// implementation can instantiate it); a PARAMETER type does not
/// (`test_class_unreachable_referencedInParameter_…`).
#[test]
fn external_variable_and_return_types_mark_reachable() {
    for source in [
        "void main() {\n  D().f;\n}\n\nclass C {}\n\nclass D {\n  external C f();\n}\n",
        "void main() {\n  D().c;\n}\n\nclass C {}\n\nclass D {\n  external C? c;\n}\n",
        "void main() {\n  D().f;\n}\n\nclass C {}\n\nclass D {\n  external C Function() f();\n}\n",
    ] {
        let f = Fixture::parse(source);
        let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
        assert!(lints.is_empty(), "{source:?} -> {lints:?}");
    }

    let f = Fixture::parse(
        "void main() {\n  D().f;\n}\n\nclass C {}\n\nclass D {\n  external f(C c);\n}\n",
    );
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C' in an executable library."
    );
}

/// `test_typedef_reachable_referencedIn*`: a `typedef A = B;` alias's
/// invocation (`T()`, `T.named()`) resolves through to B's constructor
/// element — B rides the alias's reachability. A type-parameter BOUND
/// (`<T extends C>`) never counts (`visitNamedType`'s parent is a
/// TypeParameter, not a TypeArgumentList).
#[test]
fn typedef_alias_invocation_reaches_target_but_bounds_do_not() {
    let f = Fixture::parse("void main() {\n  T();\n}\n\nclass C {}\n\ntypedef T = C;\n");
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert!(lints.is_empty(), "{lints:?}");

    let f = Fixture::parse(
        "void main() {\n  T.named();\n}\n\nclass C {\n  C.named();\n}\n\ntypedef T = C;\n",
    );
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert!(lints.is_empty(), "{lints:?}");

    let f = Fixture::parse(
        "void main() {\n  f();\n}\n\nclass C {}\n\nvoid f<T extends C>() {}\n",
    );
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C' in an executable library."
    );
}

/// `test_typedef_reachable_referencedInTypeAnnotation_topLevelVariable`:
/// a used variable's written type (`Cb? c;` with `c` assigned in
/// `main`) rides the variable's dependency set — the typedef stays
/// reachable. A reference inside a `ConstantPattern`
/// (`test_constructor_unnamed_referencedInConstantPattern`) never
/// counts (`_patternLevel`).
#[test]
fn variable_type_rides_reachability_and_const_patterns_do_not() {
    let f = Fixture::parse(
        "void main() {\n  c = () {};\n}\n\nCb? c;\n\ntypedef Cb = void Function();\n",
    );
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert!(lints.is_empty(), "{lints:?}");

    let f = Fixture::parse(
        "class C {\n  const C();\n}\n\nvoid main() {\n  f();\n}\n\nvoid f([C? c]) {\n  if (c case const C()) {}\n}\n",
    );
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C' in an executable library."
    );
}

/// `test_mixin_reachable_mixed`'s dead twin and the extension-type
/// face: mixin and extension-type names report like any container.
#[test]
fn mixin_and_extension_type_report() {
    let f = Fixture::parse("void main() {}\n\nmixin M {}\n\nextension type ET(int value) {}\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "M", 0, false),
            flag(&f, "ET", 0, false),
        ],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'M' in an executable library."
    );
    assert_eq!(
        lints[1].message,
        "Unreachable member 'ET' in an executable library."
    );
}

// —— L1 inheritance-chain family (burn wave) ————————————————————————

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
use dartq_source::{FileTable, StringTable};

/// Runs `rule` over `source` with the semantic face mounted: the source
/// folded as the sole analyzed file of `package:t` (the driver's
/// per-file build shape). `dependency` analyzes a second `kit.dart`,
/// imports it unprefixed, and indexes it — the cross-file resolution
/// face. The context tables leak (`'static`); the tests are one-shot
/// processes.
fn run_semantic(source: &str, rule: &str, dependency: Option<&str>) -> Vec<crate::engine::Lint> {
    run_semantic_face(source, rule, dependency, false, &[])
}

/// [`run_semantic`] with the reachability face mounted — the
/// dead-code face reads both the semantic bridge and the engine's
/// reachability flags.
fn run_semantic_face(
    source: &str,
    rule: &str,
    dependency: Option<&str>,
    has_entry: bool,
    flags: &[(u32, bool)],
) -> Vec<crate::engine::Lint> {
    let mut flags: Vec<(u32, bool)> = flags.to_vec();
    flags.sort_unstable_by_key(|&(offset, _)| offset);
    let face = ReachabilityFace {
        has_entry,
        flags: &flags,
    };
    let mut fixture = Fixture::parse(source);
    // The unit path is a package lib file — the path-gated rules
    // (`avoid_renaming_method_parameters`' `isInLibDir`) register.
    fixture.path = std::path::PathBuf::from("lib/lint_semantic.dart");
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("lint_semantic.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
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
        index.insert(dep_file, dep_elements);
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
                non_library: false,
            },
        );
    }
    let graph: &'static dartq_resolution::LibraryGraph = Box::leak(Box::new(builder.finish()));
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
        reachability: Some(face),
        async_context_uses: None,
        library_type_uses: None,
    };
    unit.run_with_semantics(
        &one_rule(rule),
        &rustc_hash::FxHashSet::default(),
        Some(&analysis),
    )
}

// —— unnecessary_nullable_for_final_variable_declarations semantic
// initializer faces ————————————————————————————————————————

/// The bridge initializer face: an invocation returning a non-nullable
/// type reports through the callee face — the under-report shape
/// `compute()` no longer passes.
#[test]
fn unnecessary_nullable_reports_non_nullable_invocation_initializer() {
    let source = concat!(
        "int compute() => 1;\n",
        "void f() {\n",
        "  final int? x = compute();\n",
        "}\n",
    );
    let lints = run_semantic(
        source,
        "unnecessary_nullable_for_final_variable_declarations",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Type could be non-nullable.");
    let f = Fixture::parse(source);
    let x = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
}

/// A nullable-returning or unresolved invocation initializer stays
/// silent — the gate reports only a provably non-nullable static type.
#[test]
fn unnecessary_nullable_nullable_and_unknown_invocations_pass() {
    let nullable = concat!(
        "int? maybe() => 1;\n",
        "void f() {\n",
        "  final int? x = maybe();\n",
        "}\n",
    );
    assert!(
        run_semantic(
            nullable,
            "unnecessary_nullable_for_final_variable_declarations",
            None
        )
        .is_empty()
    );
    let unknown = "void f() {\n  final int? x = missing();\n}\n";
    assert!(
        run_semantic(
            unknown,
            "unnecessary_nullable_for_final_variable_declarations",
            None
        )
        .is_empty()
    );
}

/// A bare-name initializer answers at its flow read: a declared-`int`
/// read reports, and an inferred-`var` read reports too — the local
/// adopts the staged initializer type (`_inferLocalVariable`), so its
/// read is exactly `int` (the oracle reports on that fixture,
/// `INFO|LINT|UNNECESSARY_NULLABLE_FOR_FINAL_VARIABLE_DECLARATIONS`
/// at the variable name).
#[test]
fn unnecessary_nullable_read_initializer_faces() {
    let declared = "void f() {\n  int y = 1;\n  final int? x = y;\n}\n";
    let lints = run_semantic(
        declared,
        "unnecessary_nullable_for_final_variable_declarations",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(declared);
    let x = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
    let inferred = "void f() {\n  var y = 1;\n  final int? x = y;\n}\n";
    let lints = run_semantic(
        inferred,
        "unnecessary_nullable_for_final_variable_declarations",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(inferred);
    let x = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
}

/// Creation and member-access initializers answer through the
/// container-resolution and member faces.
#[test]
fn unnecessary_nullable_creation_and_member_initializer_faces() {
    let creation = "class D {}\nvoid f() {\n  final D? d = D();\n}\n";
    let lints = run_semantic(
        creation,
        "unnecessary_nullable_for_final_variable_declarations",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    let f = Fixture::parse(creation);
    let d = tok(&f.source, &f.tokens, "d", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, d));
    let member = concat!(
        "class C {\n",
        "  int get v => 1;\n",
        "}\n",
        "void f(C c) {\n",
        "  final int? x = c.v;\n",
        "}\n",
    );
    let lints = run_semantic(
        member,
        "unnecessary_nullable_for_final_variable_declarations",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Type could be non-nullable.");
}

/// `_superParametersAreInOrder` (:65-91): named super-formals written
/// out of their super constructor's parameter order arm the flag — the
/// following required parameter reports (:55-61). The super fixture
/// spells required-first (`{required int a, int? b}`, upstream's
/// `A({required int a, int? b})` shape) so the superclass itself stays
/// clean.
#[test]
fn required_named_first_out_of_order_super_formals_report() {
    let source = concat!(
        "class S {\n",
        "  const S({required int a, int? b});\n",
        "}\n",
        "class C extends S {\n",
        "  const C({required super.b, super.a, required int x});\n",
        "}\n",
    );
    let lints = run_semantic(source, "always_put_required_named_parameters_first", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Required named parameters should be before optional named parameters."
    );
    let f = Fixture::parse(source);
    let x = tok(&f.source, &f.tokens, "x", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
}

/// In-order super-formals never arm — the required parameter after
/// them stays silent (:55-58). The super fixture spells required-first
/// so the superclass's own list stays clean.
#[test]
fn required_named_first_in_order_super_formals_pass() {
    let source = concat!(
        "class S {\n",
        "  const S({required int a, int? b});\n",
        "}\n",
        "class C extends S {\n",
        "  const C({super.a, required super.b, required int x});\n",
        "}\n",
    );
    assert!(run_semantic(source, "always_put_required_named_parameters_first", None).is_empty());
}

/// At most one named super-formal is trivially in order (:71) — no
/// constructor resolution runs.
#[test]
fn required_named_first_single_super_formal_passes() {
    let source = concat!(
        "class S {\n",
        "  const S({int? key});\n",
        "}\n",
        "class C extends S {\n",
        "  const C({super.key, required int x});\n",
        "}\n",
    );
    assert!(run_semantic(source, "always_put_required_named_parameters_first", None).is_empty());
}

/// The written `super.named(...)` initializer selects the target
/// constructor's formal table (the super fixture spells required-first
/// so its own list stays clean).
#[test]
fn required_named_first_super_initializer_selector_resolves() {
    let source = concat!(
        "class S {\n",
        "  const S.named({required int a, int? b});\n",
        "}\n",
        "class C extends S {\n",
        "  const C({required super.b, super.a, required int x}) : super.named();\n",
        "}\n",
    );
    let lints = run_semantic(source, "always_put_required_named_parameters_first", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// The supertype resolves across files through the import face.
#[test]
fn required_named_first_super_order_resolves_across_files() {
    let dependency = "class S {\n  const S({int? a, required int b});\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class C extends S {\n",
        "  const C({required super.b, super.a, required int x});\n",
        "}\n",
    );
    let lints = run_semantic(
        source,
        "always_put_required_named_parameters_first",
        Some(dependency),
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
}

// —— L1 cross-file inheritance burn (the two semantic faces) ——————————

/// `annotate_overrides`: a member overriding a cross-file inherited
/// member reports at its name token (the bridge's inheritance-chain
/// arm, anchored at the written extends head).
#[test]
fn annotate_overrides_cross_file_member_reports() {
    let dependency = "class A {\n  void m() {}\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  void m() {}\n",
        "}\n",
    );
    let lints = run_semantic(source, "annotate_overrides", Some(dependency));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'm' overrides an inherited member but isn't annotated with '@override'."
    );
    let f = Fixture::parse(source);
    let m = tok(&f.source, &f.tokens, "m", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, m));
}

/// The official silences hold cross-file: the `@override` annotation
/// (:42) and a name the chain lacks.
#[test]
fn annotate_overrides_cross_file_annotated_and_absent_pass() {
    let annotated = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  @override\n",
        "  void m() {}\n",
        "}\n",
    );
    let dependency = "class A {\n  void m() {}\n}\n";
    assert!(run_semantic(annotated, "annotate_overrides", Some(dependency)).is_empty());
    let absent = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  void n() {}\n",
        "}\n",
    );
    assert!(run_semantic(absent, "annotate_overrides", Some(dependency)).is_empty());
}

/// The accessor faces cross-file: a getter override reports `g`, the
/// setter override renders the analyzer element's `x=` name.
#[test]
fn annotate_overrides_cross_file_accessor_faces() {
    let dependency = "class A {\n  int get g => 0;\n  set s(int v) {}\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  int get g => 1;\n",
        "  set s(int v) {}\n",
        "}\n",
    );
    let lints = run_semantic(source, "annotate_overrides", Some(dependency));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'g' overrides an inherited member but isn't annotated with '@override'."
    );
    assert_eq!(
        lints[1].message,
        "The member 's=' overrides an inherited member but isn't annotated with '@override'."
    );
}

/// The chain climbs transitively: the same-file intermediate's own
/// superclass is cross-file.
#[test]
fn annotate_overrides_cross_file_transitive_chain_reports() {
    let dependency = "class A {\n  void m() {}\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {}\n",
        "\n",
        "class C extends B {\n",
        "  void m() {}\n",
        "}\n",
    );
    let lints = run_semantic(source, "annotate_overrides", Some(dependency));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// The primary-constructor parameter face rides the cross-file chain:
/// a `var` formal overriding an inherited field reports at its name.
#[test]
fn annotate_overrides_cross_file_primary_formal_reports() {
    let dependency = "class A {\n  int x = 0;\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B(var int x) extends A;\n",
    );
    let lints = run_semantic(source, "annotate_overrides", Some(dependency));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The member 'x' overrides an inherited member but isn't annotated with '@override'."
    );
}

/// `avoid_renaming_method_parameters`: the inherited concrete method's
/// positional names resolve cross-file; renames report.
#[test]
fn avoid_renaming_cross_file_rename_reports() {
    let dependency = "class A {\n  void foo(int aa) {}\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  void foo(int cc) {}\n",
        "}\n",
    );
    let lints = run_semantic(source, "avoid_renaming_method_parameters", Some(dependency));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter name 'cc' doesn't match the name 'aa' in the overridden method."
    );
}

/// The official silences hold cross-file: matching names, and an
/// abstract inherited method with no concrete parameter names (the
/// `concrete_only` face).
#[test]
fn avoid_renaming_cross_file_matching_and_abstract_pass() {
    let matching = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  void foo(int aa) {}\n",
        "}\n",
    );
    let concrete_dependency = "class A {\n  void foo(int aa) {}\n}\n";
    assert!(
        run_semantic(
            matching,
            "avoid_renaming_method_parameters",
            Some(concrete_dependency)
        )
        .is_empty()
    );
    let renaming = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  void foo(int cc) {}\n",
        "}\n",
    );
    let abstract_dependency = "abstract class A {\n  void foo(int aa);\n}\n";
    assert!(
        run_semantic(
            renaming,
            "avoid_renaming_method_parameters",
            Some(abstract_dependency)
        )
        .is_empty()
    );
}

/// `overridden_fields`: a field redeclaring a cross-file inherited
/// field reports the defining interface's name.
#[test]
fn overridden_fields_cross_file_reports() {
    let dependency = "class A {\n  int f = 0;\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  int f = 1;\n",
        "}\n",
    );
    let lints = run_semantic(source, "overridden_fields", Some(dependency));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Field overrides a field inherited from 'A'."
    );
}

/// The official silences hold cross-file: a declared inherited getter
/// is not variable-backed (`isOriginVariable`), an abstract inherited
/// field never passes the concrete cut, and the `@override`-less chain
/// end (no supertype at all) has nothing to override. The transitive
/// chain through a same-file intermediate reports the cross-file
/// interface's name.
#[test]
fn overridden_fields_cross_file_getter_abstract_and_chain() {
    let getter = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {\n",
        "  int f = 1;\n",
        "}\n",
    );
    let getter_dependency = "class A {\n  int get f => 0;\n}\n";
    assert!(run_semantic(getter, "overridden_fields", Some(getter_dependency)).is_empty());
    let abstract_dependency = "abstract class A {\n  abstract int f;\n}\n";
    assert!(run_semantic(getter, "overridden_fields", Some(abstract_dependency)).is_empty());
    let chained = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class B extends A {}\n",
        "\n",
        "class C extends B {\n",
        "  int f = 1;\n",
        "}\n",
    );
    let field_dependency = "class A {\n  int f = 0;\n}\n";
    let lints = run_semantic(chained, "overridden_fields", Some(field_dependency));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Field overrides a field inherited from 'A'."
    );
}

/// `avoid_classes_with_only_static_members`: instance members
/// inherited cross-file silence the class (the `Some(true)` face —
/// the former dartq-only over-report).
#[test]
fn avoid_only_static_cross_file_instance_members_silence() {
    let dependency = "class A {\n  int f = 0;\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class C extends A {\n",
        "  static int i = 0;\n",
        "}\n",
    );
    assert!(
        run_semantic(
            source,
            "avoid_classes_with_only_static_members",
            Some(dependency)
        )
        .is_empty()
    );
}

/// A static-only cross-file supertype keeps the report
/// (`Some(false)` — the chain is indexed and instance-free), and an
/// unresolvable written supertype conservatively silences (`None`).
#[test]
fn avoid_only_static_cross_file_static_and_unknown_supertypes() {
    let static_dependency = "class A {\n  static int s = 0;\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "class C extends A {\n",
        "  static int i = 0;\n",
        "}\n",
    );
    let lints = run_semantic(
        source,
        "avoid_classes_with_only_static_members",
        Some(static_dependency),
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Classes should define instance members.");
    let unknown = concat!(
        "class C extends Missing {\n",
        "  static int i = 0;\n",
        "}\n",
    );
    assert!(run_semantic(unknown, "avoid_classes_with_only_static_members", None).is_empty());
}

/// The binary operator face (:266-278): `super + other` over the sole
/// parameter reports at the declared operator name.
#[test]
fn unnecessary_overrides_binary_operator_forward_reports() {
    let source = concat!(
        "class A {\n",
        "  int operator +(int other) => 0;\n",
        "}\n",
        "class B extends A {\n",
        "  int operator +(int other) => super + other;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("unnecessary_overrides"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary override.");
    let plus = tok(&f.source, &f.tokens, "+", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, plus));
}

/// The prefix operator face (:280-291): `~super` with no parameters
/// reports.
#[test]
fn unnecessary_overrides_prefix_operator_forward_reports() {
    let source = concat!(
        "class A {\n",
        "  int operator ~() => 0;\n",
        "}\n",
        "class B extends A {\n",
        "  int operator ~() => ~super;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("unnecessary_overrides"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let tilde = tok(&f.source, &f.tokens, "~", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, tilde));
}

/// A body operator other than the declared one never matches (:268).
#[test]
fn unnecessary_overrides_operator_body_mismatch_passes() {
    let source = concat!(
        "class A {\n",
        "  int operator +(int other) => 0;\n",
        "}\n",
        "class B extends A {\n",
        "  int operator +(int other) => super - other;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());
}

/// A differing parameter name fails the signature guard (:149) before
/// the body walk.
#[test]
fn unnecessary_overrides_operator_renamed_parameter_passes() {
    let source = concat!(
        "class A {\n",
        "  int operator +(int other) => 0;\n",
        "}\n",
        "class B extends A {\n",
        "  int operator +(int another) => super + another;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());
}

/// Overriding a `@protected` member without `@protected` skips as the
/// visibility expansion (:93, :162-169).
#[test]
fn unnecessary_overrides_publicizing_protected_member_passes() {
    let source = concat!(
        "class A {\n",
        "  @protected\n",
        "  void g() {}\n",
        "}\n",
        "class B extends A {\n",
        "  void g() => super.g();\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());
}

/// The `@protected` pairing exempts the annotation on both sides and
/// the override reports (:122); a `@protected` annotation over a plain
/// inherited member arms the meaningful-override skip (:117-131).
#[test]
fn unnecessary_overrides_protected_pairing_and_new_annotation() {
    let pairing = concat!(
        "class A {\n",
        "  @protected\n",
        "  void g() {}\n",
        "}\n",
        "class B extends A {\n",
        "  @protected\n",
        "  void g() => super.g();\n",
        "}\n",
    );
    let f = Fixture::parse(pairing);
    let lints = f.run(&one_rule("unnecessary_overrides"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let fresh = concat!(
        "class A {\n",
        "  void g() {}\n",
        "}\n",
        "class B extends A {\n",
        "  @protected\n",
        "  void g() => super.g();\n",
        "}\n",
    );
    let f = Fixture::parse(fresh);
    assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());
}

/// The private-enclosing skip (:74 `parentElement.isPrivate`): no
/// override inside a private class checks, the lib-directory gate met.
#[test]
fn avoid_renaming_private_enclosing_class_skips() {
    // Upstream `test_positional_privateOverride` (:139-149): the skip
    // reads the *enclosing* class's `isPrivate` — the renamed
    // parameter lives inside the private class, not its supertype.
    let source = concat!(
        "class Pub {\n",
        "  void m(int a) {}\n",
        "}\n",
        "class _Priv extends Pub {\n",
        "  void m(int b) {}\n",
        "}\n",
    );
    let mut f = Fixture::parse(source);
    f.path = std::path::PathBuf::from("/pkg/lib/a.dart");
    assert!(
        f.run(&one_rule("avoid_renaming_method_parameters"))
            .is_empty()
    );
}

/// The same shape over a public class reports at the renamed
/// parameter — the skip reads the class name, not the file.
#[test]
fn avoid_renaming_public_enclosing_class_reports() {
    let source = concat!(
        "class Pub {\n",
        "  void m(int a) {}\n",
        "}\n",
        "class C extends Pub {\n",
        "  void m(int b) {}\n",
        "}\n",
    );
    let mut f = Fixture::parse(source);
    f.path = std::path::PathBuf::from("/pkg/lib/a.dart");
    let lints = f.run(&one_rule("avoid_renaming_method_parameters"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The parameter name 'b' doesn't match the name 'a' in the overridden method."
    );
    let b = tok(&f.source, &f.tokens, "b", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, b));
}

/// The primary-constructor parameter face (:51-64): a `var`/`final`
/// formal declaring a field that overrides an inherited variable-backed
/// member reports at the name token.
#[test]
fn overridden_fields_primary_constructor_formal_reports() {
    let source = concat!(
        "class A {\n",
        "  int x = 0;\n",
        "}\n",
        "\n",
        "class B(final int x) extends A {}\n",
    );
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("overridden_fields"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Field overrides a field inherited from 'A'."
    );
    let x = tok(&f.source, &f.tokens, "x", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, x));
}

/// A declared getter is not a variable-backed member (:72-73) and a
/// keywordless formal declares no field — both stay silent.
#[test]
fn overridden_fields_primary_constructor_non_field_faces_pass() {
    let getter = concat!(
        "class A {\n",
        "  int get x => 0;\n",
        "}\n",
        "\n",
        "class B(final int x) extends A {}\n",
    );
    let f = Fixture::parse(getter);
    assert!(f.run(&one_rule("overridden_fields")).is_empty());
    let keywordless = concat!(
        "class A {\n",
        "  int x = 0;\n",
        "}\n",
        "\n",
        "class B(int x) extends A {}\n",
    );
    let f = Fixture::parse(keywordless);
    assert!(f.run(&one_rule("overridden_fields")).is_empty());
}

// —— type_init_formals super-formal gate ———————————————————————————

/// The super-formal element check (:52-64) over the bridge: a
/// corresponding parameter reports when the type comparison cannot
/// disprove it (single-token equal spellings, parameterized spellings),
/// and the compile-error family stays silent — a mismatched one-token
/// type, a positional formal past the super list, a named formal the
/// super list lacks, and a class with no extends supertype (the
/// implicit `Object` forwards nothing).
#[test]
fn type_init_formals_super_formal_gate() {
    let matching = "class S { S(int y); }\nclass C extends S { C(int super.y); }\n";
    assert_eq!(run_semantic(matching, "type_init_formals", None).len(), 1);

    let mismatched = "class S { S(int y); }\nclass C extends S { C(String super.y); }\n";
    assert!(run_semantic(mismatched, "type_init_formals", None).is_empty());

    let missing_positional =
        "class S { S(int y); }\nclass C extends S { C(int a, int super.b); }\n";
    assert!(run_semantic(missing_positional, "type_init_formals", None).is_empty());

    let missing_named = "class S { S({int y}); }\nclass C extends S { C({int super.z}); }\n";
    assert!(run_semantic(missing_named, "type_init_formals", None).is_empty());

    let no_supertype = "class C { C(int super.x); }\n";
    assert!(run_semantic(no_supertype, "type_init_formals", None).is_empty());

    let parameterized =
        "class S { S(List<int> x); }\nclass C extends S { C(List<int> super.x); }\n";
    assert_eq!(
        run_semantic(parameterized, "type_init_formals", None).len(),
        1
    );
}

/// The written `super.named(...)` initializer selects the constructor
/// the super formals correspond to — a hit with an equal type reports,
/// a selector the supertype lacks stays silent (the implicit forwarding
/// is a compile error upstream).
#[test]
fn type_init_formals_super_formal_selector_gate() {
    let redirect =
        "class S { S.named(int y); }\nclass C extends S { C(int super.y) : super.named(); }\n";
    assert_eq!(run_semantic(redirect, "type_init_formals", None).len(), 1);

    let missing_target =
        "class S { S(int y); }\nclass C extends S { C(int super.y) : super.named(); }\n";
    assert!(run_semantic(missing_target, "type_init_formals", None).is_empty());
}

// —— #2927 census pins (the shell-side suppression faces) ————————————

/// `lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceGetter_reachable_overrides#0`
/// — `INFO|unreachable_from_main|lib/test.dart|7:11|3`: the base
/// declaration reports; the overriding member suppresses through the
/// same-file `isOverride` arm (the override is itself a candidate the
/// gatherer skips — the engine face carries no edge for it).
#[test]
fn overridden_getter_suppresses_the_override_reports_the_base() {
    let source = concat!(
        "void main() {\n",
        "  B();\n",
        "}\n",
        "\n",
        "class A {\n",
        "  int get foo => 0;\n",
        "}\n",
        "\n",
        "class B extends A {\n",
        "  int get foo => 0;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "A", 0, true),
            flag(&f, "B", 0, true),
            flag(&f, "foo", 0, false), // A.foo — the oracle row 7:11.
            flag(&f, "foo", 1, false), // B.foo — override-suppressed.
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'foo' in an executable library."
    );
    assert_eq!(lints[0].range.start, flag(&f, "foo", 0, false).0);
}

/// `lints/unreachable_from_main/UnreachableFromMainTest/test_class_instanceField_reachable_overrides_local#0`
/// — `INFO|unreachable_from_main|lib/test.dart|7:8|1`: the field arm of
/// the same override read (`MemberQuery::Getter` over the field's
/// accessor).
#[test]
fn overridden_field_suppresses_the_override_reports_the_base() {
    let source = concat!(
        "void main() {\n",
        "  B();\n",
        "}\n",
        "\n",
        "class A {\n",
        "  int? f;\n",
        "}\n",
        "\n",
        "class B extends A {\n",
        "  int? f;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "A", 0, true),
            flag(&f, "B", 0, true),
            flag(&f, "f", 0, false), // A.f — the oracle row 7:8.
            flag(&f, "f", 1, false), // B.f — override-suppressed.
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.start, flag(&f, "f", 0, false).0);
}

/// `lints/unreachable_from_main/UnreachableFromMainTest/test_instanceMethod_reachable_toJson#0`
/// — clean: a non-static `toJson` never gathers (the `dart:convert`
/// dynamic-invocation auto-reach arm). The flag stays `false` to keep
/// the arm honest — the suppression is the gather, not the face.
#[test]
fn non_static_to_json_never_gathers() {
    let source = concat!(
        "void main() {\n",
        "  C();\n",
        "}\n",
        "\n",
        "class C {\n",
        "  List<Object> toJson() => ['c'];\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 0, true),
            flag(&f, "toJson", 0, false),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// A `static` `toJson` gathers again — the dynamic-invocation arm
/// keys on the instance member.
#[test]
fn static_to_json_still_gathers() {
    let source = concat!(
        "void main() {\n",
        "  C();\n",
        "}\n",
        "\n",
        "class C {\n",
        "  static Map<String, Object> toJson() => {};\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 0, true),
            flag(&f, "toJson", 0, false),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'toJson' in an executable library."
    );
}

/// `lints/unreachable_from_main/UnreachableFromMainTest/test_class_reachableViaComment#0`
/// — clean: a doc-comment `[C]` reference counts as a use (the
/// comment-reference edge the engine face does not carry — the
/// `counting_uses` doc arm).
#[test]
fn doc_comment_reference_counts_as_use() {
    let source = "/// See [C].\nvoid main() {}\n\nclass C {}\n";
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "C", 0, false)],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// The anti-reach arm: an expression-position identifier use counts
/// (the official `visitSimpleIdentifier`) — the closure propagates
/// through the call chain. A plain type annotation does not count —
/// `TypeReference` rows never enter the reference set, and a
/// `Some(true)` face row riding only one reports (the official's
/// NamedType filter over the workspace-global engine graph).
#[test]
fn expression_use_counts_type_annotation_does_not() {
    // The counted arm: `dead()` inside `live()` — `live` is called
    // from `main`, so the closure reaches `dead` through it.
    let source = "void main() {\n  live();\n}\nvoid live() { dead(); }\nvoid dead() {}\n";
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "live", 0, false),
            flag(&f, "dead", 0, false),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");

    // The type-reference counterface: `Dead` appearing only in a type
    // annotation keeps its report even under a `Some(true)` face row.
    let source = "void main() {\n  f(0);\n}\nvoid f(Object o) {}\nclass Dead {}\n";
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "f", 0, false),
            flag(&f, "Dead", 0, true),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'Dead' in an executable library."
    );
}

// —— the per-library closure's shape pins ————————————————————————

/// `test_topLevelGetter_unreachable` — `INFO|…|4:9|1|'g'`: a top-level
/// getter reports at its name (the declaration-name read is
/// context-filtered — a return-type identifier never answers for the
/// name).
#[test]
fn top_level_getter_reports() {
    let f = Fixture::parse("void main() {}\nint get g => 7;\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "g", 0, false)],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'g' in an executable library."
    );
    assert_eq!(lints[0].range.start, flag(&f, "g", 0, false).0);
}

/// The operator visits (`visitBinaryExpression`,
/// `visitPrefixExpression`, `visitPostfixExpression`,
/// `visitIndexExpression`): every operator spelling the body uses
/// stays reachable — `[]`, `[]=` (the assignment target), `<`, `+`
/// (from `++`), unary `-`, `~`.
#[test]
fn operator_uses_keep_operators_reachable() {
    let source = concat!(
        "void main() {\n",
        "  final v = V();\n",
        "  v[0];\n",
        "  v[0] = 1;\n",
        "  v < 1;\n",
        "  v++;\n",
        "  -v;\n",
        "  ~v;\n",
        "}\n",
        "\n",
        "class V {\n",
        "  int operator [](int i) => 0;\n",
        "  void operator []=(int i, int x) {}\n",
        "  bool operator <(Object o) => true;\n",
        "  V operator +(int o) => this;\n",
        "  V operator -() => this;\n",
        "  V operator ~() => this;\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let entries = [
        flag(&f, "main", 0, true),
        flag(&f, "V", 1, true),
        flag(&f, "[]", 0, false),
        flag(&f, "[]=", 0, false),
        flag(&f, "<", 1, false),
        flag(&f, "+", 0, false),
        flag(&f, "-", 1, false),
        flag(&f, "~", 1, false),
    ];
    let lints = run(&f, true, &entries);
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_constructor_named_reachableViaRedirection` — the redirecting
/// `this.named()` initializer marks the target constructor and the
/// class (`visitRedirectingConstructorInvocation`): both constructors
/// and the class stay reachable.
#[test]
fn redirect_marks_the_target_constructor() {
    let source = concat!(
        "void main() {\n",
        "  C.two();\n",
        "}\n",
        "\n",
        "class C {\n",
        "  C.named();\n",
        "  C.two() : this.named();\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let entries = [
        flag(&f, "main", 0, true),
        flag(&f, "C", 1, true),
        flag(&f, "named", 1, false),
        flag(&f, "C", 3, false),
    ];
    let lints = run(&f, true, &entries);
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_constructor_named_new` — `INFO|…|7:7|5|'C.named'`: a
/// `new`-prefixed declaration displays `Type.written`, anchored at the
/// written name (`new named();`).
#[test]
fn new_named_constructor_displays_the_written_name() {
    let source = concat!(
        "void main() {\n",
        "  C;\n",
        "}\n",
        "\n",
        "class C {\n",
        "  new named();\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 1, true),
            flag(&f, "named", 0, false),
        ],
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C.named' in an executable library."
    );
    assert_eq!(lints[0].range.start, flag(&f, "named", 0, false).0);
}

/// `test_extensionType_member_redeclare_unreachable` —
/// `INFO|…|13:8|1|'m'`: an extension type's members never take the
/// override skip (`containerElement: null`), and the closure's
/// receiver-resolved key (`c.m()` with `var c = C()` marks `C.m`, not
/// every `m`) leaves the redeclared `E.m` unreachable — even where the
/// face flags it reachable.
#[test]
fn extension_type_redeclared_member_reports() {
    let source = concat!(
        "void main() {\n",
        "  var c = C();\n",
        "  c.m();\n",
        "  E(c);\n",
        "}\n",
        "\n",
        "class C {\n",
        "  void m() {}\n",
        "}\n",
        "\n",
        "extension type E(C c) implements C {\n",
        "  void m() {}\n",
        "}\n",
    );
    let f = Fixture::parse(source);
    let entries = [
        flag(&f, "main", 0, true),
        flag(&f, "C", 1, true),
        flag(&f, "m", 1, true),
        flag(&f, "E", 1, true),
        flag(&f, "m", 2, true),
    ];
    let lints = run(&f, true, &entries);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'm' in an executable library."
    );
    assert_eq!(lints[0].range.start, flag(&f, "m", 2, false).0);
}

/// `visitNamedType`'s exception list still counts: a type-argument
/// position (`List<C>`) and a typedef-alias reference keep their
/// declarations reachable through the closure.
#[test]
fn type_arguments_and_aliases_still_count() {
    let source = concat!(
        "void main() {\n",
        "  f<List<C>>();\n",
        "}\n",
        "\n",
        "void f(List<C> x) {}\n",
        "\n",
        "class C {}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "f", 0, false),
            flag(&f, "C", 1, false),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");

    let alias = concat!(
        "typedef T = int;\n",
        "\n",
        "void main() {\n",
        "  T x = 1;\n",
        "}\n",
    );
    let f = Fixture::parse(alias);
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "T", 0, false)],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_topLevelFunction_vmEntryPoint_const` — the const alias
/// `const entryPoint = pragma('vm:entry-point')` seeds an entry
/// through `@entryPoint` (`computeConstantValue`'s alias following):
/// `f6` never reports, and the alias const itself stays reachable
/// through the annotation's reference.
#[test]
fn vm_entry_point_const_alias_seeds_an_entry() {
    let source = concat!(
        "const entryPoint = pragma('vm:entry-point');\n",
        "\n",
        "@entryPoint\n",
        "void f6() {}\n",
        "\n",
        "void main() {}\n",
    );
    let f = Fixture::parse(source);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "entryPoint", 0, false),
            flag(&f, "f6", 0, false),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");
}

/// `test_widgetPreview_topLevelFunction_const` (the alias suppresses
/// through the flutter import surface) and
/// `test_widgetPreview_topLevelFunction_customPreviewClass` (a
/// unit-declared `Preview` class shadows the annotation — `f6` AND the
/// unreferenced `Preview` class report; the const constructor stays
/// suppressed behind its dead container).
#[test]
fn widget_preview_alias_and_custom_preview_class() {
    let imported = concat!(
        "import 'package:flutter/widget_previews.dart';\n",
        "\n",
        "const preview = Preview();\n",
        "\n",
        "@preview\n",
        "void f6() {}\n",
        "\n",
        "void main() {}\n",
    );
    let f = Fixture::parse(imported);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "preview", 0, false),
            flag(&f, "f6", 0, false),
        ],
    );
    assert!(lints.is_empty(), "{lints:?}");

    let custom = concat!(
        "class Preview {\n",
        "  const Preview();\n",
        "}\n",
        "\n",
        "@Preview()\n",
        "void f6() {}\n",
        "\n",
        "void main() {}\n",
    );
    let f = Fixture::parse(custom);
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "Preview", 0, false),
            flag(&f, "f6", 0, false),
        ],
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'Preview' in an executable library."
    );
    assert_eq!(
        lints[1].message,
        "Unreachable member 'f6' in an executable library."
    );
    assert_eq!(lints[1].range.start, flag(&f, "f6", 0, false).0);
}

/// `test_typedef_reachable_referencedInObjectPattern`: an object
/// pattern's type name counts when it is alias-like — the typedef
/// reference (upstream `visitNamedType`'s `type.alias` arm). The head
/// identifier tiles no Identifier row of its own, so the row pass
/// reads the ObjectPattern row's name fields. A plain class in the
/// same position stays uncounted (`test_class_referencedInObjectPattern`
/// — structural matching against an unknown layout is no use).
#[test]
fn object_pattern_type_counts_alias_like_types_only() {
    let f = Fixture::parse(
        "typedef T = int;\n\nvoid main() {\n  f();\n}\n\nvoid f([Object? c]) {\n  if (c case T()) {}\n}\n",
    );
    let lints = run(&f, true, &[flag(&f, "main", 0, true)]);
    assert!(lints.is_empty(), "{lints:?}");

    let f = Fixture::parse(
        "class C {}\n\nvoid main() {\n  f();\n}\n\nvoid f([Object? c]) {\n  if (c case C()) {}\n}\n",
    );
    let lints = run(&f, true, &[]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'C' in an executable library."
    );
}

/// `test_class_instanceMethod_reachable_overrides_imported`: the
/// override read crosses files — the semantic bridge's inheritance walk
/// anchored at the written extends head (upstream `getOverridden`
/// answers through the imported interface), so `B.foo` never gathers.
/// With the imported base declaring no `foo`, the bridge answers no and
/// the unreferenced member reports.
#[test]
fn imported_override_suppresses_the_override() {
    let dependency = "class A {\n  void foo() {}\n}\n";
    let source = concat!(
        "import 'kit.dart';\n",
        "\n",
        "void main() {\n",
        "  B();\n",
        "}\n",
        "\n",
        "class B extends A {\n",
        "  void foo() {}\n",
        "}\n",
    );
    let lints = run_semantic_face(source, "unreachable_from_main", Some(dependency), true, &[]);
    assert!(lints.is_empty(), "{lints:?}");

    let dependency = "class A {}\n";
    let lints = run_semantic_face(source, "unreachable_from_main", Some(dependency), true, &[]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unreachable member 'foo' in an executable library."
    );
}
