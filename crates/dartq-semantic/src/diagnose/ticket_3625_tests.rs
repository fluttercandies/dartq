//! The #3625 pins: the `non_exhaustive_switch_expression` extras (the
//! generic-enum constant filter of
//! `EnumStaticType._createEnumElements`, the extension-type erasure's
//! use-site substitution, the constant-pattern value leaves) and the
//! `missing_default_value_for_parameter` extras (the unresolved-type
//! `InvalidType` face, the `external` header gate's param-internal
//! braces, the augmentation chain's merged-element type and
//! positional/wildcard merge).
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) over the
//! census-11 cases — the enum/ET cases mirror
//! `LanguageFeatures/Patterns/Exhaustiveness/exhaustiveness_enum_A01_*`
//! and `Extension-types/exhaustiveness_enum_A01_*`, the external cases
//! `MissingDefaultValueForParameterTest.test_*_external*`, the augment
//! cases `Augmentations/augmenting_functions_A04_t25` and
//! `augmenting_constructors_A02_t06`.
//! Official sources: `_fe_analyzer_shared` types/enum.dart:111-150
//! (the `isSubtypeOf(element._type, overapproximate(_type))` filter),
//! types/sealed.dart:77-119, shared.dart:195 (the erase-at-entry
//! `extensionTypeErasure`), analyzer exhaustiveness.dart:744-788
//! (`PatternConverter._convertConstantValue` value leaves),
//! error_verifier.dart:7964-8034 + 8328-8381 (the merged-element
//! `isPotentiallyNonNullable`/`hasDefaultValue`/`defaultValuesAreExpected`
//! gates and the `external` exclusion).

use super::*;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The member-bearing core classes a fixture may name (the shared rig's
/// prelude grade): declared ahead of the fixture so every core spelling
/// resolves.
const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator >(int o) => true; bool operator <(int o) => true; } \
     class double extends num {} \
     class String { String operator +(String other) => this; int get length => 0; } class bool {} class Iterable<E> {} \
     class List<E> extends Iterable<E> { void add(E e) {} E operator [](int i) => throw 0; int get length => 0; } \
     class Map<K, V> { V? operator [](K k) => null; Iterable<K> get keys => throw 0; } \
     class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

/// The names-only `dart:core` stub: the special spellings the engine
/// decides by name (`Never` is the bottom type, `Object` the root).
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// The one-library rig (the shared rig's shape): `source` beside
/// [`CORE_PRELUDE`] is the sole analyzed file of `package:t`, with the
/// names-only `dart:core` stub imported for the special spellings.
fn unit_report(source: &str) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        &full,
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
    let elements = collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        dartq_resolution::ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: dartq_resolution::Combinators::unfiltered(),
        },
    );
    // The unit registration the shared rig keeps: `unit_files` (the
    // merged-element formal tables' intro walk) answers the library's
    // defining unit — without it the augmentation merge's introductory
    // formal lookup sees an empty library.
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        &full,
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
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The `code` rows of `report` (the conformance face without the file
/// column — offsets stay in the concatenated source's frame).
fn rows_of(source: &str, code: &str) -> Vec<String> {
    unit_report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// The byte offset of `needle`'s first occurrence (the anchor locator —
/// fixtures keep every spelling unique).
#[expect(dead_code)]
fn at(source: &str, needle: &str) -> u32 {
    source.find(needle).expect(needle) as u32
}

// —— non_exhaustive_switch_expression: the generic-enum filter ————

/// `E<num>` over `enum E<T> { a<int>(), b<String>(), c<double>() }`:
/// `E.b` (`E<String>`) is not a subtype of `overapproximate(E<num>)` =
/// `E<num>`, so the populated set is `{a, c}` and `E.a || E.c` covers it
/// — the oracle stays silent (exhaustiveness_enum_A01_t01).
#[test]
fn generic_enum_dead_constant_populates_nothing() {
    let src = "enum E<T> { a<int>(), b<String>(), c<double>() }\n\
               String test(E<num> e) => switch (e) { E.a => 'a', E.c => 'c' };\n";
    for code in [
        "non_exhaustive_switch_expression",
        "non_exhaustive_switch_statement",
    ] {
        assert!(rows_of(src, code).is_empty(), "{code}: {src}");
    }
}

/// The sibling arm: a non-generic enum keeps every constant in the
/// closed set, so `G.x` uncovered reports `G.y`.
#[test]
fn plain_enum_missing_constant_reports() {
    let src = "enum G { x, y }\n\
               String test(G g) => switch (g) { G.x => 'x' };\n";
    let rows = rows_of(src, "non_exhaustive_switch_expression");
    assert_eq!(rows.len(), 1, "{src} -> {rows:?}");
    assert!(rows[0].contains("G.y"), "{rows:?}");
}

/// `E<T>` over `T extends num` overapproximates to `E<num>` — the same
/// `E.b` exclusion (exhaustiveness_enum_A01_t02).
#[test]
fn generic_enum_type_param_scrutinee_still_silent() {
    let src = "enum E<T> { a<int>(), b<String>(), c<double>() }\n\
               class C<T extends num> {\n\
               String test(E<T> e) => switch (e) { E.a => 'a', E.c => 'c' };\n\
               }\n";
    assert!(
        rows_of(src, "non_exhaustive_switch_expression").is_empty(),
        "{src}"
    );
}

// —— non_exhaustive_switch_expression: the ET erasure substitution ————

/// `extension type ET<T>(Face<T> _)` erases `ET<int>` to `Face<int>` —
/// the sealed split then instantiates `Jack<int>`/`Queen<int>`/`King<int>`
/// (`exhaustiveness_object_A01_t02` shape): the `Jack<num>()`/`Queen<int>()`
/// cases cover, no `Jack<T>()`/`King<T>()` witness leaks.
#[test]
fn extension_type_use_site_args_substitute_into_rep() {
    let src = "sealed class Face<T> {}\n\
               class Jack<T> extends Face<T> {}\n\
               class Queen<T> extends Face<T> {}\n\
               class King<T> extends Face<T> {}\n\
               extension type ET<T>(Face<T> _) {}\n\
               String test(ET<int> face) => switch (face) {\n\
                 Jack<num>() => 'j',\n\
                 Queen<int>() => 'q',\n\
                 King<int>() => 'k' };\n";
    let rows = rows_of(src, "non_exhaustive_switch_expression");
    assert!(rows.is_empty(), "{src} -> {rows:?}");
}

/// The bare-`ET` arm keeps the written face (no substitution fires) —
/// the under-report direction holds.
#[test]
fn extension_type_bare_use_keeps_written_face() {
    let src = "sealed class Face<T> {}\n\
               class Jack<T> extends Face<T> {}\n\
               class Queen<T> extends Face<T> {}\n\
               class King<T> extends Face<T> {}\n\
               extension type ET<T>(Face<T> _) {}\n\
               String test(ET face) => switch (face) {\n\
                 Jack() => 'j',\n\
                 Queen() => 'q',\n\
                 King() => 'k' };\n";
    let rows = rows_of(src, "non_exhaustive_switch_expression");
    assert!(rows.is_empty(), "{src} -> {rows:?}");
}

// —— non_exhaustive_switch_expression: the const-value leaves ————

/// `const c = true; case c:` folds to `BoolValue(true)` — paired with
/// `case false` the `bool` scrutinee is covered (constant_A02 shape).
#[test]
fn const_bool_variable_pattern_covers_its_value() {
    let src = "const c = true;\n\
               String test(bool b) => switch (b) { c => 't', false => 'f' };\n";
    assert!(
        rows_of(src, "non_exhaustive_switch_expression").is_empty(),
        "{src}"
    );
}

// —— missing_default_value_for_parameter: the `InvalidType` arm ————

/// An unresolvable written type is the oracle's `InvalidType` — the
/// `isPotentiallyNonNullable` gate answers false and the row never
/// stages (the `FormalParameterParserTest` placeholders `A`/`B`).
#[test]
fn unresolved_written_type_never_reports_missing_default() {
    let src = "void f({A a}) {}\nvoid g({A a, B b}) {}\n";
    // The positional sibling row emits under the shared registry name
    // (`missingDefaultValueForParameterPositional`'s `sharedName`,
    // messages.yaml:11349-11358) — `d.code.name` never carries the
    // `_positional` spelling.
    assert!(
        rows_of(src, "missing_default_value_for_parameter").is_empty(),
        "{src}"
    );
}

/// The resolvable faces keep emitting: `{int a}` reports at `a`.
#[test]
fn non_nullable_named_optional_reports() {
    let src = "void f({int a}) {}\n";
    let rows = rows_of(src, "missing_default_value_for_parameter");
    assert_eq!(rows.len(), 1, "{src} -> {rows:?}");
}

// —— missing_default_value_for_parameter: the `external` gate ————

/// `external` headers exclude the row outright — the named-parameter
/// group's own `{` is inside the formal list, not a member boundary
/// (`test_function_external_nonNullable_named_optional_noDefault`,
/// `test_constructor_externalFactory_nonNullable_named_optional_noDefault`).
#[test]
fn external_header_never_reports_missing_default() {
    let src = "external void f({int a});\n\
               class C { external factory C({int a}); }\n";
    assert!(
        rows_of(src, "missing_default_value_for_parameter").is_empty(),
        "{src}"
    );
}

// —— missing_default_value_for_parameter: the `this.`/`super.` latches ————

/// `B({int super.a})` over `A({int? a})`: the annotated super formal's
/// own written type gates (`int` non-nullable, no default) — the
/// `FieldInitializer` name context feeds the same written face
/// (`test_constructor_generative_nonNullable_named_optional_super_noDefault`).
#[test]
fn annotated_super_formal_reports_missing_default() {
    let src = "class A { A({int? a}); }\nclass B extends A { B({int super.a}) {} }\n";
    let rows = rows_of(src, "missing_default_value_for_parameter");
    assert_eq!(rows.len(), 1, "{src} -> {rows:?}");
}

/// The unannotated `this.` formal reads the bound field's type —
/// `ET.c({this.id})` over `extension type ET(int id)` reports `id`
/// (static_analysis_extension_types_A02_t03's `c1` row).
#[test]
fn unannotated_this_formal_reads_field_ty() {
    let src = "extension type ET(int id) { ET.c1({this.id}); }\n";
    let rows = rows_of(src, "missing_default_value_for_parameter");
    assert_eq!(rows.len(), 1, "{src} -> {rows:?}");
}

// —— missing_default_value_for_parameter: the augmentation merge ————

/// `augment void f([int _])` over `void f([int x = 2])`: the wildcard
/// `_` merges positionally, `x`'s `= 2` supplies — silent
/// (`augmenting_functions_A04_t25`'s `staticMethod2` rows).
#[test]
fn wildcard_positional_augment_merges_intro_default() {
    let src = "void f([int x = 2]) {}\naugment void f([int _]) {}\n";
    // The oracle's positional row reports under the shared registry name
    // (`missingDefaultValueForParameterPositional`'s `sharedName`,
    // messages.yaml:11349-11358 — oracle-verified on dart 3.13.x: the
    // intro `x` anchors one `missing_default_value_for_parameter` row).
    assert!(
        rows_of(src, "missing_default_value_for_parameter").is_empty(),
        "{src}"
    );
}

/// `augment void f([int _])` over `void f([int x])` — `x` has no
/// default and the merged type `int` is non-nullable: the augment
/// fragment emits (`augmenting_constructors_A02_t06`'s shape).
#[test]
fn wildcard_positional_augment_without_default_reports() {
    let src = "void f([int x]) {}\naugment void f([int _]) {}\n";
    // The oracle anchors the row at the introductory `x` under the shared
    // registry name (messages.yaml:11349-11358 — oracle-verified on dart
    // 3.13.x: one `missing_default_value_for_parameter` row; the
    // `_positional` spelling is the uniqueName, never `d.code.name`).
    let rows = rows_of(src, "missing_default_value_for_parameter");
    assert_eq!(rows.len(), 1, "{src} -> {rows:?}");
}

/// The merged-type gate: `augment void f({int a})` over `void f({int? a})`
/// gates on the intro's `int?` — `isPotentiallyNonNullable` false — silent.
#[test]
fn augment_formal_gates_on_intro_ty() {
    let src = "void f({int? a}) {}\naugment void f({int a}) {}\n";
    assert!(
        rows_of(src, "missing_default_value_for_parameter").is_empty(),
        "{src}"
    );
}

/// The reverse arm: `augment void f({int? a})` over `void f({int a})`
/// gates on `int` — non-nullable, no default anywhere — the row fires
/// (each fragment's formal answers the merged element's type).
#[test]
fn augment_formal_nullable_over_non_nullable_reports() {
    let src = "void f({int a}) {}\naugment void f({int? a}) {}\n";
    let rows = rows_of(src, "missing_default_value_for_parameter");
    assert!(!rows.is_empty(), "{src} -> {rows:?}");
}
