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
/// `if (entryPoints.isEmpty) return`: nothing reports at all.
#[test]
fn library_without_entry_point_reports_nothing() {
    let f = Fixture::parse("void main() {}\nvoid stray() {}\n");
    let lints = run(
        &f,
        false,
        &[flag(&f, "main", 0, false), flag(&f, "stray", 0, false)],
    );
    assert!(lints.is_empty(), "{lints:?}");
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

/// `test_topLevelFunction_reachable` / `..._Variable_reachable` —
/// reachable flags stay silent, and a flag row absent from the face is
/// the conservative no-report.
#[test]
fn reachable_and_unrowed_declarations_stay_silent() {
    let f = Fixture::parse("void main() {}\nvoid used() {}\nint k = 0;\n");
    let lints = run(
        &f,
        true,
        &[flag(&f, "main", 0, true), flag(&f, "used", 0, true)],
    );
    assert!(lints.is_empty(), "{lints:?}");
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
    let f = Fixture::parse("void main() {}\nint used = 0, dead = 0;\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "used", 0, true),
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
        "void main() {}\n\nclass C {\n  static void stray() {}\n}\n\nclass D {\n  static void dead() {}\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 0, true),
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
/// `Type.new`, anchored at its own class-name echo (`C();` — the third
/// `C` lexeme, after the declaration and the `C.named` echo).
#[test]
fn constructors_display_qualified_names() {
    let f = Fixture::parse("void main() {}\n\nclass C {\n  C.named();\n  C();\n}\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 0, true),
            flag(&f, "named", 0, false),
            flag(&f, "C", 2, false),
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
        "void main() {}\n\nenum E {\n  a, b;\n\n  const E._();\n\n  static void stray() {}\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "E", 0, true),
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
        "void main() {}\n\nclass T {\n  void test_one() {}\n  void solo_test_two() {}\n  void setUp() {}\n  void tearDown() {}\n  void setUpClass() {}\n  void tearDownClass() {}\n}\n",
    );
    // `main` is the entry and `T` is reachable; the six unreachable
    // test-harness methods never gather, so nothing reports.
    let entries = [
        flag(&f, "main", 0, true),
        flag(&f, "T", 0, true),
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

/// The widget-preview suppression: a public static `@Preview` member
/// never reports; an ordinary dead static member next to it does.
#[test]
fn static_preview_member_suppressed() {
    let f = Fixture::parse(
        "void main() {}\n\nclass C {\n  @Preview\n  static void previewed() {}\n\n  static void stray() {}\n}\n",
    );
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 0, true),
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
    let f = Fixture::parse("void main() {}\n\nclass C {\n  int live = 0, gone = 0;\n}\n");
    let lints = run(
        &f,
        true,
        &[
            flag(&f, "main", 0, true),
            flag(&f, "C", 0, true),
            flag(&f, "live", 0, true),
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
fn run_semantic(
    source: &str,
    rule: &str,
    dependency: Option<&str>,
) -> Vec<crate::engine::Lint> {
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
    assert!(
        run_semantic(source, "always_put_required_named_parameters_first", None).is_empty()
    );
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
    assert!(
        run_semantic(source, "always_put_required_named_parameters_first", None).is_empty()
    );
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
